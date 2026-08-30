//! Running a Project's Declared Checks — the composition of ADR 0027 and 0029.
//!
//! Preflight establishes whether execution would be admissible. This runs the
//! checks it admitted, inside a disposable worktree, under the proven sandbox,
//! bounded, and emits a Receipt naming what actually happened.
//!
//! # What this module is careful about
//!
//! **Admissibility is not assumed, it is re-established.** Every execution
//! preflights first and refuses unless that preflight returns `Verified`. The
//! binding may have moved, the repository may have gone, the sandbox may have
//! stopped enforcing since the last time anyone looked.
//!
//! **A successful process exit is not a successful Mission.** `Completed` means
//! every check has a positively established terminal outcome. Whether they
//! passed is in their exit statuses, and whether the Mission succeeded is
//! judged elsewhere against its declared conditions.
//!
//! **The worktree may refuse to be removed.** That refusal is recorded rather
//! than overridden: a kept worktree is an operator-visible fact, not a cleanup
//! failure to be retried away.

use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::check_preflight::{
    CheckPreflightError, CheckPreflightService, PreflightOutcome, PreflightReceipt,
};
use crate::operator_account::{OperatorAccountError, OperatorAccountService};
use crate::sandbox::{self, ExecutionBounds, ExecutionOutcome, ExecutionRecord};
use crate::worktree::{CleanupOutcome, KeepReason, Worktree};

/// A Run's lease. Longer than a preflight's, because checks take time.
const LEASE_MS: u64 = 3_600_000;

/// How an execution ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionSummary {
    /// Preflight did not return `Verified`. Nothing was run, and the preflight
    /// Receipt embedded below says why.
    NotAdmissible,
    /// The worktree could not be created, so there was nowhere to run.
    WorktreeUnavailable,
    /// Every Declared Check ran. This says nothing about whether they passed.
    Completed,
    /// Dispatch began, but at least one Declared Check lacks a positively
    /// established terminal process outcome.
    Incomplete,
}

impl ExecutionSummary {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotAdmissible => "not_admissible",
            Self::WorktreeUnavailable => "worktree_unavailable",
            Self::Completed => "completed",
            Self::Incomplete => "incomplete",
        }
    }

    fn run_state(self) -> &'static str {
        match self {
            Self::NotAdmissible | Self::WorktreeUnavailable => "refused",
            Self::Completed => "verified",
            Self::Incomplete => "incomplete",
        }
    }
}

/// The record of one execution of a Project's Declared Checks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionReceipt {
    pub version: u32,
    pub run_id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub summary: ExecutionSummary,
    /// The exact configured bounds for each Declared Check. Records show
    /// whether dispatch reached a check; early refusal does not claim these
    /// configured bounds were exercised.
    pub per_check_bounds: ExecutionBounds,
    /// The preflight that admitted this execution, carried whole. It is the
    /// evidence of admissibility, so it travels with the thing it admitted
    /// rather than being cited by identifier and looked up later.
    pub admitted_by: PreflightReceipt,
    /// One record per Declared Check, in the canonical name order the
    /// binding stores them in (ADR 0026) rather than the order they were
    /// written. Empty only when execution was refused before dispatch.
    pub checks: Vec<ExecutionRecord>,
    /// What became of the worktree. A `Kept` outcome names its reasons.
    pub worktree: Option<CleanupOutcome>,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    /// Everything this Run did not establish, recomputed from typed execution
    /// observations rather than copied from preflight prose.
    pub not_verified: Vec<String>,
}

#[derive(Debug)]
pub enum DeclaredCheckError {
    Account(OperatorAccountError),
    Preflight(CheckPreflightError),
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for DeclaredCheckError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "{error}"),
            Self::Preflight(error) => write!(formatter, "{error}"),
            Self::InvalidStore => write!(formatter, "the managed store is inconsistent"),
            Self::Store(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for DeclaredCheckError {}

impl From<OperatorAccountError> for DeclaredCheckError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<CheckPreflightError> for DeclaredCheckError {
    fn from(error: CheckPreflightError) -> Self {
        Self::Preflight(error)
    }
}

impl From<rusqlite::Error> for DeclaredCheckError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for DeclaredCheckError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub struct DeclaredCheckService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
    preflights: CheckPreflightService,
}

