//! Disposable Git worktrees — sandbox Slice B.
//!
//! Cleanup is the only irreversible operation here, so most of these tests are
//! about refusing to perform it. They assert that a dirty tree, an unprovable
//! tree, a tree that left its boundary, a check that could not answer, and a
//! removal Git itself refused are five separate facts, that each one keeps the
//! worktree, and that a kept worktree is still on disk afterwards.
//!
//! The product spawns Git; so do these tests, to build repositories and to
//! reach states only Git can produce.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use beacond::{CleanupOutcome, KeepReason, Worktree, WorktreeError};
use uuid::Uuid;

/// A temporary directory removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("beacond-slice-b-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn git(directory: &Path, arguments: &[&str]) {
    let status = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .env("GIT_AUTHOR_NAME", "Beacon Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Beacon Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .status()
        .expect("git is available in the test environment");
    assert!(status.success(), "git {arguments:?} failed");
}

/// A repository with one commit, and that commit's id.
fn fixture_repository(label: &str) -> (TempDir, String) {
    let directory = TempDir::new(label);
    git(&directory.path, &["init", "--quiet"]);
    fs::write(directory.path.join("tracked.txt"), "one\n").unwrap();
    git(&directory.path, &["add", "."]);
    git(&directory.path, &["commit", "--quiet", "-m", "one"]);
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&directory.path)
        .output()
        .expect("git is available in the test environment");
    let revision = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    (directory, revision)
}

/// Remove a worktree these tests deliberately caused Beacon to keep.
///
/// The product refuses to do this; a test that leaves scratch roots behind is
/// its own kind of failure.
fn scrub(worktree_path: &Path) {
    if let Some(scratch_root) = worktree_path.parent() {
        let _ = fs::remove_dir_all(scratch_root);
    }
}

fn reasons(outcome: &CleanupOutcome) -> &[KeepReason] {
    match outcome {
        CleanupOutcome::Kept { reasons } => reasons,
        CleanupOutcome::Removed {} => {
            panic!("expected the worktree to be kept, and it was removed")
        }
    }
}

fn hostile_program(path: &Path, sentinel: &Path) {
    fs::write(
        path,
        format!("#!/bin/sh\n: > '{}'\nexit 0\n", sentinel.display()),
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn repository_programs_do_not_run_during_worktree_creation_or_cleanup() {
    let (repository, revision) = fixture_repository("hostile-git-configuration");
    let hook_sentinel = repository.path.join("hook-ran");
    let fsmonitor_sentinel = repository.path.join("fsmonitor-ran");
    let hook = repository.path.join(".git/hooks/post-checkout");
    let fsmonitor = repository.path.join(".git/hostile-fsmonitor");
    hostile_program(&hook, &hook_sentinel);
    hostile_program(&fsmonitor, &fsmonitor_sentinel);
    git(
        &repository.path,
        &["config", "core.fsmonitor", &fsmonitor.to_string_lossy()],
    );

    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    assert_eq!(worktree.close(), CleanupOutcome::Removed {});

    assert!(
        !hook_sentinel.exists(),
        "repository-controlled post-checkout hook ran"
    );
    assert!(
        !fsmonitor_sentinel.exists(),
        "repository-controlled fsmonitor ran"
    );
}

#[test]
fn a_worktree_is_created_outside_the_repository_at_the_pinned_revision() {
    let (repository, revision) = fixture_repository("create");

    let worktree = Worktree::create(&repository.path, &revision).unwrap();

    let path = worktree.path().to_path_buf();
    assert!(path.is_dir());
    assert!(
        !path.starts_with(fs::canonicalize(&repository.path).unwrap()),
        "Beacon never writes its scratch state into the operator's repository"
    );
    assert_eq!(
        fs::read_to_string(path.join("tracked.txt")).unwrap(),
        "one\n"
    );
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&path)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(head.stdout).unwrap().trim(), revision);

    assert_eq!(worktree.close(), CleanupOutcome::Removed {});
    scrub(&path);
}

#[test]
fn a_clean_worktree_is_removed_and_takes_its_scratch_root_with_it() {
    let (repository, revision) = fixture_repository("remove");
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let path = worktree.path().to_path_buf();
    let scratch_root = path.parent().unwrap().to_path_buf();

    let outcome = worktree.close();

    assert_eq!(outcome, CleanupOutcome::Removed {});
    assert!(!path.exists());
    assert!(!scratch_root.exists());
}

