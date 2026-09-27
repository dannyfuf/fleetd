use std::sync::atomic::Ordering;

use tempfile::tempdir;

use super::*;

#[test]
fn atomic_write_replaces_complete_text_and_leaves_no_temp() {
    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("data")]);
    let path = temp.path().join("data/config.json");
    files
        .atomic_write_text(&path, "first")
        .unwrap_or_else(|error| panic!("{error}"));
    files
        .atomic_write_text(&path, "second")
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(fs::read_to_string(&path).ok().as_deref(), Some("second"));
    let children = files
        .list(path.parent().unwrap_or(temp.path()))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(children, vec![path]);
}

#[test]
fn atomic_replace_syncs_parent_directory() {
    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("data")]);
    let path = temp.path().join("data/config.json");
    let before = PARENT_SYNCS.load(Ordering::Relaxed);
    files
        .atomic_write_text(&path, "durable")
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(PARENT_SYNCS.load(Ordering::Relaxed) > before);
}

#[test]
fn private_directory_creation_rejects_symlinked_roots_and_intermediates() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("staging")]);

    let linked_root = temp.path().join("linked-root");
    symlink(&outside, &linked_root).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        files
            .create_private_dir_all(&linked_root.join("escaped"))
            .is_err()
    );
    assert!(!outside.join("escaped").exists());

    let staging = temp.path().join("staging");
    fs::create_dir(&staging).unwrap_or_else(|error| panic!("{error}"));
    symlink(&outside, staging.join("linked")).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        files
            .create_private_dir_all(&staging.join("linked/escaped"))
            .is_err()
    );
    assert!(!outside.join("escaped").exists());
}

#[test]
fn private_leaf_creation_is_exclusive_and_part_files_do_not_follow_parents() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let staging = temp.path().join("staging");
    let outside = temp.path().join("outside");
    fs::create_dir(&staging).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir(&outside).unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [staging.clone()]);

    let part = staging.join("upload.part");
    files
        .create_private_dir(&part)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(files.create_private_dir(&part).is_err());

    let linked = staging.join("linked");
    symlink(&outside, &linked).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        files
            .create_part_file(&linked.join("escape.bin"), 1)
            .is_err()
    );
    assert!(!outside.join("escape.bin").exists());
}

#[test]
fn conditional_remove_preserves_replacement() {
    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("cache")]);
    let path = temp.path().join("cache/pr.json");
    files
        .atomic_write_text(&path, "stale")
        .unwrap_or_else(|error| panic!("{error}"));
    let inspected = files
        .metadata(&path)
        .unwrap_or_else(|error| panic!("{error}"));
    files
        .atomic_write_text(&path, "refreshed")
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(
        !files
            .remove_file_if_unchanged(&path, inspected)
            .unwrap_or_else(|error| panic!("{error}"))
    );
    assert_eq!(
        files
            .read_text(&path)
            .unwrap_or_else(|error| panic!("{error}")),
        "refreshed"
    );
}

#[test]
fn conditional_remove_restores_replacement_after_post_check_race() {
    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("cache")]);
    let path = temp.path().join("cache/pr.json");
    files
        .atomic_write_text(&path, "stale")
        .unwrap_or_else(|error| panic!("{error}"));
    let inspected = files
        .metadata(&path)
        .unwrap_or_else(|error| panic!("{error}"));
    let racing_files = RealFiles::new(temp.path().join("trash"), [temp.path().join("cache")]);
    files.set_conditional_removal_hook(move |stage, path| {
        if stage == ConditionalRemovalStage::BeforeRename {
            racing_files
                .atomic_write_text(path, "refreshed")
                .unwrap_or_else(|error| panic!("{error}"));
        }
    });

    assert!(
        !files
            .remove_file_if_unchanged(&path, inspected)
            .unwrap_or_else(|error| panic!("{error}"))
    );
    assert_eq!(
        files
            .read_text(&path)
            .unwrap_or_else(|error| panic!("{error}")),
        "refreshed"
    );
    assert_eq!(
        files
            .list(path.parent().unwrap_or(temp.path()))
            .unwrap_or_else(|error| panic!("{error}")),
        vec![path]
    );
}

#[test]
fn conditional_remove_preserves_recovery_when_restore_destination_exists() {
    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [temp.path().join("cache")]);
    let path = temp.path().join("cache/pr.json");
    files
        .atomic_write_text(&path, "stale")
        .unwrap_or_else(|error| panic!("{error}"));
    let inspected = files
        .metadata(&path)
        .unwrap_or_else(|error| panic!("{error}"));
    let racing_files = RealFiles::new(temp.path().join("trash"), [temp.path().join("cache")]);
    files.set_conditional_removal_hook(move |stage, path| {
        let text = match stage {
            ConditionalRemovalStage::BeforeRename => "refreshed",
            ConditionalRemovalStage::AfterRename => "newest",
        };
        racing_files
            .atomic_write_text(path, text)
            .unwrap_or_else(|error| panic!("{error}"));
    });

    assert!(
        !files
            .remove_file_if_unchanged(&path, inspected)
            .unwrap_or_else(|error| panic!("{error}"))
    );
    assert_eq!(
        files
            .read_text(&path)
            .unwrap_or_else(|error| panic!("{error}")),
        "newest"
    );
    let recovery = files
        .list(path.parent().unwrap_or(temp.path()))
        .unwrap_or_else(|error| panic!("{error}"))
        .into_iter()
        .find(|candidate| {
            candidate.file_name().is_some_and(|name| {
                name.as_bytes()
                    .windows(14)
                    .any(|part| part == b".fleet-expiry-")
            })
        })
        .unwrap_or_else(|| panic!("recovery file"));
    assert_eq!(
        files
            .read_text(&recovery)
            .unwrap_or_else(|error| panic!("{error}")),
        "refreshed"
    );
}