impl DeclaredCheckService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            preflights: CheckPreflightService::new(Arc::clone(&accounts), workspace_id),
            accounts,
            workspace_id,
        }
    }

    /// Preflight, then run the admitted Declared Checks in a disposable
    /// worktree under the sandbox.
    pub fn execute(
        &self,
        project_id: Uuid,
        bounds: ExecutionBounds,
    ) -> Result<ExecutionReceipt, DeclaredCheckError> {
        // Admissibility is re-established, never inherited from a previous
        // answer: the binding, the repository, and the sandbox can all have
        // changed since anyone last looked.
        let admitted_by = self.preflights.preflight(project_id)?;

        let opened = self.open_run(project_id, &admitted_by)?;

        if admitted_by.outcome != PreflightOutcome::Verified {
            let not_verified = early_refusal_not_verified(&admitted_by, false);
            return self.close(
                opened,
                ExecutionSummary::NotAdmissible,
                admitted_by,
                Vec::new(),
                None,
                not_verified,
                bounds,
            );
        }

        let worktree = match Worktree::create(
            std::path::Path::new(&admitted_by.source),
            &admitted_by.base_revision,
        ) {
            Ok(worktree) => worktree,
            Err(_) => {
                let not_verified = early_refusal_not_verified(&admitted_by, true);
                return self.close(
                    opened,
                    ExecutionSummary::WorktreeUnavailable,
                    admitted_by,
                    Vec::new(),
                    None,
                    not_verified,
                    bounds,
                );
            }
        };

        let records = self.run_checks(&worktree, &admitted_by, bounds);
        // Closing consumes the worktree and may refuse. A refusal is recorded,
        // not overridden.
        let cleanup = worktree.close();
        let summary = execution_summary(&records);
        let not_verified = execution_not_verified(&admitted_by, &records, &cleanup);
        self.close(
            opened,
            summary,
            admitted_by,
            records,
            Some(cleanup),
            not_verified,
            bounds,
        )
    }

    /// Run each Declared Check in the canonical name order the binding
    /// stores, each in its own working directory beneath the worktree.
    fn run_checks(
        &self,
        worktree: &Worktree,
        admitted_by: &PreflightReceipt,
        bounds: ExecutionBounds,
    ) -> Vec<ExecutionRecord> {
        let capability = sandbox::detect();
        let launcher = sandbox::WorktreeLauncher::bind(worktree.path(), &capability);
        admitted_by
            .resolved_checks
            .iter()
            .map(|check| launcher.run(&check.argv, &check.declared_working_directory, bounds, None))
            .collect()
    }

    fn open_run(
        &self,
        project_id: Uuid,
        admitted_by: &PreflightReceipt,
    ) -> Result<OpenRun, DeclaredCheckError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let started_at_ms = u64::try_from(now).map_err(|_| DeclaredCheckError::InvalidStore)?;
            let run_id = Uuid::new_v4();
            connection.execute(
                "INSERT INTO runs (
                     workspace_id, id, project_id, kind, repository_revision, charter_revision,
                     state, lease_expires_at_ms, started_at_ms, ended_at_ms
                 ) VALUES (?1, ?2, ?3, 'declared_checks', ?4, ?5, 'preflighting', ?6, ?7, NULL)",
                params![
                    self.workspace_id.to_string(),
                    run_id.to_string(),
                    project_id.to_string(),
                    i64::from(admitted_by.repository_revision),
                    i64::from(admitted_by.charter_revision),
                    i64::try_from(started_at_ms.saturating_add(LEASE_MS))
                        .map_err(|_| DeclaredCheckError::InvalidStore)?,
                    i64::try_from(started_at_ms).map_err(|_| DeclaredCheckError::InvalidStore)?,
                ],
            )?;
            Ok(OpenRun {
                run_id,
                project_id,
                started_at_ms,
            })
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn close(
        &self,
        opened: OpenRun,
        summary: ExecutionSummary,
        admitted_by: PreflightReceipt,
        checks: Vec<ExecutionRecord>,
        worktree: Option<CleanupOutcome>,
        not_verified: Vec<String>,
        per_check_bounds: ExecutionBounds,
    ) -> Result<ExecutionReceipt, DeclaredCheckError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let ended_at_ms = u64::try_from(now)
                .map_err(|_| DeclaredCheckError::InvalidStore)?
                .max(opened.started_at_ms);
            let receipt = ExecutionReceipt {
                version: 2,
                run_id: opened.run_id,
                workspace_id: self.workspace_id,
                project_id: opened.project_id,
                summary,
                per_check_bounds,
                admitted_by: admitted_by.clone(),
                checks: checks.clone(),
                worktree: worktree.clone(),
                started_at_ms: opened.started_at_ms,
                ended_at_ms,
                not_verified: not_verified.clone(),
            };
            let payload = serde_json::to_string(&receipt)?;
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE runs SET state = ?3, ended_at_ms = ?4 WHERE workspace_id = ?1 AND id = ?2",
                params![
                    self.workspace_id.to_string(),
                    opened.run_id.to_string(),
                    summary.run_state(),
                    i64::try_from(ended_at_ms).map_err(|_| DeclaredCheckError::InvalidStore)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO receipts (
                     workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
                 ) VALUES (?1, ?2, 2, ?3, ?4, ?5)",
                params![
                    self.workspace_id.to_string(),
                    opened.run_id.to_string(),
                    summary.as_str(),
                    payload,
                    i64::try_from(ended_at_ms).map_err(|_| DeclaredCheckError::InvalidStore)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'run.executed', ?2, ?3)",
                params![
                    self.workspace_id.to_string(),
                    payload,
                    i64::try_from(ended_at_ms).map_err(|_| DeclaredCheckError::InvalidStore)?,
                ],
            )?;
            transaction.commit()?;
            Ok(receipt)
        })
    }
}

struct OpenRun {
    run_id: Uuid,
    project_id: Uuid,
    started_at_ms: u64,
}

fn execution_summary(checks: &[ExecutionRecord]) -> ExecutionSummary {
    if checks.is_empty()
        || checks.iter().any(|record| {
            matches!(
                record.outcome,
                sandbox::ExecutionOutcome::NotStarted { .. }
                    | sandbox::ExecutionOutcome::ObservationFailed { .. }
            )
        })
    {
        ExecutionSummary::Incomplete
    } else {
        ExecutionSummary::Completed
    }
}

const REACHABILITY_GAP: &str =
    "base revision reachability from a ref (object presence verified only)";
const UDP_GAP: &str = "UDP egress (Landlock restricts TCP only)";
const FILESYSTEM_GAP: &str = "complete filesystem effects and diff digest (not recorded)";
const SOURCE_SUPPORT_GAP: &str = "repository source support for local execution";
const REPOSITORY_READABILITY_GAP: &str = "repository readability";
const BASE_PRESENCE_GAP: &str = "base revision presence in the repository object store";
const CONTAINMENT_GAP: &str = "Declared Check working-directory containment";
const SANDBOX_GAP: &str = "sandbox enforcement";
const WORKTREE_NOT_ATTEMPTED_GAP: &str = "worktree creation (preflight did not admit execution)";
const PREFLIGHT_NO_CHECKS_GAP: &str =
    "every Declared Check (preflight did not admit execution, so no check ran)";
const WORKTREE_REFUSAL_GAP: &str = "worktree creation and any partial checkout effects (creation was refused before a worktree handle was available)";
const WORKTREE_NO_CHECKS_GAP: &str =
    "every Declared Check (worktree creation was refused, so no check ran)";