#[test]
fn a_modified_tracked_file_keeps_the_worktree_on_disk() {
    let (repository, revision) = fixture_repository("dirty");
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let path = worktree.path().to_path_buf();
    fs::write(path.join("tracked.txt"), "edited\n").unwrap();

    let outcome = worktree.close();

    assert_eq!(reasons(&outcome), [KeepReason::Dirty]);
    assert!(path.is_dir(), "a kept worktree is never deleted");
    assert_eq!(
        fs::read_to_string(path.join("tracked.txt")).unwrap(),
        "edited\n",
        "the work that caused the refusal is still there"
    );
    scrub(&path);
}

#[test]
fn untracked_content_keeps_the_worktree_on_disk() {
    let (repository, revision) = fixture_repository("untracked");
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let path = worktree.path().to_path_buf();
    fs::create_dir(path.join("notes")).unwrap();
    fs::write(path.join("notes/unsaved.txt"), "work\n").unwrap();

    let outcome = worktree.close();

    assert_eq!(reasons(&outcome), [KeepReason::UntrackedContent]);
    assert!(path.join("notes/unsaved.txt").is_file());
    scrub(&path);
}

#[test]
fn operator_configuration_cannot_hide_untracked_work_from_the_gate() {
    // `status.showUntrackedFiles=no` would report a tree carrying untracked work
    // as clean, and a gate that believed it would delete that work. A linked
    // worktree shares the repository's config, so the flag on the command line
    // is the only thing standing between the two. This is that proof.
    let (repository, revision) = fixture_repository("hidden-untracked");
    git(
        &repository.path,
        &["config", "status.showUntrackedFiles", "no"],
    );
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let path = worktree.path().to_path_buf();
    fs::write(path.join("unsaved.txt"), "work\n").unwrap();

    let outcome = worktree.close();

    assert_eq!(reasons(&outcome), [KeepReason::UntrackedContent]);
    assert!(path.join("unsaved.txt").is_file());
    scrub(&path);
}

#[test]
fn a_modification_and_untracked_content_are_reported_together() {
    let (repository, revision) = fixture_repository("both");
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let path = worktree.path().to_path_buf();
    fs::write(path.join("tracked.txt"), "edited\n").unwrap();
    fs::write(path.join("extra.txt"), "new\n").unwrap();

    let outcome = worktree.close();

    let held = reasons(&outcome);
    assert_eq!(
        held.len(),
        2,
        "each distinct reason is reported once: {held:?}"
    );
    assert!(held.contains(&KeepReason::Dirty));
    assert!(held.contains(&KeepReason::UntrackedContent));
    scrub(&path);
}

#[test]
fn a_tree_that_lost_its_worktree_marker_is_kept_rather_than_deleted() {
    // Beacon cannot prove this is still the worktree it created, and an
    // unprovable claim is not a licence to delete.
    let (repository, revision) = fixture_repository("unmarked");
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let path = worktree.path().to_path_buf();
    fs::remove_file(path.join(".git")).unwrap();

    let outcome = worktree.close();

    assert_eq!(reasons(&outcome), [KeepReason::NotAWorktree]);
    assert!(path.is_dir());
    assert!(path.join("tracked.txt").is_file());
    scrub(&path);
}

#[test]
fn a_tree_that_resolves_outside_its_scratch_root_is_kept() {
    // The path is clean as Beacon remembers it and resolves elsewhere. This is
    // why containment is decided on the resolved path: a guard that trusts the
    // value the guarded path supplies can be defeated by supplying another one.
    let (repository, revision) = fixture_repository("escape");
    let elsewhere = TempDir::new("escape-target");
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let path = worktree.path().to_path_buf();
    let moved = elsewhere.path.join("tree");
    fs::rename(&path, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &path).unwrap();

    let outcome = worktree.close();

    assert_eq!(reasons(&outcome), [KeepReason::PathEscapes]);
    assert!(
        moved.join("tracked.txt").is_file(),
        "the removal was never followed out of the boundary that authorized it"
    );
    scrub(&path);
}

#[test]
fn a_status_check_that_cannot_answer_keeps_the_worktree() {
    // The worktree still carries its marker, so the structural holds pass; the
    // repository it points at is gone, so `git status` errors. An error is not
    // a clean answer.
    let (repository, revision) = fixture_repository("check-failed");
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let path = worktree.path().to_path_buf();
    fs::remove_dir_all(repository.path.join(".git")).unwrap();

    let outcome = worktree.close();

    assert_eq!(reasons(&outcome), [KeepReason::CheckFailed]);
    assert!(path.is_dir());
    scrub(&path);
}

