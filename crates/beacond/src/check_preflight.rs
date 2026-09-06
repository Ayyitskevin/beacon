//! Check Preflight and the first Receipt — ADR 0027.
//!
//! Preflight establishes whether executing a Project's Declared Checks would be
//! admissible, and then refuses to execute them. It runs no Declared Check,
//! spawns no process, opens no network connection, and creates no worktree or
//! checkout. Its only external effect is a bounded read of the named local
//! repository.
//!
//! The refusal is the design. Execution additionally requires a sandbox
//! capability that does not exist, so a binding that satisfies every
//! verification step terminates as [`PreflightOutcome::SandboxUnavailable`].
//! That outcome stays correct whenever a sandbox is absent, unsupported, or
//! unhealthy; it does not become obsolete when one ships.

use std::fmt;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::git_object_store::{ObjectStore, ObjectStoreError};
use crate::operator_account::{OperatorAccountError, OperatorAccountService};
use crate::project_repository::{
    PreflightBinding, ProjectRepositoryError, RepositorySourceKind,
    latest_repository_for_preflight, repository_for_preflight_at,
};
use crate::sandbox::SandboxCapability;

/// How long a Run may hold its lease before reconciliation calls it unconfirmed.
const LEASE_MS: u64 = 30_000;
/// How long the filesystem phase may take before the answer is unknown.
const INSPECTION_BUDGET_MS: u128 = 5_000;
/// Recorded on every Receipt that reached the base-revision step. The object
/// store answers presence, not reachability, and the difference is real: a
/// commit abandoned by a rebase is present and reachable from nothing.
const REACHABILITY_GAP: &str =
    "base revision reachability from a ref (object presence verified only)";

/// The terminal state of one preflight.
///
/// A step that returns a negative answer, a step that errors, and a step that
/// does not finish are three distinct outcomes and never collapse into one
/// another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightOutcome {
    /// Every step held and a sandbox is available. Unreachable until one exists.
    Verified,
    /// The binding names a Git URL. Confirming a pinned revision in a remote
    /// requires a network fetch the operating envelope denies by default.
    SourceKindUnsupported,
    /// The pinned binding does not apply to the Run's pinned Charter revision.
    BindingStaleForCharter,
    /// The source path is missing, unreadable, or not a Git repository.
    RepositoryUnreadable,
    /// The pinned base revision is not present in the repository's object store.
    BaseRevisionUnreachable,
    /// Historical v1 outcome from before Runs pinned revisions at open.
    /// Retained so stored Receipts remain decodable; new Runs do not emit it.
    DeclaredChecksChanged,
    /// A check's resolved working directory lies outside the repository root.
    WorkingDirectoryEscapes,
    /// Every step held; execution is refused because no sandbox capability
    /// exists. A refusal, not a defect.
    SandboxUnavailable,
    /// A verification step errored. Distinct from a negative answer.
    PreflightFailed,
    /// A verification step did not finish inside its budget. Distinct from both
    /// a negative answer and an error.
    PreflightTimedOut,
}

impl PreflightOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::SourceKindUnsupported => "source_kind_unsupported",
            Self::BindingStaleForCharter => "binding_stale_for_charter",
            Self::RepositoryUnreadable => "repository_unreadable",
            Self::BaseRevisionUnreachable => "base_revision_unreachable",
            Self::DeclaredChecksChanged => "declared_checks_changed",
            Self::WorkingDirectoryEscapes => "working_directory_escapes",
            Self::SandboxUnavailable => "sandbox_unavailable",
            Self::PreflightFailed => "preflight_failed",
            Self::PreflightTimedOut => "preflight_timed_out",
        }
    }

    /// The Run state this outcome terminates in. Only a sandbox-admissible
    /// preflight is `verified`; every other terminal outcome is a refusal.
    ///
    /// `pub(crate)` because it is the single source of this mapping: the
    /// recent-Receipts reader validates a stored `(outcome, state)` pair
    /// against it rather than keeping a third copy that a remapped variant
    /// would leave stale.
    pub(crate) fn run_state(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            _ => "refused",
        }
    }
}

