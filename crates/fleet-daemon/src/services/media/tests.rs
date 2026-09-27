use chrono::{DateTime, TimeZone as _, Utc};
use fleet_proto::{error::ErrorKind, media::encode};
use sha2::{Digest as _, Sha256};

use super::sweep::DOWNLOAD_RETENTION;
use super::*;
use crate::testing::fakes::{FakeFiles, FakeFilesCall, FixedClock};

const DOWNLOADS: &str = "/home/test/Downloads/fleet";
const ATTACHMENTS: &str = "/fleet/agents/attachments";

fn fixture() -> (Media, Arc<FakeFiles>) {
    let (media, files, _clock) = fixture_at(
        Utc.with_ymd_and_hms(2026, 9, 24, 12, 34, 56)
            .single()
            .expect("valid test timestamp"),
    );
    (media, files)
}

fn fixture_at(now: DateTime<Utc>) -> (Media, Arc<FakeFiles>, Arc<FixedClock>) {
    let files = Arc::new(FakeFiles::new(
        PathBuf::from("/trash"),
        vec![PathBuf::from(DOWNLOADS), PathBuf::from(ATTACHMENTS)],
    ));
    let clock = Arc::new(FixedClock::new(now));
    (
        Media::with_roots(
            Arc::<FakeFiles>::clone(&files),
            Arc::<FixedClock>::clone(&clock),
            PathBuf::from(DOWNLOADS),
            PathBuf::from(ATTACHMENTS),
        ),
        files,
        clock,
    )
}

fn digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    super::hex_for_test(&digest)
}

async fn begin(media: &Media, id: UploadId, anchor: MediaAnchor, entry: StageEntry) {
    assert_eq!(
        media
            .stage(anchor, id, StageOp::Begin { entry })
            .await
            .expect("begin upload"),
        ResponseBody::Ack
    );
}

async fn part_path_for(media: &Media, id: UploadId) -> PathBuf {
    let slot = media.upload(id).await.expect("live upload");
    let upload = slot.upload.lock().await;
    match &upload.state {
        UploadState::Active(prepared) => prepared.part_path.clone(),
        UploadState::Preparing | UploadState::Completed { .. } | UploadState::Closed => {
            panic!("upload is not active")
        }
    }
}

#[tokio::test]
async fn out_of_order_and_duplicate_chunks_publish_only_after_sha256_verification() {
    let (media, files) = fixture();
    let id = UploadId::new();
    let mut contents = vec![b'a'; CHUNK_BYTES];
    contents.extend_from_slice(b"end");
    begin(
        &media,
        id,
        MediaAnchor::Local,
        StageEntry::File {
            name: "design.png".to_owned(),
            size: contents.len() as u64,
        },
    )
    .await;
    media
        .stage(
            MediaAnchor::Local,
            id,
            StageOp::Chunk {
                file: 0,
                offset: CHUNK_BYTES as u64,
                data: encode(b"end"),
            },
        )
        .await
        .expect("tail chunk");
    media
        .stage(
            MediaAnchor::Local,
            id,
            StageOp::Chunk {
                file: 0,
                offset: 0,
                data: encode(&contents[..CHUNK_BYTES]),
            },
        )
        .await
        .expect("first chunk");
    media
        .stage(
            MediaAnchor::Local,
            id,
            StageOp::Chunk {
                file: 0,
                offset: 0,
                data: encode(&vec![b'x'; CHUNK_BYTES]),
            },
        )
        .await
        .expect("duplicate chunk is idempotent");

    let response = media
        .stage(
            MediaAnchor::Local,
            id,
            StageOp::Finish {
                sha256: vec![digest(&contents)],
            },
        )
        .await
        .expect("finish upload");
    let replay = media
        .stage(
            MediaAnchor::Local,
            id,
            StageOp::Finish {
                sha256: vec![digest(&contents)],
            },
        )
        .await
        .expect("a lost finish response can be replayed");
    assert_eq!(replay, response);
    let ResponseBody::Path { path, host: None } = response else {
        panic!("finish must return a local path")
    };
    assert_eq!(path, format!("{DOWNLOADS}/20260924-123456-design.png"));
    assert_eq!(
        files.bytes(Path::new(&path)).as_deref(),
        Some(contents.as_slice())
    );
}

