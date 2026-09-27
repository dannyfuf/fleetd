//! App-side media payload parsing, manifest construction and shell-safe path insertion.

use std::{fs, path::Path, path::PathBuf};

use anyhow::{Context as _, bail};
use fleet_proto::{
    media::{MAX_UPLOAD_BYTES, MAX_UPLOAD_FILES},
    request::{StageEntry, StagedFile},
};
use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths, ImageFormat};

mod upload;

pub(crate) use upload::UploadRegistry;
pub use upload::{StageOutcome, UploadProgress, UploadState, cancel, progress, stage};

/// A validated protocol manifest paired with the local files that supply its bytes.
#[derive(Debug)]
pub(crate) struct PreparedManifest {
    pub(crate) entry: StageEntry,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) total_bytes: u64,
    pub(crate) entry_count: usize,
    pub(crate) skipped: usize,
}

/// A clipboard or file-drop payload ready for media staging.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Attachment {
    /// Existing paths on the app's machine, in source order.
    Paths(Vec<PathBuf>),
    /// Image bytes supplied directly by the platform clipboard.
    Blob {
        /// Stable filename derived from the clipboard image format.
        name: String,
        /// Platform-reported format; the bytes are deliberately not sniffed.
        format: ImageFormat,
        /// Opaque encoded image bytes.
        bytes: Vec<u8>,
    },
}

/// Extracts a non-text attachment while leaving text-only paste to the existing paste route.
///
/// macOS file copies contain both external paths and a textual fallback, so paths must be
/// selected before images regardless of entry order. Linux file copies arrive as text and are
/// intentionally left untouched because GPUI does not identify them as external paths.
pub fn from_clipboard(item: &ClipboardItem) -> Option<Attachment> {
    item.entries()
        .iter()
        .find_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => Some(from_external_paths(paths)),
            ClipboardEntry::String(_) | ClipboardEntry::Image(_) => None,
        })
        .or_else(|| {
            item.entries().iter().find_map(|entry| match entry {
                ClipboardEntry::Image(image) => Some(Attachment::Blob {
                    name: format!("clipboard.{}", image.format.extension()),
                    format: image.format,
                    bytes: image.bytes.clone(),
                }),
                ClipboardEntry::String(_) | ClipboardEntry::ExternalPaths(_) => None,
            })
        })
}

/// Converts a platform file-drop payload without changing path order.
pub fn from_external_paths(paths: &ExternalPaths) -> Attachment {
    Attachment::Paths(paths.paths().to_vec())
}

/// MIME type reported for an image supplied directly by the platform clipboard.
#[must_use]
pub const fn media_type_for_image(format: ImageFormat) -> &'static str {
    format.mime_type()
}

/// MIME type inferred from a file extension for native-agent attachments.
///
/// The table is intentionally the four image types both providers agree on. Everything else is
/// an opaque file, which also selects the daemon's 50 MiB non-image limit.
#[must_use]
pub fn media_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("gif") => "image/gif",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Whether the daemon treats this MIME type as an image attachment.
#[must_use]
pub const fn is_agent_image_type(media_type: &str) -> bool {
    matches!(
        media_type.as_bytes(),
        b"image/gif" | b"image/jpeg" | b"image/png" | b"image/webp"
    )
}

/// Builds the protocol manifest for one file or directory.
///
/// This performs blocking filesystem work and must be called on GPUI's background executor.
/// Directory traversal never follows symlinks. Symlinks and other special entries below the
/// root are counted and skipped, while a special top-level path is rejected. Limits are checked
/// as entries are discovered, before an upload request can be constructed.
pub fn manifest(path: &Path) -> anyhow::Result<StageEntry> {
    Ok(prepare_manifest(path)?.entry)
}

pub(crate) fn prepare_manifest(path: &Path) -> anyhow::Result<PreparedManifest> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect media path {}", path.display()))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        bail!("cannot stage symbolic link {}", path.display());
    }

    let name = path_name(path)?;
    if file_type.is_file() {
        ensure_upload_size(0, metadata.len())?;
        return Ok(PreparedManifest {
            entry: StageEntry::File {
                name,
                size: metadata.len(),
            },
            files: vec![path.to_path_buf()],
            total_bytes: metadata.len(),
            entry_count: 1,
            skipped: 0,
        });
    }
    if !file_type.is_dir() {
        bail!(
            "cannot stage {} because it is not a regular file or directory",
            path.display()
        );
    }

    let scan = scan_directory(path)?;
    let sources = scan
        .files
        .iter()
        .map(|file| path.join(Path::new(&file.relative)))
        .collect();
    Ok(PreparedManifest {
        entry: StageEntry::Directory {
            name,
            files: scan.files,
            dirs: scan.dirs,
        },
        files: sources,
        total_bytes: scan.total_bytes,
        entry_count: scan.entries_seen,
        skipped: scan.skipped,
    })
}