fn early_refusal_not_verified(
    admitted_by: &PreflightReceipt,
    worktree_creation_refused: bool,
) -> Vec<String> {
    if worktree_creation_refused {
        return vec![
            REACHABILITY_GAP.to_owned(),
            WORKTREE_REFUSAL_GAP.to_owned(),
            WORKTREE_NO_CHECKS_GAP.to_owned(),
        ];
    }

    // A non-empty complete set of resolved paths proves that source reading,
    // object presence, and containment were reached for this captured binding.
    // It does not prove sandbox enforcement, and an empty set carries no such
    // positive evidence.
    let resolved_stages = !admitted_by.resolved_checks.is_empty()
        && admitted_by.resolved_checks.iter().all(|check| {
            matches!(
                &check.resolved_working_directory,
                crate::check_preflight::Recorded::Present { .. }
            )
        });
    let mut gaps = Vec::new();
    match admitted_by.outcome {
        PreflightOutcome::BindingStaleForCharter => {
            gaps.extend([
                "Repository Binding applicability to the current Charter revision".to_owned(),
                SOURCE_SUPPORT_GAP.to_owned(),
                REPOSITORY_READABILITY_GAP.to_owned(),
                BASE_PRESENCE_GAP.to_owned(),
                REACHABILITY_GAP.to_owned(),
                CONTAINMENT_GAP.to_owned(),
                SANDBOX_GAP.to_owned(),
            ]);
        }
        PreflightOutcome::SourceKindUnsupported => {
            gaps.extend([
                SOURCE_SUPPORT_GAP.to_owned(),
                REPOSITORY_READABILITY_GAP.to_owned(),
                BASE_PRESENCE_GAP.to_owned(),
                REACHABILITY_GAP.to_owned(),
                CONTAINMENT_GAP.to_owned(),
                SANDBOX_GAP.to_owned(),
            ]);
        }
        PreflightOutcome::RepositoryUnreadable => {
            gaps.extend([
                REPOSITORY_READABILITY_GAP.to_owned(),
                BASE_PRESENCE_GAP.to_owned(),
                REACHABILITY_GAP.to_owned(),
                CONTAINMENT_GAP.to_owned(),
                SANDBOX_GAP.to_owned(),
            ]);
        }
        PreflightOutcome::BaseRevisionUnreachable => {
            gaps.extend([
                BASE_PRESENCE_GAP.to_owned(),
                REACHABILITY_GAP.to_owned(),
                CONTAINMENT_GAP.to_owned(),
                SANDBOX_GAP.to_owned(),
            ]);
        }
        PreflightOutcome::DeclaredChecksChanged => {
            gaps.push(
                "Repository Binding stability and current Declared Check definitions".to_owned(),
            );
            if !resolved_stages {
                gaps.extend([
                    SOURCE_SUPPORT_GAP.to_owned(),
                    REPOSITORY_READABILITY_GAP.to_owned(),
                    BASE_PRESENCE_GAP.to_owned(),
                ]);
            }
            gaps.push(REACHABILITY_GAP.to_owned());
            if !resolved_stages {
                gaps.push(CONTAINMENT_GAP.to_owned());
            }
            gaps.push(SANDBOX_GAP.to_owned());
        }
        PreflightOutcome::WorkingDirectoryEscapes => {
            gaps.extend([
                REACHABILITY_GAP.to_owned(),
                CONTAINMENT_GAP.to_owned(),
                SANDBOX_GAP.to_owned(),
            ]);
        }
        PreflightOutcome::SandboxUnavailable => {
            gaps.extend([REACHABILITY_GAP.to_owned(), SANDBOX_GAP.to_owned()]);
        }
        PreflightOutcome::PreflightFailed | PreflightOutcome::PreflightTimedOut => {
            if !resolved_stages {
                gaps.push(BASE_PRESENCE_GAP.to_owned());
            }
            gaps.push(REACHABILITY_GAP.to_owned());
            if !resolved_stages {
                gaps.push(CONTAINMENT_GAP.to_owned());
            }
            gaps.push(SANDBOX_GAP.to_owned());
        }
        PreflightOutcome::Verified => return early_refusal_not_verified(admitted_by, true),
    }
    gaps.push(WORKTREE_NOT_ATTEMPTED_GAP.to_owned());
    gaps.push(PREFLIGHT_NO_CHECKS_GAP.to_owned());
    gaps
}

fn execution_not_verified(
    admitted_by: &PreflightReceipt,
    checks: &[ExecutionRecord],
    cleanup: &CleanupOutcome,
) -> Vec<String> {
    let mut gaps = vec![REACHABILITY_GAP.to_owned()];
    if checks
        .iter()
        .any(|record| !matches!(record.outcome, ExecutionOutcome::NotStarted { .. }))
    {
        gaps.push(UDP_GAP.to_owned());
    }
    gaps.push(FILESYSTEM_GAP.to_owned());

    for (index, record) in checks.iter().enumerate() {
        let label = admitted_by
            .resolved_checks
            .get(index)
            .map(|check| format!("Declared Check {:?}", check.name))
            .unwrap_or_else(|| format!("Declared Check #{}", index + 1));
        match &record.outcome {
            ExecutionOutcome::Exited { .. } => {}
            ExecutionOutcome::Signalled { signal } => gaps.push(format!(
                "{label} normal completion and exit code (terminated by signal {signal})"
            )),
            ExecutionOutcome::Bounded { ceiling } => gaps.push(format!(
                "{label} completion beyond the {} ceiling",
                ceiling_slug(*ceiling)
            )),
            ExecutionOutcome::Interrupted {} => gaps.push(format!(
                "{label} completion after operator interruption"
            )),
            ExecutionOutcome::NotStarted { reason } => gaps.push(format!(
                "{label} status, intended output, and effects (not started: {reason})"
            )),
            ExecutionOutcome::ObservationFailed { reason } => gaps.push(format!(
                "{label} terminal status, complete output, effects, and possible continued activity (observation failed after start: {reason})"
            )),
        }
        if record.output_truncated {
            gaps.push(format!(
                "{label} output beyond the configured capture ceiling"
            ));
        }
        if record.output_incomplete {
            gaps.push(format!(
                "{label} complete output (collection stopped before EOF)"
            ));
        }
    }

    if let CleanupOutcome::Kept { reasons } = cleanup {
        gaps.extend(
            reasons
                .iter()
                .map(|reason| format!("worktree removal (kept: {})", keep_reason_slug(*reason))),
        );
    }
    gaps
}

fn ceiling_slug(ceiling: sandbox::Ceiling) -> &'static str {
    match ceiling {
        sandbox::Ceiling::WallClock => "wall_clock",
        sandbox::Ceiling::Cpu => "cpu",
        sandbox::Ceiling::FileSize => "file_size",
    }
}

fn keep_reason_slug(reason: KeepReason) -> &'static str {
    match reason {
        KeepReason::Dirty => "dirty",
        KeepReason::UntrackedContent => "untracked_content",
        KeepReason::NotAWorktree => "not_a_worktree",
        KeepReason::PathEscapes => "path_escapes",
        KeepReason::CheckFailed => "check_failed",
        KeepReason::GateTimedOut => "gate_timed_out",
        KeepReason::RemovalRefused => "removal_refused",
    }
}

