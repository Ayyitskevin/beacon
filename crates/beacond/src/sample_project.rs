use std::collections::BTreeSet;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::git_object_store::{LooseRead, ObjectKind, ObjectStore};
use crate::worktree::{PinnedRepository, WRITE_BUDGET, write_pinned};
use crate::{
    AutonomyProfile, BudgetBoundary, DeclaredCheck, ExternalEffects, OperatorAccountError,
    OperatorAccountService, ProjectCharter, ProjectView, RepositoryAuthor, RepositorySourceKind,
};

pub const SAMPLE_PROJECT_BUNDLE_VERSION: u32 = 1;
const DIGEST_ALGORITHM: &str = "sha256";
const DIGEST_CANONICALIZER: &str = "beacon.sample-project.bundle.v1";
const PROVENANCE_SOURCE: &str = "beacon_bundled_sample_project";
const EXPECTED_BUNDLE_DIGEST: &str =
    "7bdcf0a732856553c529dee2ec7dfcf9bbf691a5fb9b2558e74f05c1559dde8f";
const EXPECTED_BASE_REVISION: &str = "c5a2dad9164a1fd5346fdff3d3e2d8d17af922d4";
const SAMPLE_PROJECT_NAME: &str = "Beacon Sample: Task Tracker";
const SAMPLE_REPOSITORY_DIRECTORY: &str = "task-tracker-v1";

struct BundleFile {
    path: &'static str,
    bytes: &'static [u8],
}

const BUNDLE_FILES: &[BundleFile] = &[
    BundleFile {
        path: "Cargo.lock",
        bytes: include_bytes!("../sample-project/Cargo.lock"),
    },
    BundleFile {
        path: "Cargo.toml",
        bytes: include_bytes!("../sample-project/Cargo.toml"),
    },
    BundleFile {
        path: "README.md",
        bytes: include_bytes!("../sample-project/README.md"),
    },
    BundleFile {
        path: "src/lib.rs",
        bytes: include_bytes!("../sample-project/src/lib.rs"),
    },
];