/// Quotes paths as independent shell words and preserves their source order.
#[must_use]
pub fn insert_text(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| {
            let path = path.to_string_lossy();
            shell_words::quote(path.as_ref()).into_owned()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

struct DirectoryScan {
    files: Vec<StagedFile>,
    dirs: Vec<String>,
    total_bytes: u64,
    entries_seen: usize,
    skipped: usize,
}

fn scan_directory(root: &Path) -> anyhow::Result<DirectoryScan> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    let mut entries_seen = 0_usize;
    let mut total_bytes = 0_u64;
    let mut skipped = 0_usize;

    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .with_context(|| format!("read media directory {}", directory.display()))?
            .collect::<Result<Vec<_>, std::io::Error>>()
            .with_context(|| format!("read entries in media directory {}", directory.display()))?;
        entries.sort_by_key(fs::DirEntry::file_name);

        let mut child_directories = Vec::new();
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("inspect media entry {}", path.display()))?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                skipped = skipped.saturating_add(1);
                continue;
            }

            let relative = path
                .strip_prefix(root)
                .with_context(|| format!("resolve media entry {}", path.display()))?;
            let relative = protocol_relative_path(relative)?;
            if file_type.is_dir() {
                record_manifest_entry(&mut entries_seen)?;
                dirs.push(relative);
                child_directories.push(path);
            } else if file_type.is_file() {
                record_manifest_entry(&mut entries_seen)?;
                total_bytes = ensure_upload_size(total_bytes, metadata.len())?;
                files.push(StagedFile {
                    relative,
                    size: metadata.len(),
                });
            } else {
                skipped = skipped.saturating_add(1);
            }
        }

        // The stack is LIFO; reversing preserves the sorted directory order while walking.
        pending.extend(child_directories.into_iter().rev());
    }

    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    dirs.sort();
    Ok(DirectoryScan {
        files,
        dirs,
        total_bytes,
        entries_seen,
        skipped,
    })
}

fn record_manifest_entry(entries_seen: &mut usize) -> anyhow::Result<()> {
    if *entries_seen >= MAX_UPLOAD_FILES {
        bail!(drop_entry_limit_message());
    }
    *entries_seen += 1;
    Ok(())
}

fn ensure_upload_size(total: u64, size: u64) -> anyhow::Result<u64> {
    let Some(total) = total.checked_add(size) else {
        bail!(drop_size_limit_message());
    };
    if total > MAX_UPLOAD_BYTES {
        bail!(drop_size_limit_message());
    }
    Ok(total)
}

pub(crate) fn drop_size_limit_message() -> String {
    "This drop is larger than the 1 GiB limit".to_owned()
}

pub(crate) fn drop_entry_limit_message() -> String {
    "This drop has more than 10,000 files and folders".to_owned()
}

fn path_name(path: &Path) -> anyhow::Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("media path {} has no UTF-8 file name", path.display()))
}

fn protocol_relative_path(path: &Path) -> anyhow::Result<String> {
    let mut relative = String::new();
    for component in path.components() {
        let std::path::Component::Normal(component) = component else {
            bail!("media path {} is not relative", path.display());
        };
        let component = component
            .to_str()
            .with_context(|| format!("media path {} is not valid UTF-8", path.display()))?;
        if !relative.is_empty() {
            relative.push('/');
        }
        relative.push_str(component);
    }
    if relative.is_empty() {
        bail!("media path is empty");
    }
    Ok(relative)
}

#[cfg(test)]
mod tests {
    use std::{fs::File, path::PathBuf};