/// A Receipt field that the specification requires and this producer cannot
/// fill. Recorded explicitly so a later reader can tell a measured value from
/// one that was never available — an empty string and an unavailable string are
/// different facts.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Recorded<T> {
    Present { value: T },
    Absent { reason: String },
}

impl<T> Recorded<T> {
    fn absent(reason: &str) -> Self {
        Self::Absent {
            reason: reason.to_owned(),
        }
    }
}

/// One Declared Check as preflight resolved it. `resolved_working_directory` is
/// what containment was decided on — never the declared string.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedCheck {
    pub name: String,
    pub argv: Vec<String>,
    pub declared_working_directory: String,
    pub resolved_working_directory: Recorded<String>,
}

/// The parts of the Receipt specification a preflight cannot honour.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnavailableEvidence {
    pub proposal_digest: Recorded<String>,
    pub ruling_digest: Recorded<String>,
    pub resulting_revision: Recorded<String>,
    pub filesystem_effects: Recorded<String>,
    pub diff_digest: Recorded<String>,
    pub model_invocations: Recorded<u32>,
    pub cost_usd_micros: Recorded<u64>,
}

impl UnavailableEvidence {
    fn for_preflight() -> Self {
        const NO_PROPOSAL: &str = "no Proposal surface exists";
        const NOTHING_RAN: &str = "preflight refuses to execute, so nothing ran";
        const NO_INFERENCE: &str = "preflight performs no model inference";
        Self {
            proposal_digest: Recorded::absent(NO_PROPOSAL),
            ruling_digest: Recorded::absent(NO_PROPOSAL),
            resulting_revision: Recorded::absent(NOTHING_RAN),
            filesystem_effects: Recorded::absent(NOTHING_RAN),
            diff_digest: Recorded::absent(NOTHING_RAN),
            model_invocations: Recorded::absent(NO_INFERENCE),
            cost_usd_micros: Recorded::absent(NO_INFERENCE),
        }
    }
}

/// The record of one preflight.
///
/// A `Verified` preflight is not a passing check: it reports that execution
/// would be admissible, never that a check succeeded.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreflightReceipt {
    pub version: u32,
    pub run_id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub repository_revision: u32,
    pub charter_revision: u32,
    pub outcome: PreflightOutcome,
    pub source_kind: RepositorySourceKind,
    pub source: String,
    pub base_revision: String,
    pub resolved_checks: Vec<ResolvedCheck>,
    pub sandbox_version: String,
    pub policy_charter_revision: u32,
    pub policy_repository_revision: u32,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    /// Every step this preflight did not perform, named. Required: a Receipt
    /// that cannot say what it failed to check is not evidence.
    pub not_verified: Vec<String>,
    pub unavailable: UnavailableEvidence,
}

/// Why a preflight could not be attempted at all. These produce no Run and no
/// Receipt: there was nothing to preflight.
/// Which fault the Run startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum RunStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One Run did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    Row {
        workspace_id: Uuid,
        run_id: String,
        cause: CheckPreflightError,
    },
    // NO `Totals` ARM. This pass compares each Run against its OWN Receipts and Events; it makes
    // no whole-store count comparison, so there is no totals fault to report.
    /// The clock is before the Unix epoch, so no lease can be judged expired. Not a row fault.
    Clock,
    /// A row whose Workspace or Run identity is not a UUID. The Workspace case is unreachable
    /// while the foreign-key check and Workspace pass hold; the Run key has no schema UUID check.
    Identity {
        workspace_id: String,
        run_id: String,
    },
}

impl CheckPreflightError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::ProjectNotFound => "ProjectNotFound",
            Self::RepositoryNotBound => "RepositoryNotBound",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for RunStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the Runs could not be read"),
            Self::Row {
                workspace_id,
                run_id,
                cause,
            } => write!(
                formatter,
                "Run {run_id} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Clock => formatter.write_str("the clock is before the Unix epoch"),
            Self::Identity {
                workspace_id,
                run_id,
            } => write!(
                formatter,
                "a Run row's Workspace is not readable: {workspace_id}, Run \
                 {run_id}"
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for RunStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Row {
                workspace_id,
                run_id,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("run_id", run_id)
                .field("cause", cause)
                .finish(),
            Self::Clock => formatter.debug_struct("Clock").finish(),
            Self::Identity {
                workspace_id,
                run_id,
            } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .field("run_id", run_id)
                .finish(),
        }
    }
}

