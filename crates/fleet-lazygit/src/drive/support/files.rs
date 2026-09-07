use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

pub(super) fn shot_paths(path: &Path, displays: usize) -> Vec<PathBuf> {
    let mut paths = vec![path.to_path_buf()];
    let stem = path
        .file_stem()
        .map_or_else(|| "shot".into(), |stem| stem.to_string_lossy().into_owned());
    let extension = path
        .extension()
        .map_or_else(|| "png".into(), |ext| ext.to_string_lossy().into_owned());
    for index in 2..=displays.max(1) {
        paths.push(path.with_file_name(format!("{stem}-{index}.{extension}")));
    }
    paths
}

const READ_BATCH_BYTES: u64 = 64 * 1024;

pub(super) fn log_path(script: &Path) -> PathBuf {
    let mut name = script.as_os_str().to_owned();
    name.push(".log");
    PathBuf::from(name)
}

pub(super) fn log_line(path: &Path, stamp: &str, message: &str) {
    let result = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{stamp} {message}"));
    if let Err(error) = result {
        tracing::warn!(path = %path.display(), %error, "drive: cannot append log");
    }
}

pub(super) struct Tail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl Tail {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            partial: Vec::new(),
        }
    }

    /// Reads at most 64 KiB per poll, retaining incomplete lines as bytes.
    pub(super) fn poll(&mut self) -> Vec<String> {
        let Ok(mut file) = File::open(&self.path) else {
            return Vec::new();
        };
        let Ok(length) = file.metadata().map(|meta| meta.len()) else {
            return Vec::new();
        };
        if length < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        if length == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let previous_len = self.partial.len();
        if let Err(error) = file.take(READ_BATCH_BYTES).read_to_end(&mut self.partial) {
            self.partial.truncate(previous_len);
            tracing::warn!(path = %self.path.display(), %error, "drive: cannot read script");
            return Vec::new();
        }
        self.offset += (self.partial.len() - previous_len) as u64;
        let Some(end) = self.partial[previous_len..]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|end| previous_len + end)
        else {
            return Vec::new();
        };
        let lines = self.partial[..end]
            .split(|byte| *byte == b'\n')
            .map(|line| {
                String::from_utf8_lossy(line)
                    .trim_end_matches(['\r', '\n'])
                    .to_owned()
            })
            .collect();
        self.partial.drain(..=end);
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Script(PathBuf);
    impl Script {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "fleet-drive-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::write(&path, []).expect("create script");
            Self(path)
        }
        fn append(&self, bytes: &[u8]) {
            OpenOptions::new()
                .append(true)
                .open(&self.0)
                .expect("open script")
                .write_all(bytes)
                .expect("append script");
        }
    }
    impl Drop for Script {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(log_path(&self.0));
        }
    }

    #[test]
    fn complete_lines_preserve_split_utf8_and_crlf() {
        let script = Script::new();
        let mut tail = Tail::new(script.0.clone());
        script.append(b"one\r\n\xc3");
        assert_eq!(tail.poll(), ["one"]);
        script.append(b"\xa9\nlast");
        assert_eq!(tail.poll(), ["é"]);
        assert!(tail.poll().is_empty());
        script.append(b" line\n");
        assert_eq!(tail.poll(), ["last line"]);
        std::fs::write(&script.0, b"new\n").expect("truncate script");
        assert_eq!(tail.poll(), ["new"]);
    }

    #[test]
    fn each_read_is_bounded_without_dropping_commands() {
        let script = Script::new();
        let data = "x\n".repeat(READ_BATCH_BYTES as usize);
        script.append(data.as_bytes());
        let mut tail = Tail::new(script.0.clone());
        assert_eq!(tail.poll().len(), READ_BATCH_BYTES as usize / 2);
        assert_eq!(tail.offset, READ_BATCH_BYTES);
        assert_eq!(tail.poll().len(), READ_BATCH_BYTES as usize / 2);
        assert!(tail.poll().is_empty());
    }

    #[test]
    fn screenshot_paths_and_log_bytes_keep_the_protocol() {
        assert_eq!(
            shot_paths(Path::new("/tmp/help.png"), 3),
            [
                PathBuf::from("/tmp/help.png"),
                PathBuf::from("/tmp/help-2.png"),
                PathBuf::from("/tmp/help-3.png")
            ]
        );
        let script = Script::new();
        let log = log_path(&script.0);
        log_line(&log, "12:34:56.789", "done shot /tmp/help.png");
        log_line(&log, "12345", "done key j");
        assert_eq!(
            std::fs::read_to_string(log).expect("read log"),
            "12:34:56.789 done shot /tmp/help.png\n12345 done key j\n"
        );
    }
}
