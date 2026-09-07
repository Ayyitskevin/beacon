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

/// Whether the one clock for a bounded invocation has reached its deadline.
///
/// Equality is expired: a budget is a bound, not a target plus an implicit polling grace.
fn reached_deadline(elapsed: Duration, budget: Duration) -> bool {
    elapsed >= budget
}

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

/// A repository this process has pinned: the directory it opened, the Git directory inside it, and
/// the identity of both.
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
    ///
    /// The Git directory is a different case, because it must survive the exec. `dup2` clears
    /// close-on-exec on the copy it makes -- except when there is no copy to make. POSIX says
    /// `dup2(n, n)` on an open descriptor does nothing and returns `n`, so when the pin has itself
    /// opened on [`PINNED_GIT_DIR_FD`] the placement is a successful no-op, close-on-exec survives
    /// it, and the exec closes the very descriptor `GIT_DIR` names (R49, grok's R41 root cause).
    /// That case clears the flag directly instead. The child's descriptor table is its own after
    /// the fork, so this never reaches the pin this process holds.
    pub(crate) fn enter(&self, command: &mut Command) {
        use std::os::fd::AsRawFd;
        let fd = self.directory.as_raw_fd();
        let git_fd = self.git_directory.as_raw_fd();
        // SAFETY: `fchdir`, `dup2` and `fcntl` are async-signal-safe, touch only this child's
        // working directory and descriptor table, and both descriptors outlive the spawn because
        // `self` is borrowed for the call.
        unsafe {
            command.pre_exec(move || {
                if libc::fchdir(fd) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if git_fd == PINNED_GIT_DIR_FD {
                    // Already where it has to be, so there is nothing to copy -- only the flag to
                    // clear. `F_SETFD` rather than a detour through a scratch descriptor: the
                    // detour allocates in the pre-exec window and can fail with `EMFILE` in
                    // exactly the exhausted descriptor table that produces this case, so the
                    // fallback would be least available when it is needed most.
                    if libc::fcntl(git_fd, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                } else if libc::dup2(git_fd, PINNED_GIT_DIR_FD) < 0 {
                    // `dup2` clears `O_CLOEXEC` on the copy, which is the point: the child must
                    // still hold this descriptor after the exec that resolves `GIT_DIR`.
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
    let filter_drivers = pinned_filter_drivers(repository).ok()?;
    let mut command = read_only_git_command(Path::new(GIT), &filter_drivers);
    repository.enter(&mut command);
    for argument in arguments {
        command.arg(argument);
    }
    // R47: no exit arithmetic here either. These reads answer negatively in silence --
    // `symbolic-ref --quiet` on a detached HEAD is the one that does -- and every other non-zero
    // exit carries a diagnostic. The caller's `Option` still folds the negative answer and the
    // failure together; that collapse is one layer UP and outside this lane (see the report).
    match classify_pinned_read(
        run_git_bounded_raw(command, None, budget),
        repository.still_pinned(),
        AbsenceWitness::Silence,
    ) {
        PinnedRead::Bytes(stdout) => String::from_utf8(stdout).ok(),
        PinnedRead::NotFound | PinnedRead::Unreadable(_) => None,
    }
}

/// [`read_pinned`], returning the child's **exit code** rather than its output.
///
/// For the reads whose answer IS the exit code: `merge-base --is-ancestor` says "yes" with 0 and
/// "no" with 1, and anything else is an error that must not be read as either. `read_pinned` cannot
/// express that -- it returns `None` for any unsuccessful child, so "no" and "could not tell" arrive
/// as one value, which is the absent/dead/broken collapse this crate refuses everywhere else.
///
/// `None` means the read did not answer: the child could not start, timed out, was signalled, or the
/// pin broke. A caller that needs a definite answer treats `None` as unknown, never as "no".
pub(crate) fn read_pinned_exit(
    repository: &PinnedRepository,
    arguments: &[&str],
    budget: Duration,
) -> Option<i32> {
    let filter_drivers = pinned_filter_drivers(repository).ok()?;
    let mut command = read_only_git_command(Path::new(GIT), &filter_drivers);
    repository.enter(&mut command);
    for argument in arguments {
        command.arg(argument);
    }
    let raw = run_git_bounded_raw(command, None, budget);
    let code = raw.exit_code();
    // R47: the classifier decides whether the code is an ANSWER; only then is it carried through.
    // Before, any finished child in a pinned repository handed its code back, so a broken
    // repository's 128 arrived beside the 0 and 1 that mean yes and no.
    match classify_pinned_read(raw, repository.still_pinned(), AbsenceWitness::Silence) {
        PinnedRead::Bytes(_) => Some(0),
        PinnedRead::NotFound => code,
        PinnedRead::Unreadable(_) => None,
    }
}

/// [`read_pinned`], returning the child's standard output as bytes.
///
/// For the two reads whose subject IS bytes -- the pre-image gate and the effect post-condition,
/// both `cat-file blob` -- where a declared file that is not valid UTF-8 must survive the round
/// trip rather than refuse a change Beacon wrote correctly.
/// WHY a pinned byte read produced no bytes, when the reason is not "Git says it is not there".
///
/// R46: an `Option` here erased the difference between a comparison that could not be made and an
/// object that is absent, and `pre_images_hold` then reported both as `PreImageMismatch` -- a
/// claim about a comparison that never happened. Each arm is a read that FAILED; none of them is
/// evidence about the tree's contents.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PinnedReadFailure {
    /// RETIRED (R50). The filter-driver inventory could not be read.
    ///
    /// Emitted by the builds between R46 and R50, which carried an inventory failure as a
    /// `PreImageUnreadable` subcase because a distinct outcome would have rebuilt the receipts
    /// CHECK. R48 removed that CHECK and R50 gave the fact its own outcome, so no new dispatch
    /// produces this. Retained so Receipts already recorded stay decodable -- the same reason
    /// `DispatchOutcome::Admissible` is retained -- and for no other.
    FilterInventoryUnreadable,
    /// RETIRED (R50), for the same reason and on the same terms as the variant above. Was R47's
    /// "git refused the inventory read and said why".
    FilterInventoryRefused { message: String },
    /// The `cat-file` child could not be started or could not be waited on.
    ChildFailed,
    /// The child was terminated by a signal and has no exit code.
    Signalled,
    /// The child outlived the read budget.
    TimedOut,
    /// The read returned, but the repository was no longer the one that was pinned, so its bytes
    /// describe something this attempt never measured.
    PinBrokenAfterRead,
    /// git refused the read and did NOT say the path is absent from the tree. R46.3: `cat-file`
    /// exits 128 for "path does not exist in this tree" AND for "this repository is broken", so
    /// the exit code alone cannot separate them. Anything git says that is not the absence
    /// message is this -- FAIL CLOSED -- and it carries what git said so the evidence survives.
    ReadRefused { message: String },
    /// A silent non-zero exit is not evidence that the subject is absent.
    SilentExit { code: i32 },
}

/// Why the filter-driver inventory could not be established (R50).
///
/// R47 carried this as two `PinnedReadFailure` subcases under `PreImageUnreadable`, because a
/// distinct outcome would have rebuilt the receipts CHECK (opus2's contradiction 5). R48 removed
/// the CHECK, so the fact gets its own type and its own outcome. It is not a pre-image failure and
/// never was: the inventory is a PRECONDITION of every pinned read -- it decides which filter
/// drivers the hardening suppresses -- so a Receipt reporting it as a pre-image failure named the
/// wrong read, and the two reads it could name are not even in the same gate.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterInventoryFailure {
    /// git refused the read and said why: a configuration that is missing, malformed, or one this
    /// process may not read. The message is bounded and escaped for a Receipt (R28.4).
    Refused { message: String },
    /// The read did not happen, or its result cannot be trusted: a pin broken under it, a child
    /// that could not start, a signal, a timeout, or output that is not text. There is no message
    /// because git produced none.
    Unreadable,
}

impl FilterInventoryFailure {
    /// The failure's NAME, for the Receipt line that says why the inventory could not be read.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Refused { .. } => "refused",
            Self::Unreadable => "unreadable",
        }
    }
}

/// How a read site can SHOW that a non-zero exit is an absence rather than a failure (R47).
///
/// R46.3 keyed `cat-file`'s absence on what git said ABOUT THE PATH. R47 generalises that one
/// site's rule to every site: a non-zero exit is an absence only when the site can point at the
/// evidence for it, and different reads evidence an absence differently. Nothing is classified on
/// the exit code, anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbsenceWitness {
    /// git said NOTHING -- no output and no diagnostic. `merge-base --is-ancestor` answering "no",
    /// and a configuration that names no entries. Measured, not assumed: at these sites git is
    /// silent when the answer is negative and speaks when it failed.
    Silence,
    /// git NAMED the subject as absent. `cat-file`'s `path 'x' does not exist in 'REV'` (R46.3).
    Message(&'static str),
}

/// Does this finished child SHOW the absence its site would need to see?
///
/// Empty stdout is required of both witnesses and is not negotiable: a child that produced bytes
/// produced OUTPUT, and calling that an absence discards the very bytes it read.
fn shows_absence(witness: AbsenceWitness, stdout: &[u8], stderr: &[u8]) -> bool {
    if !stdout.is_empty() {
        return false;
    }
    match witness {
        AbsenceWitness::Silence => !said_anything_diagnostic(stderr),
        AbsenceWitness::Message(said) => String::from_utf8_lossy(stderr).contains(said),
    }
}

/// git's advisory channel, which is NOT a diagnostic about the read.
///
/// MEASURED, and it cost a shipped test to find: a pinned `merge-base --is-ancestor` writes seven
/// `hint:` lines about `<GIT_DIR>/info/grafts` on this box -- on the SUCCESSFUL answer and on the
/// negative one alike (`rev-parse` writes none, so it is the commit-graph walk that loads them).
/// Under a literal "any stderr means the read failed" rule, `merge-base`'s "no" became "could not
/// tell" -- exactly the collapse R14 built `a_read_whose_answer_is_its_exit_code_tells_no_apart_from_could_not_tell`
/// to prevent, arriving from the other direction.
///
/// `hint:` is git's documented advice prefix and the thing `advice.*` switches off, so it is the
/// one prefix that is safe to discount. Everything else on stderr counts, which keeps the rule
/// FAIL CLOSED: a failure that is not prefixed at all is still read as a failure.
const ADVICE_PREFIX: &str = "hint:";

fn said_anything_diagnostic(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .any(|line| !line.is_empty() && !line.starts_with(ADVICE_PREFIX))
}

/// What git says when the path is simply not in that tree: `path 'x' does not exist in 'REV'`.
///
/// Keyed on what was said ABOUT THE PATH, not on the exit code, and not on the whole sentence: the
/// quoted path and revision are the caller's own strings and the wording around them is git's.
const PATH_ABSENT_FROM_TREE: &str = "does not exist in";

/// git's message, bounded and escaped for a Receipt. The cap is R28.4's: a diagnostic is evidence,
/// not a channel for arbitrary bytes to reach a reader.
fn quoted_git_message(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    let mut message: String = trimmed.chars().take(120).collect();
    if trimmed.chars().count() > 120 {
        message.push('…');
    }
    message
}

impl PinnedReadFailure {
    /// The subcase's NAME, for the Receipt line that says WHICH read failed.
    pub fn name(&self) -> &'static str {
        match self {
            Self::FilterInventoryUnreadable => "filter_inventory_unreadable",
            Self::FilterInventoryRefused { .. } => "filter_inventory_refused",
            Self::ChildFailed => "child_failed",
            Self::Signalled => "signalled",
            Self::TimedOut => "timed_out",
            Self::PinBrokenAfterRead => "pin_broken_after_read",
            Self::ReadRefused { .. } => "read_refused",
            Self::SilentExit { .. } => "silent_exit",
        }
    }
}

