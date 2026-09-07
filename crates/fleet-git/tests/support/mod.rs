use fleet_git::{ObjectId, Remote, Repository, Runner, SnapshotOptions};
use std::{path::Path, process::Command, sync::Arc};
use tempfile::TempDir;

pub struct TestRepo {
    directory: TempDir,
    pub repository: Repository,
}

impl TestRepo {
    pub async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "-b", "main"]);
        git(directory.path(), &["config", "user.name", "Fleet Test"]);
        git(
            directory.path(),
            &["config", "user.email", "fleet@example.test"],
        );
        git(directory.path(), &["config", "commit.gpgsign", "false"]);
        let repository = Repository::discover_with_runner(directory.path(), test_runner())
            .await
            .unwrap();
        Self {
            directory,
            repository,
        }
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn write(&self, path: &str, content: &str) {
        let path = self.path().join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    /// The remotes through the production path: a snapshot read, the only way the UI asks.
    pub async fn remotes(&self) -> Vec<Remote> {
        self.repository
            .snapshot(SnapshotOptions::default())
            .await
            .unwrap()
            .remotes
    }

    pub fn commit(&self, message: &str) -> ObjectId {
        git(self.path(), &["add", "-A"]);
        git(self.path(), &["commit", "-m", message]);
        ObjectId(
            git_output(self.path(), &["rev-parse", "HEAD"])
                .trim()
                .to_owned(),
        )
    }
}

pub fn test_runner() -> Arc<Runner> {
    Arc::new(
        Runner::default()
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1"),
    )
}

pub fn git(directory: &Path, arguments: &[&str]) {
    git_output(directory, arguments);
}

pub fn git_output(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args(arguments)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