#[tokio::test]
async fn missing_chunks_are_retryable_but_a_wrong_digest_removes_the_part() {
    let (media, files) = fixture();
    let id = UploadId::new();
    begin(
        &media,
        id,
        MediaAnchor::Local,
        StageEntry::File {
            name: "note.txt".to_owned(),
            size: 3,
        },
    )
    .await;
    let part = part_path_for(&media, id).await;
    let missing = media
        .stage(
            MediaAnchor::Local,
            id,
            StageOp::Finish {
                sha256: vec![digest(b"abc")],
            },
        )
        .await
        .expect_err("missing chunk");
    assert!(
        matches!(missing, DaemonError::Validation(message) if message.contains("missing chunk"))
    );
    assert!(files.exists(&part));
    media
        .stage(
            MediaAnchor::Local,
            id,
            StageOp::Chunk {
                file: 0,
                offset: 0,
                data: encode(b"abc"),
            },
        )
        .await
        .expect("chunk");
    let mismatch = media
        .stage(
            MediaAnchor::Local,
            id,
            StageOp::Finish {
                sha256: vec!["0".repeat(64)],
            },
        )
        .await
        .expect_err("digest mismatch");
    assert!(
        matches!(mismatch, DaemonError::Validation(message) if message.contains("SHA-256 mismatch"))
    );
    assert!(!files.exists(&part));
    let unknown = media
        .stage(MediaAnchor::Local, id, StageOp::Finish { sha256: vec![] })
        .await
        .expect_err("rejected upload was forgotten");
    assert_eq!(
        fleet_proto::error::ProtoError::from(unknown).kind,
        ErrorKind::NotFound
    );
}

#[tokio::test]
async fn repeated_begin_and_unknown_cancel_are_idempotent() {
    let (media, files) = fixture();
    let id = UploadId::new();
    let entry = StageEntry::File {
        name: "same.bin".to_owned(),
        size: 1,
    };
    begin(&media, id, MediaAnchor::Local, entry.clone()).await;
    let part = part_path_for(&media, id).await;
    begin(&media, id, MediaAnchor::Local, entry).await;
    assert_eq!(
        files
            .calls()
            .iter()
            .filter(|call| matches!(call, FakeFilesCall::CreatePart(_, _)))
            .count(),
        1
    );
    assert_eq!(
        media
            .stage(MediaAnchor::Local, id, StageOp::Cancel)
            .await
            .expect("cancel live upload"),
        ResponseBody::Ack
    );
    assert!(!files.exists(&part));
    assert_eq!(
        media
            .stage(MediaAnchor::Local, UploadId::new(), StageOp::Cancel)
            .await
            .expect("unknown cancel"),
        ResponseBody::Ack
    );
}

#[tokio::test]
async fn one_connection_cannot_exceed_the_live_upload_budget() {
    let (media, _files) = fixture();
    let owner = 41;
    let mut uploads = Vec::new();
    for index in 0..MAX_ACTIVE_UPLOADS_PER_OWNER {
        let id = UploadId::new();
        media
            .stage_owned(
                owner,
                MediaAnchor::Local,
                id,
                StageOp::Begin {
                    entry: StageEntry::File {
                        name: format!("held-{index}.bin"),
                        size: 0,
                    },
                },
            )
            .await
            .expect("upload inside the owner budget");
        uploads.push(id);
    }

    let overflow = media
        .stage_owned(
            owner,
            MediaAnchor::Local,
            UploadId::new(),
            StageOp::Begin {
                entry: StageEntry::File {
                    name: "overflow.bin".to_owned(),
                    size: 0,
                },
            },
        )
        .await
        .expect_err("the seventeenth live upload is refused");
    assert!(matches!(
        overflow,
        DaemonError::Validation(message) if message.contains("16 active media uploads")
    ));

    media
        .stage_owned(
            owner + 1,
            MediaAnchor::Local,
            UploadId::new(),
            StageOp::Begin {
                entry: StageEntry::File {
                    name: "other-owner.bin".to_owned(),
                    size: 0,
                },
            },
        )
        .await
        .expect("another connection has an independent budget");
    media
        .stage_owned(owner, MediaAnchor::Local, uploads[0], StageOp::Cancel)
        .await
        .expect("cancel frees one slot");
    media
        .stage_owned(
            owner,
            MediaAnchor::Local,
            UploadId::new(),
            StageOp::Begin {
                entry: StageEntry::File {
                    name: "replacement.bin".to_owned(),
                    size: 0,
                },
            },
        )
        .await
        .expect("the freed slot can be reused");
}