const EXPECTED_INDEX_ENTRIES: &[(&str, &str)] = &[
    ("Cargo.lock", "db6a33eb2f275ff188dd6c05a798b3e5832c98bb"),
    ("Cargo.toml", "3c0cae6f7d394e7d2cd6dcb2516a927df042abd9"),
    ("README.md", "6de4c063801d0e996e9e6e828188105fac53369a"),
    ("src/lib.rs", "46fb840cffb4154a8c1e7964f75271e9826c6e65"),
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SampleProjectDigest {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SampleProjectProvenance {
    pub source: String,
    pub bundle_version: u32,
    pub bundle_digest: SampleProjectDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SampleProjectInstallation {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub charter_revision: u32,
    pub repository_revision: u32,
    pub repository_path: String,
    pub base_revision: String,
    pub provenance: SampleProjectProvenance,
    pub installed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SampleProjectStatus {
    Available {
        bundle_version: u32,
        bundle_digest: SampleProjectDigest,
    },
    Installed {
        installation: SampleProjectInstallation,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleProjectInstallOutcome {
    Installed,
    AlreadyInstalled,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SampleProjectInstallResult {
    pub outcome: SampleProjectInstallOutcome,
    pub installation: SampleProjectInstallation,
}

/// Which fault the sample-Project startup pass hit, and WHICH ROW hit it (R28.1).
///
/// This pass's row IS a Workspace -- `sample_project_installations` is keyed by it alone -- so the
/// key has no second field. Inventing one to match the other passes would be shape for its own
/// sake.
pub(crate) enum SampleProjectStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One installation did not re-prove. `cause` is the owning reader's OWN error.
    Row {
        workspace_id: Uuid,
        cause: SampleProjectError,
    },
    /// A COUNT across the store that does not match the log.
    Totals(crate::TotalsMismatch),
    /// A row whose Workspace column is not a UUID. Unreachable while the foreign-key check and the
    /// Workspace pass run first.
    Identity { workspace_id: String },
}

impl SampleProjectError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::NotInstalled => "NotInstalled",
            Self::UnsafeDestination => "UnsafeDestination",
            Self::BundleMismatch => "BundleMismatch",
            Self::MaterializationFailed => "MaterializationFailed",
            Self::TamperedInstallation => "TamperedInstallation",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Io(_) => "Io",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for SampleProjectStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => {
                formatter.write_str("the sample Project installations could not be read")
            }
            Self::Row {
                workspace_id,
                cause,
            } => write!(
                formatter,
                "the sample Project installation in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Totals(totals) => write!(
                formatter,
                "the sample Project installations do not agree with the log -- {totals}"
            ),
            Self::Identity { workspace_id } => write!(
                formatter,
                "a sample Project row's Workspace is not readable: {workspace_id}"
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for SampleProjectStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Totals(totals) => formatter.debug_tuple("Totals").field(totals).finish(),
            Self::Row {
                workspace_id,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("cause", cause)
                .finish(),
            Self::Identity { workspace_id } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .finish(),
        }
    }
}

impl std::error::Error for SampleProjectStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            Self::Totals(_) | Self::Identity { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum SampleProjectError {
    Account(OperatorAccountError),
    /// A copy was asked for before the Sample Project was installed. Distinct from a tampered
    /// installation: there is nothing to copy rather than something wrong with what is there.
    NotInstalled,
    UnsafeDestination,
    BundleMismatch,
    MaterializationFailed,
    TamperedInstallation,
    InvalidStore,
    Store(rusqlite::Error),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for SampleProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::NotInstalled => write!(formatter, "the Sample Project is not installed"),
            Self::UnsafeDestination => write!(formatter, "Sample Project destination is unsafe"),
            Self::BundleMismatch => {
                write!(formatter, "Sample Project bundle digest does not match")
            }
            Self::MaterializationFailed => {
                write!(formatter, "Sample Project could not be materialized")
            }
            Self::TamperedInstallation => {
                write!(formatter, "Sample Project installation was changed")
            }
            Self::InvalidStore => write!(formatter, "Sample Project state is invalid"),
            Self::Store(error) => write!(formatter, "Sample Project store failed: {error}"),
            Self::Io(error) => write!(formatter, "Sample Project filesystem failed: {error}"),
            Self::Json(error) => write!(formatter, "Sample Project event is invalid: {error}"),
        }
    }
}

impl std::error::Error for SampleProjectError {}

impl From<OperatorAccountError> for SampleProjectError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for SampleProjectError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<std::io::Error> for SampleProjectError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for SampleProjectError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Owns the authenticated, opt-in installation of Beacon's inert bundled Project.
pub struct SampleProjectService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
    managed_root: PathBuf,
    git_program: OsString,
    expected_bundle_digest: String,
    sync_repository_tree: fn(&Path) -> std::io::Result<()>,
    #[cfg(debug_assertions)]
    copy_failpoints: CopyFailpoints,
    #[cfg(test)]
    before_workspace_open: fn(&Path) -> std::io::Result<()>,
    #[cfg(test)]
    test_store_path: Option<PathBuf>,
}

impl SampleProjectService {
    /// Install synchronization points inside `copy_for_dispatch`. Debug builds only.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn with_copy_failpoints(mut self, failpoints: CopyFailpoints) -> Self {
        self.copy_failpoints = failpoints;
        self
    }

    pub fn new(
        accounts: Arc<OperatorAccountService>,
        workspace_id: Uuid,
        store_path: impl AsRef<Path>,
    ) -> Result<Self, SampleProjectError> {
        let store_path = if store_path.as_ref().is_absolute() {
            store_path.as_ref().to_owned()
        } else {
            std::env::current_dir()?.join(store_path)
        };
        let managed_root = store_path
            .parent()
            .ok_or(SampleProjectError::UnsafeDestination)?
            .join("sample-projects");
        Ok(Self {
            accounts,
            workspace_id,
            managed_root,
            git_program: OsString::from("git"),
            expected_bundle_digest: EXPECTED_BUNDLE_DIGEST.to_owned(),
            sync_repository_tree: sync_directory_tree,
            #[cfg(debug_assertions)]
            copy_failpoints: CopyFailpoints::default(),
            #[cfg(test)]
            before_workspace_open: no_op_path_hook,
            #[cfg(test)]
            test_store_path: None,
        })
    }

    pub fn status(&self) -> Result<SampleProjectStatus, SampleProjectError> {
        self.with_authenticated_store(|connection, _now| {
            let bundle_digest = self.verified_bundle_digest()?;
            match load_installation(connection, self.workspace_id)? {
                Some(installation) => {
                    self.verify_installation(&installation, &bundle_digest)?;
                    Ok(SampleProjectStatus::Installed { installation })
                }
                None => Ok(SampleProjectStatus::Available {
                    bundle_version: SAMPLE_PROJECT_BUNDLE_VERSION,
                    bundle_digest,
                }),
            }
        })
    }

    pub fn install(&self) -> Result<SampleProjectInstallResult, SampleProjectError> {
        self.with_authenticated_store(|connection, now| {
            let bundle_digest = self.verified_bundle_digest()?;
            if let Some(installation) = load_installation(connection, self.workspace_id)? {
                self.verify_installation(&installation, &bundle_digest)?;
                return Ok(SampleProjectInstallResult {
                    outcome: SampleProjectInstallOutcome::AlreadyInstalled,
                    installation,
                });
            }

            let project_id = Uuid::new_v4();
            let repository_path = self.repository_path();
            let base_revision = self.materialize_repository(&repository_path)?;
            let installed_at_ms =
                u64::try_from(now).map_err(|_| SampleProjectError::InvalidStore)?;
            let installation = SampleProjectInstallation {
                workspace_id: self.workspace_id,
                project_id,
                charter_revision: 1,
                repository_revision: 1,
                repository_path: repository_path.to_string_lossy().into_owned(),
                base_revision,
                provenance: SampleProjectProvenance {
                    source: PROVENANCE_SOURCE.to_owned(),
                    bundle_version: SAMPLE_PROJECT_BUNDLE_VERSION,
                    bundle_digest: bundle_digest.clone(),
                },
                installed_at_ms,
            };

            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(existing) = load_installation(&transaction, self.workspace_id)? {
                self.verify_installation(&existing, &bundle_digest)?;
                transaction.commit()?;
                return Ok(SampleProjectInstallResult {
                    outcome: SampleProjectInstallOutcome::AlreadyInstalled,
                    installation: existing,
                });
            }
            insert_sample_project(&transaction, &installation)?;
            transaction.commit()?;
            Ok(SampleProjectInstallResult {
                outcome: SampleProjectInstallOutcome::Installed,
                installation,
            })
        })
    }

    /// Copy the installed Sample Project into a directory Beacon owns, and return where it went.
    ///
    /// The v1 dispatch path reaches **only Beacon-made copies** (R8): a Candidate that authorizes an
    /// effect is a Material Artifact and v1 has no non-author Evaluation of one, so the blast radius
    /// is bounded by construction. This is the act that makes a copy Beacon-made -- the provenance
    /// gate then asks where the directory is, not what a row claims about it.
    ///
    /// The installation is verified first, so a tampered tree is not what gets copied, and the
    /// bundled Sample Project itself is never the dispatch target: its own verifier requires
    /// `refs/heads/main` to hold the frozen revision, and a dispatch would move it.
    pub fn copy_for_dispatch(
        &self,
        copies_root: impl AsRef<Path>,
        label: &str,
    ) -> Result<PathBuf, SampleProjectError> {
        let bundle_digest = self.verified_bundle_digest()?;
        let installation = self.with_authenticated_store(|connection, _now| {
            load_installation(connection, self.workspace_id)?
                .ok_or(SampleProjectError::NotInstalled)
        })?;
        self.verify_installation(&installation, &bundle_digest)?;
        let source = self.repository_path();

        let root = copies_root.as_ref();
        fs::create_dir_all(root)?;
        let name = format!("sample-project-{label}");
        let destination = root.join(&name);

        // The copy's root is created and opened THROUGH the copies-root descriptor, with no
        // pathname resolution of the parent chain and no symlink to follow. POSIX has no `O_CREAT`
        // for a directory, so `mkdirat` and `openat` are two syscalls and an interval exists
        // between them; the previous version held the parent descriptor, which removed ancestor
        // retargeting but never bound the child inode, and a review replaced the final component in
        // exactly that interval -- `openat` answered with the replacement, which then received a
        // provenance token and was recorded as Beacon-created.
        //
        // `create_dispatch_copy_root` does not close the interval. It makes the interval unaimable
        // and the substitution detectable: the directory is made under a name the caller cannot
        // predict, the inode `mkdirat` produced is captured before the window opens, and the
        // descriptor is refused unless it is that inode -- before any provenance is recorded.
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let root_directory = options.open(root)?;
        let created = create_dispatch_copy_root(
            &root_directory,
            OsStr::new(name.as_str()),
            root,
            #[cfg(debug_assertions)]
            &self.copy_failpoints,
        )?;
        #[cfg(debug_assertions)]
        copy_failpoint(&self.copy_failpoints.after_publish, &destination);
        let created_identity = {
            let metadata = created.metadata()?;
            (metadata.dev(), metadata.ino())
        };
        // Into the descriptor `create_dispatch_copy_root` returned -- which follows the inode
        // through the publishing rename -- rather than into whatever `destination` names by now.
        copy_tree_into(&source, &created)?;

        let pinned = PinnedRepository::open(&destination)
            .map_err(|_| SampleProjectError::UnsafeDestination)?;
        if pinned.identity() != created_identity {
            return Err(SampleProjectError::UnsafeDestination);
        }
        drop(created);
        let token = Uuid::new_v4().to_string();
        write_pinned(
            &pinned,
            None,
            None,
            &[
                "config",
                "--file",
                // Through the pinned Git-directory descriptor, like every other read of this file.
                &PinnedRepository::git_dir_child_path("config"),
                crate::dispatch::PROVENANCE_TOKEN_KEY,
                &token,
            ],
            None,
            WRITE_BUDGET,
        )
        .ok_or(SampleProjectError::UnsafeDestination)?;

        // The act that makes the copy Beacon-made, recorded where a gate can read it. A dispatch
        // asks this record whether Beacon created a repository; without it, "Beacon-made" would be
        // a claim about a directory's location rather than about its origin.
        self.with_authenticated_store(|connection, now| {
            crate::dispatch::record_beacon_repository(connection, self.workspace_id, &pinned, now)
                .map_err(|_| SampleProjectError::InvalidStore)
        })?;
        if !pinned.still_pinned() {
            return Err(SampleProjectError::UnsafeDestination);
        }
        Ok(destination)
    }

    fn with_authenticated_store<T>(
        &self,
        operation: impl FnOnce(&mut Connection, i64) -> Result<T, SampleProjectError>,
    ) -> Result<T, SampleProjectError> {
        #[cfg(test)]
        if let Some(store_path) = &self.test_store_path {
            let mut connection = crate::open_store_connection(store_path)?;
            return operation(&mut connection, 1);
        }
        self.accounts.with_authenticated_store(operation)
    }

    fn verified_bundle_digest(&self) -> Result<SampleProjectDigest, SampleProjectError> {
        let value = computed_bundle_digest();
        if value != self.expected_bundle_digest {
            return Err(SampleProjectError::BundleMismatch);
        }
        Ok(SampleProjectDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: DIGEST_CANONICALIZER.to_owned(),
            value,
        })
    }

    fn repository_path(&self) -> PathBuf {
        self.managed_root
            .join(self.workspace_id.to_string())
            .join(SAMPLE_REPOSITORY_DIRECTORY)
    }

    fn verify_installation(
        &self,
        installation: &SampleProjectInstallation,
        bundle_digest: &SampleProjectDigest,
    ) -> Result<(), SampleProjectError> {
        if installation.workspace_id != self.workspace_id
            || installation.charter_revision != 1
            || installation.repository_revision != 1
            || installation.repository_path != self.repository_path().to_string_lossy()
            || installation.provenance.source != PROVENANCE_SOURCE
            || installation.provenance.bundle_version != SAMPLE_PROJECT_BUNDLE_VERSION
            || installation.provenance.bundle_digest != *bundle_digest
        {
            return Err(SampleProjectError::InvalidStore);
        }
        let revision =
            self.verify_materialized_repository(Path::new(&installation.repository_path))?;
        if revision != installation.base_revision {
            return Err(SampleProjectError::TamperedInstallation);
        }
        Ok(())
    }

    fn materialize_repository(&self, destination: &Path) -> Result<String, SampleProjectError> {
        ensure_private_directory(&self.managed_root)?;
        let workspace_root = self.managed_root.join(self.workspace_id.to_string());
        ensure_private_directory(&workspace_root)?;
        #[cfg(test)]
        (self.before_workspace_open)(&workspace_root)?;
        let workspace_directory = open_private_directory(&workspace_root)?;

        match fs::symlink_metadata(destination) {
            Ok(_) => return self.verify_materialized_repository(destination),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let staging_name = OsString::from(format!(".staging-{}", Uuid::new_v4()));
        let staging = workspace_root.join(&staging_name);
        let staging_directory = create_private_directory_at(&workspace_directory, &staging_name)?;
        let materialized = (|| {
            write_bundle_at(&staging_directory)?;
            self.initialize_repository(&staging_directory)?;
            let revision = self.verify_materialized_repository(&staging)?;
            (self.sync_repository_tree)(&staging)?;
            publish_directory(
                &workspace_directory,
                &staging_name,
                OsStr::new(SAMPLE_REPOSITORY_DIRECTORY),
            )?;
            workspace_directory.sync_all()?;
            Ok(revision)
        })();
        if materialized.is_err() {
            let _ = remove_directory_at(&workspace_directory, &staging_name, &staging_directory);
        }
        match materialized {
            Err(SampleProjectError::Io(error))
                if error.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                self.verify_materialized_repository(destination)
            }
            result => result,
        }
    }

    fn initialize_repository(&self, repository: &File) -> Result<(), SampleProjectError> {
        self.git_success(
            repository,
            &[],
            &["init", "--quiet", "--initial-branch=main", "--template="],
        )?;
        let filter_drivers = self.repository_filter_drivers(repository)?;
        self.git_success(repository, &filter_drivers, &["add", "--all"])?;
        let tree = output_text(self.git_output(repository, &filter_drivers, &["write-tree"])?)
            .map_err(|_| SampleProjectError::MaterializationFailed)?;
        let mut command = self.git_command(repository, &filter_drivers)?;
        command
            .args(["commit-tree", tree.as_str()])
            .env("GIT_AUTHOR_NAME", "Beacon")
            .env("GIT_AUTHOR_EMAIL", "sample-project@beacon.invalid")
            .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
            .env("GIT_COMMITTER_NAME", "Beacon")
            .env("GIT_COMMITTER_EMAIL", "sample-project@beacon.invalid")
            .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|_| SampleProjectError::MaterializationFailed)?;
        child
            .stdin
            .take()
            .ok_or(SampleProjectError::MaterializationFailed)?
            .write_all(b"Install Beacon Sample Project\n")?;
        let commit = output_text(
            child
                .wait_with_output()
                .map_err(|_| SampleProjectError::MaterializationFailed)?,
        )
        .map_err(|_| SampleProjectError::MaterializationFailed)?;
        self.git_success(
            repository,
            &filter_drivers,
            &["update-ref", "refs/heads/main", &commit],
        )?;
        Ok(())
    }

    fn verify_materialized_repository(
        &self,
        repository: &Path,
    ) -> Result<String, SampleProjectError> {
        let metadata = fs::symlink_metadata(repository)
            .map_err(|_| SampleProjectError::TamperedInstallation)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(SampleProjectError::TamperedInstallation);
        }
        let git_metadata = fs::symlink_metadata(repository.join(".git"))
            .map_err(|_| SampleProjectError::TamperedInstallation)?;
        if git_metadata.file_type().is_symlink() || !git_metadata.is_dir() {
            return Err(SampleProjectError::TamperedInstallation);
        }
        verify_bundled_tree(repository)?;
        verify_frozen_git_state(repository)?;
        Ok(EXPECTED_BASE_REVISION.to_owned())
    }

    fn git_command(
        &self,
        repository: &File,
        filter_drivers: &[String],
    ) -> Result<Command, SampleProjectError> {
        let mut command =
            crate::worktree::hardened_git_command(Path::new(&self.git_program), filter_drivers);
        command
            .env("LC_ALL", "C")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["-c", "core.untrackedCache=false"]);
        bind_command_to_directory(&mut command, repository)?;
        Ok(command)
    }

    fn repository_filter_drivers(
        &self,
        repository: &File,
    ) -> Result<Vec<String>, SampleProjectError> {
        let mut command = crate::worktree::hardened_git_command(Path::new("git"), &[]);
        command
            .args([
                "config",
                "--local",
                "--includes",
                "--name-only",
                "--null",
                "--list",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        bind_command_to_directory(&mut command, repository)?;
        let output = command
            .output()
            .map_err(|_| SampleProjectError::MaterializationFailed)?;
        let stdout = output_text(output).map_err(|_| SampleProjectError::MaterializationFailed)?;
        Ok(crate::worktree::filter_drivers_from_config_output(&stdout))
    }

    fn git_output(
        &self,
        repository: &File,
        filter_drivers: &[String],
        arguments: &[&str],
    ) -> Result<Output, SampleProjectError> {
        self.git_command(repository, filter_drivers)?
            .args(arguments)
            .output()
            .map_err(|_| SampleProjectError::MaterializationFailed)
    }

    fn git_success(
        &self,
        repository: &File,
        filter_drivers: &[String],
        arguments: &[&str],
    ) -> Result<(), SampleProjectError> {
        let output = self.git_output(repository, filter_drivers, arguments)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(SampleProjectError::MaterializationFailed)
        }
    }

    #[cfg(test)]
    fn with_expected_bundle_digest(mut self, value: &str) -> Self {
        self.expected_bundle_digest = value.to_owned();
        self
    }

    #[cfg(test)]
    fn with_git_program(mut self, value: impl AsRef<std::ffi::OsStr>) -> Self {
        self.git_program = value.as_ref().to_owned();
        self
    }

    #[cfg(test)]
    fn with_repository_tree_sync(mut self, value: fn(&Path) -> std::io::Result<()>) -> Self {
        self.sync_repository_tree = value;
        self
    }

    #[cfg(test)]
    fn with_before_workspace_open(mut self, value: fn(&Path) -> std::io::Result<()>) -> Self {
        self.before_workspace_open = value;
        self
    }

    #[cfg(test)]
    fn with_test_authenticated_store(mut self, store_path: impl AsRef<Path>) -> Self {
        self.test_store_path = Some(store_path.as_ref().to_owned());
        self
    }
}

/// Copy one directory tree INTO an already-opened destination directory.
///
/// No symlink is followed and none is recreated: a copy that reproduced a link would carry a
/// reference out of the tree Beacon owns, and the whole point of the copy is that everything the
/// dispatch path can reach is inside it.
///
/// **Every write goes through `destination`, never through its pathname.** The earlier version took
/// the destination as a `Path` and used `fs::create_dir`/`fs::copy`, so the whole tree was written
/// by name and the identity comparison that followed was a post-hoc detector rather than a
/// preventer: a cold review moved Beacon's published child aside, put a directory at the published
/// name whose `README.md` was a symlink pointing outside the copies root, and the copy overwrote
/// that victim before anything noticed. `copy_for_dispatch` already holds a descriptor on the
/// directory it made; this walks down from it with `mkdirat` and `openat`, so a replacement at the
/// published name is a directory nothing writes into.
///
/// `O_NOFOLLOW` refuses a symlink at a destination name and `O_EXCL` refuses an existing entry, so
/// a pre-planted file is a refusal rather than a target. The source is still read by pathname --
/// it is Beacon's own verified installation, and the tampering check above owns it.
fn copy_tree_into(source: &Path, destination: &File) -> Result<(), SampleProjectError> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        // A NUL in a filename cannot reach a syscall, and a name that is `.` or `..` would walk out
        // of the descriptor this function is bound to.
        let name = entry.file_name();
        if name.as_bytes() == b"." || name.as_bytes() == b".." {
            return Err(SampleProjectError::TamperedInstallation);
        }
        let name =
            CString::new(name.as_bytes()).map_err(|_| SampleProjectError::TamperedInstallation)?;
        if metadata.file_type().is_symlink() {
            return Err(SampleProjectError::TamperedInstallation);
        } else if metadata.is_dir() {
            // SAFETY: `destination` is an open directory descriptor for the duration of the call
            // and `name` is a NUL-terminated single component.
            if unsafe { libc::mkdirat(destination.as_raw_fd(), name.as_ptr(), 0o755) } != 0 {
                return Err(SampleProjectError::Io(std::io::Error::last_os_error()));
            }
            let child = open_directory_at(destination, &name)?;
            copy_tree_into(&entry.path(), &child)?;
        } else if metadata.is_file() {
            let mode = metadata.permissions().mode();
            // SAFETY: as above; `O_EXCL` means this call is the one that creates the entry.
            let raw = unsafe {
                libc::openat(
                    destination.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    libc::c_uint::from(u16::try_from(mode & 0o7777).unwrap_or(0o644)),
                )
            };
            if raw < 0 {
                return Err(SampleProjectError::Io(std::io::Error::last_os_error()));
            }
            // SAFETY: `raw` is a fresh descriptor this call owns.
            let mut target = unsafe { File::from_raw_fd(raw) };
            let mut input = File::open(entry.path())?;
            std::io::copy(&mut input, &mut target)?;
            // `fchmod` on the descriptor, not `chmod` on the name: the mode is set on the file this
            // call created, whatever the name resolves to by now.
            target.set_permissions(fs::Permissions::from_mode(mode))?;
        } else {
            return Err(SampleProjectError::TamperedInstallation);
        }
    }
    Ok(())
}

