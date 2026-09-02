//! Disposable Git worktrees — the filesystem half of the execution sandbox.
//!
//! # What this module does
//!
//! It creates a detached Git worktree at a pinned revision, under a path Beacon
//! owns, and later removes it. Creating a worktree spawns `git`, which is
//! exactly what separates Execution from preflight: ADR 0027 forbids preflight
//! from spawning a process, and this module is not preflight.
//!
//! # Cleanup refuses rather than deletes
//!
//! Removing a checkout is the one operation here that cannot be undone, so
//! [`Worktree::close`] is a gate, not a delete. Every condition that makes
//! removal unsafe is a named [`KeepReason`], and any reason at all keeps the
//! tree on disk and reports why. Nothing in this module calls a recursive
//! delete on a checkout — the only removal path is `git worktree remove`
//! without `--force`, so Git remains the final authority on whether the tree
//! was safe to take.
//!
//! Two of the reasons are the point. `CheckFailed` means the safety check could
//! not produce an answer, and `GateTimedOut` means it never finished. Neither is
//! "clean", and collapsing either into "clean" is how work gets deleted. The
//! hold is therefore re-checked *after* the slow check returns: a hold observed
//! before a check that took seconds is a claim about a tree that may no longer
//! exist.
//!
//! # What this module deliberately does not do
//!
//! - **It does not execute a Declared Check.** The only programs it runs are
//!   `git worktree add`, `git status`, and `git worktree remove`.
//!   Bounded execution of repository-declared commands is a separate slice.
//! - **It applies no sandbox.** No Landlock ruleset or resource limit applies.
//!   Git runs with a closed environment and explicit configuration that disables
//!   repository hooks, filesystem monitors, protocols, and ambient config. A
//!   worktree is a containment boundary for *where* work happens, never for
//!   *what* it may do.
//! - **Ignored content is not a keep reason.** A disposable worktree is expected
//!   to accumulate build output, and discarding it is the intent. Only tracked
//!   modifications and untracked-but-not-ignored files hold the tree.
//! - **It answers "is this still a linked worktree?" structurally**, from the
//!   `.git` file Git writes into a linked worktree. That proves the marker is
//!   present, not that Git agrees the worktree is registered; Git gets the last
//!   word when the removal itself runs.

use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::git_object_store::ObjectStore;

/// The Git executable, resolved through `PATH`.
const GIT: &str = "git";
/// The only executable search path inherited by a Git subprocess.
const GIT_PATH: &str = "/usr/bin:/bin";
/// A hooks directory that cannot exist because `/dev/null` is not a directory.
const DISABLED_HOOKS_PATH: &str = "/dev/null/beacon-disabled-hooks";
/// A pinned revision is a full commit id; anything shorter is ambiguous, and
/// anything else could be read by Git as an option rather than a revision.
const BASE_REVISION_CHARS: usize = 40;
/// How long `git worktree add` may take. A first checkout of a large repository
/// is legitimately slow.
const CREATE_BUDGET: Duration = Duration::from_secs(60);
/// How long the safety check may take before the answer is unknown.
const GATE_BUDGET: Duration = Duration::from_secs(10);
/// How long `git worktree remove` may take before the removal is unconfirmed.
const REMOVE_BUDGET: Duration = Duration::from_secs(30);
/// How often a bounded Git invocation is checked for completion.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Why cleanup refused to remove a worktree.
///
/// Each variant is a fact Beacon observed, never a severity. One reason is
/// enough to keep the tree; the set is reported so an operator can act on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeepReason {
    /// Tracked files are modified, staged, or otherwise not at the revision.
    Dirty,
    /// Untracked, non-ignored files are present. Something wrote work here that
    /// Git does not have.
    UntrackedContent,
    /// The path is gone, unreadable, or no longer carries a linked worktree's
    /// `.git` marker. Beacon cannot prove what it holds, and an unprovable
    /// claim is not a licence to delete.
    NotAWorktree,
    /// The path no longer resolves beneath the scratch root Beacon created for
    /// it. A path that leaves its own boundary is never followed out.
    PathEscapes,
    /// The safety check errored. Distinct from a negative answer: an error is
    /// not evidence of cleanliness.
    CheckFailed,
    /// A bounded step did not finish inside its budget — the safety check, or
    /// the removal itself. Distinct from both a negative answer and an error.
    GateTimedOut,
    /// Every check held and `git worktree remove` still refused. Git knows
    /// something this gate did not check, and Git wins.
    RemovalRefused,
}