impl std::error::Error for RunStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            Self::Clock | Self::Identity { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum CheckPreflightError {
    Account(OperatorAccountError),
    ProjectNotFound,
    RepositoryNotBound,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for CheckPreflightError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "{error}"),
            Self::ProjectNotFound => write!(formatter, "that Project is not in this Workspace"),
            Self::RepositoryNotBound => {
                write!(
                    formatter,
                    "that Project has no Repository Binding to preflight"
                )
            }
            Self::InvalidStore => write!(formatter, "the managed store is inconsistent"),
            Self::Store(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CheckPreflightError {}

impl From<OperatorAccountError> for CheckPreflightError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for CheckPreflightError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for CheckPreflightError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<ProjectRepositoryError> for CheckPreflightError {
    fn from(error: ProjectRepositoryError) -> Self {
        match error {
            ProjectRepositoryError::Account(error) => Self::Account(error),
            ProjectRepositoryError::Store(error) => Self::Store(error),
            ProjectRepositoryError::Json(error) => Self::Json(error),
            ProjectRepositoryError::ProjectNotFound => Self::ProjectNotFound,
            _ => Self::InvalidStore,
        }
    }
}

pub struct CheckPreflightService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

/// The repository and Charter policy revisions an already-open Run authorizes
/// preflight to inspect. They are also the policy revisions recorded in its
/// preflight Receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreflightPin {
    pub repository_revision: u32,
    pub charter_revision: u32,
}

impl CheckPreflightService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Open a preflight Run on a Project's current revisions, then inspect only
    /// those pinned revisions.
    ///
    /// Opens a Run and commits it before any filesystem work, so that a process
    /// that dies mid-inspection leaves a Run its lease can resolve rather than
    /// no trace at all. The verification steps touch no database. A second
    /// transaction records the Receipt, the event, and the Run's terminal state
    /// together.
    pub fn preflight(&self, project_id: Uuid) -> Result<PreflightReceipt, CheckPreflightError> {
        self.preflight_with_pin(project_id, None)
    }

    /// Preflight the exact revisions captured by an already-open execution Run.
    pub(crate) fn preflight_pinned(
        &self,
        project_id: Uuid,
        pin: PreflightPin,
    ) -> Result<PreflightReceipt, CheckPreflightError> {
        self.preflight_with_pin(project_id, Some(pin))
    }

    fn preflight_with_pin(
        &self,
        project_id: Uuid,
        pin: Option<PreflightPin>,
    ) -> Result<PreflightReceipt, CheckPreflightError> {
        let opened = self.accounts.with_authenticated_store(|connection, now| {
            let started_at_ms =
                u64::try_from(now).map_err(|_| CheckPreflightError::InvalidStore)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (charter_revision, binding) = match pin {
                Some(pin) => (
                    pin.charter_revision,
                    repository_for_preflight_at(
                        &transaction,
                        self.workspace_id,
                        project_id,
                        pin.repository_revision,
                        pin.charter_revision,
                    )?,
                ),
                None => {
                    latest_repository_for_preflight(&transaction, self.workspace_id, project_id)?
                }
            };
            let binding = binding.ok_or_else(|| {
                if pin.is_some() {
                    CheckPreflightError::InvalidStore
                } else {
                    CheckPreflightError::RepositoryNotBound
                }
            })?;
            let run_id = Uuid::new_v4();
            transaction.execute(
                "INSERT INTO runs (
                     workspace_id, id, project_id, kind, repository_revision, charter_revision,
                     state, lease_expires_at_ms, started_at_ms, ended_at_ms
                 ) VALUES (?1, ?2, ?3, 'check_preflight', ?4, ?5, 'preflighting', ?6, ?7, NULL)",
                params![
                    self.workspace_id.to_string(),
                    run_id.to_string(),
                    project_id.to_string(),
                    i64::from(binding.revision),
                    i64::from(charter_revision),
                    i64::try_from(started_at_ms.saturating_add(LEASE_MS))
                        .map_err(|_| CheckPreflightError::InvalidStore)?,
                    i64::try_from(started_at_ms).map_err(|_| CheckPreflightError::InvalidStore)?,
                ],
            )?;
            transaction.commit()?;
            Ok::<_, CheckPreflightError>(OpenRun {
                run_id,
                charter_revision,
                binding,
                started_at_ms,
            })
        })?;