#[cfg(test)]
fn no_op_path_hook(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn bind_command_to_directory(
    command: &mut Command,
    directory: &File,
) -> Result<(), SampleProjectError> {
    let directory = directory.try_clone()?;
    // SAFETY: `pre_exec` runs after `fork` and before `exec`. `fstat` and
    // `fchdir` are async-signal-safe, allocate nothing, and operate on the
    // cloned descriptor retained by this command. CLOEXEC closes it at
    // successful exec rather than leaking it to Git.
    unsafe {
        command.pre_exec(move || {
            let descriptor = directory.as_raw_fd();
            let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
            if libc::fstat(descriptor, metadata.as_mut_ptr()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let metadata = metadata.assume_init();
            if metadata.st_nlink == 0 || metadata.st_mode & libc::S_IFMT != libc::S_IFDIR {
                return Err(std::io::Error::from_raw_os_error(libc::ESTALE));
            }
            if libc::fchdir(descriptor) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

fn sync_directory_tree(root: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::other(
            "repository tree root is not a real directory",
        ));
    }
    sync_tree_entry(root)
}

fn sync_tree_entry(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(std::io::Error::other(
            "repository tree contains a symbolic link",
        ));
    }
    if metadata.is_file() {
        return File::open(path)?.sync_all();
    }
    if !metadata.is_dir() {
        return Err(std::io::Error::other(
            "repository tree contains a special file",
        ));
    }

    let mut children = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    children.sort();
    for child in children {
        sync_tree_entry(&child)?;
    }
    File::open(path)?.sync_all()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectCreatedPayload {
    project: ProjectView,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRepository {
    revision: u32,
    charter_revision: u32,
    source_kind: RepositorySourceKind,
    source: String,
    base_revision: String,
    declared_checks: Vec<DeclaredCheck>,
    author_kind: RepositoryAuthor,
    authored_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BoundProjectReference {
    id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectRepositoryBoundPayload {
    project: BoundProjectReference,
    repository: StoredRepository,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SampleProjectInstalledPayload {
    installation: SampleProjectInstallation,
}

fn insert_sample_project(
    connection: &Connection,
    installation: &SampleProjectInstallation,
) -> Result<(), SampleProjectError> {
    let now = i64::try_from(installation.installed_at_ms)
        .map_err(|_| SampleProjectError::InvalidStore)?;
    let workspace_id = installation.workspace_id.to_string();
    let project_id = installation.project_id.to_string();
    let charter = sample_charter(installation.installed_at_ms);
    connection.execute(
        "INSERT INTO projects (
             id, workspace_id, name, current_charter_revision, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, 1, ?4, ?4)",
        params![project_id, workspace_id, SAMPLE_PROJECT_NAME, now],
    )?;
    connection.execute(
        "INSERT INTO project_charter_revisions (
             workspace_id, project_id, revision, purpose, desired_outcomes_json,
             constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
             external_effects, author_kind, authored_at_ms
         ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, 'supervised', 'no_spend',
                   'prohibited', 'owner', ?7)",
        params![
            workspace_id,
            project_id,
            charter.purpose,
            serde_json::to_string(&charter.desired_outcomes)?,
            serde_json::to_string(&charter.constraints)?,
            serde_json::to_string(&charter.prohibitions)?,
            now,
        ],
    )?;
    connection.execute(
        "INSERT INTO project_creation_snapshots (
             project_id, workspace_id, initial_name, initial_charter_revision, created_at_ms
         ) VALUES (?1, ?2, ?3, 1, ?4)",
        params![project_id, workspace_id, SAMPLE_PROJECT_NAME, now],
    )?;
    let project = ProjectView {
        id: installation.project_id,
        workspace_id: installation.workspace_id,
        name: SAMPLE_PROJECT_NAME.to_owned(),
        charter,
        created_at_ms: installation.installed_at_ms,
    };
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'project.created', ?2, ?3)",
        params![
            workspace_id,
            serde_json::to_string(&ProjectCreatedPayload { project })?,
            now
        ],
    )?;

    let repository = StoredRepository {
        revision: 1,
        charter_revision: 1,
        source_kind: RepositorySourceKind::LocalPath,
        source: installation.repository_path.clone(),
        base_revision: installation.base_revision.clone(),
        declared_checks: sample_declared_checks(),
        author_kind: RepositoryAuthor::Operator,
        authored_at_ms: installation.installed_at_ms,
    };
    connection.execute(
        "INSERT INTO project_repository_revisions (
             workspace_id, project_id, revision, charter_revision, source_kind, source,
             base_revision, declared_checks_json, author_kind, authored_at_ms
         ) VALUES (?1, ?2, 1, 1, 'local_path', ?3, ?4, ?5, 'operator', ?6)",
        params![
            workspace_id,
            project_id,
            installation.repository_path,
            installation.base_revision,
            serde_json::to_string(&repository.declared_checks)?,
            now,
        ],
    )?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'project.repository_bound', ?2, ?3)",
        params![
            workspace_id,
            serde_json::to_string(&ProjectRepositoryBoundPayload {
                project: BoundProjectReference {
                    id: installation.project_id,
                },
                repository,
            })?,
            now
        ],
    )?;
    connection.execute(
        "INSERT INTO sample_project_installations (
             workspace_id, project_id, charter_revision, repository_revision,
             repository_path, base_revision, bundle_version, digest_algorithm,
             digest_canonicalizer, digest_value, provenance_source, installed_at_ms
         ) VALUES (?1, ?2, 1, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            workspace_id,
            project_id,
            installation.repository_path,
            installation.base_revision,
            i64::from(installation.provenance.bundle_version),
            installation.provenance.bundle_digest.algorithm,
            installation.provenance.bundle_digest.canonicalizer,
            installation.provenance.bundle_digest.value,
            installation.provenance.source,
            now,
        ],
    )?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'sample_project.installed', ?2, ?3)",
        params![
            workspace_id,
            serde_json::to_string(&SampleProjectInstalledPayload {
                installation: installation.clone(),
            })?,
            now
        ],
    )?;
    Ok(())
}

fn sample_charter(authored_at_ms: u64) -> ProjectCharter {
    ProjectCharter {
        revision: 1,
        purpose: "Exercise Beacon's local proving loop on a small Rust task tracker.".to_owned(),
        desired_outcomes: vec![
            "Produce a reviewed change to the task tracker.".to_owned(),
            "Verify the change with the repository's declared checks.".to_owned(),
        ],
        constraints: vec![
            "Keep all Workspace and repository data on this machine.".to_owned(),
            "Use only the bundled dependency-free Rust crate.".to_owned(),
        ],
        prohibitions: vec![
            "Do not access an Operator repository.".to_owned(),
            "Do not use the network or create external effects.".to_owned(),
        ],
        autonomy_profile: AutonomyProfile::Supervised,
        budget_boundary: BudgetBoundary::NoSpend,
        external_effects: ExternalEffects::Prohibited,
        authored_at_ms,
    }
}