#[tokio::test]
async fn concurrent_same_name_begins_reserve_distinct_paths() {
    let (media, _files) = fixture();
    let first = UploadId::new();
    let second = UploadId::new();
    let entry = StageEntry::Directory {
        name: "same".to_owned(),
        files: Vec::new(),
        dirs: Vec::new(),
    };

    let (first_begin, second_begin) = tokio::join!(
        media.stage_owned(
            1,
            MediaAnchor::Local,
            first,
            StageOp::Begin {
                entry: entry.clone()
            }
        ),
        media.stage_owned(2, MediaAnchor::Local, second, StageOp::Begin { entry })
    );
    first_begin.expect("first begin");
    second_begin.expect("second begin");

    let first_path = part_path_for(&media, first).await;
    let second_path = part_path_for(&media, second).await;
    assert_ne!(first_path, second_path);
}

#[tokio::test(start_paused = true)]
async fn idle_expiry_removes_the_part_with_virtual_time() {
    let (media, files) = fixture();
    let id = UploadId::new();
    begin(
        &media,
        id,
        MediaAnchor::Local,
        StageEntry::File {
            name: "idle.bin".to_owned(),
            size: 8,
        },
    )
    .await;
    let part = part_path_for(&media, id).await;
    tokio::time::advance(UPLOAD_IDLE_EXPIRY + std::time::Duration::from_secs(1)).await;
    assert_eq!(media.expire_idle().await.expect("expire"), 1);
    assert!(!files.exists(&part));
}

#[tokio::test(start_paused = true)]
async fn idle_expiry_continues_after_one_part_cleanup_fails() {
    let (media, files) = fixture();
    let first = UploadId::new();
    let second = UploadId::new();
    for id in [first, second] {
        begin(
            &media,
            id,
            MediaAnchor::Local,
            StageEntry::File {
                name: format!("{id}.bin"),
                size: 1,
            },
        )
        .await;
    }
    let first_part = part_path_for(&media, first).await;
    let second_part = part_path_for(&media, second).await;
    files.fail_next(
        FakeFilesCall::RemovePart(PathBuf::from(DOWNLOADS), first_part.clone()),
        std::io::ErrorKind::PermissionDenied,
    );

    tokio::time::advance(UPLOAD_IDLE_EXPIRY + std::time::Duration::from_secs(1)).await;

    assert_eq!(media.expire_idle().await.expect("expire"), 2);
    assert!(
        files.exists(&first_part),
        "failed cleanup remains for the sweep"
    );
    assert!(!files.exists(&second_part), "later cleanup still runs");
}