#[test]
fn descendant_guard_rejects_roots_and_escapes() {
    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let root = temp.path().join("repos");
    fs::create_dir_all(root.join("owner")).unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [root.clone()]);
    assert!(
        files
            .guard_strict_descendant(&root.join("owner/repo"))
            .is_ok()
    );
    assert!(files.guard_strict_descendant(&root).is_err());
    assert!(
        files
            .guard_strict_descendant(&root.join("../outside"))
            .is_err()
    );
}

#[test]
fn symlink_swap_cannot_escape_removable_root() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let root = temp.path().join("repos");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&outside).unwrap_or_else(|error| panic!("{error}"));
    fs::write(outside.join("keep"), "untouched").unwrap_or_else(|error| panic!("{error}"));
    symlink(&outside, root.join("swapped")).unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [root.clone()]);

    assert!(files.trash(&root.join("swapped/keep")).is_err());
    assert_eq!(
        fs::read_to_string(outside.join("keep")).ok().as_deref(),
        Some("untouched")
    );
}

#[test]
fn remove_detached_removes_nested_directory_and_reports_completion() {
    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let root = temp.path().join("repos");
    let trash = temp.path().join("trash");
    let target = root.join("owner/repo");
    fs::create_dir_all(target.join("nested/empty")).unwrap_or_else(|error| panic!("{error}"));
    fs::write(target.join("nested/file"), "contents").unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(&trash, [root]);

    files
        .remove_detached(&target)
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(!target.exists());
    assert!(
        fs::read_dir(&trash)
            .unwrap_or_else(|error| panic!("{error}"))
            .next()
            .is_none()
    );
}

#[test]
fn symlinked_ancestor_above_root_allows_confined_removal() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let real = temp.path().join("real");
    let link = temp.path().join("link");
    let root = link.join("repos");
    let target = root.join("owner/repo");
    fs::create_dir_all(real.join("repos/owner/repo/nested"))
        .unwrap_or_else(|error| panic!("{error}"));
    fs::write(real.join("repos/owner/repo/nested/file"), "contents")
        .unwrap_or_else(|error| panic!("{error}"));
    symlink(&real, &link).unwrap_or_else(|error| panic!("{error}"));
    let trash = temp.path().join("trash");
    let files = RealFiles::new(&trash, [root]);

    files
        .remove_detached(&target)
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(!real.join("repos/owner/repo").exists());
    assert!(
        fs::read_dir(&trash)
            .unwrap_or_else(|error| panic!("{error}"))
            .next()
            .is_none()
    );
}

#[test]
fn ordinary_rename_preserves_symlinked_ancestor_semantics() {
    use std::os::unix::fs::symlink;

    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let real = temp.path().join("real");
    let link = temp.path().join("link");
    fs::create_dir_all(&real).unwrap_or_else(|error| panic!("{error}"));
    fs::write(real.join("source"), "contents").unwrap_or_else(|error| panic!("{error}"));
    symlink(&real, &link).unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [real.clone()]);

    files
        .rename(&link.join("source"), &link.join("destination"))
        .unwrap_or_else(|error| panic!("{error}"));

    assert!(!real.join("source").exists());
    assert_eq!(
        fs::read_to_string(real.join("destination")).ok().as_deref(),
        Some("contents")
    );
}

#[test]
fn removable_roots_can_be_refreshed() {
    let temp = tempdir().unwrap_or_else(|error| panic!("{error}"));
    let old_root = temp.path().join("old");
    let new_root = temp.path().join("new");
    fs::create_dir_all(old_root.join("owner")).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(new_root.join("owner")).unwrap_or_else(|error| panic!("{error}"));
    let files = RealFiles::new(temp.path().join("trash"), [old_root.clone()]);

    assert!(
        files
            .guard_strict_descendant(&old_root.join("owner/repo"))
            .is_ok()
    );
    assert!(
        files
            .guard_strict_descendant(&new_root.join("owner/repo"))
            .is_err()
    );

    files.set_removable_roots(vec![new_root.clone()]);

    assert!(
        files
            .guard_strict_descendant(&old_root.join("owner/repo"))
            .is_err()
    );
    assert!(
        files
            .guard_strict_descendant(&new_root.join("owner/repo"))
            .is_ok()
    );
}