// These are the complete historical execution wire graphs, not aliases for
// the evolving public Rust types. Receipt v1 and v2 both bind preflight
// Receipt v1 explicitly; this shared nested graph is itself frozen. Adding a
// reachable field or variant requires a new execution Receipt version.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrozenExecutionReceiptV1 {
    version: u32,
    run_id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    summary: FrozenExecutionSummaryV1,
    admitted_by: FrozenPreflightReceiptV1,
    checks: Vec<FrozenExecutionRecordV1>,
    worktree: Option<FrozenCleanupOutcome>,
    started_at_ms: u64,
    ended_at_ms: u64,
    not_verified: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrozenExecutionReceiptV2 {
    version: u32,
    run_id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    summary: FrozenExecutionSummaryV2,
    per_check_bounds: FrozenExecutionBounds,
    admitted_by: FrozenPreflightReceiptV1,
    checks: Vec<FrozenExecutionRecordV2>,
    worktree: Option<FrozenCleanupOutcome>,
    started_at_ms: u64,
    ended_at_ms: u64,
    not_verified: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrozenExecutionSummaryV1 {
    NotAdmissible,
    WorktreeUnavailable,
    Completed,
}

impl FrozenExecutionSummaryV1 {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotAdmissible => "not_admissible",
            Self::WorktreeUnavailable => "worktree_unavailable",
            Self::Completed => "completed",
        }
    }

    fn run_state(self) -> &'static str {
        match self {
            Self::NotAdmissible | Self::WorktreeUnavailable => "refused",
            Self::Completed => "verified",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrozenExecutionSummaryV2 {
    NotAdmissible,
    WorktreeUnavailable,
    Completed,
    Incomplete,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrozenPreflightReceiptV1 {
    version: u32,
    run_id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    repository_revision: u32,
    charter_revision: u32,
    outcome: FrozenPreflightOutcomeV1,
    source_kind: FrozenRepositorySourceKind,
    source: String,
    base_revision: String,
    resolved_checks: Vec<FrozenResolvedCheckV1>,
    sandbox_version: String,
    policy_charter_revision: u32,
    policy_repository_revision: u32,
    started_at_ms: u64,
    ended_at_ms: u64,
    not_verified: Vec<String>,
    unavailable: FrozenUnavailableEvidenceV1,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrozenPreflightOutcomeV1 {
    Verified,
    SourceKindUnsupported,
    BindingStaleForCharter,
    RepositoryUnreadable,
    BaseRevisionUnreachable,
    DeclaredChecksChanged,
    WorkingDirectoryEscapes,
    SandboxUnavailable,
    PreflightFailed,
    PreflightTimedOut,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrozenRepositorySourceKind {
    LocalPath,
    GitUrl,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrozenResolvedCheckV1 {
    name: String,
    argv: Vec<String>,
    declared_working_directory: String,
    resolved_working_directory: FrozenRecorded<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum FrozenRecorded<T> {
    Present { value: T },
    Absent { reason: String },
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrozenUnavailableEvidenceV1 {
    proposal_digest: FrozenRecorded<String>,
    ruling_digest: FrozenRecorded<String>,
    resulting_revision: FrozenRecorded<String>,
    filesystem_effects: FrozenRecorded<String>,
    diff_digest: FrozenRecorded<String>,
    model_invocations: FrozenRecorded<u32>,
    cost_usd_micros: FrozenRecorded<u64>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrozenExecutionBounds {
    wall_clock_ms: u64,
    cpu_seconds: u64,
    address_space_bytes: u64,
    file_size_bytes: u64,
    output_bytes: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrozenExecutionRecordV1 {
    argv: Vec<String>,
    working_directory: String,
    outcome: FrozenExecutionOutcomeV1,
    stdout: String,
    stderr: String,
    output_truncated: bool,
    output_incomplete: bool,
    elapsed_ms: u64,
    sandbox_version: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FrozenExecutionRecordV2 {
    argv: Vec<String>,
    working_directory: String,
    outcome: FrozenExecutionOutcomeV2,
    stdout: String,
    stderr: String,
    output_truncated: bool,
    output_incomplete: bool,
    elapsed_ms: u64,
    sandbox_version: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum FrozenExecutionOutcomeV1 {
    Exited { code: i32 },
    Signalled { signal: i32 },
    Bounded { ceiling: FrozenCeiling },
    Interrupted {},
    NotStarted { reason: String },
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum FrozenExecutionOutcomeV2 {
    Exited { code: i32 },
    Signalled { signal: i32 },
    Bounded { ceiling: FrozenCeiling },
    Interrupted {},
    NotStarted { reason: String },
    ObservationFailed { reason: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrozenCeiling {
    WallClock,
    Cpu,
    FileSize,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum FrozenCleanupOutcome {
    Removed {},
    Kept { reasons: Vec<FrozenKeepReason> },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrozenKeepReason {
    Dirty,
    UntrackedContent,
    NotAWorktree,
    PathEscapes,
    CheckFailed,
    GateTimedOut,
    RemovalRefused,
}

enum DecodedExecutionReceipt {
    V1(FrozenExecutionReceiptV1),
    V2(ExecutionReceipt),
}

impl DecodedExecutionReceipt {
    fn version(&self) -> u32 {
        match self {
            Self::V1(_) => 1,
            Self::V2(_) => 2,
        }
    }

    fn run_id(&self) -> Uuid {
        match self {
            Self::V1(receipt) => receipt.run_id,
            Self::V2(receipt) => receipt.run_id,
        }
    }

    fn workspace_id(&self) -> Uuid {
        match self {
            Self::V1(receipt) => receipt.workspace_id,
            Self::V2(receipt) => receipt.workspace_id,
        }
    }

    fn project_id(&self) -> Uuid {
        match self {
            Self::V1(receipt) => receipt.project_id,
            Self::V2(receipt) => receipt.project_id,
        }
    }

    fn outcome(&self) -> &'static str {
        match self {
            Self::V1(receipt) => receipt.summary.as_str(),
            Self::V2(receipt) => receipt.summary.as_str(),
        }
    }

    fn run_state(&self) -> &'static str {
        match self {
            Self::V1(receipt) => receipt.summary.run_state(),
            Self::V2(receipt) => receipt.summary.run_state(),
        }
    }

    fn ended_at_ms(&self) -> u64 {
        match self {
            Self::V1(receipt) => receipt.ended_at_ms,
            Self::V2(receipt) => receipt.ended_at_ms,
        }
    }

    fn started_at_ms(&self) -> u64 {
        match self {
            Self::V1(receipt) => receipt.started_at_ms,
            Self::V2(receipt) => receipt.started_at_ms,
        }
    }

    fn repository_revision(&self) -> u32 {
        match self {
            Self::V1(receipt) => receipt.admitted_by.repository_revision,
            Self::V2(receipt) => receipt.admitted_by.repository_revision,
        }
    }

    fn charter_revision(&self) -> u32 {
        match self {
            Self::V1(receipt) => receipt.admitted_by.charter_revision,
            Self::V2(receipt) => receipt.admitted_by.charter_revision,
        }
    }
}

fn decode_execution_receipt(
    payload_json: &str,
) -> Result<DecodedExecutionReceipt, DeclaredCheckError> {
    let value: serde_json::Value = serde_json::from_str(payload_json)?;
    let version = value
        .as_object()
        .and_then(|object| object.get("version"))
        .and_then(serde_json::Value::as_u64)
        .ok_or(DeclaredCheckError::InvalidStore)?;
    match version {
        1 => {
            let receipt: FrozenExecutionReceiptV1 = serde_json::from_str(payload_json)?;
            if receipt.version != 1
                || receipt.admitted_by.version != 1
                || receipt.workspace_id != receipt.admitted_by.workspace_id
                || receipt.project_id != receipt.admitted_by.project_id
            {
                return Err(DeclaredCheckError::InvalidStore);
            }
            Ok(DecodedExecutionReceipt::V1(receipt))
        }
        2 => {
            // Decode through the separately frozen graph first. Parsing the
            // public type afterwards cannot silently widen this boundary.
            let frozen: FrozenExecutionReceiptV2 = serde_json::from_str(payload_json)?;
            if frozen.version != 2 || frozen.admitted_by.version != 1 {
                return Err(DeclaredCheckError::InvalidStore);
            }
            let receipt: ExecutionReceipt = serde_json::from_str(payload_json)?;
            validate_v2_receipt(&receipt)?;
            Ok(DecodedExecutionReceipt::V2(receipt))
        }
        _ => Err(DeclaredCheckError::InvalidStore),
    }
}

fn validate_v2_receipt(receipt: &ExecutionReceipt) -> Result<(), DeclaredCheckError> {
    if receipt.version != 2
        || receipt.admitted_by.version != 1
        || receipt.workspace_id != receipt.admitted_by.workspace_id
        || receipt.project_id != receipt.admitted_by.project_id
    {
        return Err(DeclaredCheckError::InvalidStore);
    }
    let has_incomplete = receipt.checks.iter().any(|record| {
        matches!(
            record.outcome,
            ExecutionOutcome::NotStarted { .. } | ExecutionOutcome::ObservationFailed { .. }
        )
    });
    let dispatched = matches!(
        receipt.summary,
        ExecutionSummary::Completed | ExecutionSummary::Incomplete
    );
    if dispatched
        && (receipt.admitted_by.outcome != PreflightOutcome::Verified
            || receipt.checks.is_empty()
            || receipt.checks.len() != receipt.admitted_by.resolved_checks.len()
            || receipt.worktree.is_none())
    {
        return Err(DeclaredCheckError::InvalidStore);
    }
    if receipt
        .checks
        .iter()
        .zip(&receipt.admitted_by.resolved_checks)
        .any(|(record, check)| {
            record.argv != check.argv
                || matches!(record.outcome, ExecutionOutcome::NotStarted { .. })
                    && (!record.stdout.is_empty()
                        || !record.stderr.is_empty()
                        || record.output_truncated
                        || record.output_incomplete)
        })
    {
        return Err(DeclaredCheckError::InvalidStore);
    }
    match receipt.summary {
        ExecutionSummary::NotAdmissible
            if receipt.admitted_by.outcome != PreflightOutcome::Verified
                && receipt.checks.is_empty()
                && receipt.worktree.is_none() =>
        {
            if receipt.not_verified != early_refusal_not_verified(&receipt.admitted_by, false) {
                return Err(DeclaredCheckError::InvalidStore);
            }
        }
        ExecutionSummary::WorktreeUnavailable
            if receipt.admitted_by.outcome == PreflightOutcome::Verified
                && receipt.checks.is_empty()
                && receipt.worktree.is_none() =>
        {
            if receipt.not_verified != early_refusal_not_verified(&receipt.admitted_by, true) {
                return Err(DeclaredCheckError::InvalidStore);
            }
        }
        ExecutionSummary::Completed if !has_incomplete => {
            if receipt.not_verified
                != execution_not_verified(
                    &receipt.admitted_by,
                    &receipt.checks,
                    receipt
                        .worktree
                        .as_ref()
                        .ok_or(DeclaredCheckError::InvalidStore)?,
                )
            {
                return Err(DeclaredCheckError::InvalidStore);
            }
        }
        ExecutionSummary::Incomplete if has_incomplete => {
            if receipt.not_verified
                != execution_not_verified(
                    &receipt.admitted_by,
                    &receipt.checks,
                    receipt
                        .worktree
                        .as_ref()
                        .ok_or(DeclaredCheckError::InvalidStore)?,
                )
            {
                return Err(DeclaredCheckError::InvalidStore);
            }
        }
        _ => return Err(DeclaredCheckError::InvalidStore),
    }
    Ok(())
}

/// Replay one `run.executed` event against its stored Receipt.
pub(crate) fn verify_execution_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), DeclaredCheckError> {
    let receipt = decode_execution_receipt(payload_json)?;
    if receipt.workspace_id() != workspace_id
        || receipt.started_at_ms() > receipt.ended_at_ms()
        || receipt.ended_at_ms() != occurred_at_ms
    {
        return Err(DeclaredCheckError::InvalidStore);
    }
    let stored: Option<StoredExecutionMetadata> = connection
        .query_row(
            "SELECT receipts.version, receipts.outcome, receipts.receipt_json,
                    receipts.recorded_at_ms, runs.kind, runs.state, runs.project_id,
                    runs.repository_revision, runs.charter_revision,
                    runs.started_at_ms, runs.ended_at_ms
             FROM receipts JOIN runs
               ON runs.workspace_id = receipts.workspace_id AND runs.id = receipts.run_id
             WHERE receipts.workspace_id = ?1 AND receipts.run_id = ?2",
            params![workspace_id.to_string(), receipt.run_id().to_string()],
            |row| {
                Ok(StoredExecutionMetadata {
                    version: row.get(0)?,
                    outcome: row.get(1)?,
                    receipt_json: row.get(2)?,
                    recorded_at_ms: row.get(3)?,
                    kind: row.get(4)?,
                    state: row.get(5)?,
                    project_id: row.get(6)?,
                    repository_revision: row.get(7)?,
                    charter_revision: row.get(8)?,
                    started_at_ms: row.get(9)?,
                    ended_at_ms: row.get(10)?,
                })
            },
        )
        .optional()?;
    let stored = stored.ok_or(DeclaredCheckError::InvalidStore)?;
    let stored_receipt = decode_execution_receipt(&stored.receipt_json)?;
    let project_id =
        Uuid::parse_str(&stored.project_id).map_err(|_| DeclaredCheckError::InvalidStore)?;
    if stored.receipt_json.as_bytes() != payload_json.as_bytes()
        || stored_receipt.version() != receipt.version()
        || stored_receipt.run_id() != receipt.run_id()
        || stored.version != i64::from(receipt.version())
        || stored.outcome != receipt.outcome()
        || stored.kind != "declared_checks"
        || stored.state != receipt.run_state()
        || project_id != receipt.project_id()
        || stored.repository_revision != i64::from(receipt.repository_revision())
        || stored.charter_revision != i64::from(receipt.charter_revision())
        || u64::try_from(stored.started_at_ms).map_err(|_| DeclaredCheckError::InvalidStore)?
            != receipt.started_at_ms()
        || u64::try_from(stored.ended_at_ms).map_err(|_| DeclaredCheckError::InvalidStore)?
            != receipt.ended_at_ms()
        || u64::try_from(stored.recorded_at_ms).map_err(|_| DeclaredCheckError::InvalidStore)?
            != receipt.ended_at_ms()
    {
        return Err(DeclaredCheckError::InvalidStore);
    }
    Ok(())
}

struct StoredExecutionMetadata {
    version: i64,
    outcome: String,
    receipt_json: String,
    recorded_at_ms: i64,
    kind: String,
    state: String,
    project_id: String,
    repository_revision: i64,
    charter_revision: i64,
    started_at_ms: i64,
    ended_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::ExecutionOutcome;
    use serde_json::json;

    #[test]
    fn every_preflight_outcome_retains_each_unestablished_execution_stage() {
        let cases = [
            (
                PreflightOutcome::BindingStaleForCharter,
                vec![
                    "Repository Binding applicability to the current Charter revision",
                    "repository source support for local execution",
                    "repository readability",
                    "base revision presence in the repository object store",
                    REACHABILITY_GAP,
                    "Declared Check working-directory containment",
                    "sandbox enforcement",
                    "worktree creation (preflight did not admit execution)",
                    "every Declared Check (preflight did not admit execution, so no check ran)",
                ],
            ),
            (
                PreflightOutcome::SourceKindUnsupported,
                vec![
                    "repository source support for local execution",
                    "repository readability",
                    "base revision presence in the repository object store",
                    REACHABILITY_GAP,
                    "Declared Check working-directory containment",
                    "sandbox enforcement",
                    "worktree creation (preflight did not admit execution)",
                    "every Declared Check (preflight did not admit execution, so no check ran)",
                ],
            ),
            (
                PreflightOutcome::RepositoryUnreadable,
                vec![
                    "repository readability",
                    "base revision presence in the repository object store",
                    REACHABILITY_GAP,
                    "Declared Check working-directory containment",
                    "sandbox enforcement",
                    "worktree creation (preflight did not admit execution)",
                    "every Declared Check (preflight did not admit execution, so no check ran)",
                ],
            ),
            (
                PreflightOutcome::BaseRevisionUnreachable,
                vec![
                    "base revision presence in the repository object store",
                    REACHABILITY_GAP,
                    "Declared Check working-directory containment",
                    "sandbox enforcement",
                    "worktree creation (preflight did not admit execution)",
                    "every Declared Check (preflight did not admit execution, so no check ran)",
                ],
            ),
            (
                PreflightOutcome::DeclaredChecksChanged,
                vec![
                    "Repository Binding stability and current Declared Check definitions",
                    "repository source support for local execution",
                    "repository readability",
                    "base revision presence in the repository object store",
                    REACHABILITY_GAP,
                    "Declared Check working-directory containment",
                    "sandbox enforcement",
                    "worktree creation (preflight did not admit execution)",
                    "every Declared Check (preflight did not admit execution, so no check ran)",
                ],
            ),
            (
                PreflightOutcome::WorkingDirectoryEscapes,
                vec![
                    REACHABILITY_GAP,
                    "Declared Check working-directory containment",
                    "sandbox enforcement",
                    "worktree creation (preflight did not admit execution)",
                    "every Declared Check (preflight did not admit execution, so no check ran)",
                ],
            ),
            (
                PreflightOutcome::SandboxUnavailable,
                vec![
                    REACHABILITY_GAP,
                    "sandbox enforcement",
                    "worktree creation (preflight did not admit execution)",
                    "every Declared Check (preflight did not admit execution, so no check ran)",
                ],
            ),
            (
                PreflightOutcome::PreflightFailed,
                vec![
                    "base revision presence in the repository object store",
                    REACHABILITY_GAP,
                    "Declared Check working-directory containment",
                    "sandbox enforcement",
                    "worktree creation (preflight did not admit execution)",
                    "every Declared Check (preflight did not admit execution, so no check ran)",
                ],
            ),
            (
                PreflightOutcome::PreflightTimedOut,
                vec![
                    "base revision presence in the repository object store",
                    REACHABILITY_GAP,
                    "Declared Check working-directory containment",
                    "sandbox enforcement",
                    "worktree creation (preflight did not admit execution)",
                    "every Declared Check (preflight did not admit execution, so no check ran)",
                ],
            ),
        ];

        for (outcome, expected) in cases {
            let mut value = valid_v2_json()["admittedBy"].clone();
            value["outcome"] = serde_json::to_value(outcome).unwrap();
            value["resolvedChecks"][0]["resolvedWorkingDirectory"] = json!({
                "kind": "absent",
                "reason": "the step was not reached"
            });
            let receipt: PreflightReceipt = serde_json::from_value(value).unwrap();
            assert_eq!(
                early_refusal_not_verified(&receipt, false),
                expected,
                "wrong gaps for {outcome:?}"
            );
        }

        for outcome in [
            PreflightOutcome::DeclaredChecksChanged,
            PreflightOutcome::PreflightFailed,
            PreflightOutcome::PreflightTimedOut,
        ] {
            let mut value = valid_v2_json()["admittedBy"].clone();
            value["outcome"] = serde_json::to_value(outcome).unwrap();
            let receipt: PreflightReceipt = serde_json::from_value(value).unwrap();
            let gaps = early_refusal_not_verified(&receipt, false);
            assert!(!gaps.iter().any(|gap| gap == "repository readability"));
            assert!(
                !gaps
                    .iter()
                    .any(|gap| gap == "base revision presence in the repository object store")
            );
            assert!(
                !gaps
                    .iter()
                    .any(|gap| gap == "Declared Check working-directory containment")
            );
            assert!(gaps.iter().any(|gap| gap == "sandbox enforcement"));
        }
    }

    #[test]
    fn observation_failure_is_incomplete() {
        let records = vec![ExecutionRecord {
            argv: vec!["/bin/true".to_owned()],
            working_directory: "/tmp/worktree".to_owned(),
            outcome: ExecutionOutcome::ObservationFailed {
                reason: "synthetic post-spawn observation failure".to_owned(),
            },
            stdout: String::new(),
            stderr: String::new(),
            output_truncated: false,
            output_incomplete: false,
            elapsed_ms: 1,
            sandbox_version: "landlock-abi-test/enforcing".to_owned(),
        }];

        assert_eq!(execution_summary(&records), ExecutionSummary::Incomplete);
        let admitted_by: PreflightReceipt =
            serde_json::from_value(valid_v2_json()["admittedBy"].clone()).unwrap();
        assert_eq!(
            execution_not_verified(&admitted_by, &records, &CleanupOutcome::Removed {}),
            vec![
                REACHABILITY_GAP,
                UDP_GAP,
                FILESYSTEM_GAP,
                "Declared Check \"check\" terminal status, complete output, effects, and possible continued activity (observation failed after start: synthetic post-spawn observation failure)",
            ]
        );

        let mut interrupted = records;
        interrupted[0].outcome = ExecutionOutcome::Interrupted {};
        assert_eq!(execution_summary(&interrupted), ExecutionSummary::Completed);
        assert_eq!(
            execution_not_verified(&admitted_by, &interrupted, &CleanupOutcome::Removed {}),
            vec![
                REACHABILITY_GAP,
                UDP_GAP,
                FILESYSTEM_GAP,
                "Declared Check \"check\" completion after operator interruption",
            ]
        );

        let mut not_started = interrupted;
        not_started[0].outcome = ExecutionOutcome::NotStarted {
            reason: "the executable was absent".to_owned(),
        };
        assert_eq!(
            execution_summary(&not_started),
            ExecutionSummary::Incomplete
        );
        assert_eq!(
            execution_not_verified(&admitted_by, &not_started, &CleanupOutcome::Removed {}),
            vec![
                REACHABILITY_GAP,
                FILESYSTEM_GAP,
                "Declared Check \"check\" status, intended output, and effects (not started: the executable was absent)",
            ]
        );

        let mut encoded_observation_failure = valid_v2_json();
        encoded_observation_failure["summary"] = json!("incomplete");
        encoded_observation_failure["checks"][0]["outcome"] = json!({
            "kind": "observationFailed",
            "reason": "synthetic post-spawn observation failure"
        });
        encoded_observation_failure["notVerified"] = json!([
            REACHABILITY_GAP,
            UDP_GAP,
            FILESYSTEM_GAP,
            "Declared Check \"check\" terminal status, complete output, effects, and possible continued activity (observation failed after start: synthetic post-spawn observation failure)"
        ]);
        let decoded = decode_execution_receipt(&encoded_observation_failure.to_string()).unwrap();
        assert_eq!(decoded.outcome(), "incomplete");
        assert_eq!(decoded.run_state(), "incomplete");

        assert_eq!(
            early_refusal_not_verified(&admitted_by, true),
            vec![
                REACHABILITY_GAP,
                WORKTREE_REFUSAL_GAP,
                WORKTREE_NO_CHECKS_GAP,
            ]
        );
    }

    #[test]
    fn versioned_decoders_are_recursively_closed() {
        let receipt_v2 = valid_v2_json();
        assert!(decode_execution_receipt(&receipt_v2.to_string()).is_ok());
        let mut worktree_unavailable = receipt_v2.clone();
        worktree_unavailable["summary"] = json!("worktree_unavailable");
        worktree_unavailable["checks"] = json!([]);
        worktree_unavailable["worktree"] = serde_json::Value::Null;
        worktree_unavailable["notVerified"] = json!([
            REACHABILITY_GAP,
            WORKTREE_REFUSAL_GAP,
            WORKTREE_NO_CHECKS_GAP
        ]);
        assert!(decode_execution_receipt(&worktree_unavailable.to_string()).is_ok());
        let mut v2_without_bounds = receipt_v2.clone();
        v2_without_bounds
            .as_object_mut()
            .unwrap()
            .remove("perCheckBounds");
        assert!(decode_execution_receipt(&v2_without_bounds.to_string()).is_err());

        let mut historical_v1 = receipt_v2.clone();
        historical_v1["version"] = json!(1);
        historical_v1
            .as_object_mut()
            .unwrap()
            .remove("perCheckBounds");
        historical_v1["checks"][0]["outcome"] = json!({
            "kind": "notStarted",
            "reason": "the Run could not be awaited: historical error"
        });
        assert!(decode_execution_receipt(&historical_v1.to_string()).is_ok());
        let mut v1_incomplete = historical_v1.clone();
        v1_incomplete["summary"] = json!("incomplete");
        assert!(decode_execution_receipt(&v1_incomplete.to_string()).is_err());
        let mut v1_future_summary = historical_v1.clone();
        v1_future_summary["summary"] = json!("future_summary");
        assert!(decode_execution_receipt(&v1_future_summary.to_string()).is_err());
        let mut v1_with_guessed_bounds = historical_v1.clone();
        v1_with_guessed_bounds["perCheckBounds"] = receipt_v2["perCheckBounds"].clone();
        assert!(decode_execution_receipt(&v1_with_guessed_bounds.to_string()).is_err());

        for receipt in [&receipt_v2, &historical_v1] {
            let mut unknown_preflight = receipt.clone();
            unknown_preflight["admittedBy"]["outcome"] = json!("future_outcome");
            assert!(decode_execution_receipt(&unknown_preflight.to_string()).is_err());

            let mut unknown_cleanup = receipt.clone();
            unknown_cleanup["worktree"] = json!({"kind": "futureCleanup"});
            assert!(decode_execution_receipt(&unknown_cleanup.to_string()).is_err());

            let mut unknown_keep_reason = receipt.clone();
            unknown_keep_reason["worktree"] = json!({"kind": "kept", "reasons": ["future_reason"]});
            assert!(decode_execution_receipt(&unknown_keep_reason.to_string()).is_err());
        }

        historical_v1["checks"][0]["outcome"] = json!({
            "kind": "observationFailed",
            "reason": "post-spawn observation failed"
        });
        assert!(decode_execution_receipt(&historical_v1.to_string()).is_err());

        let mut unknown_summary = receipt_v2.clone();
        unknown_summary["summary"] = json!("future_summary");
        assert!(decode_execution_receipt(&unknown_summary.to_string()).is_err());

        let mut unknown_outcome = receipt_v2;
        unknown_outcome["checks"][0]["outcome"] = json!({"kind": "futureOutcome"});
        assert!(decode_execution_receipt(&unknown_outcome.to_string()).is_err());
    }

    #[test]
    fn every_cleanup_keep_reason_retains_its_exact_ordered_gap() {
        let admitted_by: PreflightReceipt =
            serde_json::from_value(valid_v2_json()["admittedBy"].clone()).unwrap();
        let record: ExecutionRecord =
            serde_json::from_value(valid_v2_json()["checks"][0].clone()).unwrap();
        let cleanup = CleanupOutcome::Kept {
            reasons: vec![
                KeepReason::Dirty,
                KeepReason::UntrackedContent,
                KeepReason::NotAWorktree,
                KeepReason::PathEscapes,
                KeepReason::CheckFailed,
                KeepReason::GateTimedOut,
                KeepReason::RemovalRefused,
            ],
        };
        assert_eq!(
            execution_not_verified(&admitted_by, &[record], &cleanup),
            vec![
                REACHABILITY_GAP,
                UDP_GAP,
                FILESYSTEM_GAP,
                "worktree removal (kept: dirty)",
                "worktree removal (kept: untracked_content)",
                "worktree removal (kept: not_a_worktree)",
                "worktree removal (kept: path_escapes)",
                "worktree removal (kept: check_failed)",
                "worktree removal (kept: gate_timed_out)",
                "worktree removal (kept: removal_refused)",
            ]
        );
    }

    #[test]
    fn dispatched_v2_receipts_require_verified_preflight_evidence() {
        let mut completed = valid_v2_json();
        completed["admittedBy"]["outcome"] = json!("base_revision_unreachable");
        assert!(decode_execution_receipt(&completed.to_string()).is_err());

        let mut incomplete = completed;
        incomplete["summary"] = json!("incomplete");
        incomplete["checks"][0]["outcome"] = json!({
            "kind": "notStarted",
            "reason": "the executable was absent"
        });
        incomplete["notVerified"] = json!([
            REACHABILITY_GAP,
            FILESYSTEM_GAP,
            "Declared Check \"check\" status, intended output, and effects (not started: the executable was absent)"
        ]);
        assert!(decode_execution_receipt(&incomplete.to_string()).is_err());
    }

    fn valid_v2_json() -> serde_json::Value {
        let id = Uuid::nil().to_string();
        json!({
            "version": 2,
            "runId": id,
            "workspaceId": Uuid::nil().to_string(),
            "projectId": Uuid::nil().to_string(),
            "summary": "completed",
            "perCheckBounds": {
                "wallClockMs": 10,
                "cpuSeconds": 1,
                "addressSpaceBytes": 1024,
                "fileSizeBytes": 1024,
                "outputBytes": 128
            },
            "admittedBy": {
                "version": 1,
                "runId": Uuid::nil().to_string(),
                "workspaceId": Uuid::nil().to_string(),
                "projectId": Uuid::nil().to_string(),
                "repositoryRevision": 1,
                "charterRevision": 1,
                "outcome": "verified",
                "sourceKind": "local_path",
                "source": "/tmp/repository",
                "baseRevision": "0".repeat(40),
                "resolvedChecks": [{
                    "name": "check",
                    "argv": ["/bin/true"],
                    "declaredWorkingDirectory": ".",
                    "resolvedWorkingDirectory": {"kind": "present", "value": "/tmp/repository"}
                }],
                "sandboxVersion": "landlock-abi-test/enforcing",
                "policyCharterRevision": 1,
                "policyRepositoryRevision": 1,
                "startedAtMs": 1,
                "endedAtMs": 2,
                "notVerified": [],
                "unavailable": {
                    "proposalDigest": {"kind": "absent", "reason": "none"},
                    "rulingDigest": {"kind": "absent", "reason": "none"},
                    "resultingRevision": {"kind": "absent", "reason": "none"},
                    "filesystemEffects": {"kind": "absent", "reason": "none"},
                    "diffDigest": {"kind": "absent", "reason": "none"},
                    "modelInvocations": {"kind": "absent", "reason": "none"},
                    "costUsdMicros": {"kind": "absent", "reason": "none"}
                }
            },
            "checks": [{
                "argv": ["/bin/true"],
                "workingDirectory": "/tmp/worktree",
                "outcome": {"kind": "exited", "code": 0},
                "stdout": "",
                "stderr": "",
                "outputTruncated": false,
                "outputIncomplete": false,
                "elapsedMs": 1,
                "sandboxVersion": "landlock-abi-test/enforcing"
            }],
            "worktree": {"kind": "removed"},
            "startedAtMs": 1,
            "endedAtMs": 2,
            "notVerified": [
                "base revision reachability from a ref (object presence verified only)",
                "UDP egress (Landlock restricts TCP only)",
                "complete filesystem effects and diff digest (not recorded)"
            ]
        })
    }
}