/// What cleanup did.
///
/// `Removed {}` is only reported when `git worktree remove` succeeded. Every other
/// terminal state — including a removal that did not finish — is `Kept`.
// `Removed {}` is an empty struct variant rather than a unit variant, and the
// braces are load-bearing. Under `tag = "kind"`, serde enforces
// `deny_unknown_fields` on struct variants and silently ignores it on unit
// variants — measured, not assumed. The wire form is identical either way
// (`{"kind":"removed"}`), so the braces cost nothing and are the only thing
// keeping the attribute from reading as a guard while enforcing nothing.
// `a_cleanup_outcome_round_trips_and_refuses_anything_it_does_not_know` fails
// if they are ever dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum CleanupOutcome {
    /// Git removed the worktree.
    Removed {},
    /// The worktree is still on disk, for these reasons.
    Kept { reasons: Vec<KeepReason> },
}

/// Why a worktree could not be created. No worktree exists in any of these
/// cases, so there is nothing to keep or report.
#[derive(Debug, PartialEq, Eq)]
pub enum WorktreeError {
    /// The source path is missing, unreadable, or not a Git repository.
    RepositoryUnreadable,
    /// The revision is not a lowercase 40-character hexadecimal commit id.
    /// Anything else is either ambiguous or, if it begins with `-`, an option.
    RevisionNotPinned,
    /// The scratch root Beacon would use resolves inside the source repository.
    /// Beacon never writes its own state into an operator's repository.
    ScratchInsideRepository,
    /// The scratch root could not be created or resolved.
    ScratchUnusable,
    /// `git` could not be started.
    GitUnavailable,
    /// Git refused to add the worktree — most often because the pinned revision
    /// is not in this repository.
    RevisionUnavailable,
    /// `git worktree add` did not finish inside its budget. A partial checkout
    /// may remain; Beacon removes the scratch root only if it is empty.
    CreationTimedOut,
}

impl fmt::Display for WorktreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RepositoryUnreadable => {
                write!(
                    formatter,
                    "that source path is not a readable Git repository"
                )
            }
            Self::RevisionNotPinned => write!(
                formatter,
                "a worktree revision must be a lowercase 40-character hexadecimal commit id"
            ),
            Self::ScratchInsideRepository => write!(
                formatter,
                "the scratch location resolves inside the source repository"
            ),
            Self::ScratchUnusable => write!(formatter, "the scratch location could not be created"),
            Self::GitUnavailable => write!(formatter, "git could not be started"),
            Self::RevisionUnavailable => {
                write!(formatter, "git could not check out that revision here")
            }
            Self::CreationTimedOut => {
                write!(formatter, "creating the worktree did not finish in time")
            }
        }
    }
}

impl std::error::Error for WorktreeError {}

/// A Git worktree Beacon created and is responsible for removing.
///
/// Dropping a `Worktree` without calling [`Worktree::close`] leaves it on disk.
/// That is the fail-toward-keep direction stated as an absence: there is no
/// `Drop` that deletes a checkout, because a destructor cannot report why it
/// should not have.
#[derive(Debug)]
pub struct Worktree {
    /// The source repository, canonicalized.
    repository: PathBuf,
    /// The Beacon-managed directory that contains the worktree, canonicalized.
    scratch_root: PathBuf,
    /// The worktree itself, always directly beneath `scratch_root`.
    path: PathBuf,
}