        let inspection = inspect(&opened);

        self.accounts.with_authenticated_store(|connection, now| {
            let ended_at_ms = u64::try_from(now)
                .map_err(|_| CheckPreflightError::InvalidStore)?
                .max(opened.started_at_ms);
            let transaction = connection.transaction()?;
            let outcome = inspection.outcome;
            let receipt = PreflightReceipt {
                version: 1,
                run_id: opened.run_id,
                workspace_id: self.workspace_id,
                project_id,
                repository_revision: opened.binding.revision,
                charter_revision: opened.charter_revision,
                outcome,
                source_kind: opened.binding.source_kind,
                source: opened.binding.source.clone(),
                base_revision: opened.binding.base_revision.clone(),
                resolved_checks: inspection.resolved_checks.clone(),
                sandbox_version: inspection
                    .sandbox
                    .as_ref()
                    .map_or_else(|| "none".to_owned(), SandboxCapability::receipt_version),
                policy_charter_revision: opened.charter_revision,
                policy_repository_revision: opened.binding.revision,
                started_at_ms: opened.started_at_ms,
                ended_at_ms,
                not_verified: inspection.not_verified.clone(),
                unavailable: UnavailableEvidence::for_preflight(),
            };
            let payload = serde_json::to_string(&receipt)?;
            transaction.execute(
                "UPDATE runs SET state = ?3, ended_at_ms = ?4 WHERE workspace_id = ?1 AND id = ?2",
                params![
                    self.workspace_id.to_string(),
                    opened.run_id.to_string(),
                    outcome.run_state(),
                    i64::try_from(ended_at_ms).map_err(|_| CheckPreflightError::InvalidStore)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO receipts (
                     workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
                 ) VALUES (?1, ?2, 1, ?3, ?4, ?5)",
                params![
                    self.workspace_id.to_string(),
                    opened.run_id.to_string(),
                    outcome.as_str(),
                    payload,
                    i64::try_from(ended_at_ms).map_err(|_| CheckPreflightError::InvalidStore)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'run.preflighted', ?2, ?3)",
                params![
                    self.workspace_id.to_string(),
                    payload,
                    i64::try_from(ended_at_ms).map_err(|_| CheckPreflightError::InvalidStore)?,
                ],
            )?;
            transaction.commit()?;
            Ok(receipt)
        })
    }
}

/// What one Run captured when it opened.
struct OpenRun {
    run_id: Uuid,
    charter_revision: u32,
    binding: PreflightBinding,
    started_at_ms: u64,
}

/// The result of the filesystem phase. Touches no database.
struct Inspection {
    outcome: PreflightOutcome,
    resolved_checks: Vec<ResolvedCheck>,
    not_verified: Vec<String>,
    /// What the sandbox step proved, when it was reached.
    sandbox: Option<SandboxCapability>,
}