fn sample_declared_checks() -> Vec<DeclaredCheck> {
    vec![
        DeclaredCheck {
            name: "format".to_owned(),
            argv: ["cargo", "fmt", "--all", "--", "--check"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            working_directory: ".".to_owned(),
        },
        DeclaredCheck {
            name: "test".to_owned(),
            argv: ["cargo", "test", "--locked"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            working_directory: ".".to_owned(),
        },
    ]
}

fn computed_bundle_digest() -> String {
    let mut digest = Sha256::new();
    digest.update(DIGEST_CANONICALIZER.as_bytes());
    for bundled in BUNDLE_FILES {
        digest.update((bundled.path.len() as u64).to_be_bytes());
        digest.update(bundled.path.as_bytes());
        digest.update(0o100644_u32.to_be_bytes());
        digest.update((bundled.bytes.len() as u64).to_be_bytes());
        digest.update(bundled.bytes);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn verify_bundled_tree(repository: &Path) -> Result<(), SampleProjectError> {
    let mut actual_files = BTreeSet::new();
    let mut actual_directories = BTreeSet::new();
    collect_bundled_tree_paths(
        repository,
        repository,
        &mut actual_files,
        &mut actual_directories,
    )?;

    let expected_files = BUNDLE_FILES
        .iter()
        .map(|bundled| bundled.path.to_owned())
        .collect::<BTreeSet<_>>();
    let mut expected_directories = BTreeSet::new();
    for bundled in BUNDLE_FILES {
        let mut parent = Path::new(bundled.path).parent();
        while let Some(directory) = parent.filter(|directory| !directory.as_os_str().is_empty()) {
            expected_directories.insert(
                directory
                    .to_str()
                    .ok_or(SampleProjectError::TamperedInstallation)?
                    .to_owned(),
            );
            parent = directory.parent();
        }
    }
    if actual_files != expected_files || actual_directories != expected_directories {
        return Err(SampleProjectError::TamperedInstallation);
    }

    let mut digest = Sha256::new();
    digest.update(DIGEST_CANONICALIZER.as_bytes());
    for bundled in BUNDLE_FILES {
        let path = repository.join(bundled.path);
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| SampleProjectError::TamperedInstallation)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.permissions().mode() & 0o111 != 0
            || metadata.len()
                != u64::try_from(bundled.bytes.len())
                    .map_err(|_| SampleProjectError::TamperedInstallation)?
        {
            return Err(SampleProjectError::TamperedInstallation);
        }
        let bytes = fs::read(path).map_err(|_| SampleProjectError::TamperedInstallation)?;
        if bytes != bundled.bytes {
            return Err(SampleProjectError::TamperedInstallation);
        }
        digest.update((bundled.path.len() as u64).to_be_bytes());
        digest.update(bundled.path.as_bytes());
        digest.update(0o100644_u32.to_be_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(&bytes);
    }
    let actual_digest = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual_digest != EXPECTED_BUNDLE_DIGEST {
        return Err(SampleProjectError::TamperedInstallation);
    }
    Ok(())
}

fn collect_bundled_tree_paths(
    repository: &Path,
    directory: &Path,
    files: &mut BTreeSet<String>,
    directories: &mut BTreeSet<String>,
) -> Result<(), SampleProjectError> {
    let entries = fs::read_dir(directory).map_err(|_| SampleProjectError::TamperedInstallation)?;
    for entry in entries {
        let entry = entry.map_err(|_| SampleProjectError::TamperedInstallation)?;
        let path = entry.path();
        let relative = path
            .strip_prefix(repository)
            .map_err(|_| SampleProjectError::TamperedInstallation)?;
        if relative == Path::new(".git") {
            continue;
        }
        let relative = relative
            .to_str()
            .ok_or(SampleProjectError::TamperedInstallation)?
            .to_owned();
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| SampleProjectError::TamperedInstallation)?;
        if metadata.file_type().is_symlink() {
            return Err(SampleProjectError::TamperedInstallation);
        }
        if metadata.is_dir() {
            directories.insert(relative);
            collect_bundled_tree_paths(repository, &path, files, directories)?;
        } else if metadata.is_file() {
            files.insert(relative);
        } else {
            return Err(SampleProjectError::TamperedInstallation);
        }
    }
    Ok(())
}

/// Validate the frozen Git control files without asking Git to interpret them.
///
/// Adoption is the only call path that reaches a tree Beacon did not create in
/// this process. Keeping this parser data-only prevents repository-local hooks,
/// filters, filesystem monitors, and other configured programs from running.
fn verify_frozen_git_state(repository: &Path) -> Result<(), SampleProjectError> {
    if read_small_regular(&repository.join(".git/HEAD"), 64)? != b"ref: refs/heads/main\n" {
        return Err(SampleProjectError::TamperedInstallation);
    }
    let expected_ref = format!("{EXPECTED_BASE_REVISION}\n");
    if read_small_regular(&repository.join(".git/refs/heads/main"), 64)? != expected_ref.as_bytes()
    {
        return Err(SampleProjectError::TamperedInstallation);
    }
    verify_loose_object(repository, EXPECTED_BASE_REVISION, ObjectKind::Commit)?;
    verify_frozen_index(repository)
}

fn read_small_regular(path: &Path, maximum: u64) -> Result<Vec<u8>, SampleProjectError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| SampleProjectError::TamperedInstallation)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > maximum {
        return Err(SampleProjectError::TamperedInstallation);
    }
    fs::read(path).map_err(|_| SampleProjectError::TamperedInstallation)
}

/// The Sample Project's bundled objects, checked through the ONE loose-object reader.
///
/// This used to be a second inflate-and-self-verify implementation beside
/// `git_object_store`'s. The discipline was the same in both, which is exactly why keeping two was
/// a risk: the next correction would have had to be made twice, and the copy that missed it would
/// still have looked right.
///
/// Every outcome the shared reader distinguishes collapses to `TamperedInstallation` here, and
/// that is correct for this caller alone: the bundled repository is fixed, so an object of the
/// wrong size, a packed one, or an absent one all mean the same thing about an installation that
/// is supposed to be byte-identical to what Beacon shipped. A caller inspecting the Operator's own
/// repository must not collapse them, and does not.
fn verify_loose_object(
    repository: &Path,
    object_id: &str,
    expected: ObjectKind,
) -> Result<(), SampleProjectError> {
    let store =
        ObjectStore::open(repository).map_err(|_| SampleProjectError::TamperedInstallation)?;
    // The 1 MiB bound the path-based reader applied, preserved exactly rather than re-chosen.
    match store.read_loose_object(object_id, 1_048_576, &mut || false) {
        Ok(LooseRead::Read { kind, .. }) if kind == expected => Ok(()),
        _ => Err(SampleProjectError::TamperedInstallation),
    }
}

fn verify_frozen_index(repository: &Path) -> Result<(), SampleProjectError> {
    let index = read_small_regular(&repository.join(".git/index"), 65_536)?;
    if index.len() < 32 || index.get(..4) != Some(b"DIRC") {
        return Err(SampleProjectError::TamperedInstallation);
    }
    let version = u32::from_be_bytes(
        index[4..8]
            .try_into()
            .map_err(|_| SampleProjectError::TamperedInstallation)?,
    );
    let entry_count = u32::from_be_bytes(
        index[8..12]
            .try_into()
            .map_err(|_| SampleProjectError::TamperedInstallation)?,
    );
    if version != 2 || usize::try_from(entry_count).ok() != Some(EXPECTED_INDEX_ENTRIES.len()) {
        return Err(SampleProjectError::TamperedInstallation);
    }

    let content_end = index.len() - 20;
    if Sha1::digest(&index[..content_end])[..] != index[content_end..] {
        return Err(SampleProjectError::TamperedInstallation);
    }
    let mut offset = 12_usize;
    for (expected_path, expected_object_id) in EXPECTED_INDEX_ENTRIES {
        let fixed_end = offset
            .checked_add(62)
            .ok_or(SampleProjectError::TamperedInstallation)?;
        let fixed = index
            .get(offset..fixed_end)
            .ok_or(SampleProjectError::TamperedInstallation)?;
        let mode = u32::from_be_bytes(
            fixed[24..28]
                .try_into()
                .map_err(|_| SampleProjectError::TamperedInstallation)?,
        );
        let flags = u16::from_be_bytes(
            fixed[60..62]
                .try_into()
                .map_err(|_| SampleProjectError::TamperedInstallation)?,
        );
        let name_end = index
            .get(fixed_end..content_end)
            .and_then(|bytes| bytes.iter().position(|byte| *byte == 0))
            .map(|relative| fixed_end + relative)
            .ok_or(SampleProjectError::TamperedInstallation)?;
        let name = index
            .get(fixed_end..name_end)
            .ok_or(SampleProjectError::TamperedInstallation)?;
        if mode != 0o100644
            || flags != u16::try_from(name.len()).unwrap_or(u16::MAX)
            || name != expected_path.as_bytes()
            || fixed.get(40..60)
                != decode_object_id(expected_object_id)
                    .as_ref()
                    .map(|id| &id[..])
        {
            return Err(SampleProjectError::TamperedInstallation);
        }
        verify_loose_object(repository, expected_object_id, ObjectKind::Blob)?;
        let unpadded = 62_usize
            .checked_add(name.len())
            .and_then(|value| value.checked_add(1))
            .ok_or(SampleProjectError::TamperedInstallation)?;
        let padded = unpadded
            .checked_add(7)
            .map(|value| value & !7)
            .ok_or(SampleProjectError::TamperedInstallation)?;
        offset = offset
            .checked_add(padded)
            .ok_or(SampleProjectError::TamperedInstallation)?;
    }
    while offset < content_end {
        let header_end = offset
            .checked_add(8)
            .ok_or(SampleProjectError::TamperedInstallation)?;
        let header = index
            .get(offset..header_end)
            .ok_or(SampleProjectError::TamperedInstallation)?;
        if !header[0].is_ascii_uppercase() {
            return Err(SampleProjectError::TamperedInstallation);
        }
        let size = usize::try_from(u32::from_be_bytes(
            header[4..8]
                .try_into()
                .map_err(|_| SampleProjectError::TamperedInstallation)?,
        ))
        .map_err(|_| SampleProjectError::TamperedInstallation)?;
        offset = header_end
            .checked_add(size)
            .ok_or(SampleProjectError::TamperedInstallation)?;
        if offset > content_end {
            return Err(SampleProjectError::TamperedInstallation);
        }
    }
    Ok(())
}

fn decode_object_id(value: &str) -> Option<[u8; 20]> {
    if value.len() != 40 {
        return None;
    }
    let mut decoded = [0_u8; 20];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = (decode_hex(pair[0])? << 4) | decode_hex(pair[1])?;
    }
    Some(decoded)
}

fn decode_hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn load_installation(
    connection: &Connection,
    workspace_id: Uuid,
) -> Result<Option<SampleProjectInstallation>, SampleProjectError> {
    let row = connection
        .query_row(
            "SELECT project_id, charter_revision, repository_revision, repository_path,
                    base_revision, bundle_version, digest_algorithm, digest_canonicalizer,
                    digest_value, provenance_source, installed_at_ms
             FROM sample_project_installations WHERE workspace_id = ?1",
            [workspace_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((
        project_id,
        charter_revision,
        repository_revision,
        repository_path,
        base_revision,
        bundle_version,
        algorithm,
        canonicalizer,
        value,
        source,
        installed_at_ms,
    )) = row
    else {
        return Ok(None);
    };
    Ok(Some(SampleProjectInstallation {
        workspace_id,
        project_id: Uuid::parse_str(&project_id).map_err(|_| SampleProjectError::InvalidStore)?,
        charter_revision: positive_u32(charter_revision)?,
        repository_revision: positive_u32(repository_revision)?,
        repository_path,
        base_revision,
        provenance: SampleProjectProvenance {
            source,
            bundle_version: positive_u32(bundle_version)?,
            bundle_digest: SampleProjectDigest {
                algorithm,
                canonicalizer,
                value,
            },
        },
        installed_at_ms: u64::try_from(installed_at_ms)
            .map_err(|_| SampleProjectError::InvalidStore)?,
    }))
}

pub(crate) fn verify_sample_project_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), SampleProjectError> {
    let payload: SampleProjectInstalledPayload = serde_json::from_str(payload_json)?;
    let stored =
        load_installation(connection, workspace_id)?.ok_or(SampleProjectError::InvalidStore)?;
    if payload.installation != stored
        || payload.installation.workspace_id != workspace_id
        || payload.installation.installed_at_ms != occurred_at_ms
    {
        return Err(SampleProjectError::InvalidStore);
    }
    Ok(())
}

pub(crate) fn verify_sample_project_state(
    connection: &Connection,
) -> Result<(), SampleProjectStateFault> {
    let mut statement = connection
        .prepare("SELECT workspace_id FROM sample_project_installations ORDER BY workspace_id")
        .map_err(SampleProjectStateFault::Store)?;
    let workspaces = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(SampleProjectStateFault::Store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(SampleProjectStateFault::Store)?;
    for workspace_id in &workspaces {
        let parsed =
            Uuid::parse_str(workspace_id).map_err(|_| SampleProjectStateFault::Identity {
                workspace_id: workspace_id.clone(),
            })?;
        verify_one_installation(connection, parsed).map_err(|cause| {
            SampleProjectStateFault::Row {
                workspace_id: parsed,
                cause,
            }
        })?;
    }
    verify_sample_project_totals(connection, workspaces.len())?;
    Ok(())
}

/// One stored installation, re-proved. Unchanged from what the loop held inline.
fn verify_one_installation(
    connection: &Connection,
    workspace_id: Uuid,
) -> Result<(), SampleProjectError> {
    {
        let installation =
            load_installation(connection, workspace_id)?.ok_or(SampleProjectError::InvalidStore)?;
        validate_stored_installation(&installation)?;
        let count: i64 = connection.query_row(
            "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = 'sample_project.installed'
               AND json_extract(payload_json, '$.installation.projectId') = ?2",
            params![
                workspace_id.to_string(),
                installation.project_id.to_string()
            ],
            |row| row.get(0),
        )?;
        if count != 1 {
            return Err(SampleProjectError::InvalidStore);
        }
    }
    Ok(())
}

/// The count across the store: no row to name.
fn verify_sample_project_totals(
    connection: &Connection,
    installation_count: usize,
) -> Result<(), SampleProjectStateFault> {
    let event_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'sample_project.installed'",
            [],
            |row| row.get(0),
        )
        .map_err(SampleProjectStateFault::Store)?;
    if event_count != i64::try_from(installation_count).unwrap_or(i64::MAX) {
        return Err(SampleProjectStateFault::Totals(crate::TotalsMismatch {
            table: "events (sample_project.installed)",
            expected: installation_count as u64,
            observed: event_count.unsigned_abs(),
        }));
    }
    Ok(())
}

fn validate_stored_installation(
    installation: &SampleProjectInstallation,
) -> Result<(), SampleProjectError> {
    if installation.charter_revision != 1
        || installation.repository_revision != 1
        || installation.base_revision != EXPECTED_BASE_REVISION
        || installation.provenance.source != PROVENANCE_SOURCE
        || installation.provenance.bundle_version != SAMPLE_PROJECT_BUNDLE_VERSION
        || installation.provenance.bundle_digest.algorithm != DIGEST_ALGORITHM
        || installation.provenance.bundle_digest.canonicalizer != DIGEST_CANONICALIZER
        || installation.provenance.bundle_digest.value != EXPECTED_BUNDLE_DIGEST
    {
        return Err(SampleProjectError::InvalidStore);
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), SampleProjectError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.permissions().mode() & 0o077 != 0
            {
                return Err(SampleProjectError::UnsafeDestination);
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_directory(path)
        }
        Err(error) => Err(error.into()),
    }
}

fn create_private_directory(path: &Path) -> Result<(), SampleProjectError> {
    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SampleProjectError::UnsafeDestination);
    }
    Ok(())
}

fn open_private_directory(path: &Path) -> Result<File, SampleProjectError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let directory = options.open(path)?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(SampleProjectError::UnsafeDestination);
    }
    Ok(directory)
}

fn directory_component(name: &OsStr) -> std::io::Result<CString> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    }
    CString::new(bytes).map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))
}

