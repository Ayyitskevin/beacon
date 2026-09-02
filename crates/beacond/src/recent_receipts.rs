//! Recent Receipts for one Workspace, across every Run kind.
//!
//! Briefing needs "recent receipts" (BLUEPRINT:171). The Receipt Queries that
//! existed before this module were Project-scoped, so the desktop fanned out
//! over `projects` and chose which Projects to open, in what order, how many
//! rows to hold, and how to merge them. AGENTS.md:173 is explicit that a UI app
//! does not join module tables, rebuild domain objects, or encode domain
//! policy; that fan-out did all three. This module owns the join, the order,
//! the bound, and the per-kind vocabulary, so the interface can render.
//!
//! Two properties are worth stating before the code, because both were review
//! findings on the desktop version of this reading:
//!
//! **The window is over RUNS, not Receipts.** `LIMIT` is applied to Runs and the
//! rows are partitioned afterwards, so a request for 50 can return fewer than 50
//! Receipts while older Receipts exist, and `more_runs_exist` can be true because
//! an older Run without a Receipt exists. Every field below is named for that.
//! The alternative -- limit a Receipt-driven stream -- cannot bound its scan when
//! a run of receiptless Runs sits at the head of the order.
//!
//! **No payload body crosses this Query, and none is parsed.** An execution
//! Receipt embeds the whole preflight Receipt that admitted it, so returning
//! Receipts would put a Receipt inside a Receipt inside a list and make the cost
//! scale with payload size rather than row count. `receipt_json` is not in the
//! projection at all.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::check_preflight::PreflightOutcome;
use crate::declared_checks::ExecutionSummary;
use crate::operator_account::{OperatorAccountError, OperatorAccountService};

/// The largest Run window one reading may walk. A request above it is clamped
/// and the applied value is returned, because a silent narrowing is a bounded
/// answer wearing a complete one.
pub const MAX_RECENT_RUN_WINDOW: u32 = 200;

/// What Briefing asks for when it does not care to choose.
pub const DEFAULT_RECENT_RUN_WINDOW: u32 = 50;

/// Which Run this was, and what it concluded.
///
/// Closed PER KIND. The `receipts.outcome` column is one 14-value CHECK shared
/// by both kinds, so a flat wire enum would let a `check_preflight` row carry
/// `completed` and a `declared_checks` row carry `preflight_failed` -- a widened
/// vocabulary wearing a closed one. `deny_unknown_fields` is on this type as
/// well as on the row: without it Rust ignores the other arm's field while the
/// TypeScript adapter refuses it, which is two halves of one wire disagreeing
/// about what "closed" means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecentReceiptOutcome {
    CheckPreflight { outcome: PreflightOutcome },
    DeclaredChecks { summary: ExecutionSummary },
}

/// One Receipt, as Briefing needs it: enough to name what happened and find the
/// Run it happened to, and nothing that grows with the payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecentReceipt {
    pub run_id: Uuid,
    pub project_id: Uuid,
    /// The Project's current name. The daemon joins it because this is exactly
    /// the join AGENTS.md:173 keeps out of the interface.
    pub project_name: String,
    /// The stored Receipt version. Data, not a variant -- but validated data:
    /// preflight admits v1 only, execution admits v1 (historical) or v2.
    pub version: u32,
    pub outcome: RecentReceiptOutcome,
    /// The RUN's clock, which is also the sort key. A row whose displayed time
    /// is not the time it was ordered by is a lie waiting to be found.
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    /// When the Receipt itself was written.
    pub recorded_at_ms: u64,
}

/// One bounded reading: the Receipts among the most recent Runs, and an honest
/// account of what else was in that window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecentReceiptPage {
    /// Receipts of every kind, newest first by `(started_at_ms, run id)`.
    pub receipts: Vec<RecentReceipt>,
    /// Runs INSIDE THIS WINDOW still holding a live lease. A Run count, not a
    /// Receipt count.
    pub runs_in_flight_in_window: u32,
    /// Runs INSIDE THIS WINDOW whose lease expired without a Receipt.
    ///
    /// This is NOT the Resume Work review set: an unconfirmed Run older than
    /// the window does not appear here, and BLUEPRINT:385's review before
    /// Resume Work needs every unresolved Run. That surface must query for
    /// them; reading this one would give a bounded subset an exhaustive
    /// meaning it does not have.
    pub runs_unconfirmed_in_window: u32,
    /// How many Runs the daemon walked, after clamping.
    pub run_window_applied: u32,
    /// Whether older RUNS exist beyond the window. It does not claim another
    /// Receipt exists, because an older receiptless Run also sets it.
    pub more_runs_exist: bool,
}