#[tokio::test(start_paused = true)]
async fn sweep_removes_only_old_safe_stamped_download_children() {
    let old = Utc
        .with_ymd_and_hms(2026, 9, 10, 8, 0, 0)
        .single()
        .expect("valid old timestamp");
    let now = Utc
        .with_ymd_and_hms(2026, 9, 24, 12, 34, 56)
        .single()
        .expect("valid current timestamp");
    let (media, files, clock) = fixture_at(old);
    let live_id = UploadId::new();
    begin(
        &media,
        live_id,
        MediaAnchor::Local,
        StageEntry::File {
            name: "still-uploading.bin".to_owned(),
            size: 1,
        },
    )
    .await;
    let live_part = part_path_for(&media, live_id).await;
    clock.set(now);

    let downloads = PathBuf::from(DOWNLOADS);
    let user_file = downloads.join("notes.txt");
    let user_directory = downloads.join("personal");
    let stamped_symlink = downloads.join("20260910-080000-outside-link");
    let unsafe_directory = downloads.join("20260910-080000-unsafe-folder");
    let staged_file = downloads.join("20260910-080000-design.png");
    let staged_folder = downloads.join("20260910-080000-bundle");
    let orphan_part = downloads.join("20260922-080000-interrupted.bin.part");
    let fresh_staged_file = downloads.join("20260923-080000-recent.png");
    let thread_file = PathBuf::from(ATTACHMENTS)
        .join(ThreadId::new().to_string())
        .join("20260910-080000-attachment.png");

    files.insert_text(&user_file, "keep");
    files
        .create_dir_all(&user_directory)
        .expect("seed user directory");
    files.insert_text(user_directory.join("20260910-080000-nested.txt"), "keep");
    files.insert_other(&stamped_symlink);
    files
        .create_dir_all(&unsafe_directory)
        .expect("seed unsafe staged directory");
    files.insert_text(unsafe_directory.join("ordinary.txt"), "keep");
    files.insert_other(unsafe_directory.join("outside-link"));
    files.insert_text(&staged_file, "stale");
    files
        .create_dir_all(&staged_folder.join("nested/empty"))
        .expect("seed staged folder");
    files.insert_text(staged_folder.join("nested/file.txt"), "stale");
    files.insert_text(&orphan_part, "partial");
    files.insert_text(&fresh_staged_file, "fresh");
    files.insert_text(&thread_file, "attachment");

    assert_eq!(
        media
            .sweep(DOWNLOAD_RETENTION)
            .await
            .expect("sweep staged downloads"),
        3
    );
    for removed in [&staged_file, &staged_folder, &orphan_part] {
        assert!(
            !files.exists(removed),
            "{} should be removed",
            removed.display()
        );
    }
    for kept in [
        &user_file,
        &user_directory,
        &stamped_symlink,
        &unsafe_directory,
        &fresh_staged_file,
        &live_part,
        &thread_file,
    ] {
        assert!(files.exists(kept), "{} should survive", kept.display());
    }
}

#[tokio::test]
async fn sweep_continues_after_candidate_inspection_and_removal_failures() {
    let old = Utc
        .with_ymd_and_hms(2026, 9, 10, 8, 0, 0)
        .single()
        .expect("valid old timestamp");
    let now = Utc
        .with_ymd_and_hms(2026, 9, 24, 12, 34, 56)
        .single()
        .expect("valid current timestamp");
    let (media, files, clock) = fixture_at(old);
    clock.set(now);
    let downloads = PathBuf::from(DOWNLOADS);
    let unreadable = downloads.join("20260910-080000-a.bin");
    let unremovable = downloads.join("20260910-080000-b.bin");
    let removable = downloads.join("20260910-080000-c.bin");
    for path in [&unreadable, &unremovable, &removable] {
        files.insert_text(path, "stale");
    }
    files.fail_next(
        FakeFilesCall::Metadata(unreadable.clone()),
        std::io::ErrorKind::PermissionDenied,
    );
    files.fail_next(
        FakeFilesCall::RemovePart(downloads.clone(), unremovable.clone()),
        std::io::ErrorKind::PermissionDenied,
    );

    assert_eq!(
        media
            .sweep(DOWNLOAD_RETENTION)
            .await
            .expect("sweep staged downloads"),
        1
    );
    assert!(files.exists(&unreadable));
    assert!(files.exists(&unremovable));
    assert!(!files.exists(&removable));
}

#[tokio::test]
async fn a_directory_manifest_recreates_files_and_nested_empty_directories() {
    let (media, files) = fixture();
    let id = UploadId::new();
    begin(
        &media,
        id,
        MediaAnchor::Local,
        StageEntry::Directory {
            name: "bundle".to_owned(),
            files: vec![StagedFile {
                relative: "images/icon.png".to_owned(),
                size: 4,
            }],
            dirs: vec!["empty/nested".to_owned()],
        },
    )
    .await;
    media
        .stage(
            MediaAnchor::Local,
            id,
            StageOp::Chunk {
                file: 0,
                offset: 0,
                data: encode(b"icon"),
            },
        )
        .await
        .expect("folder chunk");
    let response = media
        .stage(
            MediaAnchor::Local,
            id,
            StageOp::Finish {
                sha256: vec![digest(b"icon")],
            },
        )
        .await
        .expect("folder finish");
    let ResponseBody::Path { path, .. } = response else {
        panic!("folder finish path")
    };
    let root = PathBuf::from(path);
    assert_eq!(
        files.bytes(&root.join("images/icon.png")).as_deref(),
        Some(b"icon".as_slice())
    );
    assert!(files.exists(&root.join("empty/nested")));
}