fn create_private_directory_at(parent: &File, name: &OsStr) -> Result<File, SampleProjectError> {
    let name = directory_component(name)?;
    // SAFETY: `parent` and `name` remain live for the syscall and `name` is a
    // single NUL-terminated component. Mode 0700 is never broadened by umask.
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let directory = match open_directory_at(parent, name.as_c_str()) {
        Ok(directory) => directory,
        Err(error) => {
            unsafe {
                libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR);
            }
            return Err(error.into());
        }
    };
    // The mode is set on the DESCRIPTOR, not on the name. `fchmodat` re-resolved the component and
    // was the second of the two pathname operations in this function; `fchmod` cannot be pointed at
    // anything but the directory already opened (gpt1, C35 item 1).
    if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR);
        }
        return Err(error.into());
    }
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(SampleProjectError::UnsafeDestination);
    }
    Ok(directory)
}

/// Synchronization points inside the dispatch copy's creation. Debug builds only.
///
/// Held per service, the way [`crate::DispatchFailpoints`] is, rather than in a global: the tests
/// share one process, and a global was overwritten by whichever test set it last -- four tests that
/// each passed alone failed together.
///
/// This is gpt1's probe boundary made permanent. The interval between creating the directory and
/// opening it is unreachable from any test the tree can express -- every run of it so far has been a
/// reviewer's temporary patch, which is why a hole this old had no committed test.
///
/// There are TWO points because the function makes two different claims, and one point could only
/// ever exercise both at once. `before_open` moves the substitution into the create-to-open window,
/// where only the DESCRIPTOR comparison can see it; `after_open` moves it after the descriptor is
/// held, where only the NAME comparison can. With a single point both checks fire together and
/// deleting either one leaves the test green -- measured, not assumed.
///
/// Each hook receives the staging directory's path so a test can act on its own copy and leave every
/// other test's alone; the tests run in one process.
#[cfg(debug_assertions)]
type CopyHook = Arc<dyn Fn(&Path) + Send + Sync>;

#[cfg(debug_assertions)]
#[derive(Clone, Default)]
#[doc(hidden)]
pub struct CopyFailpoints {
    /// Between `mkdirat` and `openat`: the window gpt1's probe found.
    pub before_open: Option<CopyHook>,
    /// After `openat`, before the staging name is checked and published.
    pub after_open: Option<CopyHook>,
    /// After the verified child is published, before the tree is copied by pathname.
    pub after_publish: Option<CopyHook>,
}

#[cfg(debug_assertions)]
fn copy_failpoint(hook: &Option<CopyHook>, path: &Path) {
    if let Some(hook) = hook {
        hook(path);
    }
}

fn file_identity(file: &File) -> Result<(u64, u64), SampleProjectError> {
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

/// `fstatat(parent, name, AT_SYMLINK_NOFOLLOW)` as a device+inode pair.
fn identity_at(parent: &File, name: &CStr) -> std::io::Result<(u64, u64)> {
    let mut status = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `parent` and `name` remain live for the call, and `status` is written only on success.
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            status.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let status = unsafe { status.assume_init() };
    Ok((status.st_dev, status.st_ino))
}

/// Create the dispatch copy's root, prove the descriptor is the directory we made, and only then
/// publish it under `name`.
///
/// The property this establishes is narrower than "creation and opening are one operation", which
/// POSIX cannot give for a directory, and it is stated as what it is in ADR 0039:
///
///   * the interval cannot be **aimed at** -- the directory is made under a name carrying a fresh
///     UUID, so a substitution cannot be prepared against a name it can predict;
///   * a substitution inside the interval is **detected before any provenance is recorded** -- the
///     inode `mkdirat` produced is captured through the parent descriptor before the window opens,
///     and the opened descriptor is refused unless it is that inode;
///   * the published name refers to that same inode at the moment it is published.
///
/// A witness file written through the descriptor was in the plan and is not here: no test could
/// make its absence fail. Anything that substitutes the directory changes the inode, so the
/// comparisons below fire first, and a check that cannot fail is a sentence rather than a guard.
///
/// Every failure after `mkdirat` removes the child rather than leaving a directory behind that a
/// later run might adopt.
fn create_dispatch_copy_root(
    parent: &File,
    name: &OsStr,
    parent_path: &Path,
    #[cfg(debug_assertions)] failpoints: &CopyFailpoints,
) -> Result<File, SampleProjectError> {
    let published = directory_component(name)?;
    // Unpredictable, so the window cannot be aimed at: an attacker who cannot name the component
    // cannot prepare a replacement for it.
    let staging_name = OsString::from(format!(
        ".{}.{}",
        name.to_string_lossy(),
        Uuid::new_v4().simple()
    ));
    let staging = directory_component(&staging_name)?;

    // SAFETY: `parent` and `staging` remain live for the syscall and `staging` is a single
    // NUL-terminated component. Mode 0700 is never broadened by umask.
    if unsafe { libc::mkdirat(parent.as_raw_fd(), staging.as_ptr(), 0o700) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let outcome = publish_dispatch_copy_root(
        parent,
        &staging,
        &published,
        parent_path,
        #[cfg(debug_assertions)]
        failpoints,
    );
    if outcome.is_err() {
        // SAFETY: single component under the parent descriptor; AT_REMOVEDIR removes only an empty
        // directory, so a substitution that brought contents with it is left for a human to see.
        unsafe {
            libc::unlinkat(parent.as_raw_fd(), staging.as_ptr(), libc::AT_REMOVEDIR);
        }
    }
    outcome
}

fn publish_dispatch_copy_root(
    parent: &File,
    staging: &CStr,
    published: &CStr,
    parent_path: &Path,
    #[cfg(debug_assertions)] failpoints: &CopyFailpoints,
) -> Result<File, SampleProjectError> {
    // Captured BEFORE the window opens: this is the inode `mkdirat` produced, and it is the only
    // thing that can tell the directory we made from one substituted for it.
    let created_identity = identity_at(parent, staging)?;

    #[cfg(debug_assertions)]
    let staging_path = parent_path.join(Path::new(OsStr::from_bytes(staging.to_bytes())));
    #[cfg(not(debug_assertions))]
    let _ = parent_path;
    #[cfg(debug_assertions)]
    copy_failpoint(&failpoints.before_open, &staging_path);

    let directory = open_directory_at(parent, staging)?;

    // The descriptor must BE the directory we created. A replacement installed in the interval is
    // refused here -- before the copy is written into it, before a provenance token is generated,
    // and before any `beacon_repositories` row exists.
    if file_identity(&directory)? != created_identity {
        return Err(SampleProjectError::UnsafeDestination);
    }

    #[cfg(debug_assertions)]
    copy_failpoint(&failpoints.after_open, &staging_path);

    // The mode is set on the descriptor, never on a name.
    if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(SampleProjectError::UnsafeDestination);
    }

    // The name still refers to the inode we verified, at the moment it is published.
    if identity_at(parent, staging)? != created_identity {
        return Err(SampleProjectError::UnsafeDestination);
    }
    // SAFETY: both are single components under the same parent descriptor.
    if unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            staging.as_ptr(),
            parent.as_raw_fd(),
            published.as_ptr(),
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(directory)
}

fn open_directory_at(parent: &File, name: &CStr) -> std::io::Result<File> {
    // SAFETY: `parent` and `name` remain live and the returned descriptor is
    // uniquely owned below.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn write_bundle_at(staging: &File) -> Result<(), SampleProjectError> {
    let source_directory = create_private_directory_at(staging, OsStr::new("src"))?;
    for bundled in BUNDLE_FILES {
        let path = Path::new(bundled.path);
        let parent = match path.parent() {
            Some(parent) if parent.as_os_str().is_empty() => staging,
            Some(parent) if parent == Path::new("src") => &source_directory,
            _ => return Err(SampleProjectError::BundleMismatch),
        };
        let name = path.file_name().ok_or(SampleProjectError::BundleMismatch)?;
        write_private_file_at(parent, name, bundled.bytes)?;
    }
    Ok(())
}

fn write_private_file_at(
    parent: &File,
    name: &OsStr,
    bytes: &[u8],
) -> Result<(), SampleProjectError> {
    let name = directory_component(name)?;
    // SAFETY: the held parent fixes ancestor identity; O_EXCL and O_NOFOLLOW
    // bind creation to one absent final component.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut file = unsafe { File::from_raw_fd(descriptor) };
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

struct DirectoryStream(*mut libc::DIR);

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}

fn directory_entries(directory: &File) -> std::io::Result<Vec<OsString>> {
    let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let raw_stream = unsafe { libc::fdopendir(duplicate) };
    if raw_stream.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::close(duplicate);
        }
        return Err(error);
    }
    let stream = DirectoryStream(raw_stream);
    let mut entries = Vec::new();
    loop {
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(0) {
                break;
            }
            return Err(error);
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name != b"." && name != b".." {
            entries.push(OsString::from_vec(name.to_vec()));
        }
    }
    entries.sort();
    Ok(entries)
}

fn remove_directory_at(parent: &File, name: &OsStr, directory: &File) -> std::io::Result<()> {
    remove_directory_contents(directory)?;
    unlink_at(parent, name, libc::AT_REMOVEDIR)
}

fn remove_directory_contents(directory: &File) -> std::io::Result<()> {
    for name in directory_entries(directory)? {
        let name_c = directory_component(&name)?;
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name_c.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let metadata = unsafe { metadata.assume_init() };
        if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
            let child = open_directory_at(directory, name_c.as_c_str())?;
            remove_directory_contents(&child)?;
            unlink_at(directory, &name, libc::AT_REMOVEDIR)?;
        } else {
            unlink_at(directory, &name, 0)?;
        }
    }
    Ok(())
}