/// What a pinned byte read established. THREE answers, never two (R46/R46.1).
///
/// `NotFound` is a fact about the tree -- Git ran, under the hardening, in the pinned repository,
/// and reported no such object. `Unreadable` is the absence of a fact. Collapsing them is the
/// absent/broken collapse this codebase refuses on the write side already (R28.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinnedRead {
    /// The child exited 0 in the still-pinned repository. These are its bytes.
    Bytes(Vec<u8>),
    /// The child ran and reported a non-zero status: Git says the object is not there.
    NotFound,
    /// The read did not happen, or its result cannot be trusted. It says NOTHING about the tree.
    Unreadable(PinnedReadFailure),
}

impl PinnedRead {
    /// The bytes, if any were read. For callers whose behaviour does not distinguish the two
    /// no-bytes answers; a caller that refuses must distinguish them.
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            Self::NotFound | Self::Unreadable(_) => None,
        }
    }
}

pub(crate) fn read_pinned_bytes(
    repository: &PinnedRepository,
    arguments: &[&str],
    budget: Duration,
) -> Result<PinnedRead, FilterInventoryFailure> {
    // R50: the inventory leaves by the ERROR channel, not as a `PinnedRead` variant. It is a
    // PRECONDITION of this read rather than one of its three answers, and folding it in was what
    // let R47's refusal name the byte read when the read that failed was the inventory. The type
    // now says which: `Err` is "the hardening could not be established, so no child ran"; `Ok` is
    // "a child ran, and here is what it established".
    let filter_drivers = pinned_filter_drivers(repository)?;
    let mut command = read_only_git_command(Path::new(GIT), &filter_drivers);
    repository.enter(&mut command);
    // `--literal-pathspecs` is not needed here: `cat-file` takes an object name, not a pathspec.
    for argument in arguments {
        command.arg(argument);
    }
    Ok(classify_pinned_read(
        run_git_bounded_raw(command, None, budget),
        repository.still_pinned(),
        // `cat-file` says so when the path is not in the tree; silence from it is not an absence.
        AbsenceWitness::Message(PATH_ABSENT_FROM_TREE),
    ))
}

