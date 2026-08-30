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
//! the checks ran. Whether they passed is in their exit statuses, and whether
//! the Mission succeeded is judged elsewhere against its declared conditions.
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
use crate::sandbox::{self, ExecutionBounds, ExecutionRecord};
use crate::worktree::{CleanupOutcome, Worktree};

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
}

impl ExecutionSummary {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotAdmissible => "not_admissible",
            Self::WorktreeUnavailable => "worktree_unavailable",
            Self::Completed => "completed",
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
    /// The preflight that admitted this execution, carried whole. It is the
    /// evidence of admissibility, so it travels with the thing it admitted
    /// rather than being cited by identifier and looked up later.
    pub admitted_by: PreflightReceipt,
    /// One record per Declared Check, in the canonical name order the
    /// binding stores them in (ADR 0026) rather than the order they were
    /// written. Empty unless the summary is `Completed`.
    pub checks: Vec<ExecutionRecord>,
    /// What became of the worktree. A `Kept` outcome names its reasons.
    pub worktree: Option<CleanupOutcome>,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    /// Everything this Run did not establish. Inherited from the preflight and
    /// added to, never shortened.
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
            let mut not_verified = admitted_by.not_verified.clone();
            not_verified
                .push("every Declared Check (preflight did not admit execution)".to_owned());
            return self.close(
                opened,
                ExecutionSummary::NotAdmissible,
                admitted_by,
                Vec::new(),
                None,
                not_verified,
            );
        }

        let worktree = match Worktree::create(
            std::path::Path::new(&admitted_by.source),
            &admitted_by.base_revision,
        ) {
            Ok(worktree) => worktree,
            Err(error) => {
                let mut not_verified = admitted_by.not_verified.clone();
                not_verified.push(format!(
                    "every Declared Check (no worktree to run in: {error})"
                ));
                return self.close(
                    opened,
                    ExecutionSummary::WorktreeUnavailable,
                    admitted_by,
                    Vec::new(),
                    None,
                    not_verified,
                );
            }
        };

        let records = self.run_checks(&worktree, &admitted_by, bounds);
        // Closing consumes the worktree and may refuse. A refusal is recorded,
        // not overridden.
        let cleanup = worktree.close();
        let mut not_verified = admitted_by.not_verified.clone();
        if let CleanupOutcome::Kept { reasons } = &cleanup {
            not_verified.push(format!(
                "worktree removal (kept: {} reason(s) the cleanup gate would not override)",
                reasons.len()
            ));
        }
        self.close(
            opened,
            ExecutionSummary::Completed,
            admitted_by,
            records,
            Some(cleanup),
            not_verified,
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
        admitted_by
            .resolved_checks
            .iter()
            .map(|check| {
                // The declared directory is re-resolved against *this*
                // worktree. The preflight resolved it against the source
                // repository, and that path is not this one.
                let cwd = worktree.path().join(&check.declared_working_directory);
                sandbox::run_in(
                    &check.argv,
                    &cwd,
                    worktree.path(),
                    bounds,
                    &capability,
                    None,
                )
            })
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
    ) -> Result<ExecutionReceipt, DeclaredCheckError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let ended_at_ms = u64::try_from(now)
                .map_err(|_| DeclaredCheckError::InvalidStore)?
                .max(opened.started_at_ms);
            let receipt = ExecutionReceipt {
                version: 1,
                run_id: opened.run_id,
                workspace_id: self.workspace_id,
                project_id: opened.project_id,
                summary,
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
                    // A Run that ran its checks is `verified`; one that never
                    // got there is `refused`. Neither claims the checks passed.
                    if summary == ExecutionSummary::Completed {
                        "verified"
                    } else {
                        "refused"
                    },
                    i64::try_from(ended_at_ms).map_err(|_| DeclaredCheckError::InvalidStore)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO receipts (
                     workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
                 ) VALUES (?1, ?2, 1, ?3, ?4, ?5)",
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

/// Replay one `run.executed` event against its stored Receipt.
pub(crate) fn verify_execution_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), DeclaredCheckError> {
    let receipt: ExecutionReceipt = serde_json::from_str(payload_json)?;
    if receipt.version != 1 || receipt.ended_at_ms != occurred_at_ms {
        return Err(DeclaredCheckError::InvalidStore);
    }
    let stored: Option<String> = connection
        .query_row(
            "SELECT receipt_json FROM receipts WHERE workspace_id = ?1 AND run_id = ?2",
            params![workspace_id.to_string(), receipt.run_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    let stored: ExecutionReceipt =
        serde_json::from_str(&stored.ok_or(DeclaredCheckError::InvalidStore)?)?;
    if stored != receipt {
        return Err(DeclaredCheckError::InvalidStore);
    }
    Ok(())
}