    use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths, Image, ImageFormat};

    use super::*;

    #[test]
    fn clipboard_prefers_external_paths_over_images_and_text() {
        let paths =
            ExternalPaths(vec![PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")].into());
        let image = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]);
        let item = ClipboardItem {
            entries: vec![
                ClipboardEntry::Image(image),
                ClipboardEntry::String("fallback".to_owned().into()),
                ClipboardEntry::ExternalPaths(paths),
            ],
        };

        assert_eq!(
            from_clipboard(&item),
            Some(Attachment::Paths(vec![
                PathBuf::from("/tmp/one"),
                PathBuf::from("/tmp/two"),
            ]))
        );
    }

    #[test]
    fn clipboard_image_keeps_reported_format_and_opaque_bytes() {
        let item = ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Tiff, vec![9, 8, 7]));

        assert_eq!(
            from_clipboard(&item),
            Some(Attachment::Blob {
                name: "clipboard.tiff".to_owned(),
                format: ImageFormat::Tiff,
                bytes: vec![9, 8, 7],
            })
        );
    }

    #[test]
    fn text_only_clipboard_is_left_byte_identical_for_the_existing_route() {
        let original = "first line\r\nsecond line\0tail";
        let item = ClipboardItem::new_string(original.to_owned());

        assert_eq!(from_clipboard(&item), None);
        assert_eq!(item.text().as_deref(), Some(original));
    }

    #[test]
    fn wayland_file_uri_text_stays_on_the_text_paste_route() {
        let original = "file:///tmp/fleet-one.png\nfile:///tmp/fleet-folder/\n";
        let item = ClipboardItem::new_string(original.to_owned());

        assert_eq!(from_clipboard(&item), None);
        assert_eq!(item.text().as_deref(), Some(original));
    }

    #[test]
    fn external_paths_keep_source_order() {
        let paths = ExternalPaths(vec![PathBuf::from("second"), PathBuf::from("first")].into());

        assert_eq!(
            from_external_paths(&paths),
            Attachment::Paths(vec![PathBuf::from("second"), PathBuf::from("first")])
        );
    }

    #[test]
    fn agent_media_types_use_the_fixed_provider_table() {
        assert_eq!(media_type_for_image(ImageFormat::Png), "image/png");
        assert_eq!(media_type_for_image(ImageFormat::Tiff), "image/tiff");
        assert_eq!(media_type_for_path(Path::new("photo.PNG")), "image/png");
        assert_eq!(media_type_for_path(Path::new("photo.jpeg")), "image/jpeg");
        assert_eq!(
            media_type_for_path(Path::new("archive.zip")),
            "application/octet-stream"
        );
        assert!(is_agent_image_type("image/webp"));
        assert!(!is_agent_image_type("image/tiff"));
    }

    #[test]
    fn inserted_paths_are_individually_shell_quoted() {
        let paths = vec![
            PathBuf::from("/tmp/plain"),
            PathBuf::from("/tmp/with space"),
            PathBuf::from("/tmp/it's-here"),
        ];

        assert_eq!(
            insert_text(&paths),
            "/tmp/plain '/tmp/with space' '/tmp/it'\\''s-here'"
        );
    }

    #[test]
    fn file_manifest_records_name_and_size() {
        let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = temporary.path().join("photo.png");
        fs::write(&path, [1, 2, 3, 4]).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            manifest(&path).unwrap_or_else(|error| panic!("{error}")),
            StageEntry::File {
                name: "photo.png".to_owned(),
                size: 4,
            }
        );
    }

    #[test]
    fn directory_manifest_is_sorted_and_preserves_empty_directories() {
        let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let root = temporary.path().join("folder");
        fs::create_dir_all(root.join("nested/deeper")).unwrap_or_else(|error| panic!("{error}"));
        fs::create_dir_all(root.join("empty")).unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.join("z.txt"), [1]).unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.join("nested/deeper/a.txt"), [1, 2])
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            manifest(&root).unwrap_or_else(|error| panic!("{error}")),
            StageEntry::Directory {
                name: "folder".to_owned(),
                files: vec![
                    StagedFile {
                        relative: "nested/deeper/a.txt".to_owned(),
                        size: 2,
                    },
                    StagedFile {
                        relative: "z.txt".to_owned(),
                        size: 1,
                    },
                ],
                dirs: vec![
                    "empty".to_owned(),
                    "nested".to_owned(),
                    "nested/deeper".to_owned(),
                ],
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn directory_manifest_skips_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let root = temporary.path().join("folder");
        let outside = temporary.path().join("outside");
        fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
        fs::create_dir_all(&outside).unwrap_or_else(|error| panic!("{error}"));
        fs::write(root.join("target.txt"), [1, 2, 3]).unwrap_or_else(|error| panic!("{error}"));
        fs::write(outside.join("must-not-appear.txt"), [4, 5, 6])
            .unwrap_or_else(|error| panic!("{error}"));
        symlink(root.join("target.txt"), root.join("linked.txt"))
            .unwrap_or_else(|error| panic!("{error}"));
        symlink(&outside, root.join("linked-directory")).unwrap_or_else(|error| panic!("{error}"));

        let scan = scan_directory(&root).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            scan.files,
            vec![StagedFile {
                relative: "target.txt".to_owned(),
                size: 3,
            }]
        );
        assert!(scan.dirs.is_empty());
        assert_eq!(scan.skipped, 2);
    }

    #[test]
    fn directory_manifest_refuses_the_upload_size_limit_before_reading_bytes() {
        let temporary = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let root = temporary.path().join("folder");
        fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
        let oversized = root.join("oversized.bin");
        File::create(&oversized)
            .and_then(|file| file.set_len(MAX_UPLOAD_BYTES + 1))
            .unwrap_or_else(|error| panic!("{error}"));

        let error = manifest(&root).expect_err("an oversized directory must be rejected");
        assert!(error.to_string().contains("1 GiB"));
    }

    #[test]
    fn manifest_entry_limit_is_enforced_at_the_boundary() {
        let mut entries_seen = MAX_UPLOAD_FILES;

        let error = record_manifest_entry(&mut entries_seen)
            .expect_err("one entry beyond the maximum must be rejected");
        assert_eq!(
            error.to_string(),
            "This drop has more than 10,000 files and folders"
        );
    }
}