/// Which of the three answers a finished (or unfinished) child amounts to.
///
/// Split out from `read_pinned_bytes` so every subcase can be DRIVEN in a test: four of the five
/// -- a child that cannot start, one killed by a signal, one that outlives its budget, and a pin
/// broken after the read -- cannot be constructed from an integration test without racing the
/// kernel or removing `git` from the machine. The one that can (an unreadable filter inventory)
/// has its constructed-fault test at the dispatch level as well.
fn classify_pinned_read(
    raw: BoundedRaw,
    still_pinned: bool,
    absence: AbsenceWitness,
) -> PinnedRead {
    match raw {
        // The pin is checked BEFORE the code is read: bytes from a repository that is no longer
        // the pinned one are not evidence about it, whatever the child's status said.
        BoundedRaw::Finished {
            code,
            stdout,
            stderr,
        } => {
            if !still_pinned {
                return PinnedRead::Unreadable(PinnedReadFailure::PinBrokenAfterRead);
            }
            match code {
                Some(0) => PinnedRead::Bytes(stdout),
                // R46.3, generalised by R47: an absence is what the site can SHOW, never what the
                // exit code was. Anything else non-zero is a failure carrying what git said.
                Some(code) if matches!(absence, AbsenceWitness::Silence) && !said_anything_diagnostic(&stderr) && stdout.is_empty() =>
                    PinnedRead::Unreadable(PinnedReadFailure::SilentExit { code }),
                Some(_) if shows_absence(absence, &stdout, &stderr) => PinnedRead::NotFound,
                Some(_) => PinnedRead::Unreadable(PinnedReadFailure::ReadRefused {
                    message: quoted_git_message(&stderr),
                }),
                None => PinnedRead::Unreadable(PinnedReadFailure::Signalled),
            }
        }
        BoundedRaw::TimedOut => PinnedRead::Unreadable(PinnedReadFailure::TimedOut),
        BoundedRaw::Failed => PinnedRead::Unreadable(PinnedReadFailure::ChildFailed),
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
    let filter_drivers = pinned_filter_drivers(repository).ok()?;
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
fn pinned_filter_drivers(
    repository: &PinnedRepository,
) -> Result<Vec<String>, FilterInventoryFailure> {
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
    // R47: through the ONE classifier, with this site's witness. `config --list` over a
    // configuration that names no entries exits 0 and says nothing (MEASURED on git 2.43.0 --
    // tools/r47/logs/measure-config-site.log), so silence with a non-zero exit is the only shape
    // that could ever be an empty inventory here, and every diagnostic is a failure.
    //
    // What this replaces: `if !success { return still_pinned().then(Vec::new) }` -- an unreadable,
    // missing or malformed configuration became an EMPTY filter-driver inventory, and that
    // inventory is what decides which drivers the hardening suppresses. The read then proceeded
    // into the repository with the hardening switched off (opus, R46 report; R47).
    match classify_pinned_read(
        run_git_bounded_raw(command, None, READ_BUDGET),
        repository.still_pinned(),
        AbsenceWitness::Silence,
    ) {
        // The shared parser, not a second one: this inventory used to recognize only `.smudge` and
        // `.clean`, so a driver declared with `filter.<name>.process` alone was never discovered
        // and therefore never neutralized -- reopening the repository-supplied execution boundary
        // the hardened command exists to close (gpt1). One parser, one definition of a driver.
        PinnedRead::Bytes(stdout) => match String::from_utf8(stdout) {
            Ok(stdout) => Ok(filter_drivers_from_config_output(&stdout)),
            // Unchanged: an inventory that is not text is not an inventory.
            Err(_) => Err(FilterInventoryFailure::Unreadable),
        },
        // Silence with a non-zero exit: git found nothing to list and said nothing about it.
        PinnedRead::NotFound => Ok(Vec::new()),
        // git said WHY, so the Receipt can too.
        PinnedRead::Unreadable(PinnedReadFailure::ReadRefused { message }) => {
            Err(FilterInventoryFailure::Refused { message })
        }
        // A pin broken under the read, a child that could not start, a signal, a timeout: the
        // inventory was not established and there is no message to carry.
        PinnedRead::Unreadable(_) => Err(FilterInventoryFailure::Unreadable),
    }
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
    /// `code` is `None` when the child was terminated by a signal and therefore has no exit code.
    Finished {
        code: Option<i32>,
        stdout: Vec<u8>,
        /// What git SAID. Kept because an exit code cannot tell "this path is not in that tree"
        /// from "this repository is broken" -- both are 128 (R46.3).
        stderr: Vec<u8>,
    },
    TimedOut,
    Failed,
}

impl BoundedRaw {
    /// The child's exit code, for the ONE site whose answer IS the code
    /// (`merge-base --is-ancestor`). Reading it is not classifying on it: the classifier still
    /// decides whether the code is an answer at all, and this only carries it through.
    fn exit_code(&self) -> Option<i32> {
        match self {
            Self::Finished { code, .. } => *code,
            Self::TimedOut | Self::Failed => None,
        }
    }
}

fn run_git_bounded_inner(
    command: Command,
    stdin_bytes: Option<&[u8]>,
    budget: Duration,
) -> Bounded {
    match run_git_bounded_raw(command, stdin_bytes, budget) {
        BoundedRaw::Finished { code, stdout, .. } => match String::from_utf8(stdout) {
            Ok(stdout) => Bounded::Finished {
                success: code == Some(0),
                stdout,
            },
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
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // One clock for the whole invocation: a start that cannot be reached inside the budget is a
    // check that did not finish, and counting it from after the spawn would let a retry below add
    // a second budget's worth of waiting to every call.
    let started = Instant::now();
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            // `ETXTBSY` is not an answer about this worktree. The kernel refuses to exec a file
            // that any process holds open for writing, and Beacon is multi-threaded: a thread that
            // forks while a sibling holds a freshly written program open hands that write
            // descriptor to its child, and the program stays busy until the child reaches its own
            // exec (rust-lang/rust#114554). Wait it out inside the budget rather than record a
            // failure. Every other spawn error is the answer.
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                if reached_deadline(started.elapsed(), budget) {
                    return BoundedRaw::TimedOut;
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return BoundedRaw::Failed,
        }
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
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Completion is an answer only while it is inside the same invocation budget
                // whose clock started before spawn. Observing a finished child after that clock
                // reached the deadline cannot add a grace window merely because `try_wait` was
                // polled first.
                if reached_deadline(started.elapsed(), budget) {
                    return BoundedRaw::TimedOut;
                }
                break status;
            }
            Ok(None) => {
                if reached_deadline(started.elapsed(), budget) {
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
    // Read after the child exited, like stdout above: git's diagnostics are one line and cannot
    // fill the pipe, and a child that somehow did would be killed by the budget rather than
    // deadlock here.
    let mut stderr = Vec::new();
    if let Some(mut pipe) = child.stderr.take()
        && pipe.read_to_end(&mut stderr).is_err()
    {
        return BoundedRaw::Failed;
    }
    BoundedRaw::Finished {
        code: status.code(),
        stdout,
        stderr,
    }
}

#[cfg(test)]
mod r46_read_classification {
    use super::{
        AbsenceWitness, BoundedRaw, PATH_ABSENT_FROM_TREE, PinnedRead, PinnedReadFailure,
        classify_pinned_read,
    };

    /// Every R46 case is a `cat-file` read, whose absence witness is git naming the path.
    const CAT_FILE: AbsenceWitness = AbsenceWitness::Message(PATH_ABSENT_FROM_TREE);

    /// R46: each subcase is DRIVEN, one per constructed fault, and asserted BY NAME.
    ///
    /// The fault is constructed at the boundary the product itself classifies: a child that could
    /// not start, one killed by a signal, one that outlived its budget, a pin that broke after the
    /// read. Driving them here rather than through a live `git` is deliberate -- a test that
    /// removed `git` from the machine or raced a rename would be testing the harness.
    #[test]
    fn a_child_that_could_not_start_is_unreadable_and_says_so() {
        assert_eq!(
            classify_pinned_read(BoundedRaw::Failed, true, CAT_FILE),
            PinnedRead::Unreadable(PinnedReadFailure::ChildFailed)
        );
    }

    #[test]
    fn a_child_killed_by_a_signal_is_unreadable_and_says_so() {
        assert_eq!(
            classify_pinned_read(
                BoundedRaw::Finished {
                    code: None,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                },
                true,
                CAT_FILE,
            ),
            PinnedRead::Unreadable(PinnedReadFailure::Signalled)
        );
    }

    #[test]
    fn a_child_that_outlived_its_budget_is_unreadable_and_says_so() {
        assert_eq!(
            classify_pinned_read(BoundedRaw::TimedOut, true, CAT_FILE),
            PinnedRead::Unreadable(PinnedReadFailure::TimedOut)
        );
    }

    /// The pin is checked BEFORE the exit code, so bytes from a repository that is no longer the
    /// pinned one are `Unreadable` even though the child succeeded. The old code guarded only the
    /// success arm, which let a non-zero exit in an unpinned repository read as "not there".
    #[test]
    fn a_pin_broken_after_the_read_is_unreadable_even_when_the_child_succeeded() {
        assert_eq!(
            classify_pinned_read(
                BoundedRaw::Finished {
                    code: Some(0),
                    stdout: b"bytes this attempt never measured".to_vec(),
                    stderr: Vec::new(),
                },
                false,
                CAT_FILE,
            ),
            PinnedRead::Unreadable(PinnedReadFailure::PinBrokenAfterRead)
        );
        assert_eq!(
            classify_pinned_read(
                BoundedRaw::Finished {
                    code: Some(1),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                },
                false,
                CAT_FILE,
            ),
            PinnedRead::Unreadable(PinnedReadFailure::PinBrokenAfterRead)
        );
    }

    /// R47 (RED against R46's classifier, kept as the item-1 bar): an absence has NOTHING to show.
    ///
    /// A child that wrote bytes to stdout and then exited non-zero produced OUTPUT, not an
    /// absence. R46 keyed only on stderr, so a truncated read whose diagnostic happened to carry
    /// the absence wording read as "Git says it is not there" while its own bytes sat in `stdout`.
    #[test]
    fn a_non_zero_exit_that_produced_output_is_never_an_absence() {
        assert_eq!(
            classify_pinned_read(
                BoundedRaw::Finished {
                    code: Some(128),
                    stdout: b"half a blob".to_vec(),
                    stderr: b"fatal: path 'a.txt' does not exist in 'HEAD'".to_vec(),
                },
                true,
                CAT_FILE,
            ),
            PinnedRead::Unreadable(PinnedReadFailure::ReadRefused {
                message: "fatal: path 'a.txt' does not exist in 'HEAD'".to_owned(),
            }),
            "a read that produced bytes and then failed was reported as an absence"
        );
    }

    /// R46.3: NotFound is what GIT SAID about the path, never what the exit code was.
    ///
    /// `cat-file` exits 128 for "path does not exist in this tree" and for "this repository is
    /// broken". Classifying on the code made the second satisfy "declared absent, and absent" --
    /// R46's own collapse, one level down, on the boundary this lane exists to close.
    #[test]
    fn a_broken_repository_is_unreadable_and_never_not_found() {
        let broken = classify_pinned_read(
            BoundedRaw::Finished {
                code: Some(128),
                stdout: Vec::new(),
                stderr: b"fatal: not a git repository (or any of the parent directories): .git"
                    .to_vec(),
            },
            true,
            CAT_FILE,
        );
        assert_eq!(
            broken,
            PinnedRead::Unreadable(PinnedReadFailure::ReadRefused {
                message: "fatal: not a git repository (or any of the parent directories): .git"
                    .to_owned(),
            }),
            "a broken repository was read as a path that is not there"
        );
        assert!(
            !matches!(broken, PinnedRead::NotFound),
            "the exit code decided this, not what git said"
        );
    }

    /// FAIL CLOSED: an exit git explains in words this build does not recognise is unreadable.
    #[test]
    fn an_unrecognised_diagnostic_is_unreadable_rather_than_assumed_absent() {
        assert_eq!(
            classify_pinned_read(
                BoundedRaw::Finished {
                    code: Some(128),
                    stdout: Vec::new(),
                    stderr: b"fatal: something this build has never seen".to_vec(),
                },
                true,
                CAT_FILE,
            ),
            PinnedRead::Unreadable(PinnedReadFailure::ReadRefused {
                message: "fatal: something this build has never seen".to_owned(),
            })
        );
        // ...and a non-zero exit with NOTHING said is not evidence of absence either.
        assert!(matches!(
            classify_pinned_read(
                BoundedRaw::Finished {
                    code: Some(1),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                },
                true,
                CAT_FILE,
            ),
            PinnedRead::Unreadable(PinnedReadFailure::ReadRefused { .. })
        ));
    }

    /// git's message is bounded and trimmed before it can reach a Receipt (R28.4's cap).
    #[test]
    fn a_git_message_reaches_the_receipt_bounded() {
        let long = vec![b'x'; 400];
        let PinnedRead::Unreadable(PinnedReadFailure::ReadRefused { message }) =
            classify_pinned_read(
                BoundedRaw::Finished {
                    code: Some(128),
                    stdout: Vec::new(),
                    stderr: long,
                },
                true,
                CAT_FILE,
            )
        else {
            panic!("a refused read must carry what git said");
        };
        assert_eq!(message.chars().count(), 121, "120 characters and the mark");
        assert!(message.ends_with('…'));
    }

    /// The two answers that are FACTS, and they are not the same fact.
    #[test]
    fn a_non_zero_exit_in_a_pinned_repository_is_not_found_and_never_unreadable() {
        assert_eq!(
            classify_pinned_read(
                BoundedRaw::Finished {
                    code: Some(128),
                    stdout: Vec::new(),
                    stderr: b"fatal: path 'gone.txt' does not exist in 'HEAD'".to_vec(),
                },
                true,
                CAT_FILE,
            ),
            PinnedRead::NotFound,
            "Git ran and reported no such object: that is evidence about the tree"
        );
        assert_eq!(
            classify_pinned_read(
                BoundedRaw::Finished {
                    code: Some(0),
                    stdout: b"the bytes".to_vec(),
                    stderr: Vec::new(),
                },
                true,
                CAT_FILE,
            ),
            PinnedRead::Bytes(b"the bytes".to_vec())
        );
    }

    /// `bytes()` folds the two no-bytes answers for callers that do not refuse on the difference,
    /// and a caller that DOES refuse must not reach for it.
    #[test]
    fn bytes_is_none_for_both_no_bytes_answers() {
        assert!(PinnedRead::NotFound.bytes().is_none());
        assert!(
            PinnedRead::Unreadable(PinnedReadFailure::TimedOut)
                .bytes()
                .is_none()
        );
        assert_eq!(
            PinnedRead::Bytes(b"x".to_vec()).bytes(),
            Some(b"x".as_slice())
        );
    }

    #[test]
    fn sol1_exit_128_with_empty_stderr_is_not_an_absence_at_every_silent_site() {
        assert_eq!(
            classify_pinned_read(BoundedRaw::Finished { code: Some(128), stdout: Vec::new(), stderr: Vec::new() }, true, AbsenceWitness::Silence),
            PinnedRead::Unreadable(PinnedReadFailure::SilentExit { code: 128 })
        );
    }

    /// R47: the two witnesses are not interchangeable, and neither is the exit code.
    ///
    /// `config --list` and `merge-base --is-ancestor` answer negatively in SILENCE; `cat-file`
    /// answers negatively by NAMING the path. A site that accepted the other's evidence would be
    /// back to classifying on the exit code with extra steps.
    #[test]
    fn an_absence_is_what_the_site_can_show_and_the_sites_show_it_differently() {
        let silent = || BoundedRaw::Finished {
            code: Some(1),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert_eq!(
            classify_pinned_read(silent(), true, AbsenceWitness::Silence),
            PinnedRead::Unreadable(PinnedReadFailure::SilentExit { code: 1 }),
            "a silent non-zero exit is not evidence of absence"
        );
        assert_eq!(
            classify_pinned_read(silent(), true, CAT_FILE),
            PinnedRead::Unreadable(PinnedReadFailure::ReadRefused {
                message: String::new(),
            }),
            "cat-file said nothing about the path, so nothing shows the path is absent"
        );

        let spoke = || BoundedRaw::Finished {
            code: Some(128),
            stdout: Vec::new(),
            stderr:
                b"fatal: unable to read config file '/proc/self/fd/100/config': Permission denied"
                    .to_vec(),
        };
        assert_eq!(
            classify_pinned_read(spoke(), true, AbsenceWitness::Silence),
            PinnedRead::Unreadable(PinnedReadFailure::ReadRefused {
                message:
                    "fatal: unable to read config file '/proc/self/fd/100/config': Permission denied"
                        .to_owned(),
            }),
            "git explained why it failed and the site read that as an empty answer"
        );
        assert_eq!(
            classify_pinned_read(spoke(), true, CAT_FILE),
            PinnedRead::Unreadable(PinnedReadFailure::ReadRefused {
                message:
                    "fatal: unable to read config file '/proc/self/fd/100/config': Permission denied"
                        .to_owned(),
            })
        );
    }

    /// R47, measured the hard way: git's advisory channel is not a diagnostic about the read.
    ///
    /// A pinned `merge-base --is-ancestor` writes seven `hint:` lines about `<GIT_DIR>/info/grafts`
    /// on this box, on the successful answer and the negative one alike. Reading "any stderr" as a
    /// failure turned its legitimate "no" into "could not tell" -- R14's collapse, from the other
    /// side. Only `hint:` is discounted; anything else keeps the rule fail-closed.
    #[test]
    fn git_advice_is_not_a_diagnostic_but_everything_else_is() {
        let hint = b"hint: Support for <GIT_DIR>/info/grafts is deprecated\nhint: \nhint: Turn this message off by running\n";
        assert_eq!(
            classify_pinned_read(
                BoundedRaw::Finished {
                    code: Some(1),
                    stdout: Vec::new(),
                    stderr: hint.to_vec(),
                },
                true,
                AbsenceWitness::Silence,
            ),
            PinnedRead::Unreadable(PinnedReadFailure::SilentExit { code: 1 }),
            "advice-only stderr remains a silent exit"
        );

        let mut warned =
            b"warning: unable to access '/proc/self/fd/100/config': Permission denied\n".to_vec();
        warned.extend_from_slice(hint);
        assert!(
            matches!(
                classify_pinned_read(
                    BoundedRaw::Finished {
                        code: Some(128),
                        stdout: Vec::new(),
                        stderr: warned,
                    },
                    true,
                    AbsenceWitness::Silence,
                ),
                PinnedRead::Unreadable(PinnedReadFailure::ReadRefused { .. })
            ),
            "a diagnostic buried among advice was discounted with it"
        );
    }

    /// Output plus a non-zero exit is never an absence at EITHER site.
    #[test]
    fn output_defeats_both_witnesses() {
        for witness in [AbsenceWitness::Silence, CAT_FILE] {
            assert!(
                matches!(
                    classify_pinned_read(
                        BoundedRaw::Finished {
                            code: Some(1),
                            stdout: b"filter.lfs.smudge\0".to_vec(),
                            stderr: Vec::new(),
                        },
                        true,
                        witness,
                    ),
                    PinnedRead::Unreadable(PinnedReadFailure::ReadRefused { .. })
                ),
                "{witness:?}: bytes were read and then discarded as an absence"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::thread;
    use std::time::{Duration, Instant};

    use uuid::Uuid;

    use super::{
        Bounded, CLOSED_GIT_ENVIRONMENT_KEYS, CleanupOutcome, FilterInventoryFailure, GIT,
        KeepReason, PINNED_GIT_DIR_FD, PinnedRead, PinnedReadFailure, PinnedRepository,
        READ_BUDGET, Worktree, closed_git_command, configure_closed_git_environment,
        pinned_filter_drivers, read_only_git_command, read_pinned, read_pinned_bytes,
        read_pinned_exit, run_git_bounded, status_holds,
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
    fn a_read_whose_answer_is_its_exit_code_tells_no_apart_from_could_not_tell() {
        // `merge-base --is-ancestor` answers with the exit code: 0 yes, 1 no, anything else an
        // error. `read_pinned` reports every unsuccessful child as one absent value, which would
        // make "no" and "could not tell" the same answer -- and those are the two the residue token
        // now turns on (R14). This pins the distinction at the level where it would be lost.
        let (root, first, _second, third) = three_commits("exit-code");
        let repository = PinnedRepository::open(&root.path).unwrap();

        // Yes: the root commit is an ancestor of the tip.
        assert_eq!(
            read_pinned_exit(
                &repository,
                &["merge-base", "--is-ancestor", &first, &third],
                READ_BUDGET,
            ),
            Some(0)
        );
        // No: the tip is not an ancestor of the root. An ANSWER, not a failure.
        assert_eq!(
            read_pinned_exit(
                &repository,
                &["merge-base", "--is-ancestor", &third, &first],
                READ_BUDGET,
            ),
            Some(1)
        );
        // Could not tell: an object the repository does not have. Not `Some(1)`, which is the whole
        // point -- a caller that treated any non-zero exit as "no" would claim a fact from an error.
        let missing = "0".repeat(40);
        let error = read_pinned_exit(
            &repository,
            &["merge-base", "--is-ancestor", &missing, &third],
            READ_BUDGET,
        );
        assert_ne!(error, Some(0));
        assert_ne!(error, Some(1), "an error was reported as a definite \"no\"");
        // And `read_pinned` cannot make the distinction at all, which is why this function exists.
        assert_eq!(
            read_pinned(
                &repository,
                &["merge-base", "--is-ancestor", &third, &first],
                READ_BUDGET,
            ),
            read_pinned(
                &repository,
                &["merge-base", "--is-ancestor", &missing, &third],
                READ_BUDGET,
            )
        );
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
    fn sol1_a_child_finishing_one_millisecond_past_the_bound_is_timed_out() {
        let budget = Duration::from_millis(5);
        assert!(
            !super::reached_deadline(Duration::from_millis(4), budget),
            "R53.BOUND.MINUS_ONE: a child finished inside the bound was timed out"
        );
        assert!(
            super::reached_deadline(Duration::from_millis(5), budget),
            "R53.BOUND.NO_GRACE: the deadline itself was treated as inside the bound"
        );
        assert!(
            super::reached_deadline(Duration::from_millis(6), budget),
            "R53.BOUND.PLUS_ONE: a child finished outside the bound was published as finished"
        );
    }

    #[test]
    fn a_check_that_never_finishes_is_a_timeout_and_never_a_clean_answer() {
        let directory = TempDir::new("gate-timeout");
        let hanging = program(&directory.path, "sleep 30\n");

        let holds = status_holds(&hanging, &directory.path, Duration::from_millis(50), &[]);

        assert_eq!(holds, vec![KeepReason::GateTimedOut]);
    }

    /// Open `program` for writing, hand that descriptor to a forked child, and return once the
    /// child is holding it.
    ///
    /// The kernel refuses to exec a file that any process holds open for writing. Beacon is
    /// multi-threaded, so when one thread forks while a sibling holds a freshly written program
    /// open, the child inherits that write descriptor and every exec of the program fails with
    /// `ETXTBSY` until the child reaches its own exec and `CLOEXEC` closes it
    /// (rust-lang/rust#114554). That window turned the 50 ms timeout test above into
    /// `CheckFailed` on the main gate at 77f430c, under a load average of 17.
    ///
    /// The window is held open here rather than raced for, because racing for it reproduced in
    /// only 1 to 6 runs of 10 and a bar that usually passes is not a bar. `spawn` does not return
    /// until the child execs, so the child announces itself down a pipe from between its own fork
    /// and exec and then sleeps there; the child runs on its own thread for the same reason.
    ///
    /// Returns the child's thread and this side's own descriptor. Drop that descriptor and the
    /// child is the only holder left.
    fn hold_open_across_a_fork(
        program: &Path,
    ) -> (thread::JoinHandle<std::process::ExitStatus>, fs::File) {
        let mut ends = [0_i32; 2];
        assert_eq!(
            unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC) },
            0,
            "the announcement pipe did not open"
        );
        // SAFETY: `pipe2` filled both ends, and each is owned by exactly one of these.
        let (reader, writer_end) =
            unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
        let announce = writer_end.as_raw_fd();

        let held = fs::OpenOptions::new().write(true).open(program).unwrap();

        let mut command = Command::new("/bin/true");
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        // SAFETY: `write` and `nanosleep` are async-signal-safe, allocate nothing, and run between
        // fork and exec on a descriptor this command retains. Setting `pre_exec` at all also
        // forces the real fork/exec path rather than `posix_spawn`, which is the path that shares
        // the descriptor table.
        unsafe {
            command.pre_exec(move || {
                libc::write(announce, [1_u8].as_ptr().cast(), 1);
                let pause = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 300_000_000,
                };
                libc::nanosleep(&pause, std::ptr::null_mut());
                Ok(())
            });
        }
        let forked = thread::spawn(move || command.spawn().unwrap().wait().unwrap());

        let mut announcement = [0_u8; 1];
        std::io::Read::read_exact(&mut fs::File::from(reader), &mut announcement).unwrap();
        drop(writer_end);
        (forked, held)
    }

    /// A transient kernel condition is not an answer about the worktree. See
    /// [`hold_open_across_a_fork`] for the race this closes.
    #[test]
    fn a_check_a_forked_child_holds_open_is_never_a_failure() {
        let directory = TempDir::new("gate-text-file-busy");
        let fresh = program(&directory.path, "exit 0\n");
        let (forked, held) = hold_open_across_a_fork(&fresh);
        drop(held);

        let holds = status_holds(&fresh, &directory.path, Duration::from_secs(5), &[]);
        forked.join().unwrap();

        assert_eq!(
            holds,
            Vec::new(),
            "a program a sibling's child held open was recorded as an answer about the worktree"
        );
    }

    /// The retry waits for a busy program inside the caller's budget and no longer. A program
    /// still held open when the budget runs out is a check that did not finish in time, which is
    /// a timeout; calling it a failure would claim an answer nobody got, and retrying past the
    /// budget would leave the bound unenforceable. Same held-open window, a budget far shorter
    /// than the hold.
    #[test]
    fn a_program_still_held_open_when_the_budget_ends_is_a_timeout() {
        let directory = TempDir::new("gate-text-file-busy-budget");
        let fresh = program(&directory.path, "exit 0\n");
        let (forked, held) = hold_open_across_a_fork(&fresh);
        drop(held);

        let started = Instant::now();
        let holds = status_holds(&fresh, &directory.path, Duration::from_millis(50), &[]);
        let waited = started.elapsed();
        forked.join().unwrap();

        assert_eq!(holds, vec![KeepReason::GateTimedOut]);
        assert!(
            waited < Duration::from_millis(250),
            "the retry outlived the 50 ms budget by waiting {waited:?}, so the bound is not a bound"
        );
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

    /// Hold every descriptor through `last`, so the next open in this process gets `last + 1`.
    ///
    /// The state grok measured under a concurrent build: a descriptor table grown by 32 test
    /// threads, live repositories and pipes until `openat` reaches the number the pin is placed
    /// at. Forced here rather than waited for, because a regression test for a flake may not be
    /// one itself.
    fn hold_descriptors_through(last: RawFd) -> Vec<fs::File> {
        let mut held = Vec::new();
        loop {
            let file = fs::File::open("/dev/null").expect("/dev/null opens");
            let number = file.as_raw_fd();
            if number > last {
                break;
            }
            held.push(file);
            if number == last {
                break;
            }
        }
        held
    }

    #[test]
    fn a_git_directory_that_opens_on_the_placement_descriptor_still_reaches_the_child() {
        // grok's R41 root cause (R49). `PINNED_GIT_DIR_FD` is a fixed destination because
        // `GIT_DIR` is baked before the fork, and `PinnedRepository::open` takes `.git` with
        // `O_CLOEXEC`. When the descriptor table has grown that far, `openat(.git)` returns that
        // very number, and `dup2(n, n)` is a successful NO-OP by POSIX -- it does not clear
        // close-on-exec. The exec then closes the pin, the child's `/proc/self/fd/100` is gone,
        // and Git reports the Git directory it was pointed at as missing. Placement "succeeds"
        // and returns the destination, so nothing upstream can see it; the loss arrives four
        // gates later as a content disagreement about a repository that was never read.
        let (root, _first, _second, head) = three_commits("pin-on-placement-descriptor");
        let repository = fs::canonicalize(&root.path).unwrap();

        // Two descriptors below the placement number are left free, which is what
        // `PinnedRepository::open` consumes in order: the root, then `.git` through it. Its
        // object-store check opens above them and closes again before the second. A sibling test
        // thread can take either slot first -- that is a setup failure, never a pass, so it is
        // retried and its exhaustion is a panic rather than a skip.
        let mut placed = None;
        for _ in 0..16 {
            let held = hold_descriptors_through(PINNED_GIT_DIR_FD - 2);
            let pinned = PinnedRepository::open(&repository).expect("a repository pins");
            if pinned.git_directory.as_raw_fd() == PINNED_GIT_DIR_FD {
                placed = Some((pinned, held));
                break;
            }
        }
        let Some((pinned, _held)) = placed else {
            panic!("the Git directory never opened on descriptor {PINNED_GIT_DIR_FD}");
        };
        // The other half of the collision. Without close-on-exec on the source there is nothing
        // for the no-op to fail to clear, and this test would pass against the defect.
        let flags = unsafe { libc::fcntl(PINNED_GIT_DIR_FD, libc::F_GETFD) };
        assert!(
            flags >= 0 && flags & libc::FD_CLOEXEC != 0,
            "the pinned Git directory is not close-on-exec, so this no longer reproduces R49"
        );

        // The mechanism, in Git's own words. The bounded runner sends stderr to `Stdio::null`, so
        // the message is read here rather than out of a refusal: at the level of the Receipt this
        // defect is indistinguishable from a Candidate that declared the wrong bytes.
        let mut command = read_only_git_command(Path::new(GIT), &[]);
        pinned.enter(&mut command);
        let output = command
            .arg("rev-parse")
            .arg("--is-inside-work-tree")
            .stdin(Stdio::null())
            .output()
            .expect("git starts");
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            !stderr.contains(&format!(
                "not a git repository: '/proc/self/fd/{PINNED_GIT_DIR_FD}'"
            )),
            "the exec closed the pinned Git directory: {stderr}"
        );
        assert!(
            output.status.success(),
            "the child entered through the pin could not start: {stderr}"
        );

        // ...and the read the dispatch pre-image gate makes through that pin answers, rather than
        // arriving as an absent blob the gate can only read as a disagreement.
        let object = format!("{head}:a.txt");
        // R46 typed this read, so the assertion names the answer rather than "some bytes": a pin
        // closed by exec produced `Unreadable(ReadRefused)` carrying git's
        // `not a git repository: '/proc/self/fd/100'` -- which is how R49's cause became visible
        // at all. Asserting `Bytes` is asserting the fix; asserting `Some(..)` would also accept a
        // future in which the read answers for the wrong reason.
        assert_eq!(
            read_pinned_bytes(&pinned, &["cat-file", "blob", object.as_str()], READ_BUDGET),
            Ok(PinnedRead::Bytes(b"three\n".to_vec())),
            "the pre-image read through a pin placed on its own descriptor did not answer"
        );
    }

    /// A repository with one commit whose `.git/config` this test can break.
    fn pinned_one_commit(label: &str) -> (TempDir, PinnedRepository) {
        let (root, _first, _second, _third) = three_commits(label);
        let path = fs::canonicalize(&root.path).unwrap();
        let pinned = PinnedRepository::open(&path).expect("a repository pins");
        (root, pinned)
    }

    /// R47: an inventory git REFUSED is a failure, never "this repository configures no drivers".
    ///
    /// The inventory decides which filter drivers the hardening suppresses. Returning an empty set
    /// for a configuration that could not be read does not lose a diagnostic -- it switches the
    /// hardening off and then runs the child, which is the security half of this lane.
    #[test]
    fn an_inventory_git_refused_is_never_an_empty_filter_set() {
        let (root, pinned) = pinned_one_commit("inventory-refused");
        let configuration = root.path.join(".git").join("config");
        let restore = fs::metadata(&configuration).unwrap().permissions();
        fs::set_permissions(&configuration, fs::Permissions::from_mode(0o000)).unwrap();

        let answer = pinned_filter_drivers(&pinned);

        fs::set_permissions(&configuration, restore).unwrap();
        assert_ne!(
            answer,
            Ok(Vec::new()),
            "git refused the inventory and the read proceeded with the hardening switched off"
        );
        let Err(FilterInventoryFailure::Refused { message }) = answer else {
            panic!("the inventory failure does not name itself: {answer:?}");
        };
        assert!(
            message.contains("Permission denied"),
            "the refusal does not carry what git said: {message}"
        );
    }

    /// R47: every shape of a configuration git cannot read is a failure that carries WHY.
    ///
    /// Three faults at one site, because "could not read" arrives three ways and a rule that
    /// caught only the one that was measured first would leave the others open. Missing and
    /// malformed exit 128 with a `fatal:` line; mode 000 adds a `warning:` before it.
    #[test]
    fn every_configuration_git_cannot_read_is_a_refusal_that_carries_why() {
        for (label, break_it) in [
            (
                "missing",
                Box::new(|git_dir: &Path| fs::remove_file(git_dir.join("config")).unwrap())
                    as Box<dyn Fn(&Path)>,
            ),
            (
                "malformed",
                Box::new(|git_dir: &Path| {
                    fs::write(git_dir.join("config"), "[core\nnot a config\n").unwrap()
                }),
            ),
            (
                "unreadable",
                Box::new(|git_dir: &Path| {
                    fs::set_permissions(git_dir.join("config"), fs::Permissions::from_mode(0o000))
                        .unwrap()
                }),
            ),
        ] {
            let (root, pinned) = pinned_one_commit(&format!("inventory-{label}"));
            let git_dir = root.path.join(".git");
            break_it(&git_dir);
            let answer = pinned_filter_drivers(&pinned);
            // Restored before the assertion, so a failure does not leave an undeletable tree.
            let _ = fs::set_permissions(git_dir.join("config"), fs::Permissions::from_mode(0o644));
            let Err(FilterInventoryFailure::Refused { message }) = answer else {
                panic!("{label}: the inventory did not refuse: {answer:?}");
            };
            assert!(
                message.starts_with("fatal:") || message.contains("fatal:"),
                "{label}: the refusal carries no diagnostic from git: {message}"
            );
        }
    }

    /// R47, measured against the brief: a corrupt OBJECT STORE does not make the INVENTORY
    /// unreadable, so the two sites fail apart and each names itself.
    ///
    /// The brief expected a repository whose object store is corrupt at inventory time to refuse
    /// at the inventory. It does not: the inventory reads `.git/config`, a plain file the object
    /// store has nothing to do with, and it exits 0 with the configuration intact
    /// (tools/r47/logs/measure-git-reads.log). The corruption is caught one read later, by R46.3's
    /// classification of `cat-file`. Asserting that here is what keeps a future "simplification"
    /// from collapsing the two sites onto one witness.
    #[test]
    fn a_corrupt_object_store_leaves_the_inventory_readable_and_fails_at_the_byte_read() {
        let (root, pinned) = pinned_one_commit("inventory-vs-objects");
        let head = read_pinned(&pinned, &["rev-parse", "--verify", "HEAD"], READ_BUDGET)
            .expect("the head reads before the store is broken");
        fs::remove_dir_all(root.path.join(".git").join("objects")).unwrap();

        assert!(
            pinned_filter_drivers(&pinned).is_ok(),
            "a corrupt object store was reported as an unreadable filter inventory"
        );
        let object = format!("{}:a.txt", head.trim());
        assert!(
            matches!(
                read_pinned_bytes(&pinned, &["cat-file", "blob", object.as_str()], READ_BUDGET),
                Ok(PinnedRead::Unreadable(
                    PinnedReadFailure::ReadRefused { .. }
                ))
            ),
            "a broken object store was read as a path that is simply not there"
        );
    }
}