/// Perform the verification steps in order, stopping at the first that fails.
///
/// Every step this function does not reach is named in `not_verified`, so the
/// Receipt can say what it did not check rather than implying it checked
/// everything.
fn inspect(opened: &OpenRun) -> Inspection {
    let binding = &opened.binding;
    let started = Instant::now();
    let mut deadline = move || started.elapsed().as_millis() > INSPECTION_BUDGET_MS;

    let declared_checks: Vec<ResolvedCheck> = binding
        .declared_checks
        .iter()
        .map(|check| ResolvedCheck {
            name: check.name.clone(),
            argv: check.argv.clone(),
            declared_working_directory: check.working_directory.clone(),
            resolved_working_directory: Recorded::absent("the step was not reached"),
        })
        .collect();

    let refuse = |outcome: PreflightOutcome, reached: &[&str]| Inspection {
        outcome,
        resolved_checks: declared_checks.clone(),
        not_verified: reached.iter().map(|entry| (*entry).to_owned()).collect(),
        // A refusal before the sandbox step proves nothing about the sandbox.
        sandbox: None,
    };

    // Step 0: the binding applies to this Run's pinned Charter revision.
    if !binding.applies_to_charter {
        return refuse(
            PreflightOutcome::BindingStaleForCharter,
            &[
                "repository readability",
                "base revision presence",
                "working directory containment",
                "sandbox availability",
            ],
        );
    }

    // Step 1: only a local path can be inspected without a network fetch.
    if binding.source_kind != RepositorySourceKind::LocalPath {
        return refuse(
            PreflightOutcome::SourceKindUnsupported,
            &[
                "repository readability (a Git URL cannot be read without a network fetch)",
                "base revision presence",
                "working directory containment",
                "sandbox availability",
            ],
        );
    }

    // Step 2: the source is a readable Git repository.
    let source = Path::new(&binding.source);
    let store = match ObjectStore::open(source) {
        Ok(store) => store,
        Err(_) => {
            return refuse(
                PreflightOutcome::RepositoryUnreadable,
                &[
                    "base revision presence",
                    "working directory containment",
                    "sandbox availability",
                ],
            );
        }
    };
    let root = match store.canonical_root() {
        Ok(root) => root,
        Err(_) => {
            return refuse(
                PreflightOutcome::RepositoryUnreadable,
                &[
                    "base revision presence",
                    "working directory containment",
                    "sandbox availability",
                ],
            );
        }
    };

    // Step 3: the pinned base revision is present in the object store.
    match store.contains(&binding.base_revision, &mut deadline) {
        Ok(true) => {}
        Ok(false) => {
            return refuse(
                PreflightOutcome::BaseRevisionUnreachable,
                &[
                    "working directory containment",
                    "sandbox availability",
                    REACHABILITY_GAP,
                ],
            );
        }
        Err(ObjectStoreError::Elapsed) => {
            return refuse(
                PreflightOutcome::PreflightTimedOut,
                &[
                    "base revision presence (the inspection budget elapsed)",
                    "working directory containment",
                    "sandbox availability",
                ],
            );
        }
        Err(_) => {
            return refuse(
                PreflightOutcome::PreflightFailed,
                &[
                    "base revision presence (the object store could not be read)",
                    "working directory containment",
                    "sandbox availability",
                ],
            );
        }
    }

    // Step 4: every check's resolved working directory is inside the repository.
    let mut resolved_checks = Vec::with_capacity(binding.declared_checks.len());
    for check in &binding.declared_checks {
        if deadline() {
            return refuse(
                PreflightOutcome::PreflightTimedOut,
                &[
                    "working directory containment (the inspection budget elapsed)",
                    "sandbox availability",
                    REACHABILITY_GAP,
                ],
            );
        }
        match resolve_within(&root, &check.working_directory) {
            Containment::Inside(resolved) => resolved_checks.push(ResolvedCheck {
                name: check.name.clone(),
                argv: check.argv.clone(),
                declared_working_directory: check.working_directory.clone(),
                resolved_working_directory: Recorded::Present {
                    value: resolved.to_string_lossy().into_owned(),
                },
            }),
            Containment::Escapes => {
                return refuse(
                    PreflightOutcome::WorkingDirectoryEscapes,
                    &["sandbox availability", REACHABILITY_GAP],
                );
            }
            Containment::Unresolvable => {
                // A declared working directory that does not exist is an error,
                // not an escape: nothing was found to be outside anything.
                return refuse(
                    PreflightOutcome::PreflightFailed,
                    &["sandbox availability", REACHABILITY_GAP],
                );
            }
        }
    }

    // Step 5: execution requires a sandbox this host can prove it enforces.
    // `detect` builds a real ruleset and applies it in a child, so what comes
    // back is an observation rather than a configuration (ADR 0029).
    let capability = crate::sandbox::detect();
    let mut not_verified = vec![
        REACHABILITY_GAP.to_owned(),
        "check execution, exit statuses, and their effects (preflight never runs a check)"
            .to_owned(),
    ];
    if !capability.admits_execution() {
        not_verified.push(format!(
            "sandbox enforcement ({})",
            capability.receipt_version()
        ));
        return Inspection {
            outcome: PreflightOutcome::SandboxUnavailable,
            resolved_checks,
            not_verified,
            sandbox: Some(capability),
        };
    }
    // Containment is proven, so it leaves the residual-uncertainty list. What
    // the sandbox still does not bound stays on it: Landlock governs
    // filesystem access and TCP, and nothing else.
    not_verified.push(
        "CPU, memory, and wall-clock bounds (the sandbox contains, it does not meter)".to_owned(),
    );
    not_verified.push("UDP egress (Landlock restricts TCP only)".to_owned());
    Inspection {
        outcome: PreflightOutcome::Verified,
        resolved_checks,
        not_verified,
        sandbox: Some(capability),
    }
}