impl Worktree {
    /// Create a detached worktree of `repository` at `revision`.
    ///
    /// The worktree is placed beneath a fresh Beacon-managed scratch root in the
    /// system temporary directory, never inside the source repository.
    pub fn create(repository: &Path, revision: &str) -> Result<Self, WorktreeError> {
        if !is_pinned_revision(revision) {
            return Err(WorktreeError::RevisionNotPinned);
        }
        let repository =
            fs::canonicalize(repository).map_err(|_| WorktreeError::RepositoryUnreadable)?;
        // Ask the object store whether this is a repository at all, so that a
        // plain directory is refused as unreadable rather than reported as a
        // missing revision. The two are different facts.
        ObjectStore::open(&repository).map_err(|_| WorktreeError::RepositoryUnreadable)?;

        let scratch_root = std::env::temp_dir().join(format!("beacon-worktree-{}", Uuid::new_v4()));
        fs::create_dir(&scratch_root).map_err(|_| WorktreeError::ScratchUnusable)?;
        let scratch_root = match fs::canonicalize(&scratch_root) {
            Ok(resolved) => resolved,
            Err(_) => {
                let _ = fs::remove_dir(&scratch_root);
                return Err(WorktreeError::ScratchUnusable);
            }
        };
        if scratch_root.starts_with(&repository) {
            let _ = fs::remove_dir(&scratch_root);
            return Err(WorktreeError::ScratchInsideRepository);
        }

        let path = scratch_root.join("tree");
        let mut command = hardened_git_command(Path::new(GIT));
        command
            .arg("-C")
            .arg(&repository)
            .arg("worktree")
            .arg("add")
            .arg("--detach")
            .arg(&path)
            .arg(revision);
        match run_git_bounded(command, CREATE_BUDGET) {
            Bounded::Finished { success: true, .. } => Ok(Self {
                repository,
                scratch_root,
                path,
            }),
            Bounded::Finished { success: false, .. } => {
                abandon_scratch(&scratch_root);
                Err(WorktreeError::RevisionUnavailable)
            }
            Bounded::TimedOut => {
                abandon_scratch(&scratch_root);
                Err(WorktreeError::CreationTimedOut)
            }
            Bounded::Failed => {
                abandon_scratch(&scratch_root);
                Err(WorktreeError::GitUnavailable)
            }
        }
    }

    /// Where the worktree is. Callers run their work here and nowhere else.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the worktree, or keep it and say why.
    pub fn close(self) -> CleanupOutcome {
        self.close_with(Path::new(GIT), GATE_BUDGET)
    }

    /// [`Worktree::close`], with the Git program and the safety check's budget
    /// supplied.
    ///
    /// `close` passes the real program and budget. The module's own tests pass a
    /// program that hangs, that errors, or that mutates the tree while it runs,
    /// because those three cases are exactly the ones a working `git status`
    /// will never produce on demand — and they are the cases the gate exists for.
    fn close_with(self, git: &Path, budget: Duration) -> CleanupOutcome {
        let mut reasons = self.structural_holds();
        if reasons.is_empty() {
            reasons = status_holds(git, &self.path, budget);
            // Re-check after the slow check. `git status` can take seconds, and
            // the structural facts observed before it are claims about a tree
            // that may have been moved, unlinked, or replaced since.
            for reason in self.structural_holds() {
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
            }
        }
        if !reasons.is_empty() {
            return CleanupOutcome::Kept { reasons };
        }

        let mut command = hardened_git_command(git);
        command
            .arg("-C")
            .arg(&self.repository)
            .arg("worktree")
            .arg("remove")
            .arg(&self.path);
        match run_git_bounded(command, REMOVE_BUDGET) {
            Bounded::Finished { success: true, .. } => {
                // Best effort, and non-recursive on purpose: the scratch root is
                // removed only if Git left it empty.
                let _ = fs::remove_dir(&self.scratch_root);
                CleanupOutcome::Removed {}
            }
            Bounded::Finished { success: false, .. } => CleanupOutcome::Kept {
                reasons: vec![KeepReason::RemovalRefused],
            },
            // A removal that did not finish is not a removal that happened.
            Bounded::TimedOut => CleanupOutcome::Kept {
                reasons: vec![KeepReason::GateTimedOut],
            },
            Bounded::Failed => CleanupOutcome::Kept {
                reasons: vec![KeepReason::CheckFailed],
            },
        }
    }