#[tokio::test]
async fn thread_uploads_use_the_uuid_leaf_and_are_confined_to_it() {
    let (media, files) = fixture();
    let thread = ThreadId::new();
    let id = UploadId::new();
    begin(
        &media,
        id,
        MediaAnchor::Thread { thread },
        StageEntry::File {
            name: "../../outside\n\0.png".to_owned(),
            size: 0,
        },
    )
    .await;
    let response = media
        .stage(
            MediaAnchor::Thread { thread },
            id,
            StageOp::Finish {
                sha256: vec![digest(b"")],
            },
        )
        .await
        .expect("finish empty attachment");
    let ResponseBody::Path { path, .. } = response else {
        panic!("thread path")
    };
    let leaf = PathBuf::from(ATTACHMENTS).join(thread.to_string());
    assert!(Path::new(&path).starts_with(&leaf));
    assert!(files.exists(Path::new(&path)));
    assert!(!files.exists(Path::new("/outside.png")));
}

#[tokio::test]
async fn crafted_top_level_names_stay_inside_download_and_thread_roots() {
    let names = [
        "../../etc/passwd".to_owned(),
        "foo/bar.png".to_owned(),
        format!("{}.png", "x".repeat(300)),
        String::new(),
        "...".to_owned(),
        "line\nname.png".to_owned(),
        "nul\0name.png".to_owned(),
    ];
    for thread_target in [false, true] {
        let (media, files) = fixture();
        let thread = ThreadId::new();
        let (anchor, root) = if thread_target {
            (
                MediaAnchor::Thread { thread },
                PathBuf::from(ATTACHMENTS).join(thread.to_string()),
            )
        } else {
            (MediaAnchor::Local, PathBuf::from(DOWNLOADS))
        };
        for name in &names {
            let id = UploadId::new();
            begin(
                &media,
                id,
                anchor.clone(),
                StageEntry::File {
                    name: name.clone(),
                    size: 0,
                },
            )
            .await;
            let response = media
                .stage(
                    anchor.clone(),
                    id,
                    StageOp::Finish {
                        sha256: vec![digest(b"")],
                    },
                )
                .await
                .expect("finish crafted name");
            let ResponseBody::Path { path, .. } = response else {
                panic!("finish path")
            };
            assert!(Path::new(&path).starts_with(&root), "{path}");
            assert!(files.exists(Path::new(&path)), "{path}");
        }
    }
}

#[test]
fn sanitiser_rejects_unsafe_relative_paths_and_collisions() {
    for relative in ["", ".", "..", "../../etc/passwd", "/etc/passwd", "a//b"] {
        assert!(sanitize_relative(relative).is_err(), "{relative:?}");
    }
    assert_eq!(sanitize_top_name(""), "paste");
    assert_eq!(sanitize_top_name("..."), "paste");
    assert_eq!(sanitize_top_name("foo/bar\n\0.png"), "foobar.png");
    assert_eq!(
        sanitize_relative("line\n/nul\0name.png").expect("sanitised relative path"),
        PathBuf::from("line/nulname.png")
    );
    let long = format!("{}.png", "x".repeat(300));
    let sanitized = sanitize_top_name(&long);
    assert_eq!(sanitized.chars().count(), MAX_COMPONENT_CHARS);
    assert!(sanitized.ends_with(".png"));

    let collision = StageEntry::Directory {
        name: "folder".to_owned(),
        files: vec![
            StagedFile {
                relative: "a\n.png".to_owned(),
                size: 0,
            },
            StagedFile {
                relative: "a.png".to_owned(),
                size: 0,
            },
        ],
        dirs: vec![],
    };
    assert!(
        matches!(validate_manifest(&collision), Err(DaemonError::Validation(message)) if message.contains("collides"))
    );
    let file_owns_directory_parent = StageEntry::Directory {
        name: "folder".to_owned(),
        files: vec![StagedFile {
            relative: "a".to_owned(),
            size: 0,
        }],
        dirs: vec!["a/b".to_owned()],
    };
    assert!(
        matches!(validate_manifest(&file_owns_directory_parent), Err(DaemonError::Validation(message)) if message.contains("both a file and a directory"))
    );
    assert_eq!(
        with_numeric_suffix("20260924-123456-image.png", 2),
        "20260924-123456-image-2.png"
    );
}