enum Containment {
    Inside(PathBuf),
    Escapes,
    Unresolvable,
}

/// Decide containment on the resolved path, never on the declared string.
///
/// A guard that reads the value the guarded path supplies can be defeated by
/// supplying a different one, so both sides are canonicalized before comparison.
fn resolve_within(root: &Path, declared: &str) -> Containment {
    let candidate = root.join(declared);
    match std::fs::canonicalize(&candidate) {
        Ok(resolved) => {
            if resolved.starts_with(root) {
                Containment::Inside(resolved)
            } else {
                Containment::Escapes
            }
        }
        Err(_) => {
            // The path does not exist. Ask whether its nearest existing ancestor
            // already escapes: `../../elsewhere/missing` is an escape, while
            // `missing` inside the repository is merely absent.
            let mut ancestor = candidate.as_path();
            while let Some(parent) = ancestor.parent() {
                if let Ok(resolved) = std::fs::canonicalize(parent) {
                    return if resolved.starts_with(root) {
                        Containment::Unresolvable
                    } else {
                        Containment::Escapes
                    };
                }
                ancestor = parent;
            }
            Containment::Escapes
        }
    }
}

/// Startup reconciliation: a Run whose lease expired without a terminal Receipt
/// is `unconfirmed`, never absent. Lease expiry is what proves that state.
/// Reconcile every Run against its Receipts and Events.
///
/// Returns nothing. It used to return the number of leases it expired, which no caller read, and
/// that dead `u64` was the pass's only fallible conversion -- forcing a fault arm that would have
/// had to be named for a count rather than for anything a reader could act on.
pub(crate) fn verify_run_state(connection: &Connection) -> Result<(), RunStateFault> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| RunStateFault::Clock)?
        .as_millis();
    let now = i64::try_from(now).map_err(|_| RunStateFault::Clock)?;
    // Both open states expire the same way: a lease that ran out without a terminal Receipt is
    // `unconfirmed` whatever kind of Run held it.
    connection
        .execute(
            "UPDATE runs SET state = 'unconfirmed', ended_at_ms = ?1
         WHERE state IN ('preflighting', 'dispatching') AND lease_expires_at_ms < ?1",
            params![now],
        )
        .map_err(RunStateFault::Store)?;

    // One reconciliation owns kind, state, Receipt, and terminal-event
    // cardinality so compound relabelling cannot move a Run between separate
    // validators and disappear from both.
    let mut statement = connection
        .prepare(
            // `runs.id` is selected so a refusal can NAME the Run (R28.1). Before this the loop had
            // no identity at all -- the pass could say a Run disagreed with the log and never which.
            "SELECT runs.id, runs.workspace_id, runs.kind, runs.state,
                (SELECT count(*) FROM receipts
                 WHERE receipts.workspace_id = runs.workspace_id
                   AND receipts.run_id = runs.id),
                (SELECT count(*) FROM events
                 WHERE events.workspace_id = runs.workspace_id
                   AND events.kind = 'run.preflighted'
                   AND json_extract(events.payload_json, '$.runId') = runs.id),
                (SELECT count(*) FROM events
                 WHERE events.workspace_id = runs.workspace_id
                   AND events.kind = 'run.executed'
                   AND json_extract(events.payload_json, '$.runId') = runs.id),
                (SELECT count(*) FROM events
                 WHERE events.workspace_id = runs.workspace_id
                   AND events.kind = 'proposal.dispatched'
                   AND json_extract(events.payload_json, '$.runId') = runs.id)
         FROM runs
         -- Ordered like every other pass, so a bar can plant a failing row that is NOT the row
         -- this pass reads first. Without it the enumeration order is whatever SQLite returns.
         ORDER BY runs.workspace_id, runs.id",
        )
        .map_err(RunStateFault::Store)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .map_err(RunStateFault::Store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(RunStateFault::Store)?;
    for (
        run_id,
        workspace_text,
        kind,
        state,
        receipts,
        preflight_events,
        execution_events,
        dispatch_events,
    ) in rows
    {
        let identity = || RunStateFault::Identity {
            workspace_id: workspace_text.clone(),
            run_id: run_id.clone(),
        };
        let workspace_id = Uuid::parse_str(&workspace_text).map_err(|_| identity())?;
        Uuid::parse_str(&run_id).map_err(|_| identity())?;
        let named = |cause: CheckPreflightError| RunStateFault::Row {
            workspace_id,
            run_id: run_id.clone(),
            cause,
        };
        let expected = match (kind.as_str(), state.as_str()) {
            ("check_preflight", "refused" | "verified") => (1, 1, 0, 0),
            ("declared_checks", "refused" | "verified" | "incomplete") => (1, 0, 1, 0),
            // A dispatch that reached a terminal state has exactly one Receipt and one Event, and
            // is a Run like any other: this function owns every kind's cardinality so a relabelled
            // Run cannot fall between two validators and be checked by neither.
            ("proposal_dispatch", "refused" | "applied") => (1, 0, 0, 1),
            // An unconfirmed Run is precisely one that never recorded a Receipt.
            (
                "check_preflight" | "declared_checks" | "proposal_dispatch",
                "preflighting" | "dispatching" | "unconfirmed",
            ) => (0, 0, 0, 0),
            _ => return Err(named(CheckPreflightError::InvalidStore)),
        };
        if (
            receipts,
            preflight_events,
            execution_events,
            dispatch_events,
        ) != expected
        {
            return Err(named(CheckPreflightError::InvalidStore));
        }
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) enum RunEventReplayError {
    EvidenceMismatch,
    /// The stored Run disagrees with the Receipt this event carries. It names the Run, because a
    /// replay that says only "a Run disagreed" leaves the reader the whole table to search
    /// (R28.1); the id is the one the event's own payload cites and the stored row was proved
    /// equal to above, not a fresh one.
    RunStateMismatch {
        run_id: Uuid,
    },
    Store(rusqlite::Error),
}

impl From<rusqlite::Error> for RunEventReplayError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for RunEventReplayError {
    fn from(_error: serde_json::Error) -> Self {
        Self::EvidenceMismatch
    }
}

/// Replay one `run.preflighted` event against the stored Receipt.
pub(crate) fn verify_run_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), RunEventReplayError> {
    let receipt: PreflightReceipt = serde_json::from_str(payload_json)?;
    if receipt.version != 1
        || receipt.workspace_id != workspace_id
        || receipt.started_at_ms > receipt.ended_at_ms
        || receipt.ended_at_ms != occurred_at_ms
    {
        return Err(RunEventReplayError::EvidenceMismatch);
    }
    let stored: Option<StoredPreflightMetadata> = connection
        .query_row(
            "SELECT receipts.version, receipts.outcome, receipts.receipt_json,
                    receipts.recorded_at_ms, runs.id, runs.project_id,
                    runs.repository_revision, runs.charter_revision, runs.kind, runs.state,
                    runs.started_at_ms, runs.ended_at_ms
             FROM receipts
             JOIN runs
               ON runs.workspace_id = receipts.workspace_id
              AND runs.id = receipts.run_id
             WHERE receipts.workspace_id = ?1 AND receipts.run_id = ?2",
            params![workspace_id.to_string(), receipt.run_id.to_string()],
            |row| {
                Ok(StoredPreflightMetadata {
                    version: row.get(0)?,
                    outcome: row.get(1)?,
                    receipt_json: row.get(2)?,
                    recorded_at_ms: row.get(3)?,
                    run_id: row.get(4)?,
                    project_id: row.get(5)?,
                    repository_revision: row.get(6)?,
                    charter_revision: row.get(7)?,
                    kind: row.get(8)?,
                    state: row.get(9)?,
                    started_at_ms: row.get(10)?,
                    ended_at_ms: row.get(11)?,
                })
            },
        )
        .optional()?;
    let metadata = stored.ok_or(RunEventReplayError::EvidenceMismatch)?;
    let stored_receipt: PreflightReceipt = serde_json::from_str(&metadata.receipt_json)?;
    let stored_run_id =
        Uuid::parse_str(&metadata.run_id).map_err(|_| RunEventReplayError::EvidenceMismatch)?;
    let stored_project_id =
        Uuid::parse_str(&metadata.project_id).map_err(|_| RunEventReplayError::EvidenceMismatch)?;
    if metadata.version != i64::from(receipt.version)
        || metadata.outcome != receipt.outcome.as_str()
        || metadata.receipt_json.as_bytes() != payload_json.as_bytes()
        || stored_receipt != receipt
        || stored_run_id != receipt.run_id
        || stored_project_id != receipt.project_id
        || metadata.repository_revision != i64::from(receipt.repository_revision)
        || metadata.repository_revision != i64::from(receipt.policy_repository_revision)
        || metadata.charter_revision != i64::from(receipt.charter_revision)
        || metadata.charter_revision != i64::from(receipt.policy_charter_revision)
        || u64::try_from(metadata.started_at_ms)
            .map_err(|_| RunEventReplayError::EvidenceMismatch)?
            != receipt.started_at_ms
        || u64::try_from(metadata.ended_at_ms).map_err(|_| RunEventReplayError::EvidenceMismatch)?
            != receipt.ended_at_ms
        || u64::try_from(metadata.recorded_at_ms)
            .map_err(|_| RunEventReplayError::EvidenceMismatch)?
            != receipt.ended_at_ms
    {
        return Err(RunEventReplayError::EvidenceMismatch);
    }
    if metadata.kind != "check_preflight" || metadata.state != receipt.outcome.run_state() {
        return Err(RunEventReplayError::RunStateMismatch {
            run_id: receipt.run_id,
        });
    }
    Ok(())
}

