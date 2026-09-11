//! A scriptable mock harness peer, spawned as a real child process.
//!
//! spec A.7.6 asks for "a mock peer per harness — a small stdlib script the tests spawn, which
//! replays a captured notification script and records interrupts and responses into sidecar
//! files… what makes Stop-everything, wedged-child, hang-on-interrupt and elicitation-response
//! tests possible without the real binary".
//!
//! It is `/bin/sh`, not Python or Node: `fleetd` already depends on a POSIX shell for its login
//! environment, and a test suite that needs an interpreter the machine may not have is a test
//! suite that silently skips. The script greets, then reads one frame at a time, records it, and
//! answers by method — the replies come from real captures, so schema validation passes.

use std::{fs, path::PathBuf};

use tempfile::TempDir;

/// A mock peer's on-disk script and sidecar files.
pub(crate) struct MockPeer {
    directory: TempDir,
    /// Lines printed before Fleet writes anything (a Claude `system/init`, say).
    greeting: Vec<String>,
    /// `(method, response line, follow-up notification lines)`, matched in order.
    replies: Vec<(String, Option<String>, Vec<String>)>,
    /// Whether the peer ignores SIGTERM, so the kill ladder has to escalate.
    ignore_term: bool,
    /// Whether the peer keeps running after stdin closes.
    linger: bool,
}

impl MockPeer {
    /// A peer that greets with `greeting` and then answers nothing.
    pub(crate) fn new() -> Self {
        Self {
            directory: tempfile::tempdir().unwrap_or_else(|error| panic!("mock peer dir: {error}")),
            greeting: Vec::new(),
            replies: Vec::new(),
            ignore_term: false,
            linger: false,
        }
    }

    /// Lines the peer prints as soon as it starts.
    pub(crate) fn greeting(mut self, lines: &[&str]) -> Self {
        self.greeting = lines.iter().map(|line| (*line).to_owned()).collect();
        self
    }

    /// Answers `method` with `response` (its `__ID__` replaced by the request's id) and then
    /// prints `notifications`.
    pub(crate) fn on(
        mut self,
        method: &str,
        response: Option<&str>,
        notifications: &[&str],
    ) -> Self {
        self.replies.push((
            method.to_owned(),
            response.map(ToOwned::to_owned),
            notifications
                .iter()
                .map(|line| (*line).to_owned())
                .collect(),
        ));
        self
    }

    /// Makes the peer ignore SIGTERM, so only SIGKILL ends it.
    pub(crate) fn ignoring_sigterm(mut self) -> Self {
        self.ignore_term = true;
        self.linger = true;
        self
    }

    /// Keeps the peer alive after its stdin closes.
    pub(crate) fn lingering(mut self) -> Self {
        self.linger = true;
        self
    }

    /// Writes the script and returns the command line to spawn it with.
    pub(crate) fn build(self) -> BuiltPeer {
        let root = self.directory.path().to_path_buf();
        let write = |name: &str, contents: &str| {
            let path = root.join(name);
            fs::write(&path, contents)
                .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
            path
        };
        if !self.greeting.is_empty() {
            write(
                "greeting.ndjson",
                &format!("{}\n", self.greeting.join("\n")),
            );
        }
        let mut cases = String::new();
        for (index, (method, response, notifications)) in self.replies.iter().enumerate() {
            if let Some(response) = response {
                write(&format!("resp_{index}.json"), &format!("{response}\n"));
            }
            if !notifications.is_empty() {
                write(
                    &format!("notify_{index}.ndjson"),
                    &format!("{}\n", notifications.join("\n")),
                );
            }
            cases.push_str(&format!("    '{method}')\n"));
            if response.is_some() {
                cases.push_str(&format!(
                    "      sed \"s/__ID__/$id/\" \"$DIR/resp_{index}.json\"\n"
                ));
            }
            if !notifications.is_empty() {
                cases.push_str(&format!("      cat \"$DIR/notify_{index}.ndjson\"\n"));
            }
            cases.push_str("      ;;\n");
        }
        let trap = if self.ignore_term {
            "trap '' TERM INT\n"
        } else {
            ""
        };
        let tail = if self.linger { "sleep 120\n" } else { "" };
        // `^{\"id\":` is anchored because a greedy `.*` would otherwise pick up an id nested in
        // params; both adapters write the envelope id first for exactly this reason.
        let script = format!(
            "#!/bin/sh\n\
             {trap}\
             DIR=\"$(dirname \"$0\")\"\n\
             if [ -f \"$DIR/greeting.ndjson\" ]; then cat \"$DIR/greeting.ndjson\"; fi\n\
             while IFS= read -r line; do\n\
             \x20 printf '%s\\n' \"$line\" >> \"$DIR/record.ndjson\"\n\
             \x20 id=$(printf '%s' \"$line\" | sed -n 's/^{{\"id\":\\([0-9][0-9]*\\).*/\\1/p')\n\
             \x20 method=$(printf '%s' \"$line\" | sed -n 's/.*\"method\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             \x20 subtype=$(printf '%s' \"$line\" | sed -n 's/.*\"subtype\":\"\\([^\"]*\\)\".*/\\1/p')\n\
             \x20 case \"$method$subtype\" in\n\
             {cases}\
             \x20 esac\n\
             done\n\
             {tail}"
        );
        let path = write("peer.sh", &script);
        BuiltPeer {
            _directory: self.directory,
            command: format!("/bin/sh {}", path.display()),
            record: root.join("record.ndjson"),
        }
    }
}

/// A written mock peer, ready to spawn.
pub(crate) struct BuiltPeer {
    _directory: TempDir,
    command: String,
    record: PathBuf,
}

impl BuiltPeer {
    /// The command line to hand to a [`super::HarnessConfig`].
    pub(crate) fn command(&self) -> String {
        self.command.clone()
    }

    /// Every frame Fleet has written so far, in order.
    pub(crate) fn recorded(&self) -> Vec<String> {
        fs::read_to_string(&self.record)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(ToOwned::to_owned)
            .collect()
    }

    /// Waits until `count` frames have been recorded, or the deadline expires.
    pub(crate) async fn wait_for_frames(
        &self,
        count: usize,
        within: std::time::Duration,
    ) -> Vec<String> {
        let deadline = std::time::Instant::now() + within;
        loop {
            let frames = self.recorded();
            if frames.len() >= count || std::time::Instant::now() > deadline {
                return frames;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
