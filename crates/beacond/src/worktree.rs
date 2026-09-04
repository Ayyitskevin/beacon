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
//! - **It does not execute a Declared Check.** It runs `git rev-parse` and
//!   `git config --file` as data-only reads: Beacon fixes their arguments and
//!   file paths, while the repository supplies the bytes being inspected. It
//!   also runs `git worktree add`, `git status`, and `git worktree remove`,
//!   which operate on the bound source repository and linked worktree. Bounded
//!   execution of repository-declared commands is a separate slice.
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

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(debug_assertions)]
use std::sync::{Arc, Mutex};
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
        let filter_drivers = repository_filter_drivers(&repository)
            .map_err(|_| WorktreeError::RepositoryUnreadable)?;

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
        let mut command = hardened_git_command(Path::new(GIT), &filter_drivers);
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
        let reasons = self.structural_holds();
        if !reasons.is_empty() {
            return CleanupOutcome::Kept { reasons };
        }
        let Ok(mut filter_drivers) = repository_filter_drivers(&self.repository) else {
            return CleanupOutcome::Kept {
                reasons: vec![KeepReason::CheckFailed],
            };
        };
        let Ok(worktree_filter_drivers) = repository_filter_drivers(&self.path) else {
            return CleanupOutcome::Kept {
                reasons: vec![KeepReason::CheckFailed],
            };
        };
        filter_drivers.extend(worktree_filter_drivers);
        filter_drivers.sort();
        filter_drivers.dedup();
        self.close_with(Path::new(GIT), GATE_BUDGET, &filter_drivers)
    }

    /// [`Worktree::close`], with the Git program and the safety check's budget
    /// supplied.
    ///
    /// `close` passes the real program and budget. The module's own tests pass a
    /// program that hangs, that errors, or that mutates the tree while it runs,
    /// because those three cases are exactly the ones a working `git status`
    /// will never produce on demand — and they are the cases the gate exists for.
    fn close_with(self, git: &Path, budget: Duration, filter_drivers: &[String]) -> CleanupOutcome {
        let mut reasons = self.structural_holds();
        if reasons.is_empty() {
            reasons = status_holds(git, &self.path, budget, filter_drivers);
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

        let mut command = hardened_git_command(git, filter_drivers);
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
fn status_holds(
    git: &Path,
    tree: &Path,
    budget: Duration,
    filter_drivers: &[String],
) -> Vec<KeepReason> {
    let mut command = hardened_git_command(git, filter_drivers);
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

/// Read every filter driver named by repository-local configuration.
///
/// `rev-parse` and `config --file` are Git's data-only builtins: neither checks
/// out content nor invokes a repository-configured program. Explicit `--file`
/// reads keep system and user configuration out while `--includes` ensures an
/// included local filter cannot escape the command-line overrides.
fn repository_filter_drivers(repository: &Path) -> Result<Vec<String>, ()> {
    let mut locate = closed_git_command(Path::new(GIT));
    locate.arg("-C").arg(repository).args([
        "rev-parse",
        "--path-format=absolute",
        "--git-dir",
        "--git-common-dir",
    ]);
    let Bounded::Finished {
        success: true,
        stdout,
    } = run_git_bounded(locate, GATE_BUDGET)
    else {
        return Err(());
    };
    let mut directories = stdout.lines();
    let git_directory = PathBuf::from(directories.next().ok_or(())?);
    let common_directory = PathBuf::from(directories.next().ok_or(())?);
    if directories.next().is_some()
        || !git_directory.is_absolute()
        || !common_directory.is_absolute()
    {
        return Err(());
    }

    let mut configuration_files = vec![common_directory.join("config")];
    let worktree_configuration = git_directory.join("config.worktree");
    match fs::symlink_metadata(&worktree_configuration) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => {
            configuration_files.push(worktree_configuration);
        }
        Ok(_) => return Err(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(()),
    }

    let mut drivers = BTreeSet::new();
    for configuration_file in configuration_files {
        let mut read = closed_git_command(Path::new(GIT));
        read.arg("-C")
            .arg(repository)
            .args(["config", "--file"])
            .arg(configuration_file)
            .args(["--includes", "--name-only", "--null", "--list"]);
        let Bounded::Finished {
            success: true,
            stdout,
        } = run_git_bounded(read, GATE_BUDGET)
        else {
            return Err(());
        };
        drivers.extend(filter_drivers_from_config_output(&stdout));
    }
    Ok(drivers.into_iter().collect())
}

pub(crate) fn filter_drivers_from_config_output(stdout: &str) -> Vec<String> {
    let mut drivers = BTreeSet::new();
    for key in stdout.split('\0').filter(|key| !key.is_empty()) {
        let Some((driver, field)) = key.rsplit_once('.') else {
            continue;
        };
        if driver
            .get(.."filter.".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("filter."))
            && driver.len() > "filter.".len()
            && matches!(
                field.to_ascii_lowercase().as_str(),
                "smudge" | "clean" | "process" | "required"
            )
        {
            drivers.insert(driver.to_owned());
        }
    }
    drivers.into_iter().collect()
}

/// Construct the one production Git command shape used for untrusted trees.
/// A repository this process has pinned: the directory it opened, and the identity of the
/// directory it opened.
///
/// A kernel review redirected a mutation by renaming the source directory and installing an
/// equivalent decoy at the same pathname between the gate that measured the repository and the act
/// that changed it -- `-C <path>` re-resolves the name every time. So a caller that must measure and
/// then act pins the directory once, carries this value, and re-checks it: `still_pinned` fails
/// closed if the pathname now names a different directory, or none.
///
/// The identity is the device and inode of the opened directory, which is what "the same directory"
/// means to the kernel; the retained descriptor is what keeps that identity from being reused
/// underneath us while this value lives.
///
/// **The root alone is not the repository.** A cold review swapped only `repo/.git` — leaving the
/// root's inode, its pathname and its contents untouched — and every Git child, which discovers
/// `.git` by name, wrote its objects and moved its ref inside the replacement while the Receipt
/// said `Applied` about the directory Beacon pinned. Entering by descriptor solves a different
/// race: it fixes *which directory the child starts in*, not *which repository the child finds
/// there*. So the Git directory is opened through the root descriptor at pin time and held too,
/// and every child is pointed at it by descriptor rather than by name.
#[derive(Debug)]
pub(crate) struct PinnedRepository {
    path: PathBuf,
    device: u64,
    inode: u64,
    directory: fs::File,
    /// The `.git` directory inside `directory`, opened through it and held for the same reason.
    git_directory: fs::File,
    git_identity: (u64, u64),
}

/// The descriptor number a Git child receives the pinned Git directory on.
///
/// Fixed rather than inherited: the value has to be in `GIT_DIR` when the command is built, which
/// is before the fork that would tell us what the kernel picked. High enough to be clear of the
/// standard three and of anything `Command` arranges for the child.
const PINNED_GIT_DIR_FD: std::os::fd::RawFd = 100;

impl PinnedRepository {
    /// Open `path` as a directory without following its final component, and pin what was opened.
    pub(crate) fn open(path: &Path) -> Result<Self, WorktreeError> {
        let path = fs::canonicalize(path).map_err(|_| WorktreeError::RepositoryUnreadable)?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let directory = options
            .open(&path)
            .map_err(|_| WorktreeError::RepositoryUnreadable)?;
        let metadata = directory
            .metadata()
            .map_err(|_| WorktreeError::RepositoryUnreadable)?;
        if !metadata.is_dir() {
            return Err(WorktreeError::RepositoryUnreadable);
        }
        // A directory with no `.git` is not a repository, and saying "the revision is missing"
        // about one would be the wrong fact.
        ObjectStore::open(&path).map_err(|_| WorktreeError::RepositoryUnreadable)?;
        // ...and the `.git` inside it is opened THROUGH the descriptor just taken, so the entry
        // that is pinned is the one that directory holds rather than whatever the pathname
        // `<root>/.git` resolves to a moment later. `O_NOFOLLOW` refuses a symlinked `.git`, and
        // `O_DIRECTORY` refuses a gitfile: a linked worktree or a submodule is not the v1 scope,
        // and admitting one would mean following a name out of the pinned directory.
        let git_directory = open_git_directory(&directory)?;
        let git_metadata = git_directory
            .metadata()
            .map_err(|_| WorktreeError::RepositoryUnreadable)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            path,
            directory,
            git_identity: (git_metadata.dev(), git_metadata.ino()),
            git_directory,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Enter this repository in a child process **by descriptor**.
    ///
    /// `fchdir` in `pre_exec` runs after the fork and before the exec, so the child's working
    /// directory is the directory this value opened -- not whatever the pathname resolves to when
    /// the child starts. A kernel review swapped a decoy in for the duration of a Git invocation
    /// and restored the original before the post-check, which a pathname-entering command cannot
    /// notice; entering by descriptor means there is no pathname resolution left to race.
    ///
    /// The descriptor is `O_CLOEXEC`, which is correct: `fchdir` happens before the exec that would
    /// close it.
    pub(crate) fn enter(&self, command: &mut Command) {
        use std::os::fd::AsRawFd;
        let fd = self.directory.as_raw_fd();
        let git_fd = self.git_directory.as_raw_fd();
        // SAFETY: `fchdir` and `dup2` are async-signal-safe, touch only this child's working
        // directory and descriptor table, and both descriptors outlive the spawn because `self` is
        // borrowed for the call.
        unsafe {
            command.pre_exec(move || {
                if libc::fchdir(fd) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // `dup2` clears `O_CLOEXEC` on the copy, which is the point: the child must still
                // hold this descriptor after the exec that resolves `GIT_DIR`.
                if libc::dup2(git_fd, PINNED_GIT_DIR_FD) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        // With `GIT_DIR` set and no `GIT_WORK_TREE`, Git takes the working directory as the work
        // tree -- and the working directory is the descriptor `fchdir` above installed. So both
        // halves are pinned by descriptor and neither is rediscovered by name.
        command.env("GIT_DIR", pinned_git_dir_path());
    }

    /// The path a child of this repository uses to name its own Git directory.
    ///
    /// A caller that must hand Git a path *inside* the pinned Git directory builds it from here,
    /// so `.git/config` and its neighbours are read through the descriptor like everything else.
    pub(crate) fn git_dir_child_path(component: &str) -> String {
        format!("{}/{component}", pinned_git_dir_path())
    }

    /// The device and inode this value opened. Recorded with a dispatch so a later reader can ask
    /// whether the repository it is looking at is the one that was written to.
    pub(crate) fn identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }

    /// Does the pinned pathname still name the directory this value opened?
    ///
    /// Answered by device and inode rather than by the name, because the name is exactly what an
    /// attacker or an ordinary `mv` can move. A pathname that no longer resolves, or that resolves
    /// to a different directory, is not "the repository changed" -- it is a different repository,
    /// and every measurement taken through the pinned value is void.
    /// ...and does that directory still hold the Git directory this value opened? Both, because a
    /// swap of either one alone is enough to make every measurement taken through this value void,
    /// and a cold review demonstrated the second half by swapping only `.git`.
    pub(crate) fn still_pinned(&self) -> bool {
        let Ok(metadata) = fs::metadata(&self.path) else {
            return false;
        };
        if !(metadata.is_dir() && metadata.dev() == self.device && metadata.ino() == self.inode) {
            return false;
        }
        let Ok(git_metadata) = fs::symlink_metadata(self.path.join(".git")) else {
            return false;
        };
        git_metadata.is_dir() && (git_metadata.dev(), git_metadata.ino()) == self.git_identity
    }
}

/// `/proc/self/fd/<N>`, the pathname a child uses to name the descriptor it was handed.
///
/// The same mechanism `sandbox::worktree_launcher` already uses to name an opened directory to a
/// child: procfs resolves it to the inode the descriptor holds, so a rename or replacement of the
/// pathname it was opened from changes nothing about where the child writes.
fn pinned_git_dir_path() -> String {
    format!("/proc/self/fd/{PINNED_GIT_DIR_FD}")
}

/// Open `.git` inside an already-opened repository root, without following a name out of it.
fn open_git_directory(root: &fs::File) -> Result<fs::File, WorktreeError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = c".git";
    // SAFETY: `root` is an open directory descriptor for the duration of the call, and the
    // returned descriptor is adopted exactly once.
    let raw = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(WorktreeError::RepositoryUnreadable);
    }
    // SAFETY: `raw` is a fresh descriptor this call owns.
    Ok(unsafe { fs::File::from_raw_fd(raw) })
}

/// A Git invocation that may not write.
///
/// The same hardening as [`hardened_git_command`], plus `GIT_OPTIONAL_LOCKS=0`. A kernel review
/// measured `git status --porcelain` refreshing and rewriting `.git/index`: a gate that mutates the
/// repository it is measuring cannot support a zero-mutation claim, and the optional-locks
/// environment variable is what stops it.
/// A hook that runs after a pinned command is built and before it is spawned. Debug builds only.
///
/// This is the one window `still_pinned` cannot see: it checks the pathname before the command runs
/// and again after, so a directory swapped in for the duration of the invocation and swapped back
/// is invisible to it -- and that window is the whole reason the child enters by descriptor rather
/// than by name. Without a synchronization point here, replacing descriptor entry with
/// `current_dir(path)` survives every test in the repository.
///
/// The hook receives the repository's path so a test can act on its own and leave every other
/// test's invocations alone; the tests run in one process.
#[cfg(debug_assertions)]
type PinnedSpawnHook = Arc<dyn Fn(&Path) + Send + Sync>;

#[cfg(debug_assertions)]
static BEFORE_PINNED_SPAWN: Mutex<Option<PinnedSpawnHook>> = Mutex::new(None);

#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn set_pinned_spawn_hook(hook: Option<PinnedSpawnHook>) {
    *BEFORE_PINNED_SPAWN.lock().unwrap() = hook;
}

#[cfg(debug_assertions)]
fn before_pinned_spawn(repository: &PinnedRepository) {
    let hook = BEFORE_PINNED_SPAWN.lock().unwrap().clone();
    if let Some(hook) = hook {
        hook(repository.path());
    }
}

pub(crate) fn read_only_git_command(git: &Path, filter_drivers: &[String]) -> Command {
    let mut command = Command::new(git);
    apply_git_hardening(&mut command, filter_drivers);
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command
}

/// Run one read-only Git invocation in a pinned repository, bounded.
///
/// The repository is entered through the descriptor it was pinned by -- never re-resolved from a
/// pathname -- and the pin is re-checked immediately afterwards, so a rename between the check and
/// the read makes the answer void rather than wrong.
pub(crate) fn read_pinned(
    repository: &PinnedRepository,
    arguments: &[&str],
    budget: Duration,
) -> Option<String> {
    // The filter inventory is read through the SAME pinned directory: reading it by pathname would
    // let a swap decide which drivers the hardening neutralizes, before the write enters the
    // original descriptor (gpt1).
    let filter_drivers = pinned_filter_drivers(repository)?;
    let mut command = read_only_git_command(Path::new(GIT), &filter_drivers);
    repository.enter(&mut command);
    for argument in arguments {
        command.arg(argument);
    }
    match run_git_bounded(command, budget) {
        Bounded::Finished {
            success: true,
            stdout,
        } if repository.still_pinned() => Some(stdout),
        _ => None,
    }
}

/// [`read_pinned`], returning the child's standard output as bytes.
///
/// For the two reads whose subject IS bytes -- the pre-image gate and the effect post-condition,
/// both `cat-file blob` -- where a declared file that is not valid UTF-8 must survive the round
/// trip rather than refuse a change Beacon wrote correctly.
pub(crate) fn read_pinned_bytes(
    repository: &PinnedRepository,
    arguments: &[&str],
    budget: Duration,
) -> Option<Vec<u8>> {
    let filter_drivers = pinned_filter_drivers(repository)?;
    let mut command = read_only_git_command(Path::new(GIT), &filter_drivers);
    repository.enter(&mut command);
    // `--literal-pathspecs` is not needed here: `cat-file` takes an object name, not a pathspec.
    for argument in arguments {
        command.arg(argument);
    }
    match run_git_bounded_raw(command, None, budget) {
        BoundedRaw::Finished {
            success: true,
            stdout,
        } if repository.still_pinned() => Some(stdout),
        _ => None,
    }
}

/// Run one **mutating** Git invocation in a pinned repository, bounded, with optional stdin.
///
/// The mutating half of the split gpt1's probes forced: gate reads carry `GIT_OPTIONAL_LOCKS=0`
/// because a read that rewrites `.git/index` cannot support a zero-mutation claim, and these
/// invocations are the ones allowed to write. Everything else is the same hardened shape -- no
/// hooks, no filters, no fsmonitor, no protocols, no credential helper, a closed environment.
///
/// `index_file` points Git at a scratch index outside the repository, so `read-tree`,
/// `update-index` and `write-tree` build a tree without touching the repository's own index or its
/// working tree. `--literal-pathspecs` is passed to every invocation because a Proposal path may
/// begin with `:` and pathspec magic is not something a declared path opts into.
///
/// The pin is re-checked after the child exits: a rename between the check and the write makes the
/// result void rather than wrong.
pub(crate) fn write_pinned(
    repository: &PinnedRepository,
    index_file: Option<&Path>,
    identity: Option<&GitIdentity>,
    arguments: &[&str],
    stdin_bytes: Option<&[u8]>,
    budget: Duration,
) -> Option<String> {
    let filter_drivers = pinned_filter_drivers(repository)?;
    let mut command = hardened_git_command(Path::new(GIT), &filter_drivers);
    if let Some(index) = index_file {
        command.env("GIT_INDEX_FILE", index);
    }
    if let Some(identity) = identity {
        command
            .env("GIT_AUTHOR_NAME", &identity.name)
            .env("GIT_AUTHOR_EMAIL", &identity.email)
            .env("GIT_AUTHOR_DATE", &identity.date)
            .env("GIT_COMMITTER_NAME", &identity.name)
            .env("GIT_COMMITTER_EMAIL", &identity.email)
            .env("GIT_COMMITTER_DATE", &identity.date);
    }
    repository.enter(&mut command);
    command.arg("--literal-pathspecs");
    for argument in arguments {
        command.arg(argument);
    }
    #[cfg(debug_assertions)]
    before_pinned_spawn(repository);
    match run_git_bounded_with_stdin(command, stdin_bytes, budget) {
        Bounded::Finished {
            success: true,
            stdout,
        } if repository.still_pinned() => Some(stdout),
        _ => None,
    }
}

/// Every filter driver a pinned repository's own configuration names, read through its descriptor.
///
/// The same data-only `config --file` read `repository_filter_drivers` performs, entered by
/// descriptor rather than by `-C <path>`: a rename between the pin and this read would otherwise
/// let a different repository decide what the hardening suppresses.
fn pinned_filter_drivers(repository: &PinnedRepository) -> Option<Vec<String>> {
    let mut command = closed_git_command(Path::new(GIT));
    repository.enter(&mut command);
    command
        .arg("config")
        .arg("--file")
        // Through the pinned descriptor, not `.git/config`: the pathname is exactly what a swap
        // moves, and this read decides which filter drivers the hardening then suppresses.
        .arg(PinnedRepository::git_dir_child_path("config"))
        .arg("--includes")
        .arg("--name-only")
        .arg("--null")
        .arg("--list");
    let Bounded::Finished { success, stdout } = run_git_bounded(command, READ_BUDGET) else {
        return None;
    };
    if !success {
        // No repository-local configuration is not a failure: it is an empty inventory.
        return repository.still_pinned().then(Vec::new);
    }
    if !repository.still_pinned() {
        return None;
    }
    // The shared parser, not a second one: this inventory used to recognize only `.smudge` and
    // `.clean`, so a driver declared with `filter.<name>.process` alone was never discovered and
    // therefore never neutralized -- reopening the repository-supplied execution boundary the
    // hardened command exists to close (gpt1). One parser, one definition of what a driver is.
    Some(filter_drivers_from_config_output(&stdout))
}

/// Who Beacon says wrote a commit, and when.
///
/// Fixed by the caller rather than read from the environment, so the resulting revision is a
/// function of its inputs: two dispatches of one Candidate onto one base produce one commit id.
#[derive(Debug, Clone)]
pub(crate) struct GitIdentity {
    pub(crate) name: String,
    pub(crate) email: String,
    /// A Git date string. `<unix seconds> +0000` is the form this uses.
    pub(crate) date: String,
}

/// How long a mutating Git invocation may take. Longer than a read: writing objects and updating a
/// checkout is real work.
pub(crate) const WRITE_BUDGET: Duration = Duration::from_secs(60);

/// How long a read-only gate read may take before its answer is unknown.
pub(crate) const READ_BUDGET: Duration = Duration::from_secs(10);

pub(crate) fn hardened_git_command(git: &Path, filter_drivers: &[String]) -> Command {
    let mut command = Command::new(git);
    apply_git_hardening(&mut command, filter_drivers);
    command
}

pub(crate) fn apply_git_hardening(command: &mut Command, filter_drivers: &[String]) {
    configure_closed_git_environment(command);
    command
        .arg("-c")
        .arg(format!("core.hooksPath={DISABLED_HOOKS_PATH}"))
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg("protocol.allow=never")
        .arg("-c")
        .arg("core.sshCommand=")
        .arg("-c")
        .arg("core.askPass=")
        .arg("-c")
        .arg("credential.helper=")
        .arg("-c")
        .arg("gpg.program=")
        .arg("-c")
        .arg("core.editor=")
        .arg("-c")
        .arg("core.pager=");
    for driver in filter_drivers {
        for (field, value) in [
            ("smudge", ""),
            ("clean", ""),
            ("process", ""),
            ("required", "false"),
        ] {
            command.arg("-c").arg(format!("{driver}.{field}={value}"));
        }
    }
}

fn closed_git_command(git: &Path) -> Command {
    let mut command = Command::new(git);
    configure_closed_git_environment(&mut command);
    command
}

/// The environment every Git child of this crate runs under.
///
/// `env_clear` first, so what follows is the whole environment rather than an amendment to the
/// operator's.
///
/// **The last three close the repository's own view of its objects.** A repository can rewrite what
/// its objects *are* without touching a single byte a gate reads: `.git/refs/replace/*` substitutes
/// one object for another, `.git/info/grafts` rewrites parentage, and `.git/shallow` truncates it.
/// All three live inside the repository under measurement, so a gate that honours them is measuring
/// the repository's account of itself. A cold review published the counterexample for the first:
/// with a replacement installed, `diff --name-status base commit` reported only the declared path
/// while the literal graph carried an undeclared one, and dispatch applied it. Each neutralizer was
/// measured against the installed Git before it was written down, and the closed-environment test
/// pins the exact key set so a fourth knob of this class is a deliberate addition rather than a
/// silent omission.
///
/// A genuinely shallow or grafted repository therefore looks incomplete to Beacon and its reads
/// fail rather than succeed against a rewritten graph. That is the fail-closed direction.
fn configure_closed_git_environment(command: &mut Command) {
    command
        .env_clear()
        .env("PATH", GIT_PATH)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_GRAFT_FILE", "/dev/null")
        .env("GIT_SHALLOW_FILE", "/dev/null");
}

/// The keys [`configure_closed_git_environment`] sets, and the only ones a Git child receives.
///
/// Named here rather than only inside the builder so a test can assert the whole set: an omission
/// from this list is what the replacement-ref counterexample was.
#[cfg(test)]
const CLOSED_GIT_ENVIRONMENT_KEYS: [&str; 7] = [
    "PATH",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_CONFIG_GLOBAL",
    "GIT_TERMINAL_PROMPT",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_GRAFT_FILE",
    "GIT_SHALLOW_FILE",
];

/// Run one Git invocation with a deadline.
///
/// Standard output is a pipe that is read only after the child exits. A child
/// that produces more output than the pipe holds therefore blocks and is
/// reported as [`Bounded::TimedOut`] rather than as a short read — the
/// fail-toward-keep direction for every caller here.
///
/// The kill on timeout reaches this child, not a process group; under the
/// hardened configuration above, the bounded `rev-parse`, `config --file`,
/// `worktree add`, `status`, and `worktree remove` invocations spawn no
/// long-lived processes, while process-group interruption belongs to whichever
/// layer runs repository-declared programs.
fn run_git_bounded(command: Command, budget: Duration) -> Bounded {
    run_git_bounded_with_stdin(command, None, budget)
}

/// [`run_git_bounded`], with bytes to feed the child.
///
/// The write happens before the wait loop and the pipe is closed immediately after, because a child
/// reading standard input to EOF -- `hash-object --stdin` is exactly that -- never exits while the
/// pipe is open, and would be reported as a timeout it did not have.
fn run_git_bounded_with_stdin(
    mut command: Command,
    stdin_bytes: Option<&[u8]>,
    budget: Duration,
) -> Bounded {
    if stdin_bytes.is_some() {
        command.stdin(Stdio::piped());
    }
    run_git_bounded_inner(command, stdin_bytes, budget)
}

/// [`Bounded`], before its standard output is required to be text.
///
/// A Candidate carries `Vec<u8>` and neither ADR 0034 nor ADR 0039 narrows a declared file to
/// UTF-8, so the post-condition's `cat-file blob` may legitimately produce bytes that are not.
/// Decoding them was a fail-closed refusal (`EffectDoesNotMatchDeclaration`) of a change Beacon had
/// already written correctly -- the digest is over bytes, and "exact bytes" has to mean bytes all
/// the way out (sol1). There is still one runner; this is its output before the text conversion.
enum BoundedRaw {
    Finished { success: bool, stdout: Vec<u8> },
    TimedOut,
    Failed,
}

fn run_git_bounded_inner(
    command: Command,
    stdin_bytes: Option<&[u8]>,
    budget: Duration,
) -> Bounded {
    match run_git_bounded_raw(command, stdin_bytes, budget) {
        BoundedRaw::Finished { success, stdout } => match String::from_utf8(stdout) {
            Ok(stdout) => Bounded::Finished { success, stdout },
            // Unchanged behaviour for every textual caller: `rev-parse`, `status` and
            // `diff --name-status` producing invalid UTF-8 is not an answer.
            Err(_) => Bounded::Failed,
        },
        BoundedRaw::TimedOut => Bounded::TimedOut,
        BoundedRaw::Failed => Bounded::Failed,
    }
}

fn run_git_bounded_raw(
    mut command: Command,
    stdin_bytes: Option<&[u8]>,
    budget: Duration,
) -> BoundedRaw {
    if stdin_bytes.is_none() {
        command.stdin(Stdio::null());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return BoundedRaw::Failed;
    };
    if let Some(bytes) = stdin_bytes {
        let Some(mut pipe) = child.stdin.take() else {
            let _ = child.kill();
            return BoundedRaw::Failed;
        };
        if std::io::Write::write_all(&mut pipe, bytes).is_err() {
            let _ = child.kill();
            return BoundedRaw::Failed;
        }
        drop(pipe);
    }
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() >= budget {
                    let _ = child.kill();
                    let _ = child.wait();
                    return BoundedRaw::TimedOut;
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return BoundedRaw::Failed;
            }
        }
    };
    let Some(mut pipe) = child.stdout.take() else {
        return BoundedRaw::Failed;
    };
    let mut stdout = Vec::new();
    if pipe.read_to_end(&mut stdout).is_err() {
        return BoundedRaw::Failed;
    }
    BoundedRaw::Finished {
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

    use super::{
        Bounded, CLOSED_GIT_ENVIRONMENT_KEYS, CleanupOutcome, KeepReason, PinnedRepository,
        READ_BUDGET, Worktree, closed_git_command, configure_closed_git_environment,
        run_git_bounded, status_holds,
    };
    use std::process::Command;

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

    /// Build a repository with three commits and return `(root, first, second, third)`.
    fn three_commits(label: &str) -> (TempDir, String, String, String) {
        let root = TempDir::new(label);
        let at = |arguments: &[&str]| -> String {
            let output = Command::new("git")
                .args(arguments)
                .current_dir(&root.path)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {arguments:?} failed");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        at(&["init", "--quiet", "--initial-branch=main", "."]);
        at(&["config", "user.email", "beacon@localhost"]);
        at(&["config", "user.name", "Beacon"]);
        for content in ["one\n", "two\n", "three\n"] {
            fs::write(root.path.join("a.txt"), content).unwrap();
            at(&["add", "-A"]);
            at(&["commit", "--quiet", "-m", content.trim()]);
        }
        let third = at(&["rev-parse", "HEAD"]);
        let second = at(&["rev-parse", "HEAD~1"]);
        let first = at(&["rev-parse", "HEAD~2"]);
        (root, first, second, third)
    }

    /// Run one argument list under the closed environment, from `root`, and return stdout.
    fn closed(root: &Path, arguments: &[&str]) -> String {
        let mut command = closed_git_command(Path::new("/usr/bin/git"));
        command.args(arguments).current_dir(root);
        match run_git_bounded(command, READ_BUDGET) {
            Bounded::Finished { success, stdout } => {
                assert!(success, "closed git {arguments:?} failed");
                stdout.trim().to_owned()
            }
            _ => panic!("closed git {arguments:?} did not finish"),
        }
    }

    #[test]
    fn the_closed_environment_is_exactly_these_keys() {
        // The whole environment a Git child receives, asserted as a set rather than checked one key
        // at a time. A cold review found `GIT_NO_REPLACE_OBJECTS` missing from it, and a missing key
        // is invisible to every test that only asserts the keys it remembers. Adding a fourth
        // object-view knob without adding it here is now a failing test rather than an omission.
        let mut command = Command::new("/usr/bin/git");
        configure_closed_git_environment(&mut command);
        let mut observed: Vec<String> = command
            .get_envs()
            .map(|(key, value)| {
                assert!(value.is_some(), "{key:?} was cleared rather than set");
                key.to_string_lossy().into_owned()
            })
            .collect();
        observed.sort();
        let mut expected: Vec<String> = CLOSED_GIT_ENVIRONMENT_KEYS
            .iter()
            .map(|key| (*key).to_owned())
            .collect();
        expected.sort();
        assert_eq!(observed, expected);
    }

    #[test]
    fn the_closed_environment_reads_the_literal_object_graph() {
        // Three ways a repository rewrites what its own objects ARE, each measured against the
        // installed Git before it was written down. Every one of them lives inside the repository
        // under measurement, so a gate that honours them is reading the repository's account of
        // itself rather than its bytes. The ambient half of each pair is the control: without it, a
        // neutralizer that did nothing would pass.
        let (root, first, second, third) = three_commits("object-view");
        let ambient = |arguments: &[&str]| -> String {
            let output = Command::new("git")
                .args(arguments)
                .current_dir(&root.path)
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        let run = |arguments: &[&str]| {
            assert!(
                Command::new("git")
                    .args(arguments)
                    .current_dir(&root.path)
                    .output()
                    .unwrap()
                    .status
                    .success(),
                "git {arguments:?} failed"
            );
        };

        // 1. A replacement ref substitutes one object for another.
        run(&["replace", "-f", &first, &third]);
        assert_ne!(
            ambient(&["cat-file", "-p", &first]),
            closed(&root.path, &["cat-file", "-p", &first]),
            "the replacement did not change the ambient view, so this proves nothing"
        );
        assert!(
            !closed(&root.path, &["cat-file", "-p", &first]).contains("parent "),
            "the closed environment read the replacement, not the root commit"
        );
        run(&["replace", "-d", &first]);

        // 2. A graft file rewrites parentage.
        fs::create_dir_all(root.path.join(".git/info")).unwrap();
        fs::write(
            root.path.join(".git/info/grafts"),
            format!("{third} {first}\n"),
        )
        .unwrap();
        assert_eq!(
            ambient(&["rev-list", "--parents", "-n1", &third]),
            format!("{third} {first}"),
            "the graft did not take, so this proves nothing"
        );
        assert_eq!(
            closed(&root.path, &["rev-list", "--parents", "-n1", &third]),
            format!("{third} {second}"),
            "the closed environment honoured .git/info/grafts"
        );
        fs::remove_file(root.path.join(".git/info/grafts")).unwrap();

        // 3. A shallow file truncates it.
        fs::write(root.path.join(".git/shallow"), format!("{third}\n")).unwrap();
        assert_eq!(
            ambient(&["rev-list", "--parents", "-n1", &third]),
            third,
            "the shallow file did not take, so this proves nothing"
        );
        assert_eq!(
            closed(&root.path, &["rev-list", "--parents", "-n1", &third]),
            format!("{third} {second}"),
            "the closed environment honoured .git/shallow"
        );
    }

    #[test]
    fn a_check_that_never_finishes_is_a_timeout_and_never_a_clean_answer() {
        let directory = TempDir::new("gate-timeout");
        let hanging = program(&directory.path, "sleep 30\n");

        let holds = status_holds(&hanging, &directory.path, Duration::from_millis(50), &[]);

        assert_eq!(holds, vec![KeepReason::GateTimedOut]);
    }

    #[test]
    fn a_check_that_errors_is_a_failure_and_never_a_clean_answer() {
        let directory = TempDir::new("gate-error");
        let failing = program(&directory.path, "exit 3\n");

        let holds = status_holds(&failing, &directory.path, Duration::from_secs(5), &[]);

        assert_eq!(holds, vec![KeepReason::CheckFailed]);
    }

    #[test]
    fn a_check_program_that_cannot_start_is_a_failure_and_never_a_clean_answer() {
        let directory = TempDir::new("gate-absent");

        let holds = status_holds(
            Path::new("/nonexistent/beacon/git"),
            &directory.path,
            Duration::from_secs(5),
            &[],
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

        let outcome = worktree.close_with(&saboteur, Duration::from_secs(5), &[]);

        assert_eq!(
            outcome,
            CleanupOutcome::Kept {
                reasons: vec![KeepReason::NotAWorktree],
            }
        );
        assert!(path.is_dir(), "the tree was kept, so it is still on disk");
    }

    #[test]
    fn a_pinned_repository_is_entered_by_descriptor_and_not_by_its_pathname() {
        // gpt1: replacing `enter` with `current_dir(path)` left both claimed guards green, because
        // neither distinguished the two. This one does, without a race: the directory is renamed
        // away AFTER the pin, so a child entering by pathname cannot start at all while a child
        // entering by descriptor runs in the directory that was opened -- and reports its new name,
        // which is how we know it is the same inode rather than a re-resolution.
        let root = std::env::temp_dir().join(format!("beacon-enter-{}", Uuid::new_v4()));
        let repository = root.join("repository");
        let moved = root.join("moved");
        fs::create_dir_all(repository.join(".git/objects")).unwrap();

        let pinned = PinnedRepository::open(&repository).unwrap();
        fs::rename(&repository, &moved).unwrap();
        assert!(
            !repository.exists(),
            "the pathname must not resolve for this test to mean anything"
        );

        let mut command = Command::new("/bin/sh");
        pinned.enter(&mut command);
        command.arg("-c").arg("pwd");
        let Bounded::Finished { success, stdout } =
            run_git_bounded(command, Duration::from_secs(10))
        else {
            panic!("the child did not finish");
        };
        assert!(success, "a child entering by descriptor could not start");
        assert_eq!(
            fs::canonicalize(stdout.trim()).unwrap(),
            fs::canonicalize(&moved).unwrap(),
            "the child did not enter the directory that was pinned"
        );

        // The control: entering the same pathname the ordinary way cannot start at all.
        let mut by_path = Command::new("/bin/sh");
        by_path.current_dir(&repository).arg("-c").arg("pwd");
        assert!(
            matches!(
                run_git_bounded(by_path, Duration::from_secs(10)),
                Bounded::Failed
            ),
            "a child entering by pathname started after the rename"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_pinned_repository_notices_a_rename_and_a_decoy_at_its_pathname() {
        // gpt1's path-swap probe, as a deterministic unit test rather than a race. `git -C <path>`
        // re-resolves the name on every invocation, so a directory renamed away and replaced by an
        // equivalent one redirects everything that follows. The pin is device and inode, which is
        // what "the same directory" means to the kernel.
        let root = std::env::temp_dir().join(format!("beacon-pin-{}", Uuid::new_v4()));
        let repository = root.join("repository");
        let elsewhere = root.join("moved");
        fs::create_dir_all(repository.join(".git/objects")).unwrap();
        fs::create_dir_all(root.join("decoy/.git/objects")).unwrap();

        let pinned = PinnedRepository::open(&repository).expect("a directory with .git is a store");
        assert!(
            pinned.still_pinned(),
            "a untouched repository is not pinned"
        );

        // Renamed away: the pathname resolves to nothing.
        fs::rename(&repository, &elsewhere).unwrap();
        assert!(
            !pinned.still_pinned(),
            "a renamed repository still read as pinned"
        );

        // And renamed away with an equivalent repository put in its place: the pathname resolves,
        // and to the wrong directory.
        fs::rename(root.join("decoy"), &repository).unwrap();
        assert!(
            !pinned.still_pinned(),
            "a decoy at the pinned pathname read as the repository that was measured"
        );

        // The original, wherever it now lives, is still the directory that was pinned.
        let moved = PinnedRepository::open(&elsewhere).unwrap();
        assert_eq!(moved.identity(), pinned.identity());

        fs::remove_dir_all(&root).unwrap();
    }
}
