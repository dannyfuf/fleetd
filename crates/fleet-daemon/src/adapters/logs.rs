//! Bounded log reads and rotation with stable active paths.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

/// Reads only the suffix needed for `lines`, plus at most one scan block.
pub(crate) fn tail(path: &Path, lines: usize) -> io::Result<Vec<String>> {
    let mut file = File::open(path)?;
    if lines == 0 {
        return Ok(Vec::new());
    }
    let end = file.metadata()?.len();
    let mut position = end;
    let mut start = 0;
    let mut remaining = lines;
    let mut block = [0; 8192];
    'scan: while position > 0 {
        let length = position.min(block.len() as u64) as usize;
        position -= length as u64;
        file.seek(SeekFrom::Start(position))?;
        file.read_exact(&mut block[..length])?;
        for index in (0..length).rev() {
            let offset = position + index as u64;
            if block[index] == b'\n' && offset + 1 != end {
                remaining -= 1;
                if remaining == 0 {
                    start = offset + 1;
                    break 'scan;
                }
            }
        }
    }
    file.seek(SeekFrom::Start(start))?;
    let mut text = String::new();
    file.take(end - start).read_to_string(&mut text)?;
    Ok(text.lines().map(str::to_owned).collect())
}

/// Size-based log rotation that keeps the active filename unchanged.
/// Each write is kept intact; one oversized write can exceed the size limit.
pub struct RotatingLog {
    path: PathBuf,
    file: Option<File>,
    length: u64,
    max_bytes: u64,
    archives: usize,
}

impl RotatingLog {
    pub fn new(path: PathBuf, max_bytes: u64, archives: usize) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let length = file.metadata()?.len();
        Ok(Self {
            path,
            file: Some(file),
            length,
            max_bytes,
            archives,
        })
    }

    fn archive_path(&self, index: usize) -> PathBuf {
        let mut path = self.path.as_os_str().to_owned();
        path.push(format!(".{index}"));
        PathBuf::from(path)
    }

    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        if self.archives == 0 {
            fs::remove_file(&self.path)?;
        } else {
            for index in (1..self.archives).rev() {
                match fs::rename(self.archive_path(index), self.archive_path(index + 1)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            fs::rename(&self.path, self.archive_path(1))?;
        }
        self.length = 0;
        Ok(())
    }
}

impl Write for RotatingLog {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.length > 0 && self.length.saturating_add(bytes.len() as u64) > self.max_bytes {
            self.rotate()?;
        }
        let file = match &mut self.file {
            Some(file) => file,
            slot @ None => slot.insert(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            ),
        };
        let written = file.write(bytes)?;
        self.length += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_matches_lines_for_newlines_unicode_and_long_lines() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("job.log");
        for text in [
            "".to_owned(),
            "a".to_owned(),
            "a\n".to_owned(),
            "a\r\nb\r\n\n".to_owned(),
            format!("{}\n終わり\n", "é".repeat(20_000)),
        ] {
            fs::write(&path, &text).expect("write log");
            let all: Vec<_> = text.lines().map(str::to_owned).collect();
            for lines in 0..=5 {
                assert_eq!(
                    tail(&path, lines).expect("tail"),
                    all[all.len().saturating_sub(lines)..]
                );
            }
        }
    }

    #[test]
    fn short_tail_does_not_read_the_large_prefix() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("job.log");
        let mut file = File::create(&path).expect("create log");
        // A sparse prefix is intentionally invalid UTF-8: a whole-file text read would fail.
        file.write_all(&[0xff]).expect("invalid prefix");
        file.seek(SeekFrom::Start(64 * 1024 * 1024)).expect("seek");
        file.write_all(b"\nfirst\nlast\n").expect("suffix");
        assert_eq!(tail(&path, 2).expect("tail"), ["first", "last"]);
    }

    #[test]
    fn rotation_bounds_archives_and_preserves_the_active_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("fleetd.log");
        let mut log = RotatingLog::new(path.clone(), 4, 2).expect("open log");
        for line in [b"one\n", b"two\n", b"tri\n", b"end\n"] {
            log.write_all(line).expect("write");
        }
        log.flush().expect("flush");
        assert_eq!(fs::read_to_string(path).expect("active log"), "end\n");
        assert_eq!(
            fs::read_to_string(log.archive_path(1)).expect("first archive"),
            "tri\n"
        );
        assert_eq!(
            fs::read_to_string(log.archive_path(2)).expect("second archive"),
            "two\n"
        );
        assert_eq!(fs::read_dir(temp.path()).expect("list logs").count(), 3);
    }
}