struct StoredPreflightMetadata {
    version: i64,
    outcome: String,
    receipt_json: String,
    recorded_at_ms: i64,
    run_id: String,
    project_id: String,
    repository_revision: i64,
    charter_revision: i64,
    kind: String,
    state: String,
    started_at_ms: i64,
    ended_at_ms: i64,
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()` (the fold at 30d2a20): an exhaustive `name()` catches an
    /// added arm, only a second table catches a wrong or duplicated name.
    fn expected_name(error: &CheckPreflightError) -> &'static str {
        match error {
            CheckPreflightError::Account(_) => "Account",
            CheckPreflightError::ProjectNotFound => "ProjectNotFound",
            CheckPreflightError::RepositoryNotBound => "RepositoryNotBound",
            CheckPreflightError::InvalidStore => "InvalidStore",
            CheckPreflightError::Store(_) => "Store",
            CheckPreflightError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_check_preflight_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            CheckPreflightError::Account(
                crate::operator_account::OperatorAccountError::PasswordTooShort,
            ),
            CheckPreflightError::ProjectNotFound,
            CheckPreflightError::RepositoryNotBound,
            CheckPreflightError::InvalidStore,
            CheckPreflightError::Store(rusqlite::Error::QueryReturnedNoRows),
            CheckPreflightError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            6,
            "an arm was added to `CheckPreflightError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(CheckPreflightError::name).collect();
        assert_eq!(
            names.len(),
            6,
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

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::RunEventReplayError;

    #[test]
    fn run_event_replay_preserves_sqlite_errors_as_store_failures() {
        let connection = Connection::open_in_memory().unwrap();
        let sqlite_error = connection
            .query_row("SELECT value FROM missing_table", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_err();

        let replay_error = RunEventReplayError::from(sqlite_error);

        match replay_error {
            RunEventReplayError::Store(rusqlite::Error::SqliteFailure(_, Some(message))) => {
                assert!(message.contains("no such table"));
            }
            other => panic!("SQLite error was not preserved as Store: {other:?}"),
        }
    }
}