/// Why a reading could not be produced.
///
/// There is no `Json` arm, and that absence is the point: this Query parses no
/// Receipt body, so it has no way to fail at one.
#[derive(Debug)]
pub enum RecentReceiptError {
    Account(OperatorAccountError),
    /// The store contradicts the Receipt contract -- a kind paired with the
    /// other kind's outcome, a version that kind never writes, a Receipt on a
    /// nonterminal Run, or a terminal Run without one.
    InvalidStore,
    Store(rusqlite::Error),
}

impl std::fmt::Display for RecentReceiptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "{error}"),
            Self::InvalidStore => write!(formatter, "the managed store is inconsistent"),
            Self::Store(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for RecentReceiptError {}

impl From<OperatorAccountError> for RecentReceiptError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for RecentReceiptError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

/// Reads recent Receipts for one Workspace. Owns the join, the order, and the
/// bound, so no caller has to.
pub struct RecentReceiptService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl RecentReceiptService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// One indexed walk over the most recent Runs, newest first.
    ///
    /// Driven from `runs` with a LEFT JOIN rather than from `receipts`, because
    /// filtering for "has a Receipt" and then applying LIMIT is a DIFFERENT
    /// WINDOW: the receiptless counts would then be drawn from the whole table
    /// rather than from the rows the Receipts came from.
    pub fn recent(&self, requested_window: u32) -> Result<RecentReceiptPage, RecentReceiptError> {
        let run_window_applied = requested_window.clamp(1, MAX_RECENT_RUN_WINDOW);
        self.accounts.with_authenticated_store(|connection, _now| {
            read_recent(connection, self.workspace_id, run_window_applied)
        })
    }
}

fn read_recent(
    connection: &Connection,
    workspace_id: Uuid,
    run_window_applied: u32,
) -> Result<RecentReceiptPage, RecentReceiptError> {
    // One row past the window, so the overflow is observed rather than inferred
    // -- the same reason `read_bounded` reads one byte past its budget.
    let probe = i64::from(run_window_applied) + 1;
    let mut statement = connection.prepare(RECENT_RUNS_SQL)?;
    let rows = statement.query_map(params![workspace_id.to_string(), probe], |row| {
        Ok(StoredRun {
            run_id: row.get::<_, String>(0)?,
            project_id: row.get::<_, String>(1)?,
            project_name: row.get::<_, String>(2)?,
            kind: row.get::<_, String>(3)?,
            state: row.get::<_, String>(4)?,
            started_at_ms: row.get::<_, i64>(5)?,
            ended_at_ms: row.get::<_, Option<i64>>(6)?,
            version: row.get::<_, Option<i64>>(7)?,
            outcome: row.get::<_, Option<String>>(8)?,
            recorded_at_ms: row.get::<_, Option<i64>>(9)?,
        })
    })?;

    // The iterator is STEPPED at most `probe` times and each row is classified
    // as it arrives. Collecting first and trimming afterwards would read the
    // whole Workspace into memory whenever the SQL bound was absent or wrong --
    // the same defect, one layer down, that made the desktop version of this
    // reading hold 4,096 rows to draw five. With the bound here, the `LIMIT` in
    // the statement is an optimisation that lets SQLite stop early, not the
    // thing that makes the read bounded.
    let mut receipts = Vec::new();
    let mut runs_in_flight_in_window = 0u32;
    let mut runs_unconfirmed_in_window = 0u32;
    let mut stepped = 0usize;
    let mut more_runs_exist = false;
    for stored in rows.take(probe as usize) {
        stepped += 1;
        if stepped > run_window_applied as usize {
            // The extra row exists only to answer "is there more?".
            more_runs_exist = true;
            break;
        }
        match stored?.classify()? {
            Classified::Receipt(receipt) => receipts.push(*receipt),
            Classified::InFlight => runs_in_flight_in_window += 1,
            Classified::Unconfirmed => runs_unconfirmed_in_window += 1,
        }
    }

    Ok(RecentReceiptPage {
        receipts,
        runs_in_flight_in_window,
        runs_unconfirmed_in_window,
        run_window_applied,
        more_runs_exist,
    })
}

/// `receipt_json` is deliberately absent from this projection.
const RECENT_RUNS_SQL: &str = "SELECT runs.id, runs.project_id, projects.name, runs.kind,
                runs.state, runs.started_at_ms, runs.ended_at_ms,
                receipts.version, receipts.outcome, receipts.recorded_at_ms
         FROM runs
         JOIN projects
           ON projects.workspace_id = runs.workspace_id AND projects.id = runs.project_id
         LEFT JOIN receipts
           ON receipts.workspace_id = runs.workspace_id AND receipts.run_id = runs.id
         WHERE runs.workspace_id = ?1
         ORDER BY runs.started_at_ms DESC, runs.id DESC
         LIMIT ?2";

