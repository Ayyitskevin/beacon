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
#[derive(Debug)]
pub(crate) struct PinnedRepository {
    path: PathBuf,
    device: u64,
    inode: u64,
    directory: fs::File,
}

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
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            path,
            directory,
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
        // SAFETY: `fchdir` is async-signal-safe, touches only this child's working directory, and
        // the descriptor outlives the spawn because `self` is borrowed for the call.
        unsafe {
            command.pre_exec(move || {
                if libc::fchdir(fd) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
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
    pub(crate) fn still_pinned(&self) -> bool {
        let Ok(metadata) = fs::metadata(&self.path) else {
            return false;
        };
        metadata.is_dir() && metadata.dev() == self.device && metadata.ino() == self.inode
    }
}

/// A Git invocation that may not write.
///
/// The same hardening as [`hardened_git_command`], plus `GIT_OPTIONAL_LOCKS=0`. A kernel review
/// measured `git status --porcelain` refreshing and rewriting `.git/index`: a gate that mutates the
/// repository it is measuring cannot support a zero-mutation claim, and the optional-locks
/// environment variable is what stops it.
pub(crate) fn read_only_git_command(git: &Path, filter_drivers: &[String]) -> Command {
    let mut command = Command::new(git);
    apply_git_hardening(&mut command, filter_drivers);
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command
}

/// Run one read-only Git invocation in a pinned repository, bounded.
///
/// The repository is entered by its pinned path and the pin is re-checked immediately afterwards,
/// so a rename between the check and the read makes the answer void rather than wrong.
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
        .arg(".git/config")
        .arg("--includes")
        .arg("--name-only")
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
    Some(
        stdout
            .lines()
            .filter_map(|line| {
                line.strip_suffix(".smudge")
                    .or_else(|| line.strip_suffix(".clean"))
            })
            .filter(|name| name.starts_with("filter."))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    )
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

fn configure_closed_git_environment(command: &mut Command) {
    command
        .env_clear()
        .env("PATH", GIT_PATH)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
}

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

fn run_git_bounded_inner(
    mut command: Command,
    stdin_bytes: Option<&[u8]>,
    budget: Duration,
) -> Bounded {
    if stdin_bytes.is_none() {
        command.stdin(Stdio::null());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return Bounded::Failed;
    };
    if let Some(bytes) = stdin_bytes {
        let Some(mut pipe) = child.stdin.take() else {
            let _ = child.kill();
            return Bounded::Failed;
        };
        if std::io::Write::write_all(&mut pipe, bytes).is_err() {
            let _ = child.kill();
            return Bounded::Failed;
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

    use super::{
        Bounded, CleanupOutcome, KeepReason, PinnedRepository, Worktree, run_git_bounded,
        status_holds,
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