#[test]
fn manifest_limits_are_rejected_before_disk_allocation() {
    let too_large = StageEntry::File {
        name: "large.bin".to_owned(),
        size: MAX_UPLOAD_BYTES + 1,
    };
    assert!(
        matches!(validate_manifest(&too_large), Err(DaemonError::Validation(message)) if message.contains(&MAX_UPLOAD_BYTES.to_string()))
    );
    let too_many = StageEntry::Directory {
        name: "many".to_owned(),
        files: vec![],
        dirs: vec!["empty".to_owned(); MAX_UPLOAD_FILES + 1],
    };
    assert!(
        matches!(validate_manifest(&too_many), Err(DaemonError::Validation(message)) if message.contains(&MAX_UPLOAD_FILES.to_string()))
    );
}

#[test]
fn download_selection_falls_back_when_the_home_downloads_directory_is_absent() {
    let temp = tempfile::tempdir().expect("temp home");
    // The selection canonicalizes; macOS temp directories live under the `/var` symlink.
    let temp_root = temp.path().canonicalize().expect("canonical temp home");
    let fleet_home = FleetHome::new(temp_root.join("fleet-home"));
    let os_home = temp_root.join("missing-home");
    assert!(!os_home.join("Downloads").exists());
    assert_eq!(
        selected_downloads_root_from(&fleet_home, Some(os_home.clone())),
        fleet_home.root().join("media")
    );

    std::fs::create_dir_all(os_home.join("Downloads")).expect("downloads directory");
    assert_eq!(
        selected_downloads_root_from(&fleet_home, Some(os_home.clone())),
        os_home.join("Downloads/fleet")
    );
}

#[test]
fn download_selection_canonicalizes_symlinked_home_and_downloads() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().expect("temp home");
    // The selection canonicalizes; macOS temp directories live under the `/var` symlink.
    let temp_root = temp.path().canonicalize().expect("canonical temp home");
    let real_home = temp_root.join("real-home");
    let external_downloads = temp_root.join("external-downloads");
    std::fs::create_dir_all(&real_home).expect("real home");
    std::fs::create_dir_all(&external_downloads).expect("external downloads");
    symlink(&external_downloads, real_home.join("Downloads")).expect("downloads symlink");
    let linked_home = temp_root.join("linked-home");
    symlink(&real_home, &linked_home).expect("home symlink");
    let fleet_home = FleetHome::new(temp_root.join("fleet-home"));

    assert_eq!(
        selected_downloads_root_from(&fleet_home, Some(linked_home)),
        external_downloads.join("fleet")
    );

    let real_fleet_home = temp_root.join("real-fleet-home");
    std::fs::create_dir_all(&real_fleet_home).expect("real Fleet home");
    let linked_fleet_home = temp_root.join("linked-fleet-home");
    symlink(&real_fleet_home, &linked_fleet_home).expect("Fleet home symlink");
    let fleet_home = FleetHome::new(linked_fleet_home);
    assert_eq!(
        canonical_fleet_root(&fleet_home).join("agents/attachments"),
        real_fleet_home.join("agents/attachments")
    );
}

#[test]
fn unique_name_search_is_bounded() {
    let (_media, files) = fixture();
    let root = PathBuf::from(DOWNLOADS);
    for suffix in 1_u64..=10_000 {
        let base = "20260924-123456-image.png";
        let name = if suffix == 1 {
            base.to_owned()
        } else {
            with_numeric_suffix(base, suffix)
        };
        files.insert_text(root.join(name), "occupied");
    }

    assert!(
        matches!(
            unique_final_path(files.as_ref(), &root, "20260924-123456", "image.png"),
            Err(DaemonError::Validation(message)) if message.contains("10000")
        ),
        "allocation must report exhaustion instead of panicking or looping forever"
    );
}