    /// The holds that need no process: the path still resolves inside the
    /// scratch root, and it still carries a linked worktree's marker.
    fn structural_holds(&self) -> Vec<KeepReason> {
        let Ok(resolved) = fs::canonicalize(&self.path) else {
            return vec![KeepReason::NotAWorktree];
        };
        let mut reasons = Vec::new();
        // Containment is decided on the resolved path, never on the path Beacon
        // remembers: a symlink swapped in underneath would otherwise carry the
        // removal out of the boundary that authorized it.
        if !resolved.starts_with(&self.scratch_root) {
            reasons.push(KeepReason::PathEscapes);
        }
        if !is_linked_worktree(&self.path) {
            reasons.push(KeepReason::NotAWorktree);
        }
        reasons
    }
}

/// A pinned revision, by the same rule a Repository Binding uses.
fn is_pinned_revision(revision: &str) -> bool {
    revision.len() == BASE_REVISION_CHARS
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Does this path carry the `.git` file Git writes into a linked worktree?
fn is_linked_worktree(path: &Path) -> bool {
    let marker = path.join(".git");
    match fs::metadata(&marker) {
        Ok(metadata) if metadata.is_file() => fs::read_to_string(&marker)
            .map(|contents| contents.lines().any(|line| line.starts_with("gitdir:")))
            .unwrap_or(false),
        _ => false,
    }
}

/// Give up a scratch root that never became a worktree.
///
/// Non-recursive on purpose. If Git left anything behind, the directory stays,
/// because this module never deletes a checkout it cannot account for.
fn abandon_scratch(scratch_root: &Path) {
    let _ = fs::remove_dir(scratch_root);
}

/// Ask Git whether the worktree holds anything a removal would destroy.
///
/// `--untracked-files=all` is passed explicitly so that an operator's
/// `status.showUntrackedFiles` setting cannot turn untracked work invisible,
/// and `--ignore-submodules=none` so a dirty submodule still counts.
fn status_holds(git: &Path, tree: &Path, budget: Duration) -> Vec<KeepReason> {
    let mut command = hardened_git_command(git);
    command
        .arg("status")
        .arg("--porcelain")
        .arg("--untracked-files=all")
        .arg("--ignore-submodules=none")
        .current_dir(tree);
    match run_git_bounded(command, budget) {
        Bounded::Finished {
            success: true,
            stdout,
        } => {
            let mut reasons = Vec::new();
            for line in stdout.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                let reason = if line.starts_with("??") {
                    KeepReason::UntrackedContent
                } else {
                    KeepReason::Dirty
                };
                if !reasons.contains(&reason) {
                    reasons.push(reason);
                }
            }
            reasons
        }
        Bounded::Finished { success: false, .. } | Bounded::Failed => vec![KeepReason::CheckFailed],
        Bounded::TimedOut => vec![KeepReason::GateTimedOut],
    }
}

/// The result of one bounded Git invocation. "Did not finish" is its own answer.
enum Bounded {
    Finished { success: bool, stdout: String },
    TimedOut,
    Failed,
}

/// Construct the one production Git command shape used for untrusted trees.
///
/// The repository's local configuration still exists because Git needs it to
/// understand linked worktrees. Disabling known program-valued configuration
/// here therefore does not protect against clean/smudge filters or another
/// executable configured by an actor that can write inside `.git`; that local
/// write capability remains the explicit residual boundary.
fn hardened_git_command(git: &Path) -> Command {
    let mut command = Command::new(git);
    command
        .env_clear()
        .env("PATH", GIT_PATH)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-c")
        .arg(format!("core.hooksPath={DISABLED_HOOKS_PATH}"))
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg("protocol.allow=never");
    command
}