struct StoredRun {
    run_id: String,
    project_id: String,
    project_name: String,
    kind: String,
    state: String,
    started_at_ms: i64,
    ended_at_ms: Option<i64>,
    version: Option<i64>,
    outcome: Option<String>,
    recorded_at_ms: Option<i64>,
}

enum Classified {
    Receipt(Box<RecentReceipt>),
    InFlight,
    Unconfirmed,
}

impl StoredRun {
    /// Validates the whole stored tuple before it becomes a row.
    ///
    /// Checking `(kind, outcome)` alone leaves the contract half-enforced: ADR
    /// 0031 makes the state pairing part of the Receipt contract, not startup
    /// trivia. `runs.state` is deliberately not on the wire, so the adapter
    /// cannot check it -- which is why it is checked here, where the field is.
    fn classify(self) -> Result<Classified, RecentReceiptError> {
        let Some(outcome) = self.outcome.as_deref() else {
            // No Receipt. Both writers insert the Run's terminal state and its
            // Receipt in one transaction, and only lease reconciliation sets
            // `unconfirmed`, so any other state here is an inconsistent store.
            //
            // The END TIME is part of that tuple, not decoration. A
            // `preflighting` Run is holding its lease and has not finished; one
            // carrying an end time says both, and the table's CHECK only
            // requires `ended >= started`, so the store admits it. Lease
            // reconciliation sets `unconfirmed` and `ended_at_ms` in one
            // statement, so an unconfirmed Run without an end is the same
            // contradiction from the other side. Either is refused rather than
            // counted: a contradiction must not become a presence.
            return match (self.state.as_str(), self.ended_at_ms) {
                ("preflighting", None) => Ok(Classified::InFlight),
                ("unconfirmed", Some(_)) => Ok(Classified::Unconfirmed),
                _ => Err(RecentReceiptError::InvalidStore),
            };
        };
        let (version, recorded_at_ms, ended_at_ms) =
            match (self.version, self.recorded_at_ms, self.ended_at_ms) {
                (Some(version), Some(recorded), Some(ended)) => (version, recorded, ended),
                // A Receipt with no recorded time, or attached to a Run that
                // never ended, is not a Receipt this reader can have written.
                _ => return Err(RecentReceiptError::InvalidStore),
            };
        let outcome = match self.kind.as_str() {
            "check_preflight" => {
                if version != 1 {
                    return Err(RecentReceiptError::InvalidStore);
                }
                RecentReceiptOutcome::CheckPreflight {
                    outcome: preflight_outcome(outcome)?,
                }
            }
            "declared_checks" => {
                // v1 is historical and still decodable; v2 is current.
                if version != 1 && version != 2 {
                    return Err(RecentReceiptError::InvalidStore);
                }
                RecentReceiptOutcome::DeclaredChecks {
                    summary: execution_summary(outcome)?,
                }
            }
            _ => return Err(RecentReceiptError::InvalidStore),
        };
        if self.state != terminal_state_for(outcome) {
            return Err(RecentReceiptError::InvalidStore);
        }
        Ok(Classified::Receipt(Box::new(RecentReceipt {
            run_id: parse_uuid(&self.run_id)?,
            project_id: parse_uuid(&self.project_id)?,
            project_name: self.project_name,
            version: u32::try_from(version).map_err(|_| RecentReceiptError::InvalidStore)?,
            outcome,
            started_at_ms: nonnegative(self.started_at_ms)?,
            ended_at_ms: nonnegative(ended_at_ms)?,
            recorded_at_ms: nonnegative(recorded_at_ms)?,
        })))
    }
}

/// The Run state each outcome terminates in, read off the two producers rather
/// than restated: `check_preflight.rs`'s `PreflightOutcome::run_state` and
/// `declared_checks.rs`'s `ExecutionSummary::run_state`.
fn terminal_state_for(outcome: RecentReceiptOutcome) -> &'static str {
    match outcome {
        RecentReceiptOutcome::CheckPreflight {
            outcome: PreflightOutcome::Verified,
        } => "verified",
        RecentReceiptOutcome::CheckPreflight { .. } => "refused",
        RecentReceiptOutcome::DeclaredChecks {
            summary: ExecutionSummary::Completed,
        } => "verified",
        RecentReceiptOutcome::DeclaredChecks {
            summary: ExecutionSummary::Incomplete,
        } => "incomplete",
        RecentReceiptOutcome::DeclaredChecks { .. } => "refused",
    }
}