#[test]
fn a_locked_worktree_is_kept_because_git_refuses_to_remove_it() {
    // Every check this module performs holds, and Git still says no. Git wins:
    // the gate is a floor on safety, never a ceiling.
    let (repository, revision) = fixture_repository("locked");
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let path = worktree.path().to_path_buf();
    git(
        &repository.path,
        &["worktree", "lock", &path.to_string_lossy()],
    );

    let outcome = worktree.close();

    assert_eq!(reasons(&outcome), [KeepReason::RemovalRefused]);
    assert!(path.is_dir());
    scrub(&path);
}

#[test]
fn a_worktree_dropped_without_close_stays_on_disk() {
    // There is no destructor that deletes a checkout, because a destructor
    // cannot report why it should not have.
    let (repository, revision) = fixture_repository("dropped");
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let path = worktree.path().to_path_buf();

    drop(worktree);

    assert!(path.is_dir());
    scrub(&path);
}

#[test]
fn a_revision_that_is_not_pinned_is_refused_before_git_runs() {
    let (repository, _revision) = fixture_repository("unpinned");

    for revision in ["HEAD", "main", "--upload-pack=touch", "abc123", ""] {
        assert_eq!(
            Worktree::create(&repository.path, revision).unwrap_err(),
            WorktreeError::RevisionNotPinned,
            "{revision:?} is not a pinned commit id"
        );
    }
}

#[test]
fn a_revision_absent_from_the_repository_is_refused() {
    let (repository, _revision) = fixture_repository("absent-revision");

    let error = Worktree::create(&repository.path, &"0".repeat(40)).unwrap_err();

    assert_eq!(error, WorktreeError::RevisionUnavailable);
}

#[test]
fn a_source_that_is_not_a_repository_is_refused_as_unreadable() {
    // Distinct from a missing revision: nothing was looked for, because there
    // was nowhere to look.
    let directory = TempDir::new("not-a-repository");

    let error = Worktree::create(&directory.path, &"a".repeat(40)).unwrap_err();

    assert_eq!(error, WorktreeError::RepositoryUnreadable);
}

#[test]
fn a_missing_source_is_refused_as_unreadable() {
    let directory = TempDir::new("missing-source");

    let error = Worktree::create(&directory.path.join("absent"), &"a".repeat(40)).unwrap_err();

    assert_eq!(error, WorktreeError::RepositoryUnreadable);
}

#[test]
fn a_cleanup_outcome_round_trips_and_refuses_anything_it_does_not_know() {
    // The braces on `Removed {}` are what make `deny_unknown_fields` real.
    // Under `tag = "kind"` serde enforces the attribute on a struct variant and
    // silently ignores it on a unit variant, so this test is the guard on the
    // guard: drop the braces in `worktree.rs` and the last assertion fails.
    for outcome in [
        CleanupOutcome::Removed {},
        CleanupOutcome::Kept {
            reasons: vec![KeepReason::GateTimedOut, KeepReason::CheckFailed],
        },
    ] {
        let json = serde_json::to_string(&outcome).unwrap();
        assert_eq!(
            serde_json::from_str::<CleanupOutcome>(&json).unwrap(),
            outcome
        );
    }

    // The wire form an empty struct variant produces is the one a unit variant
    // would have produced. Nothing downstream can tell them apart.
    assert_eq!(
        serde_json::to_string(&CleanupOutcome::Removed {}).unwrap(),
        r#"{"kind":"removed"}"#
    );

    assert!(serde_json::from_str::<CleanupOutcome>(r#"{"kind":"vanished"}"#).is_err());
    assert!(serde_json::from_str::<CleanupOutcome>(r#"{"kind":"kept"}"#).is_err());
    assert!(
        serde_json::from_str::<CleanupOutcome>(r#"{"kind":"kept","reasons":["nope"]}"#).is_err()
    );
    assert!(
        serde_json::from_str::<CleanupOutcome>(r#"{"kind":"kept","reasons":[],"surprise":true}"#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<CleanupOutcome>(r#"{"kind":"removed","surprise":true}"#).is_err(),
        "a unit variant would accept this; the braces on Removed {{}} are why it does not"
    );
}