fn unlink_at(parent: &File, name: &OsStr, flags: libc::c_int) -> std::io::Result<()> {
    let name = directory_component(name)?;
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn publish_directory(
    parent: &File,
    staging: &OsStr,
    destination: &OsStr,
) -> Result<(), SampleProjectError> {
    let staging = directory_component(staging)?;
    let destination = directory_component(destination)?;
    let result = unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            staging.as_ptr(),
            parent.as_raw_fd(),
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

fn output_text(output: Output) -> Result<String, ()> {
    if !output.status.success() {
        return Err(());
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| ())
}

fn positive_u32(value: i64) -> Result<u32, SampleProjectError> {
    let value = u32::try_from(value).map_err(|_| SampleProjectError::InvalidStore)?;
    if value == 0 {
        Err(SampleProjectError::InvalidStore)
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};

    use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
    use rusqlite::Connection;
    use uuid::Uuid;

    use super::{
        EXPECTED_BASE_REVISION, EXPECTED_BUNDLE_DIGEST, SAMPLE_PROJECT_BUNDLE_VERSION,
        SampleProjectError, SampleProjectInstallOutcome, SampleProjectService, SampleProjectStatus,
        computed_bundle_digest,
    };
    use crate::{
        AutonomyProfile, OperatorAccountService, ProjectRepositoryService, ProjectService,
        SecretString, StartupConfig, Supervisor,
    };

    static STORE_BOOTSTRAP: Mutex<()> = Mutex::new(());

    struct TestStore {
        root: PathBuf,
        path: PathBuf,
        workspace_id: Uuid,
    }

    impl TestStore {
        fn new(label: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("beacond-sample-{label}-{}", Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let path = root.join("beacon.sqlite3");
            let report = {
                // Schema reconstruction is intentionally thorough. Avoid making
                // unrelated concurrent-startup tests contend with nine fixtures.
                let _bootstrap = STORE_BOOTSTRAP
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                Supervisor::bootstrap(StartupConfig {
                    store_path: path.clone(),
                    initial_workspace_name: "Personal".to_owned(),
                })
                .unwrap()
            };
            Self {
                root,
                path,
                workspace_id: report.workspace.id,
            }
        }

        fn enrolled_accounts(&self) -> Arc<OperatorAccountService> {
            let accounts = Arc::new(OperatorAccountService::open(&self.path).unwrap());
            accounts
                .begin_password_setup(
                    SecretString::new("a long local passphrase".to_owned()),
                    SecretString::new("a long local passphrase".to_owned()),
                )
                .unwrap();
            accounts.complete_operator_setup(true).unwrap();
            accounts
        }

        fn service(&self, accounts: Arc<OperatorAccountService>) -> SampleProjectService {
            SampleProjectService::new(accounts, self.workspace_id, &self.path).unwrap()
        }

        fn isolated_service(&self) -> SampleProjectService {
            self.service(Arc::new(OperatorAccountService::open(&self.path).unwrap()))
                .with_test_authenticated_store(&self.path)
        }

        fn count(&self, table: &str) -> i64 {
            let sql = match table {
                "projects" => "SELECT count(*) FROM projects",
                "sample_project_installations" => {
                    "SELECT count(*) FROM sample_project_installations"
                }
                "sample_project_events" => {
                    "SELECT count(*) FROM events WHERE kind = 'sample_project.installed'"
                }
                "runs" => "SELECT count(*) FROM runs",
                "receipts" => "SELECT count(*) FROM receipts",
                _ => unreachable!("test table is allowlisted"),
            };
            Connection::open(&self.path)
                .unwrap()
                .query_row(sql, [], |row| row.get(0))
                .unwrap()
        }
    }

    impl Drop for TestStore {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn sentinel_program(path: &std::path::Path, sentinel: &std::path::Path) {
        fs::write(
            path,
            format!("#!/bin/sh\n: > '{}'\nexit 97\n", sentinel.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn hardening_probe_git_program(
        path: &std::path::Path,
        hook_sentinel: &std::path::Path,
        filter_sentinel: &std::path::Path,
        ready_sentinel: &std::path::Path,
    ) {
        fs::write(
            path,
            format!(
                "#!/bin/sh\n\
                 set -eu\n\
                 is_init=0\n\
                 for argument in \"$@\"; do\n\
                   if [ \"$argument\" = init ]; then is_init=1; fi\n\
                 done\n\
                 /usr/bin/git \"$@\"\n\
                 if [ \"$is_init\" = 1 ]; then\n\
                   /usr/bin/mkdir -p .git/hooks\n\
                   /usr/bin/printf '%s\\n' '#!/bin/sh' ': > \"{}\"' 'exit 0' > .git/hooks/post-index-change\n\
                   /usr/bin/chmod 700 .git/hooks/post-index-change\n\
                   /usr/bin/printf '%s\\n' '#!/bin/sh' ': > \"{}\"' 'cat' > .git/beacon-hostile-filter\n\
                   /usr/bin/chmod 700 .git/beacon-hostile-filter\n\
                   /usr/bin/printf '* filter=beacon-hostile\\n' > .gitattributes\n\
                   /usr/bin/git config filter.beacon-hostile.clean \"$PWD/.git/beacon-hostile-filter\"\n\
                   /usr/bin/git config filter.beacon-hostile.required true\n\
                   : > \"{}\"\n\
                 fi\n",
                hook_sentinel.display(),
                filter_sentinel.display(),
                ready_sentinel.display(),
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn ancestor_swapping_git_program(
        path: &std::path::Path,
        workspace_root: &std::path::Path,
        moved_workspace: &std::path::Path,
        attacker_workspace: &std::path::Path,
        hostile_filter: &std::path::Path,
        swap_marker: &std::path::Path,
    ) {
        fs::write(
            path,
            format!(
                "#!/bin/sh\n\
                 set -eu\n\
                 if [ ! -e '{}' ]; then\n\
                   if [ \"$1\" = -C ]; then staging_path=$2; else staging_path=$PWD; fi\n\
                   staging_name=$(/usr/bin/basename \"$staging_path\")\n\
                   /usr/bin/mv '{}' '{}'\n\
                   /usr/bin/mkdir -p '{}/'\"$staging_name\"\n\
                   /usr/bin/cp -R '{}/'\"$staging_name\"/. '{}/'\"$staging_name\"/\n\
                   /usr/bin/git -C '{}/'\"$staging_name\" init --quiet --initial-branch=main --template=\n\
                   /usr/bin/printf '* filter=beacon-attack\\n' > '{}/'\"$staging_name\"/.gitattributes\n\
                   /usr/bin/git -C '{}/'\"$staging_name\" config filter.beacon-attack.clean '{}'\n\
                   /usr/bin/git -C '{}/'\"$staging_name\" config filter.beacon-attack.required true\n\
                   /usr/bin/ln -s '{}' '{}'\n\
                   : > '{}'\n\
                 fi\n\
                 exec /usr/bin/git \"$@\"\n",
                swap_marker.display(),
                workspace_root.display(),
                moved_workspace.display(),
                attacker_workspace.display(),
                moved_workspace.display(),
                attacker_workspace.display(),
                attacker_workspace.display(),
                attacker_workspace.display(),
                attacker_workspace.display(),
                hostile_filter.display(),
                attacker_workspace.display(),
                attacker_workspace.display(),
                workspace_root.display(),
                swap_marker.display(),
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn sync_then_swap_workspace_ancestor(path: &std::path::Path) -> std::io::Result<()> {
        super::sync_directory_tree(path)?;
        let workspace_root = path
            .parent()
            .ok_or_else(|| std::io::Error::other("staging directory has no workspace parent"))?;
        let managed_root = workspace_root
            .parent()
            .ok_or_else(|| std::io::Error::other("workspace has no managed parent"))?;
        let test_root = managed_root
            .parent()
            .ok_or_else(|| std::io::Error::other("managed root has no test parent"))?;
        let moved_workspace = test_root.join("post-sync-workspace");
        let attacker_workspace = test_root.join("operator-publish");

        fs::rename(workspace_root, &moved_workspace)?;
        fs::create_dir(&attacker_workspace)?;
        fs::write(attacker_workspace.join("keep"), b"operator data")?;
        symlink(&attacker_workspace, workspace_root)?;
        Ok(())
    }

    fn replace_workspace_with_private_symlink(path: &std::path::Path) -> std::io::Result<()> {
        let managed_root = path
            .parent()
            .ok_or_else(|| std::io::Error::other("workspace has no managed parent"))?;
        let test_root = managed_root
            .parent()
            .ok_or_else(|| std::io::Error::other("managed root has no test parent"))?;
        let original_workspace = test_root.join("workspace-before-open");
        let operator_workspace = test_root.join("operator-workspace");

        fs::rename(path, original_workspace)?;
        fs::create_dir(&operator_workspace)?;
        fs::set_permissions(&operator_workspace, fs::Permissions::from_mode(0o700))?;
        fs::write(operator_workspace.join("keep"), b"operator data")?;
        symlink(&operator_workspace, path)?;
        Ok(())
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn seccomp_statement(code: u16, value: u32) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k: value,
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn seccomp_jump(code: u16, value: u32, jt: u8, jf: u8) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt,
            jf,
            k: value,
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn forbid_at_fdcwd_rename() -> std::io::Result<()> {
        const BPF_LOAD_WORD_ABSOLUTE: u16 = 0x20;
        const BPF_JUMP_EQUAL: u16 = 0x15;
        const BPF_RETURN: u16 = 0x06;
        const SECCOMP_DATA_ARCH_OFFSET: u32 = 4;
        const SECCOMP_DATA_NUMBER_OFFSET: u32 = 0;
        const SECCOMP_DATA_ARGUMENT_ZERO_OFFSET: u32 = 16;
        const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;
        const SECCOMP_RETURN_KILL_PROCESS: u32 = 0x8000_0000;
        const SECCOMP_RETURN_ERRNO: u32 = 0x0005_0000;
        const SECCOMP_RETURN_ALLOW: u32 = 0x7fff_0000;
        const AT_FDCWD_LOW_32: u32 = 0xffff_ff9c;

        let mut filter = [
            seccomp_statement(BPF_LOAD_WORD_ABSOLUTE, SECCOMP_DATA_ARCH_OFFSET),
            seccomp_jump(BPF_JUMP_EQUAL, AUDIT_ARCH_X86_64, 1, 0),
            seccomp_statement(BPF_RETURN, SECCOMP_RETURN_KILL_PROCESS),
            seccomp_statement(BPF_LOAD_WORD_ABSOLUTE, SECCOMP_DATA_NUMBER_OFFSET),
            seccomp_jump(BPF_JUMP_EQUAL, libc::SYS_rename as u32, 5, 0),
            seccomp_jump(BPF_JUMP_EQUAL, libc::SYS_renameat as u32, 1, 0),
            seccomp_jump(BPF_JUMP_EQUAL, libc::SYS_renameat2 as u32, 0, 2),
            seccomp_statement(BPF_LOAD_WORD_ABSOLUTE, SECCOMP_DATA_ARGUMENT_ZERO_OFFSET),
            seccomp_jump(BPF_JUMP_EQUAL, AT_FDCWD_LOW_32, 1, 0),
            seccomp_statement(BPF_RETURN, SECCOMP_RETURN_ALLOW),
            seccomp_statement(BPF_RETURN, SECCOMP_RETURN_ERRNO | libc::EPERM as u32),
        ];
        let program = libc::sock_fprog {
            len: filter.len() as u16,
            filter: filter.as_mut_ptr(),
        };

        // SAFETY: both prctl calls affect only this re-executed test thread.
        // The BPF program is live for the duration of PR_SET_SECCOMP and its
        // branches stay within the fixed instruction array above.
        unsafe {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                &program as *const libc::sock_fprog,
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn assert_at_fdcwd_renameat2_filter_live(path: &std::path::Path) -> std::io::Result<()> {
        let scratch_directory = path
            .parent()
            .expect("staging directory has a parent")
            .join(format!(".seccomp-control-{}", Uuid::new_v4()));
        fs::create_dir(&scratch_directory)?;
        let source_path = scratch_directory.join("a");
        let destination_path = scratch_directory.join("b");
        fs::write(&source_path, b"seccomp control")?;

        let source_path = CString::new(source_path.as_os_str().as_bytes()).unwrap();
        let destination_path = CString::new(destination_path.as_os_str().as_bytes()).unwrap();
        let at_fdcwd_result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source_path.as_ptr(),
                libc::AT_FDCWD,
                destination_path.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        let at_fdcwd_error = std::io::Error::last_os_error();
        assert_eq!(
            at_fdcwd_result, -1,
            "AT_FDCWD renameat2 unexpectedly bypassed the seccomp filter"
        );
        assert_eq!(
            at_fdcwd_error.raw_os_error(),
            Some(libc::EPERM),
            "AT_FDCWD renameat2 failed for the wrong reason: {at_fdcwd_error}"
        );

        let directory = fs::File::open(&scratch_directory)?;
        let source_name = CString::new("a").unwrap();
        let destination_name = CString::new("b").unwrap();
        let descriptor_relative_result = unsafe {
            libc::renameat2(
                directory.as_raw_fd(),
                source_name.as_ptr(),
                directory.as_raw_fd(),
                destination_name.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        assert_eq!(
            descriptor_relative_result,
            0,
            "descriptor-relative renameat2 was blocked: {}",
            std::io::Error::last_os_error()
        );
        assert_eq!(fs::read(scratch_directory.join("b"))?, b"seccomp control");

        fs::remove_file(scratch_directory.join("b"))?;
        fs::remove_dir(scratch_directory)?;
        Ok(())
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn sync_then_forbid_at_fdcwd_rename(path: &std::path::Path) -> std::io::Result<()> {
        super::sync_directory_tree(path)?;
        forbid_at_fdcwd_rename()?;
        assert_at_fdcwd_renameat2_filter_live(path)
    }

    /// TF-2's precondition, made re-runnable rather than left as a claim in a report.
    ///
    /// `sandbox/bounded.rs` spawns a Declared Check's own program, and the kernel refuses to exec
    /// a file that any process holds open for writing. That `ETXTBSY` is a transient condition the
    /// spawn site records as a permanent `NotStarted` outcome -- but it is unreachable in
    /// production, and this is one of the three measured reasons why: nothing the materializer
    /// writes is executable, so no file it produces can be the target of an exec at all. The other
    /// two closers, and the conditions that would flip them, are in
    /// `beacon-team/OTF2-opus2-build-report.md`.
    ///
    /// Regular files only. Directories carry the execute bit because traversal needs it, and a
    /// symlink is not a file the materializer is allowed to create.
    #[test]
    fn the_production_materializer_writes_no_executable_file() {
        let store = TestStore::new("materializer-modes");
        let accounts = store.enrolled_accounts();
        let service =
            SampleProjectService::new(Arc::clone(&accounts), store.workspace_id, &store.path)
                .unwrap();
        let installed = service.install().unwrap();
        let root = PathBuf::from(&installed.installation.repository_path);

        let mut executable = Vec::new();
        let mut files = 0_usize;
        let mut directories = vec![root.clone()];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                let metadata = fs::symlink_metadata(&path).unwrap();
                if metadata.is_dir() {
                    directories.push(path);
                    continue;
                }
                if !metadata.is_file() {
                    continue;
                }
                files += 1;
                let mode = metadata.permissions().mode() & 0o7777;
                if mode & 0o111 != 0 {
                    executable.push(format!(
                        "{} is mode {mode:o}",
                        path.strip_prefix(&root).unwrap_or(&path).display()
                    ));
                }
            }
        }

        assert!(files > 0, "the walk found no files, so it proved nothing");
        assert!(
            executable.is_empty(),
            "the materializer wrote {} executable file(s) of {files}: {}.\n\
             This is TF-2's precondition. An executable file written in Beacon's own address space \
             can be held open for writing by a sibling fork and then exec'd, and the kernel answers \
             ETXTBSY -- which sandbox/bounded.rs records as a permanent NotStarted outcome for a \
             transient cause. See beacon-team/OTF2-opus2-build-report.md, \"the condition to \
             watch\": if this is now intended, that report's port is the fix.",
            executable.len(),
            executable.join(", ")
        );
    }

    #[test]
    fn authenticated_operator_installs_an_inert_supervised_sample_project() {
        let store = TestStore::new("install");
        let accounts = store.enrolled_accounts();
        let service =
            SampleProjectService::new(Arc::clone(&accounts), store.workspace_id, &store.path)
                .unwrap();

        assert!(matches!(
            service.status().unwrap(),
            SampleProjectStatus::Available { .. }
        ));
        let result = service.install().unwrap();
        assert_eq!(result.outcome, SampleProjectInstallOutcome::Installed);
        assert_eq!(result.installation.workspace_id, store.workspace_id);
        assert_eq!(result.installation.charter_revision, 1);
        assert_eq!(result.installation.repository_revision, 1);

        let project = ProjectService::new(Arc::clone(&accounts), store.workspace_id)
            .get(result.installation.project_id)
            .unwrap();
        assert_eq!(
            project.charter.autonomy_profile,
            AutonomyProfile::Supervised
        );
        let repository = ProjectRepositoryService::new(accounts, store.workspace_id)
            .current(project.id)
            .unwrap()
            .unwrap();
        assert_eq!(repository.base_revision, result.installation.base_revision);
        assert_eq!(repository.declared_checks.len(), 2);
        assert!(
            repository
                .declared_checks
                .iter()
                .all(|check| !check.argv.is_empty())
        );
        for check in &repository.declared_checks {
            let output = Command::new(&check.argv[0])
                .args(&check.argv[1..])
                .current_dir(&result.installation.repository_path)
                .env("CARGO_TARGET_DIR", store.root.join("sample-cargo-target"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "declared check {} failed: {}",
                check.name,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let output = Command::new("git")
            .args([
                "-C",
                &result.installation.repository_path,
                "rev-parse",
                "HEAD",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            result.installation.base_revision
        );

        let connection = Connection::open(&store.path).unwrap();
        for table in ["runs", "receipts"] {
            let count: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "installation must not publish {table}");
        }
    }

    #[test]
    fn bundle_v1_identity_is_permanent_and_future_bundles_are_additive() {
        assert_eq!(SAMPLE_PROJECT_BUNDLE_VERSION, 1);
        assert_eq!(computed_bundle_digest(), EXPECTED_BUNDLE_DIGEST);
        assert_eq!(
            EXPECTED_BASE_REVISION,
            "c5a2dad9164a1fd5346fdff3d3e2d8d17af922d4"
        );
    }

    #[test]
    fn available_status_uses_the_exact_camel_case_wire_shape() {
        let status = SampleProjectStatus::Available {
            bundle_version: 1,
            bundle_digest: super::SampleProjectDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.sample-project.bundle.v1".to_owned(),
                value: "0".repeat(64),
            },
        };

        assert_eq!(
            serde_json::to_value(status).unwrap(),
            serde_json::json!({
                "state": "available",
                "bundleVersion": 1,
                "bundleDigest": {
                    "algorithm": "sha256",
                    "canonicalizer": "beacon.sample-project.bundle.v1",
                    "value": "0000000000000000000000000000000000000000000000000000000000000000"
                }
            })
        );
    }

    #[test]
    fn status_and_install_refuse_an_unauthenticated_operator_without_filesystem_effects() {
        let store = TestStore::new("unauthenticated");
        let accounts = Arc::new(OperatorAccountService::open(&store.path).unwrap());
        let service = store.service(accounts);

        assert!(matches!(
            service.status(),
            Err(SampleProjectError::Account(
                crate::OperatorAccountError::NotAuthenticated
            ))
        ));
        assert!(matches!(
            service.install(),
            Err(SampleProjectError::Account(
                crate::OperatorAccountError::NotAuthenticated
            ))
        ));
        assert!(!store.root.join("sample-projects").exists());
        assert_eq!(store.count("projects"), 0);
    }

    #[test]
    fn repeat_install_is_explicitly_idempotent_and_revision_is_deterministic() {
        let first = TestStore::new("deterministic-first");
        let first_service = first.isolated_service();
        let installed = first_service.install().unwrap();
        let repeated = first_service.install().unwrap();

        assert_eq!(
            repeated.outcome,
            SampleProjectInstallOutcome::AlreadyInstalled
        );
        assert_eq!(repeated.installation, installed.installation);
        assert_eq!(first.count("projects"), 1);
        assert_eq!(first.count("sample_project_installations"), 1);
        assert_eq!(first.count("sample_project_events"), 1);

        let second = TestStore::new("deterministic-second");
        let second_service = second.isolated_service();
        let second_install = second_service.install().unwrap();
        assert_eq!(
            second_install.installation.base_revision,
            installed.installation.base_revision
        );
        assert_eq!(
            installed.installation.base_revision,
            "c5a2dad9164a1fd5346fdff3d3e2d8d17af922d4"
        );
    }

    #[test]
    fn orphaned_repository_with_a_different_commit_is_refused() {
        let store = TestStore::new("orphaned-revision");
        let service = store.isolated_service();
        let destination = service.repository_path();
        service.materialize_repository(&destination).unwrap();

        let mut command = Command::new("git");
        command
            .args(["-C"])
            .arg(&destination)
            .args([
                "-c",
                "user.name=Beacon",
                "-c",
                "user.email=sample-project@beacon.invalid",
                "commit",
                "--allow-empty",
                "--amend",
                "--no-gpg-sign",
                "-m",
                "Different commit with the same tree",
            ])
            .env("GIT_AUTHOR_DATE", "2001-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2001-01-01T00:00:00Z")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "failed to prepare orphaned repository: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        assert!(matches!(
            service.install(),
            Err(SampleProjectError::TamperedInstallation)
        ));
        assert_eq!(store.count("sample_project_installations"), 0);
    }

    #[test]
    fn orphan_with_rewritten_expected_object_is_refused() {
        let store = TestStore::new("orphan-rewritten-object");
        let service = store.isolated_service();
        let destination = service.repository_path();
        service.materialize_repository(&destination).unwrap();
        let object = destination
            .join(".git/objects")
            .join(&EXPECTED_BASE_REVISION[..2])
            .join(&EXPECTED_BASE_REVISION[2..]);
        let mut permissions = fs::metadata(&object).unwrap().permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&object, permissions).unwrap();
        let compressed = fs::read(&object).unwrap();
        let mut decoded = Vec::new();
        ZlibDecoder::new(compressed.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        *decoded.last_mut().unwrap() ^= 0x01;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&decoded).unwrap();
        fs::write(object, encoder.finish().unwrap()).unwrap();

        assert!(matches!(
            service.install(),
            Err(SampleProjectError::TamperedInstallation)
        ));
        assert_eq!(store.count("sample_project_installations"), 0);
    }

    #[test]
    fn orphan_with_trailing_bytes_after_expected_object_is_refused() {
        let store = TestStore::new("orphan-trailing-object-bytes");
        let service = store.isolated_service();
        let destination = service.repository_path();
        service.materialize_repository(&destination).unwrap();
        let object = destination
            .join(".git/objects")
            .join(&EXPECTED_BASE_REVISION[..2])
            .join(&EXPECTED_BASE_REVISION[2..]);
        let mut permissions = fs::metadata(&object).unwrap().permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&object, permissions).unwrap();
        let mut bytes = fs::read(&object).unwrap();
        bytes.extend_from_slice(b"trailing-garbage");
        fs::write(object, bytes).unwrap();

        assert!(matches!(
            service.install(),
            Err(SampleProjectError::TamperedInstallation)
        ));
        assert_eq!(store.count("sample_project_installations"), 0);
    }

    #[test]
    fn orphan_with_truncated_expected_object_is_refused() {
        let store = TestStore::new("orphan-truncated-object");
        let service = store.isolated_service();
        let destination = service.repository_path();
        service.materialize_repository(&destination).unwrap();
        let object = destination
            .join(".git/objects")
            .join(&EXPECTED_BASE_REVISION[..2])
            .join(&EXPECTED_BASE_REVISION[2..]);
        let mut permissions = fs::metadata(&object).unwrap().permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&object, permissions).unwrap();
        let mut bytes = fs::read(&object).unwrap();
        bytes.pop().unwrap();
        fs::write(object, bytes).unwrap();

        assert!(matches!(
            service.install(),
            Err(SampleProjectError::TamperedInstallation)
        ));
        assert_eq!(store.count("sample_project_installations"), 0);
    }

    #[test]
    fn orphan_with_rewritten_index_checksum_is_refused() {
        let store = TestStore::new("orphan-rewritten-index-checksum");
        let service = store.isolated_service();
        let destination = service.repository_path();
        service.materialize_repository(&destination).unwrap();
        let index = destination.join(".git/index");
        let mut bytes = fs::read(&index).unwrap();
        let last = bytes.last_mut().unwrap();
        *last ^= 0xff;
        fs::write(index, bytes).unwrap();

        assert!(matches!(
            service.install(),
            Err(SampleProjectError::TamperedInstallation)
        ));
        assert_eq!(store.count("sample_project_installations"), 0);
    }

    #[test]
    fn adopting_an_exact_orphan_executes_no_program_from_its_tree() {
        let store = TestStore::new("orphan-no-program");
        let service = store.isolated_service();
        let destination = service.repository_path();
        service.materialize_repository(&destination).unwrap();
        let sentinel = store.root.join("orphan-program-ran");
        let hostile_git = destination.join(".git/hostile-git");
        sentinel_program(&hostile_git, &sentinel);

        let result = store
            .isolated_service()
            .with_git_program(&hostile_git)
            .install();

        assert!(
            !sentinel.exists(),
            "orphan verification executed a program from the tree"
        );
        assert!(
            result.is_ok(),
            "an exact orphan remains adoptable: {result:?}"
        );
    }

    #[test]
    fn git_environment_drops_ambient_repository_redirection() {
        let store = TestStore::new("git-environment");
        let env_program = store.root.join("print-environment");
        fs::write(&env_program, "#!/bin/sh\nexec /usr/bin/env\n").unwrap();
        fs::set_permissions(&env_program, fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = Command::new(env_program);
        command
            .env("GIT_DIR", "/operator/repository/.git")
            .env("GIT_WORK_TREE", "/operator/repository")
            .env("GIT_INDEX_FILE", "/operator/repository/.git/index")
            .stdout(Stdio::piped());
        crate::worktree::apply_git_hardening(&mut command, &[]);

        let output = command.output().unwrap();
        assert!(output.status.success());
        let environment = String::from_utf8(output.stdout).unwrap();
        assert!(!environment.contains("GIT_DIR="));
        assert!(!environment.contains("GIT_WORK_TREE="));
        assert!(!environment.contains("GIT_INDEX_FILE="));
    }

    #[test]
    fn sample_project_git_never_runs_repository_hooks_or_filter_drivers() {
        let store = TestStore::new("git-hardening");
        let hook_sentinel = store.root.join("post-index-change-ran");
        let filter_sentinel = store.root.join("filter-ran");
        let ready_sentinel = store.root.join("probe-ready");
        let git_program = store.root.join("hardening-probe-git");
        hardening_probe_git_program(
            &git_program,
            &hook_sentinel,
            &filter_sentinel,
            &ready_sentinel,
        );

        let result = store
            .isolated_service()
            .with_git_program(&git_program)
            .install();

        assert!(result.is_err(), "the injected tree must not be installed");
        assert!(ready_sentinel.exists(), "the hardening probe did not run");
        assert!(
            !hook_sentinel.exists(),
            "repository-controlled post-index-change hook ran"
        );
        assert!(
            !filter_sentinel.exists(),
            "repository-controlled filter driver ran"
        );
        assert_eq!(store.count("sample_project_installations"), 0);
    }

    #[test]
    fn bundle_mismatch_fails_before_staging_or_database_publication() {
        let store = TestStore::new("bundle-mismatch");
        let service = store
            .isolated_service()
            .with_expected_bundle_digest(&"0".repeat(64));

        assert!(matches!(
            service.install(),
            Err(SampleProjectError::BundleMismatch)
        ));
        assert!(!store.root.join("sample-projects").exists());
        assert_eq!(store.count("projects"), 0);
        assert_eq!(store.count("sample_project_installations"), 0);
    }

    #[test]
    fn symlinked_managed_root_is_refused_without_touching_its_target() {
        let store = TestStore::new("symlink");
        let target = store.root.join("operator-owned");
        fs::create_dir(&target).unwrap();
        symlink(&target, store.root.join("sample-projects")).unwrap();
        let service = store.isolated_service();

        assert!(matches!(
            service.install(),
            Err(SampleProjectError::UnsafeDestination)
        ));
        assert_eq!(fs::read_dir(target).unwrap().count(), 0);
        assert_eq!(store.count("projects"), 0);
        assert_eq!(store.count("sample_project_installations"), 0);
    }

    #[test]
    fn install_refuses_a_workspace_root_swapped_for_a_private_symlink() {
        let store = TestStore::new("workspace-open-symlink");
        let service = store
            .isolated_service()
            .with_before_workspace_open(replace_workspace_with_private_symlink);

        assert!(service.install().is_err());
        let operator_workspace = store.root.join("operator-workspace");
        assert_eq!(
            fs::read(operator_workspace.join("keep")).unwrap(),
            b"operator data"
        );
        assert_eq!(fs::read_dir(operator_workspace).unwrap().count(), 1);
        assert_eq!(store.count("projects"), 0);
        assert_eq!(store.count("sample_project_installations"), 0);
    }

    #[test]
    fn git_failure_cleans_partial_staging_and_publishes_no_database_state() {
        let store = TestStore::new("git-failure");
        let service = store.isolated_service().with_git_program("/bin/false");

        assert!(matches!(
            service.install(),
            Err(SampleProjectError::MaterializationFailed)
        ));
        let workspace_root = store
            .root
            .join("sample-projects")
            .join(store.workspace_id.to_string());
        assert_eq!(fs::read_dir(workspace_root).unwrap().count(), 0);
        assert_eq!(store.count("projects"), 0);
        assert_eq!(store.count("sample_project_installations"), 0);
        assert_eq!(store.count("sample_project_events"), 0);
        assert_eq!(store.count("runs"), 0);
        assert_eq!(store.count("receipts"), 0);
    }

    #[test]
    fn ancestor_swap_before_git_cannot_execute_attacker_filter_or_redirect_cleanup() {
        let store = TestStore::new("git-ancestor-swap");
        let workspace_root = store
            .root
            .join("sample-projects")
            .join(store.workspace_id.to_string());
        let moved_workspace = store.root.join("moved-workspace");
        let attacker_workspace = store.root.join("operator-owned");
        fs::create_dir(&attacker_workspace).unwrap();
        let operator_data = attacker_workspace.join("keep");
        fs::write(&operator_data, b"operator data").unwrap();
        let filter_sentinel = store.root.join("attacker-filter-ran");
        let hostile_filter = store.root.join("hostile-filter");
        sentinel_program(&hostile_filter, &filter_sentinel);
        let swap_marker = store.root.join("ancestor-was-swapped");
        let hostile_git = store.root.join("ancestor-swapping-git");
        ancestor_swapping_git_program(
            &hostile_git,
            &workspace_root,
            &moved_workspace,
            &attacker_workspace,
            &hostile_filter,
            &swap_marker,
        );
        let service = store.isolated_service().with_git_program(&hostile_git);

        let result = service.install();
        assert!(matches!(
            result,
            Err(SampleProjectError::TamperedInstallation)
        ));
        assert!(swap_marker.exists(), "the exact race hook did not run");
        assert!(
            !filter_sentinel.exists(),
            "Git followed the replaced ancestor into an attacker repository"
        );
        assert_eq!(fs::read(&operator_data).unwrap(), b"operator data");
        assert_eq!(
            fs::read_dir(&attacker_workspace)
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_dir())
                .count(),
            1,
            "cleanup followed the replaced ancestor into the attacker repository"
        );
        assert_eq!(
            fs::read_dir(&moved_workspace).unwrap().count(),
            0,
            "cleanup did not remove the descriptor-bound staging directory"
        );
        assert_eq!(store.count("projects"), 0);
        assert_eq!(store.count("sample_project_installations"), 0);
        assert_eq!(store.count("sample_project_events"), 0);
        assert_eq!(store.count("runs"), 0);
        assert_eq!(store.count("receipts"), 0);
    }

    #[test]
    fn post_sync_ancestor_swap_publishes_only_beneath_held_workspace() {
        let store = TestStore::new("post-sync-ancestor-swap");
        let service = store
            .isolated_service()
            .with_repository_tree_sync(sync_then_swap_workspace_ancestor);
        let destination = service.repository_path();

        let revision = service.materialize_repository(&destination).unwrap();

        assert_eq!(revision, EXPECTED_BASE_REVISION);
        assert!(
            store
                .root
                .join("post-sync-workspace")
                .join(super::SAMPLE_REPOSITORY_DIRECTORY)
                .is_dir(),
            "publication did not stay beneath the held workspace descriptor"
        );
        let attacker_workspace = store.root.join("operator-publish");
        assert!(
            !attacker_workspace
                .join(super::SAMPLE_REPOSITORY_DIRECTORY)
                .exists(),
            "publication followed the replaced workspace pathname"
        );
        assert_eq!(
            fs::read(attacker_workspace.join("keep")).unwrap(),
            b"operator data"
        );
    }

    #[test]
    fn publication_never_resolves_names_from_at_fdcwd() {
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            const CHILD_ENV: &str = "BEACON_SAMPLE_PUBLICATION_SECCOMP_CHILD";
            if std::env::var_os(CHILD_ENV).is_some() {
                let store = TestStore::new("publication-seccomp-child");
                let service = store
                    .isolated_service()
                    .with_repository_tree_sync(sync_then_forbid_at_fdcwd_rename);

                let installed = service.install().unwrap();

                assert_eq!(installed.outcome, SampleProjectInstallOutcome::Installed);
                assert!(service.repository_path().is_dir());
                return;
            }

            let output = Command::new(std::env::current_exe().unwrap())
                .arg("sample_project::tests::publication_never_resolves_names_from_at_fdcwd")
                .args(["--exact", "--test-threads=1"])
                .env(CHILD_ENV, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "seccomp publication child failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        eprintln!("skipped: AT_FDCWD seccomp guard requires Linux x86_64");
    }

    #[test]
    fn repository_tree_sync_failure_cleans_staging_before_publication() {
        fn fail_sync(_path: &std::path::Path) -> std::io::Result<()> {
            Err(std::io::Error::other(
                "injected repository tree sync failure",
            ))
        }

        let store = TestStore::new("tree-sync-failure");
        let service = store
            .isolated_service()
            .with_repository_tree_sync(fail_sync);

        assert!(matches!(service.install(), Err(SampleProjectError::Io(_))));
        let workspace_root = store
            .root
            .join("sample-projects")
            .join(store.workspace_id.to_string());
        assert_eq!(fs::read_dir(workspace_root).unwrap().count(), 0);
        assert_eq!(store.count("projects"), 0);
        assert_eq!(store.count("sample_project_installations"), 0);
        assert_eq!(store.count("sample_project_events"), 0);
    }

    #[test]
    fn database_publication_failure_leaves_only_an_exact_adoptable_orphan() {
        let store = TestStore::new("database-publication-failure");
        let service = store.isolated_service();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_sample_project_publication
                 BEFORE INSERT ON sample_project_installations
                 BEGIN
                     SELECT RAISE(ABORT, 'injected publication failure');
                 END;",
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            service.install(),
            Err(SampleProjectError::Store(_))
        ));
        assert!(service.repository_path().is_dir());
        assert_eq!(store.count("projects"), 0);
        assert_eq!(store.count("sample_project_installations"), 0);
        assert_eq!(store.count("sample_project_events"), 0);

        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute_batch("DROP TRIGGER fail_sample_project_publication;")
            .unwrap();
        drop(connection);

        let recovered = service.install().unwrap();
        assert_eq!(recovered.outcome, SampleProjectInstallOutcome::Installed);
        assert_eq!(recovered.installation.base_revision, EXPECTED_BASE_REVISION);
        assert_eq!(store.count("projects"), 1);
        assert_eq!(store.count("sample_project_installations"), 1);
        assert_eq!(store.count("sample_project_events"), 1);
    }

    #[test]
    fn installed_repository_tamper_fails_status_closed() {
        let store = TestStore::new("repository-tamper");
        let service = store.isolated_service();
        let installed = service.install().unwrap();
        fs::write(
            PathBuf::from(&installed.installation.repository_path).join("README.md"),
            "changed outside Beacon",
        )
        .unwrap();

        assert!(matches!(
            service.status(),
            Err(SampleProjectError::TamperedInstallation)
        ));
        assert_eq!(store.count("sample_project_installations"), 1);
    }

    #[test]
    fn startup_reconciliation_refuses_missing_installation_event() {
        let store = TestStore::new("event-corruption");
        store.isolated_service().install().unwrap();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute(
                "DELETE FROM events WHERE kind = 'sample_project.installed'",
                [],
            )
            .unwrap();
        drop(connection);

        let error = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .unwrap_err();
        // R28.1: the refusal names the Workspace it refused, and R28.4's line is the key plus
        // the fault's NAME with the reader's own sentence absent.
        assert!(
            matches!(
                &error,
                crate::SupervisorError::SampleProjectStateMismatch { workspace_id, .. }
                    if !workspace_id.is_nil()
            ),
            "the refusal does not name the Workspace it refused: {error:?}"
        );
        let line = error.to_string();
        let named = line.rsplit(": ").next().unwrap().to_owned();
        assert!(
            !named.contains(' ') && !named.contains('{'),
            "the tail of the line is a rendering, not a name: {line}"
        );
        assert!(
            format!("{error:?}").contains(&named),
            "the named fault is not reachable through Debug: {error:?}"
        );
    }
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()` (the fold at 30d2a20): an exhaustive `name()` catches an
    /// added arm, only a second table catches a wrong or duplicated name.
    fn expected_name(error: &SampleProjectError) -> &'static str {
        match error {
            SampleProjectError::Account(_) => "Account",
            SampleProjectError::NotInstalled => "NotInstalled",
            SampleProjectError::UnsafeDestination => "UnsafeDestination",
            SampleProjectError::BundleMismatch => "BundleMismatch",
            SampleProjectError::MaterializationFailed => "MaterializationFailed",
            SampleProjectError::TamperedInstallation => "TamperedInstallation",
            SampleProjectError::InvalidStore => "InvalidStore",
            SampleProjectError::Store(_) => "Store",
            SampleProjectError::Io(_) => "Io",
            SampleProjectError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_sample_project_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            SampleProjectError::Account(
                crate::operator_account::OperatorAccountError::PasswordTooShort,
            ),
            SampleProjectError::NotInstalled,
            SampleProjectError::UnsafeDestination,
            SampleProjectError::BundleMismatch,
            SampleProjectError::MaterializationFailed,
            SampleProjectError::TamperedInstallation,
            SampleProjectError::InvalidStore,
            SampleProjectError::Store(rusqlite::Error::QueryReturnedNoRows),
            SampleProjectError::Io(std::io::Error::other("x")),
            SampleProjectError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            10,
            "an arm was added to `SampleProjectError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(SampleProjectError::name).collect();
        assert_eq!(
            names.len(),
            10,
            "two arms share a startup name, so a swap between them is invisible: {names:?}"
        );
        for name in names {
            assert!(
                !name.contains(' ') && !name.contains('{'),
                "a name is a name, not a rendering: {name}"
            );
        }
    }
}