fn preflight_outcome(value: &str) -> Result<PreflightOutcome, RecentReceiptError> {
    match value {
        "verified" => Ok(PreflightOutcome::Verified),
        "source_kind_unsupported" => Ok(PreflightOutcome::SourceKindUnsupported),
        "binding_stale_for_charter" => Ok(PreflightOutcome::BindingStaleForCharter),
        "repository_unreadable" => Ok(PreflightOutcome::RepositoryUnreadable),
        "base_revision_unreachable" => Ok(PreflightOutcome::BaseRevisionUnreachable),
        "declared_checks_changed" => Ok(PreflightOutcome::DeclaredChecksChanged),
        "working_directory_escapes" => Ok(PreflightOutcome::WorkingDirectoryEscapes),
        "sandbox_unavailable" => Ok(PreflightOutcome::SandboxUnavailable),
        "preflight_failed" => Ok(PreflightOutcome::PreflightFailed),
        "preflight_timed_out" => Ok(PreflightOutcome::PreflightTimedOut),
        _ => Err(RecentReceiptError::InvalidStore),
    }
}

fn execution_summary(value: &str) -> Result<ExecutionSummary, RecentReceiptError> {
    match value {
        "not_admissible" => Ok(ExecutionSummary::NotAdmissible),
        "worktree_unavailable" => Ok(ExecutionSummary::WorktreeUnavailable),
        "completed" => Ok(ExecutionSummary::Completed),
        "incomplete" => Ok(ExecutionSummary::Incomplete),
        _ => Err(RecentReceiptError::InvalidStore),
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, RecentReceiptError> {
    Uuid::parse_str(value).map_err(|_| RecentReceiptError::InvalidStore)
}

fn nonnegative(value: i64) -> Result<u64, RecentReceiptError> {
    u64::try_from(value).map_err(|_| RecentReceiptError::InvalidStore)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// A store at the current schema, in memory, so the plan below is measured
    /// against the indexes migrations actually create.
    fn migrated() -> Connection {
        let connection = Connection::open_in_memory().expect("an in-memory store");
        crate::apply_every_migration(&connection).expect("the migration ladder applies");
        connection
    }

    /// gpt1's P1: the previous guards read a serialized `RecentReceiptPage` and
    /// a COPY of the SQL kept in the test file. Adding `receipts.receipt_json`
    /// to the production projection left all 14 tests green, because neither
    /// assertion could see the production statement. Both now read the constant
    /// itself, so the mutant has nowhere to hide.
    #[test]
    fn the_production_statement_selects_no_receipt_payload() {
        assert!(
            !RECENT_RUNS_SQL.contains("receipt_json"),
            "the projection grew a payload column:\n{RECENT_RUNS_SQL}"
        );
        // The whole projection, named: a new column has to be added here on
        // purpose, and a payload column cannot arrive as a silent widening.
        let selected = RECENT_RUNS_SQL
            .split_once("SELECT ")
            .and_then(|(_, rest)| rest.split_once("FROM "))
            .map(|(columns, _)| {
                columns
                    .split(',')
                    .map(|column| column.split_whitespace().collect::<Vec<_>>().join(" "))
                    .collect::<Vec<_>>()
            })
            .expect("the statement is a SELECT ... FROM");
        assert_eq!(
            selected,
            vec![
                "runs.id",
                "runs.project_id",
                "projects.name",
                "runs.kind",
                "runs.state",
                "runs.started_at_ms",
                "runs.ended_at_ms",
                "receipts.version",
                "receipts.outcome",
                "receipts.recorded_at_ms",
            ]
        );
    }

    /// The wrong answer no row assertion can see: the same rows, in the same
    /// order, produced by sorting every Run in the Workspace. Without
    /// `runs_workspace_started` SQLite plans `USE TEMP B-TREE FOR ORDER BY` and
    /// the cost grows with the Workspace rather than with the window.
    ///
    /// Measured against `RECENT_RUNS_SQL` itself, not a copy of it.
    #[test]
    fn the_production_statement_walks_the_workspace_index_and_sorts_nothing() {
        let connection = migrated();
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {RECENT_RUNS_SQL}"))
            .expect("the production statement prepares");
        let plan = statement
            .query_map(params!["00000000-0000-4000-8000-000000000000", 51], |row| {
                row.get::<_, String>(3)
            })
            .expect("the plan is readable")
            .collect::<Result<Vec<_>, _>>()
            .expect("every plan row reads")
            .join("\n");

        assert!(
            plan.contains("runs_workspace_started"),
            "the ordering index is not used:\n{plan}"
        );
        assert!(
            !plan.contains("TEMP B-TREE"),
            "the plan sorts the Workspace before the window:\n{plan}"
        );
    }
}