/// Run one Git invocation with a deadline.
///
/// Standard output is a pipe that is read only after the child exits. A child
/// that produces more output than the pipe holds therefore blocks and is
/// reported as [`Bounded::TimedOut`] rather than as a short read — the
/// fail-toward-keep direction for every caller here.
///
/// The kill on timeout reaches this child, not a process group; Git spawns
/// nothing long-lived for these three commands, and process-group interruption
/// belongs to whichever layer runs repository-declared programs.
fn run_git_bounded(mut command: Command, budget: Duration) -> Bounded {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return Bounded::Failed;
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= budget {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Bounded::TimedOut;
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Bounded::Failed;
            }
        }
    };
    let Some(mut pipe) = child.stdout.take() else {
        return Bounded::Failed;
    };
    let mut stdout = String::new();
    if pipe.read_to_string(&mut stdout).is_err() {
        return Bounded::Failed;
    }
    Bounded::Finished {
        success: status.success(),
        stdout,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use uuid::Uuid;

    use super::{CleanupOutcome, KeepReason, Worktree, status_holds};

    /// A temporary directory removed on drop.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("beacond-worktree-{label}-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// An executable stand-in for `git`.
    fn program(directory: &Path, body: &str) -> PathBuf {
        let path = directory.join("fake-git");
        fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A worktree-shaped directory that no repository backs. Enough for the
    /// structural holds, which is all these tests exercise.
    fn shaped_worktree(root: &Path) -> Worktree {
        let scratch_root = fs::canonicalize(root).unwrap();
        let path = scratch_root.join("tree");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(".git"), "gitdir: /nowhere/.git/worktrees/tree\n").unwrap();
        Worktree {
            repository: scratch_root.clone(),
            scratch_root,
            path,
        }
    }

    #[test]
    fn a_check_that_never_finishes_is_a_timeout_and_never_a_clean_answer() {
        let directory = TempDir::new("gate-timeout");
        let hanging = program(&directory.path, "sleep 30\n");

        let holds = status_holds(&hanging, &directory.path, Duration::from_millis(50));

        assert_eq!(holds, vec![KeepReason::GateTimedOut]);
    }

    #[test]
    fn a_check_that_errors_is_a_failure_and_never_a_clean_answer() {
        let directory = TempDir::new("gate-error");
        let failing = program(&directory.path, "exit 3\n");

        let holds = status_holds(&failing, &directory.path, Duration::from_secs(5));

        assert_eq!(holds, vec![KeepReason::CheckFailed]);
    }

    #[test]
    fn a_check_program_that_cannot_start_is_a_failure_and_never_a_clean_answer() {
        let directory = TempDir::new("gate-absent");

        let holds = status_holds(
            Path::new("/nonexistent/beacon/git"),
            &directory.path,
            Duration::from_secs(5),
        );

        assert_eq!(holds, vec![KeepReason::CheckFailed]);
    }

    #[test]
    fn the_hold_is_rechecked_after_the_slow_check_returns() {
        let directory = TempDir::new("recheck");
        let worktree = shaped_worktree(&directory.path);
        let path = worktree.path().to_path_buf();
        // Reports a clean tree, and unlinks the worktree while doing so. Only a
        // gate that re-reads the hold afterwards can see it.
        let saboteur = program(&directory.path, "rm -f .git\nexit 0\n");

        let outcome = worktree.close_with(&saboteur, Duration::from_secs(5));

        assert_eq!(
            outcome,
            CleanupOutcome::Kept {
                reasons: vec![KeepReason::NotAWorktree],
            }
        );
        assert!(path.is_dir(), "the tree was kept, so it is still on disk");
    }
}
