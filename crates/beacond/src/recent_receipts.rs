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
//! **No payload body crosses this Query.** The body is nevertheless read and
//! decoded by its owning module before the bounded projection becomes a value.
//! That owner read is the I9 gate for unknown tags, versions and fields, and it
//! proves the SQL outcome projection agrees with the typed Receipt.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::check_preflight::{PreflightOutcome, decode_preflight_receipt};
use crate::declared_checks::{ExecutionSummary, decode_execution_receipt_projection};
use crate::operator_account::{OperatorAccountError, OperatorAccountService};

/// The largest Run window one reading may walk. A request above it is clamped
/// and the applied value is returned, because a silent narrowing is a bounded
/// answer wearing a complete one.
pub const MAX_RECENT_RUN_WINDOW: u32 = 200;

/// What Briefing asks for when it does not care to choose.
pub const DEFAULT_RECENT_RUN_WINDOW: u32 = 50;

/// Which Run this was, and what it concluded.
///
/// Closed PER KIND. SQL deliberately carries no shared outcome-membership
/// vocabulary; the Preflight and Execution owners decode their complete Receipt
/// first, then this projection carries the already-verified typed outcome.
/// `deny_unknown_fields` is on this type as well as on the row: without it Rust
/// ignores the other arm's field while the TypeScript adapter refuses it, which
/// is two halves of one wire disagreeing about what "closed" means.
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
/// Owner decoding failures become `InvalidStore`: malformed or unknown persisted
/// bytes are a typed store refusal, not an implementation detail exposed to the
/// Briefing caller as a serde error.
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
            receipt_json: row.get::<_, Option<String>>(10)?,
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
        match stored?.classify(workspace_id)? {
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

const RECENT_RUNS_SQL: &str = "SELECT runs.id, runs.project_id, projects.name, runs.kind,
                runs.state, runs.started_at_ms, runs.ended_at_ms,
                receipts.version, receipts.outcome, receipts.recorded_at_ms,
                receipts.receipt_json
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
    receipt_json: Option<String>,
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
    fn classify(self, workspace_id: Uuid) -> Result<Classified, RecentReceiptError> {
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
        let (version, recorded_at_ms, ended_at_ms, receipt_json) = match (
            self.version,
            self.recorded_at_ms,
            self.ended_at_ms,
            self.receipt_json.as_deref(),
        ) {
            (Some(version), Some(recorded), Some(ended), Some(receipt_json)) => {
                (version, recorded, ended, receipt_json)
            }
            // A Receipt with no recorded time, or attached to a Run that
            // never ended, is not a Receipt this reader can have written.
            _ => return Err(RecentReceiptError::InvalidStore),
        };
        let run_id = parse_uuid(&self.run_id)?;
        let project_id = parse_uuid(&self.project_id)?;
        let started_at_ms = nonnegative(self.started_at_ms)?;
        let ended_at_ms = nonnegative(ended_at_ms)?;
        let recorded_at_ms = nonnegative(recorded_at_ms)?;
        let outcome = match self.kind.as_str() {
            "check_preflight" => {
                let receipt = decode_preflight_receipt(receipt_json)
                    .map_err(|_| RecentReceiptError::InvalidStore)?;
                if version != i64::from(receipt.version)
                    || outcome != receipt.outcome.as_str()
                    || receipt.run_id != run_id
                    || receipt.workspace_id != workspace_id
                    || receipt.project_id != project_id
                    || receipt.started_at_ms != started_at_ms
                    || receipt.ended_at_ms != ended_at_ms
                    || receipt.ended_at_ms != recorded_at_ms
                {
                    return Err(RecentReceiptError::InvalidStore);
                }
                RecentReceiptOutcome::CheckPreflight {
                    outcome: receipt.outcome,
                }
            }
            "declared_checks" => {
                let receipt = decode_execution_receipt_projection(receipt_json)
                    .map_err(|_| RecentReceiptError::InvalidStore)?;
                if version != i64::from(receipt.version)
                    || outcome != receipt.summary.as_str()
                    || receipt.run_id != run_id
                    || receipt.workspace_id != workspace_id
                    || receipt.project_id != project_id
                    || receipt.started_at_ms != started_at_ms
                    || receipt.ended_at_ms != ended_at_ms
                    || receipt.ended_at_ms != recorded_at_ms
                {
                    return Err(RecentReceiptError::InvalidStore);
                }
                RecentReceiptOutcome::DeclaredChecks {
                    summary: receipt.summary,
                }
            }
            _ => return Err(RecentReceiptError::InvalidStore),
        };
        if self.state != terminal_state_for(outcome) {
            return Err(RecentReceiptError::InvalidStore);
        }
        Ok(Classified::Receipt(Box::new(RecentReceipt {
            run_id,
            project_id,
            project_name: self.project_name,
            version: u32::try_from(version).map_err(|_| RecentReceiptError::InvalidStore)?,
            outcome,
            started_at_ms,
            ended_at_ms,
            recorded_at_ms,
        })))
    }
}

/// The Run state each outcome terminates in, READ from the two producers.
///
/// This restated their tables in a third match until gpt1's review. The enum
/// match was exhaustive, so a new variant would have been compiler-visible --
/// but a REMAPPED existing variant would not, and the reader would have gone
/// on validating against a mapping its producers had abandoned. One owner per
/// mapping; this asks them.
fn terminal_state_for(outcome: RecentReceiptOutcome) -> &'static str {
    match outcome {
        RecentReceiptOutcome::CheckPreflight { outcome } => outcome.run_state(),
        RecentReceiptOutcome::DeclaredChecks { summary } => summary.run_state(),
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

    /// R48: the owner cannot decode bytes the Query never reads. This assertion
    /// names the production projection so removing the typed-deserialization
    /// gate is visible before an unknown stored token reaches Briefing.
    #[test]
    fn the_production_statement_selects_the_payload_for_owner_verification() {
        assert!(
            RECENT_RUNS_SQL.contains("receipts.receipt_json"),
            "the typed owner lost its Receipt bytes:\n{RECENT_RUNS_SQL}"
        );
        // The whole projection, named: a new column has to be added here on
        // purpose; the payload column exists only for the owner gate and never
        // crosses the public `RecentReceipt` interface.
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
                "receipts.receipt_json",
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
