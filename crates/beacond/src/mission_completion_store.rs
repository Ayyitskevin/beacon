//! The store, the Event, and the verified reader for `MissionCompletionV1` (migration 0027).
//!
//! The record's grammar is `mission_completion`; this module is what makes one durable and what
//! proves a stored one is what it says it is. Nothing here confers authority: recording a completion
//! opens no Gate, publishes no Outcome, and answers no question with a boolean. **Publishing an
//! Outcome never completes a Mission** (R4 item 1) -- completion is this record's Event, and the
//! Outcome cites it.
//!
//! # What "verified" means on a read
//!
//! Every read re-proves the record in full, in this order, before any field of it is returned:
//!
//! 1. the stored digest is SHA-256 over the stored bytes -- identity is a computation, never a
//!    column;
//! 2. the bytes decode strictly under `beacon.mission_completion.v1`, and re-encode to the same
//!    bytes;
//! 3. every indexed column repeats what the bytes say -- a row that disagrees with itself is
//!    refused;
//! 4. the Workspace is the caller's. A cross-Workspace digest is a **refusal**, never an empty
//!    result: "not yours" and "not there" are different answers;
//! 5. **exactly once**: the condition ordinals stored for this completion are exactly
//!    `{0..n-1}` for the Mission's declared count, and the record's own results agree;
//! 6. `declared_conditions_sha256` equals the digest recomputed over the Mission's stored
//!    conditions -- so a completion about a tampered Mission is refused even when its ordinal set
//!    is complete;
//! 7. the boundary is at or after the Mission's admission Event, and strictly before this
//!    completion's own Event.
//!
//! 5 and 6 are separate on purpose, and the mutation gate asserts the separation: a Mission whose
//! condition text was rewritten has a complete ordinal set, and a completion with a gap has a
//! matching Mission. Neither scenario fires the other's check.
//!
//! Everything above is a fact about the **record**, re-proved now. The record's *claims* -- that a
//! condition was met, that an Artifact was delivered -- are facts as of `boundary_event_seq`, and
//! this reader does not re-verify the referents. A consumer that needs one verified reads it
//! through its owning module and says so.

use std::fmt;

use std::collections::BTreeSet;
use std::sync::Arc;

use data_encoding::HEXLOWER;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mission::MissionError;
use crate::mission_completion::{
    CompletionActor, CompletionSupersedes, ConditionOutcome, MissionCompletionV1,
};
use crate::operator_account::{OperatorAccountError, OperatorAccountService};
use crate::proposal::sha256;

/// The indexed columns of one stored completion, named rather than spelled as a tuple: the query's
/// column order and the code that reads it cannot drift apart when each field has a name.
struct StoredCompletion {
    workspace_id: String,
    canonical_bytes: Vec<u8>,
    completion_id: String,
    mission_id: String,
    project_id: String,
    accountable_resident_id: String,
    accountable_configuration_id: String,
    boundary_event_seq: i64,
    disposition: String,
    completed_at_ms: i64,
}

/// Why a stored completion is not a completion.
/// Which fault the completion startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
///
/// This is the pass with the richest reader in tier 1 -- seventeen refusals, most of them about
/// one completion's own bytes -- so it is the one where carrying the cause buys the most.
pub(crate) enum CompletionStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One completion did not re-prove. `cause` is the owning reader's OWN refusal, carried whole.
    Row {
        workspace_id: Uuid,
        digest_value: String,
        cause: CompletionReadRefusal,
    },
    /// One `mission.completed` Event per stored completion: a count across the store, with no row
    /// to name.
    Totals(crate::TotalsMismatch),
    /// A row whose Workspace column is not a UUID. Unreachable while the foreign-key check and the
    /// Workspace pass run first.
    Identity {
        workspace_id: String,
        digest_value: String,
    },
}

impl CompletionReadRefusal {
    /// The variant's NAME, for R28.4's one-line startup message. Several of these carry column
    /// names, ordinals and reader sentences; none of that belongs in a refusal line.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Absent => "Absent",
            Self::CrossWorkspace => "CrossWorkspace",
            Self::BytesDisagree => "BytesDisagree",
            Self::ColumnDisagrees(..) => "ColumnDisagrees",
            Self::SupersessionAbsent => "SupersessionAbsent",
            Self::SupersessionUnverified(..) => "SupersessionUnverified",
            Self::SupersessionOtherMission => "SupersessionOtherMission",
            Self::SupersessionCycle => "SupersessionCycle",
            Self::MissionProjectDisagrees(..) => "MissionProjectDisagrees",
            Self::ConditionProjectionDisagrees { .. } => "ConditionProjectionDisagrees",
            Self::ConditionsNotExactlyOnce { .. } => "ConditionsNotExactlyOnce",
            Self::DeclaredConditionsMoved => "DeclaredConditionsMoved",
            Self::MissionNotAdmitted => "MissionNotAdmitted",
            Self::MissionAdmissionUnreadable { .. } => "MissionAdmissionUnreadable",
            Self::BoundaryOutOfRange => "BoundaryOutOfRange",
            Self::EventCount(..) => "EventCount",
            Self::Store(..) => "Store",
        }
    }
}

impl fmt::Display for CompletionStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried refusal's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the Mission completions could not be read"),
            Self::Row {
                workspace_id,
                digest_value,
                cause,
            } => write!(
                formatter,
                "completion {digest_value} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Totals(totals) => write!(
                formatter,
                "the Mission completions do not agree with the log as a whole -- {totals}"
            ),
            Self::Identity {
                workspace_id,
                digest_value,
            } => write!(
                formatter,
                "a completion row's Workspace is not readable: {workspace_id}, completion \
                 {digest_value}"
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for CompletionStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Totals(totals) => formatter.debug_tuple("Totals").field(totals).finish(),
            Self::Row {
                workspace_id,
                digest_value,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("digest_value", digest_value)
                .field("cause", cause)
                .finish(),
            Self::Identity {
                workspace_id,
                digest_value,
            } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .field("digest_value", digest_value)
                .finish(),
        }
    }
}

impl std::error::Error for CompletionStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            // A `TotalsMismatch` is DATA, not a nested error: it has no source of its own.
            Self::Totals(_) => None,
            Self::Identity { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionReadRefusal {
    /// No completion with that digest in this Workspace.
    Absent,
    /// The digest exists, in another Workspace. Not `Absent`: "not yours" and "not there" are
    /// different answers, and collapsing them tells a caller the wrong thing about their own store.
    CrossWorkspace,
    /// The stored bytes do not hash to the stored digest, do not decode, or do not re-encode
    /// identically.
    BytesDisagree,
    /// An indexed column says something the bytes do not.
    ColumnDisagrees(&'static str),
    /// No completion in this Workspace carries the predecessor digest this record supersedes.
    ///
    /// Distinct from `SupersessionUnverified`: nothing is wrong with a record, there is no record.
    SupersessionAbsent,
    /// The predecessor exists in this Workspace and does not verify. Its owning reader's reason is
    /// repeated, never reinterpreted.
    SupersessionUnverified(String),
    /// The predecessor is a completion of a different Mission. `CompletionSupersedes::Record` means
    /// an earlier completion of the SAME Mission, and nothing checked it.
    SupersessionOtherMission,
    /// The supersession chain returns to a record already on the path. A chain that never ends is
    /// refused rather than walked.
    SupersessionCycle,
    /// The Project this completion names does not own the Mission it names (sol1's #5).
    ///
    /// Two distinct ways to be wrong, one refusal with a reason: the Project is not in this
    /// Workspace at all, or it is, and the Mission belongs to a different one.
    MissionProjectDisagrees(&'static str),
    /// A stored condition row says something its completion's canonical bytes do not.
    ///
    /// Named by the ordinal and the column, because the parent row's `ColumnDisagrees` names its
    /// column and a child row needs one more coordinate to be findable.
    ConditionProjectionDisagrees {
        ordinal: u32,
        column: &'static str,
    },
    /// The condition ordinals are not exactly the Mission's declared set.
    ConditionsNotExactlyOnce {
        declared: usize,
        stored: usize,
    },
    /// The Mission's conditions are not the ones this completion was written against.
    DeclaredConditionsMoved,
    /// The Mission this completion names has no `mission.admitted` Event in this Workspace.
    ///
    /// A corrupt-store state, named (R21.4): the boundary check has nothing to measure against.
    /// Before this arm existed the state surfaced as a driver error a caller had to parse.
    MissionNotAdmitted,
    /// The Mission's admission Event is present and its OWNER refuses it: the Mission module's
    /// verifier -- the one startup runs -- finds the Event's bytes disagree with the stored
    /// Mission (sol1's seams read, P0). Distinct from `MissionNotAdmitted`: absent, unreadable
    /// and verified are three states, and the second used to be reported as the third, because
    /// the boundary query trusted `kind` and a projected id instead of the Event's bytes.
    MissionAdmissionUnreadable {
        sequence: i64,
    },
    /// The boundary is before the Mission's admission, or at/after this completion's own Event.
    BoundaryOutOfRange,
    /// Exactly one `mission.completed` Event must name this completion.
    EventCount(i64),
    Store(String),
}

impl std::fmt::Display for CompletionReadRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absent => formatter.write_str("no such completion in this workspace"),
            Self::CrossWorkspace => formatter.write_str("completion belongs to another workspace"),
            Self::BytesDisagree => formatter.write_str("stored bytes disagree with their digest"),
            Self::ColumnDisagrees(column) => {
                write!(formatter, "column {column} disagrees with the bytes")
            }
            Self::SupersessionAbsent => {
                formatter.write_str("no completion in this workspace carries the superseded digest")
            }
            Self::SupersessionUnverified(reason) => {
                write!(formatter, "the superseded completion: {reason}")
            }
            Self::SupersessionOtherMission => {
                formatter.write_str("the superseded completion is another mission's")
            }
            Self::SupersessionCycle => {
                formatter.write_str("the supersession chain returns to a record already on it")
            }
            Self::MissionProjectDisagrees(reason) => write!(
                formatter,
                "the completion's Project does not own its Mission: {reason}"
            ),
            Self::ConditionProjectionDisagrees { ordinal, column } => write!(
                formatter,
                "the stored condition row for ordinal {ordinal} disagrees with the record's bytes \
                 about {column}"
            ),
            Self::ConditionsNotExactlyOnce { declared, stored } => write!(
                formatter,
                "{stored} condition results for {declared} declared conditions"
            ),
            Self::DeclaredConditionsMoved => {
                formatter.write_str("the mission's declared conditions are not the ones recorded")
            }
            Self::MissionNotAdmitted => {
                formatter.write_str("the mission has no admission event in this workspace")
            }
            Self::MissionAdmissionUnreadable { sequence } => write!(
                formatter,
                "the mission's admission event at sequence {sequence} does not verify against the \
                 stored mission"
            ),
            Self::BoundaryOutOfRange => formatter.write_str("boundary event sequence out of range"),
            Self::EventCount(count) => write!(formatter, "{count} completion events, expected 1"),
            Self::Store(reason) => write!(formatter, "store: {reason}"),
        }
    }
}

impl std::error::Error for CompletionReadRefusal {}

fn store<E: std::fmt::Display>(error: E) -> CompletionReadRefusal {
    CompletionReadRefusal::Store(error.to_string())
}

/// The `mission.completed` Event payload: the record's identity and its exact canonical bytes.
///
/// The bytes travel as padded base64 so the Event and the row can be compared byte-for-byte at
/// startup without the Event ledger becoming a second encoding of the record.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MissionCompletedPayload {
    pub(crate) workspace_id: Uuid,
    pub(crate) mission_id: Uuid,
    pub(crate) completion_id: Uuid,
    pub(crate) digest_value: String,
    pub(crate) canonical_bytes_base64: String,
}

/// The digest over a Mission's declared conditions, in the framing the record's field 11 names.
///
/// `u32` count, then each condition's UTF-8 as a `u64` length and its bytes, in ascending ordinal.
/// Published in the record's own table so a second implementation can reproduce it.
pub fn declared_conditions_digest(conditions: &[String]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(
        &u32::try_from(conditions.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    for condition in conditions {
        preimage.extend_from_slice(&(condition.len() as u64).to_be_bytes());
        preimage.extend_from_slice(condition.as_bytes());
    }
    sha256(&preimage)
}

/// The Mission's declared conditions, from the table migration 0024 gives them.
fn declared_conditions(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Vec<String>, CompletionReadRefusal> {
    let mut statement = connection
        .prepare(
            "SELECT text FROM mission_success_conditions
             WHERE workspace_id = ?1 AND mission_id = ?2 ORDER BY ordinal",
        )
        .map_err(store)?;
    let rows = statement
        .query_map(
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .map_err(store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(store)?;
    Ok(rows)
}

/// Read one completion by its digest, proving every claim in the module docs.
///
/// Crate-internal in this slice: the first caller is the Outcome collector, and there is no
/// transport, no Command and no Agent path to it (`record` is not built here either -- a completion
/// is written by the same transaction that writes its Event, and that Command is the next slice's).
pub fn verified_completion_within(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<MissionCompletionV1, CompletionReadRefusal> {
    verified_completion_on_path(connection, workspace_id, digest_value, &mut BTreeSet::new())
}

/// The reader proper, carrying the supersession path walked so far.
///
/// A completion may name a predecessor, and proving this record means proving that one too --
/// which is a walk, and a walk needs an end. `path` holds every digest already on it, so a chain
/// that returns to itself is refused rather than followed (sol1's #10).
fn verified_completion_on_path(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
    path: &mut BTreeSet<String>,
) -> Result<MissionCompletionV1, CompletionReadRefusal> {
    if !path.insert(digest_value.to_owned()) {
        return Err(CompletionReadRefusal::SupersessionCycle);
    }
    // 1. The row, scoped to the caller's Workspace. A digest that exists elsewhere is a REFUSAL --
    //    the whole point of scoping being structural rather than a filter that returns nothing.
    let row: Option<StoredCompletion> = connection
        .query_row(
            "SELECT workspace_id, canonical_bytes, completion_id, mission_id, project_id,
                    accountable_resident_id, accountable_configuration_id,
                    boundary_event_seq, disposition, completed_at_ms
             FROM mission_completions WHERE digest_value = ?1",
            params![digest_value],
            |row| {
                Ok(StoredCompletion {
                    workspace_id: row.get(0)?,
                    canonical_bytes: row.get(1)?,
                    completion_id: row.get(2)?,
                    mission_id: row.get(3)?,
                    project_id: row.get(4)?,
                    accountable_resident_id: row.get(5)?,
                    accountable_configuration_id: row.get(6)?,
                    boundary_event_seq: row.get(7)?,
                    disposition: row.get(8)?,
                    completed_at_ms: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(store)?;
    let Some(stored) = row else {
        return Err(CompletionReadRefusal::Absent);
    };
    if stored.workspace_id != workspace_id.to_string() {
        return Err(CompletionReadRefusal::CrossWorkspace);
    }

    // 2. Identity is a computation over the stored bytes, never the column it sits beside.
    if HEXLOWER.encode(&sha256(&stored.canonical_bytes)) != digest_value {
        return Err(CompletionReadRefusal::BytesDisagree);
    }
    let record = MissionCompletionV1::from_canonical_bytes(&stored.canonical_bytes)
        .map_err(|_| CompletionReadRefusal::BytesDisagree)?;
    if record.canonical_bytes().as_deref() != Ok(stored.canonical_bytes.as_slice()) {
        return Err(CompletionReadRefusal::BytesDisagree);
    }

    // 3. Every indexed column repeats what the bytes say. Each names itself, because "a column
    //    disagrees" and "which column" are different amounts of help.
    // `column` shadows nothing: the loop variable is named `observed` so the struct stays
    // reachable inside it.
    for (column, observed, from_bytes) in [
        (
            "workspace_id",
            &stored.workspace_id,
            record.workspace_id.to_string(),
        ),
        (
            "completion_id",
            &stored.completion_id,
            record.completion_id.to_string(),
        ),
        (
            "mission_id",
            &stored.mission_id,
            record.mission_id.to_string(),
        ),
        (
            "project_id",
            &stored.project_id,
            record.project_id.to_string(),
        ),
        (
            "accountable_resident_id",
            &stored.accountable_resident_id,
            record.accountable_resident_id.to_string(),
        ),
        (
            "accountable_configuration_id",
            &stored.accountable_configuration_id,
            record.accountable_resident_configuration_id.to_string(),
        ),
        (
            "disposition",
            &stored.disposition,
            disposition_token(&record).to_owned(),
        ),
    ] {
        if observed != &from_bytes {
            return Err(CompletionReadRefusal::ColumnDisagrees(column));
        }
    }
    if u64::try_from(stored.boundary_event_seq) != Ok(record.boundary_event_seq) {
        return Err(CompletionReadRefusal::ColumnDisagrees("boundary_event_seq"));
    }
    if u64::try_from(stored.completed_at_ms) != Ok(record.completed_at_ms) {
        return Err(CompletionReadRefusal::ColumnDisagrees("completed_at_ms"));
    }

    // 3a. The Mission's admission Event, in three states, BEFORE the Project proof below: #71's
    //     `verified_mission_project` re-proves the same Event and folds "no Event" and "Event
    //     disagrees" into one `MissionProjectDisagrees`, so if it ran first the two names R21.4
    //     and sol1's P0 gave these states would be unreachable. Here the Event's own state is
    //     answered first, by name, and 3b is left with the question it exists for.
    // Three states, three answers. ABSENT: no row at all -- `min` over no rows was a NULL read
    // into an `i64`, the driver error R21.4 names, and is refused by name. UNREADABLE: a row is
    // there and the Mission owner's verifier -- the one startup runs -- refuses its bytes; until
    // this check the query trusted `kind` and a projected id, so a corrupted admission Event
    // still positioned a boundary while startup refused the whole store (sol1's seams read, P0).
    // VERIFIED: the Event's bytes agree with the stored Mission, and only then is its sequence a
    // fact this reader may measure against.
    let admission: Option<(i64, i64, String)> = connection
        .query_row(
            "SELECT sequence, occurred_at_ms, payload_json FROM events
             WHERE workspace_id = ?1 AND kind = 'mission.admitted'
               AND json_extract(payload_json, '$.mission.id') = ?2
             ORDER BY sequence LIMIT 1",
            params![workspace_id.to_string(), record.mission_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(store)?;
    let Some((admitted, occurred_at_ms, payload_json)) = admission else {
        return Err(CompletionReadRefusal::MissionNotAdmitted);
    };
    let readable = u64::try_from(occurred_at_ms).is_ok_and(|occurred_at_ms| {
        crate::mission::verify_mission_event(
            connection,
            workspace_id,
            occurred_at_ms,
            &payload_json,
        )
        .is_ok()
    });
    if !readable {
        return Err(CompletionReadRefusal::MissionAdmissionUnreadable { sequence: admitted });
    }
    // 3b. The Project named in the bytes actually owns the Mission named in the bytes.
    //
    //     Migration 0026's comment says `project_id` is compared across the bytes, the column and
    //     the Mission row; only the first two were ever compared, and the schema has no composite
    //     key binding a Mission to a Project. sol1 recorded a completion whose Mission belongs to
    //     Project A while its canonical `project_id` names unrelated Project B, and both the write
    //     and the read-back accepted it.
    //
    //     Both owners are proved through their own verified readers, never joined: `missions` and
    //     `projects` are projections like any other, and the whole point of this reader is that it
    //     does not answer from one.
    let owning_project =
        crate::mission::verified_mission_project(connection, workspace_id, record.mission_id)
            .map_err(|_| {
                CompletionReadRefusal::MissionProjectDisagrees(
                    "the Mission does not verify against its own admission Event",
                )
            })?
            .ok_or(CompletionReadRefusal::MissionProjectDisagrees(
                "this Workspace admitted no such Mission",
            ))?;
    if !crate::project::verified_project_exists(connection, workspace_id, record.project_id)
        .map_err(|_| {
            CompletionReadRefusal::MissionProjectDisagrees(
                "the Project does not verify against its own creation Event",
            )
        })?
    {
        return Err(CompletionReadRefusal::MissionProjectDisagrees(
            "this Workspace holds no such Project",
        ));
    }
    if owning_project != record.project_id {
        return Err(CompletionReadRefusal::MissionProjectDisagrees(
            "the Mission's verified record names a different Project",
        ));
    }

    // 4. Exactly once, against the Mission's own declaration -- and then every row read, not
    //    counted. The store's primary key forbids a duplicate ordinal and its foreign key forbids
    //    one past the end; the count catches a missing or extra row. None of the three can see a
    //    row whose VALUE was changed, and `count(*)` was all this reader looked at: sol1 changed
    //    ordinal 0 from `met` to `not_met` and the reader returned `Met`, because the canonical
    //    bytes still said so and nothing compared the two. Every projection in the child table is
    //    now compared with the record, the same rule the parent row's columns already hold.
    let declared = declared_conditions(connection, workspace_id, record.mission_id)?;
    // Scoped by the parent digest ALONE, deliberately. Filtering by Workspace as well would hide a
    // foreign-Workspace child row instead of naming it: it would simply not come back, and the
    // count below would report a missing row rather than the row that is actually wrong.
    let mut statement = connection
        .prepare(
            "SELECT workspace_id, mission_id, condition_ordinal, outcome
             FROM mission_completion_conditions
             WHERE completion_digest = ?1
             ORDER BY condition_ordinal",
        )
        .map_err(store)?;
    let stored_conditions: Vec<(String, String, i64, String)> = statement
        .query_map(params![digest_value], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(store)?;
    drop(statement);
    let stored_count = stored_conditions.len();
    if stored_count != declared.len() || record.condition_results.len() != declared.len() {
        return Err(CompletionReadRefusal::ConditionsNotExactlyOnce {
            declared: declared.len(),
            stored: stored_count,
        });
    }
    // The record's results are dense from zero and ascending -- its own decoder refuses anything
    // else -- and the rows come back in ordinal order, so the two line up positionally. Each pair
    // is compared field by field, and a disagreement names the row and the column: "a condition row
    // disagrees" and "which one, about what" are different amounts of help.
    for ((stored_workspace, stored_mission, stored_ordinal, stored_outcome), result) in
        stored_conditions
            .iter()
            .zip(record.condition_results.iter())
    {
        if stored_workspace != &record.workspace_id.to_string() {
            return Err(CompletionReadRefusal::ConditionProjectionDisagrees {
                ordinal: result.ordinal,
                column: "workspace_id",
            });
        }
        // MEASURED UNREACHABLE TODAY, and kept deliberately. Deleting this comparison leaves the
        // whole `--test mission` suite green, because three schema facts already force the ordinal
        // set: the child primary key forbids a duplicate ordinal, its foreign key into
        // `mission_success_conditions` forbids one past the declared end, and the count above
        // proves there are exactly as many rows as declared conditions. Between them the stored
        // ordinals can only be `0..declared`, in the order this query returns them, so the
        // positional pairing cannot be misaligned while the counts agree.
        //
        // It stays because it is the one of the three that does not depend on the schema keeping
        // those constraints, and because a reader that pairs two lists positionally should say out
        // loud what it assumes about the pairing. Any change to the child key or that foreign key
        // makes this reachable, and then it needs a test. It is reported as unwitnessed rather than
        // counted as covered.
        // The child's own `mission_id`. Its foreign key points at a Mission CONDITION, not at this
        // completion's Mission, so a row can name a different Mission's ordinal and satisfy every
        // key in the schema -- sol1 retargeted the one row to a real second Mission and the reader
        // returned the first Mission's completion. The Mission compared against is the one in the
        // completion's canonical bytes, never the column beside them.
        if stored_mission != &record.mission_id.to_string() {
            return Err(CompletionReadRefusal::ConditionProjectionDisagrees {
                ordinal: result.ordinal,
                column: "mission_id",
            });
        }
        if u32::try_from(*stored_ordinal) != Ok(result.ordinal) {
            return Err(CompletionReadRefusal::ConditionProjectionDisagrees {
                ordinal: result.ordinal,
                column: "condition_ordinal",
            });
        }
        if stored_outcome != outcome_token(result.outcome) {
            return Err(CompletionReadRefusal::ConditionProjectionDisagrees {
                ordinal: result.ordinal,
                column: "outcome",
            });
        }
    }

    // 5. ...and about the SAME conditions. A complete ordinal set over rewritten text is exactly the
    //    scenario the count cannot see, which is why this is a second check and not a second reason.
    if declared_conditions_digest(&declared) != record.declared_conditions_sha256 {
        return Err(CompletionReadRefusal::DeclaredConditionsMoved);
    }

    // 6. The boundary: at or after the Mission's admission, and strictly before this completion's
    //    own Event. Two invariants, two scenarios -- a backdated boundary, and a record that claims
    //    a cut at or after the moment it was itself written.
    //    `admitted` is the sequence of the admission Event step 3a proved.
    let completed: Option<i64> = connection
        .query_row(
            "SELECT sequence FROM events
             WHERE workspace_id = ?1 AND kind = 'mission.completed'
               AND json_extract(payload_json, '$.digestValue') = ?2",
            params![workspace_id.to_string(), digest_value],
            |row| row.get(0),
        )
        .optional()
        .map_err(store)?;
    let Some(completed) = completed else {
        return Err(CompletionReadRefusal::EventCount(0));
    };
    if stored.boundary_event_seq < admitted || stored.boundary_event_seq >= completed {
        return Err(CompletionReadRefusal::BoundaryOutOfRange);
    }

    // 8. The predecessor, if this record names one, is a completion of the SAME Mission in THIS
    //    Workspace -- which is what `CompletionSupersedes::Record` means and what nothing checked.
    //    `record_completion` wrote the digest into the canonical bytes and never resolved it, so
    //    sol1 put another store's completion digest in a local record and both the write and the
    //    read-back accepted it.
    //
    //    NOTE, and it is the interesting part: migration 0026's
    //    `mission_completions_one_per_mission` index permits one completion per Mission per
    //    Workspace, so a same-Mission predecessor cannot coexist with its successor TODAY. This
    //    check therefore refuses every `Record` predecessor there can currently be -- which is
    //    correct, because there can currently be no valid one. Supporting a real ladder is a
    //    migration, and sol1's scope note says so.
    if let CompletionSupersedes::Record(predecessor) = record.supersedes {
        let predecessor = HEXLOWER.encode(&predecessor);
        let earlier = verified_completion_on_path(connection, workspace_id, &predecessor, path)
            .map_err(|refusal| match refusal {
                CompletionReadRefusal::Absent | CompletionReadRefusal::CrossWorkspace => {
                    CompletionReadRefusal::SupersessionAbsent
                }
                CompletionReadRefusal::SupersessionCycle => {
                    CompletionReadRefusal::SupersessionCycle
                }
                other => CompletionReadRefusal::SupersessionUnverified(other.to_string()),
            })?;
        if earlier.mission_id != record.mission_id {
            return Err(CompletionReadRefusal::SupersessionOtherMission);
        }
    }

    Ok(record)
}

fn disposition_token(record: &MissionCompletionV1) -> &'static str {
    match record.disposition {
        crate::mission_completion::CompletionDisposition::Concluded => "concluded",
        crate::mission_completion::CompletionDisposition::Abandoned => "abandoned",
    }
}

/// The token a condition outcome is stored as. One spelling, shared by the writer and the reader.
pub(crate) fn outcome_token(outcome: ConditionOutcome) -> &'static str {
    match outcome {
        ConditionOutcome::Met => "met",
        ConditionOutcome::NotMet => "not_met",
        ConditionOutcome::NotAssessed => "not_assessed",
        ConditionOutcome::AssessmentInconclusive => "assessment_inconclusive",
    }
}

/// The actor token the boundary DTO carries. One arm today, and closed so a second cannot appear
/// silently.
pub(crate) fn actor_token(actor: CompletionActor) -> &'static str {
    match actor {
        CompletionActor::Operator => "operator",
    }
}

/// Startup: every stored completion re-proves itself, and every `mission.completed` Event names one.
/// The rows `verify_completion_state` enumerates, in the order it enumerates them.
///
/// EXTRACTED SO A BAR CAN CALL IT (R44.6): a placement assertion that COPIES this query agrees
/// with itself, so flipping the pass's own `ORDER BY` leaves it decorative. One function, called
/// by both, moves the pass and the bar's view of it together.
pub fn enumerate_completions(connection: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "SELECT workspace_id, digest_value FROM mission_completions ORDER BY digest_value",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn verify_completion_state(connection: &Connection) -> Result<(), CompletionStateFault> {
    let rows = enumerate_completions(connection).map_err(CompletionStateFault::Store)?;
    for (workspace_id, digest_value) in &rows {
        let parsed = Uuid::parse_str(workspace_id).map_err(|_| CompletionStateFault::Identity {
            workspace_id: workspace_id.clone(),
            digest_value: digest_value.clone(),
        })?;
        verified_completion_within(connection, parsed, digest_value).map_err(|cause| {
            CompletionStateFault::Row {
                workspace_id: parsed,
                digest_value: digest_value.clone(),
                cause,
            }
        })?;
    }
    // No orphan Event: one `mission.completed` per stored completion, and none naming a completion
    // that is not there.
    let events: i64 = connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'mission.completed'",
            [],
            |row| row.get(0),
        )
        .map_err(CompletionStateFault::Store)?;
    if usize::try_from(events) != Ok(rows.len()) {
        return Err(CompletionStateFault::Totals(crate::TotalsMismatch {
            table: "events (mission.completed)",
            expected: rows.len() as u64,
            observed: events.unsigned_abs(),
        }));
    }
    Ok(())
}

/// Startup: one `mission.completed` Event, checked against the completion it names.
///
/// The Event carries the record's exact canonical bytes, so the check is byte equality against the
/// stored row -- not a field-by-field comparison, which would prove the two agree about the fields
/// somebody remembered to compare.
pub(crate) fn verify_completion_event(
    connection: &Connection,
    workspace_id: Uuid,
    payload_json: &str,
) -> Result<(), CompletionReadRefusal> {
    let payload: MissionCompletedPayload =
        serde_json::from_str(payload_json).map_err(|_| CompletionReadRefusal::BytesDisagree)?;
    if payload.workspace_id != workspace_id {
        return Err(CompletionReadRefusal::CrossWorkspace);
    }
    let stored: Option<Vec<u8>> = connection
        .query_row(
            "SELECT canonical_bytes FROM mission_completions
             WHERE workspace_id = ?1 AND digest_value = ?2",
            params![workspace_id.to_string(), payload.digest_value],
            |row| row.get(0),
        )
        .optional()
        .map_err(store)?;
    let Some(stored) = stored else {
        return Err(CompletionReadRefusal::EventCount(0));
    };
    let from_event = data_encoding::BASE64
        .decode(payload.canonical_bytes_base64.as_bytes())
        .map_err(|_| CompletionReadRefusal::BytesDisagree)?;
    if from_event != stored {
        return Err(CompletionReadRefusal::BytesDisagree);
    }
    Ok(())
}

/// Write one completion, its condition rows and its Event, in one transaction.
///
/// Crate-internal, and there is no Command, transport or Agent path to it in this slice: the
/// authenticated surface arrives with the Operator command, and an input surface before its
/// consumers exist is how a grammar gets frozen around a guess.
///
/// The Event is written **after** the row, in the same transaction, so `boundary_event_seq` can be
/// strictly less than the Event's own sequence -- the invariant the reader checks, and one the
/// caller cannot satisfy by choosing a number, because the sequence is the store's to assign.
pub fn record_completion(
    connection: &mut Connection,
    record: &MissionCompletionV1,
) -> Result<String, CompletionReadRefusal> {
    // PRESERVED WRAPPER. Ten existing callers pass a `&mut Connection` and expect this to own its
    // own transaction; the body moved to `record_completion_within` so a caller that already holds
    // one can write inside it. No behaviour moved with it.
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(store)?;
    let digest = record_completion_within(&transaction, record)?;
    transaction.commit().map_err(store)?;
    Ok(digest)
}

/// Write one completion, its condition rows and its Event **inside the caller's transaction**.
///
/// Crate-internal. The C4 owner derives identity, time and boundary in the same transaction, so it
/// cannot call the wrapper above without nesting -- and a second writer would be a second place to
/// keep the row, its projections and its Event atomic.
pub(crate) fn record_completion_within(
    transaction: &Transaction<'_>,
    record: &MissionCompletionV1,
) -> Result<String, CompletionReadRefusal> {
    // A delivered-Artifact digest is stored as cited, not verified here (R19.1, R32.1). The
    // collector reads each one through the Artifact module's own verified reader at the
    // completion's boundary and ABORTS on one that does not resolve or is scoped elsewhere
    // (`outcome_collector.rs`, `artifact_at_or_before`). Re-checking it here would be a second
    // copy of that rule, in a second place, to keep true forever.
    let bytes = record
        .canonical_bytes()
        .map_err(|_| CompletionReadRefusal::BytesDisagree)?;
    let digest = record
        .digest()
        .map_err(|_| CompletionReadRefusal::BytesDisagree)?;
    transaction
        .execute(
            "INSERT INTO mission_completions (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, completion_id, mission_id, project_id,
                 accountable_resident_id, accountable_configuration_id,
                 boundary_event_seq, disposition, completed_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission_completion.v1', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                       ?9, ?10, ?11)",
            params![
                record.workspace_id.to_string(),
                digest.value,
                bytes,
                record.completion_id.to_string(),
                record.mission_id.to_string(),
                record.project_id.to_string(),
                record.accountable_resident_id.to_string(),
                record.accountable_resident_configuration_id.to_string(),
                i64::try_from(record.boundary_event_seq).unwrap_or(i64::MAX),
                disposition_token(record),
                i64::try_from(record.completed_at_ms).unwrap_or(i64::MAX),
            ],
        )
        .map_err(store)?;
    for result in &record.condition_results {
        transaction
            .execute(
                "INSERT INTO mission_completion_conditions (
                     workspace_id, completion_digest, mission_id, condition_ordinal, outcome
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    record.workspace_id.to_string(),
                    digest.value,
                    record.mission_id.to_string(),
                    i64::from(result.ordinal),
                    outcome_token(result.outcome),
                ],
            )
            .map_err(store)?;
    }
    let payload = serde_json::to_string(&MissionCompletedPayload {
        workspace_id: record.workspace_id,
        mission_id: record.mission_id,
        completion_id: record.completion_id,
        digest_value: digest.value.clone(),
        canonical_bytes_base64: data_encoding::BASE64.encode(&bytes),
    })
    .map_err(|_| CompletionReadRefusal::BytesDisagree)?;
    crate::event_chain::append_event(
        transaction,
        &record.workspace_id.to_string(),
        "mission.completed",
        &payload,
        i64::try_from(record.completed_at_ms).unwrap_or(i64::MAX),
    )
    .map_err(store)?;

    // Read the record back through the verified reader, inside the transaction, before committing.
    //
    // Without this a write can leave the store in a state its own reader refuses: the exactly-once
    // rule, the declared-conditions digest and the boundary are all the READER's checks, and none
    // of them is a key or a trigger. A completion with a gap in its condition set was storable and
    // then unreadable -- found by the Outcome collector's slice-2 test, which is one lane too late
    // for a caller who was told the write succeeded. `MeasuredSpendService::record` already had
    // this shape; this module did not, and the difference was invisible until something read.
    let stored = verified_completion_within(transaction, record.workspace_id, &digest.value)?;
    if &stored != record {
        return Err(CompletionReadRefusal::BytesDisagree);
    }
    Ok(digest.value)
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()`; see the note in `outcome_publication`'s twin.
    fn expected_name(refusal: &CompletionReadRefusal) -> &'static str {
        match refusal {
            CompletionReadRefusal::Absent => "Absent",
            CompletionReadRefusal::CrossWorkspace => "CrossWorkspace",
            CompletionReadRefusal::BytesDisagree => "BytesDisagree",
            CompletionReadRefusal::ColumnDisagrees(_) => "ColumnDisagrees",
            CompletionReadRefusal::SupersessionAbsent => "SupersessionAbsent",
            CompletionReadRefusal::SupersessionUnverified(_) => "SupersessionUnverified",
            CompletionReadRefusal::SupersessionOtherMission => "SupersessionOtherMission",
            CompletionReadRefusal::SupersessionCycle => "SupersessionCycle",
            CompletionReadRefusal::MissionProjectDisagrees(_) => "MissionProjectDisagrees",
            CompletionReadRefusal::ConditionProjectionDisagrees { .. } => {
                "ConditionProjectionDisagrees"
            }
            CompletionReadRefusal::ConditionsNotExactlyOnce { .. } => "ConditionsNotExactlyOnce",
            CompletionReadRefusal::DeclaredConditionsMoved => "DeclaredConditionsMoved",
            CompletionReadRefusal::MissionNotAdmitted => "MissionNotAdmitted",
            CompletionReadRefusal::MissionAdmissionUnreadable { .. } => {
                "MissionAdmissionUnreadable"
            }
            CompletionReadRefusal::BoundaryOutOfRange => "BoundaryOutOfRange",
            CompletionReadRefusal::EventCount(_) => "EventCount",
            CompletionReadRefusal::Store(_) => "Store",
        }
    }

    #[test]
    fn every_completion_refusal_arm_has_its_own_pinned_name() {
        let every_arm = [
            CompletionReadRefusal::Absent,
            CompletionReadRefusal::CrossWorkspace,
            CompletionReadRefusal::BytesDisagree,
            CompletionReadRefusal::ColumnDisagrees("anything"),
            CompletionReadRefusal::SupersessionAbsent,
            CompletionReadRefusal::SupersessionUnverified("anything at all".to_owned()),
            CompletionReadRefusal::SupersessionOtherMission,
            CompletionReadRefusal::SupersessionCycle,
            CompletionReadRefusal::MissionProjectDisagrees("anything"),
            CompletionReadRefusal::ConditionProjectionDisagrees {
                ordinal: 0,
                column: "anything",
            },
            CompletionReadRefusal::ConditionsNotExactlyOnce {
                declared: 1,
                stored: 2,
            },
            CompletionReadRefusal::DeclaredConditionsMoved,
            CompletionReadRefusal::MissionNotAdmitted,
            CompletionReadRefusal::MissionAdmissionUnreadable { sequence: 1 },
            CompletionReadRefusal::BoundaryOutOfRange,
            CompletionReadRefusal::EventCount(1),
            CompletionReadRefusal::Store("anything at all".to_owned()),
        ];
        assert_eq!(
            every_arm.len(),
            17,
            "an arm was added to `CompletionReadRefusal` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(CompletionReadRefusal::name).collect();
        assert_eq!(
            names.len(),
            17,
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

// ---------------------------------------------------------------------------------------------
// C3: the read-only completion summary
//
// A narrow by-Mission reader beside the existing by-digest one. It adds no write, changes no
// prepare or publish behaviour, and reinterprets nothing: cited support is COUNTED, never read as
// verified supporting content, and `concluded` is a disposition rather than a success.
// ---------------------------------------------------------------------------------------------

/// One condition's recorded outcome, as the desktop summary shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConditionResultView {
    pub ordinal: u32,
    pub outcome: String,
    /// How many support references the record CITES for this condition. A count of citations, not
    /// a claim that any of them was read or verified.
    pub support_count: u32,
    pub note: Option<String>,
}

/// The verified Mission completion, exactly the C3 boundary DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionView {
    pub mission_id: Uuid,
    pub completion_id: Uuid,
    pub actor: String,
    pub accountable_resident_id: Uuid,
    pub accountable_resident_configuration_id: Uuid,
    pub disposition: String,
    pub condition_results: Vec<ConditionResultView>,
    /// Artifact record digests the completion RECORDS as delivered. A recorded count.
    pub delivered_artifact_count: u32,
    /// Items the completion RECORDS as unfinished. Zero is not a claim that nothing is unfinished.
    pub unfinished_work_count: u32,
}

/// Why a completion read could not answer.
///
/// Two arms, and the split is the contract's: an authentication or session refusal keeps its own
/// existing semantics all the way out, while every verification failure becomes one closed
/// unavailable answer. The real cause rides in `Unavailable` for `Debug` and `source`, and never
/// reaches the outward line.
#[derive(Debug)]
pub enum MissionCompletionReadError {
    Account(OperatorAccountError),
    Unavailable(CompletionUnavailable),
}

/// What actually went wrong, for a diagnostician. Never projected outward.
#[derive(Debug)]
pub enum CompletionUnavailable {
    Store(rusqlite::Error),
    /// The Mission itself does not re-prove against its own admission Event.
    MissionUnverified(MissionError),
    /// The completion record does not re-prove. Its owner's reason, carried whole.
    CompletionUnverified(CompletionReadRefusal),
    /// A retained `mission.completed` Event for this Mission has no row. Positive evidence that a
    /// completion was recorded, so absence would be a statement the store contradicts.
    OrphanCompletedEvent,
    /// More than one scoped completion row or Event for one Mission: divergent owner evidence.
    DivergentEvidence,
}

impl MissionCompletionReadError {
    /// The closed outward token. `Debug` and `source` carry everything else.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::Unavailable(_) => "Unavailable",
        }
    }
}

impl fmt::Display for MissionCompletionReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl std::error::Error for MissionCompletionReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Account(error) => Some(error),
            Self::Unavailable(_) => None,
        }
    }
}

impl From<OperatorAccountError> for MissionCompletionReadError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

fn unavailable(reason: CompletionUnavailable) -> MissionCompletionReadError {
    MissionCompletionReadError::Unavailable(reason)
}

/// The verified completion of one Mission, or an honest absence, inside the caller's snapshot.
///
/// `Ok(None)` means this Workspace admitted no such Mission **or** admitted it and recorded no
/// completion. Those are deliberately indistinguishable, so a caller cannot use the shape of the
/// answer to learn that a Mission it cannot see exists.
///
/// Scoped both ways: every retained `mission.completed` Event for this Mission must have its row,
/// and every row must have its Event. No global replay runs for a scoped read.
/// C3's read, unchanged: the same proof, projected to the same view.
///
/// Factored so C4's recovery can compute a token over the ACTUAL FULL stored record rather than
/// reconstructing one from counts or trusting a weaker by-digest read. The projection below is the
/// only difference between the two, and it is byte-identical to what C3 returned.
pub(crate) fn verified_completion_for_mission_within(
    transaction: &Transaction<'_>,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Option<CompletionView>, MissionCompletionReadError> {
    let Some(completion) =
        verified_full_completion_for_mission_within(transaction, workspace_id, mission_id)?
    else {
        return Ok(None);
    };
    Ok(Some(project_completion_view(&completion)?))
}

/// The whole verified record for one Mission, with every proof C3 performs.
pub(crate) fn verified_full_completion_for_mission_within(
    transaction: &Transaction<'_>,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Option<MissionCompletionV1>, MissionCompletionReadError> {
    // 1. The Mission, through its own owner. Absent is absence; unverified is a refusal.
    let mission = crate::mission::verified_mission_admission(transaction, workspace_id, mission_id)
        .map_err(|error| unavailable(CompletionUnavailable::MissionUnverified(error)))?;

    // 2. Scoped owner evidence, counted in both directions BEFORE anything is selected.
    let rows: Vec<String> = {
        let mut statement = transaction
            .prepare(
                "SELECT digest_value FROM mission_completions
                 WHERE workspace_id = ?1 AND mission_id = ?2
                 ORDER BY digest_value",
            )
            .map_err(|error| unavailable(CompletionUnavailable::Store(error)))?;
        statement
            .query_map(
                params![workspace_id.to_string(), mission_id.to_string()],
                |row| row.get(0),
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|error| unavailable(CompletionUnavailable::Store(error)))?
    };
    let events: i64 = transaction
        .query_row(
            "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = 'mission.completed'
               AND json_extract(payload_json, '$.missionId') = ?2",
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|error| unavailable(CompletionUnavailable::Store(error)))?;

    if mission.is_none() {
        // The Mission is not this Workspace's. Any completion evidence bearing its id would be a
        // contradiction, and silence about it is the scope answer.
        if !rows.is_empty() || events > 0 {
            return Err(unavailable(CompletionUnavailable::DivergentEvidence));
        }
        return Ok(None);
    }

    if rows.len() > 1 || events > 1 {
        return Err(unavailable(CompletionUnavailable::DivergentEvidence));
    }
    let Some(digest_value) = rows.first() else {
        // A retained Event with no row is positive evidence a completion was recorded.
        if events > 0 {
            return Err(unavailable(CompletionUnavailable::OrphanCompletedEvent));
        }
        return Ok(None);
    };
    if events == 0 {
        return Err(unavailable(CompletionUnavailable::OrphanCompletedEvent));
    }

    // 3. The record, through its existing owner. No weaker reconstruction.
    let completion = verified_completion_within(transaction, workspace_id, digest_value)
        .map_err(|refusal| unavailable(CompletionUnavailable::CompletionUnverified(refusal)))?;
    if completion.mission_id != mission_id {
        return Err(unavailable(CompletionUnavailable::DivergentEvidence));
    }

    // 4. The owner Event's BYTES, in this same snapshot.
    //
    // `verified_completion_within` finds the Event by `digestValue` and checks its sequence
    // against the boundary; it does NOT open the payload. `verify_completion_event` is the
    // verifier that does, and nothing called it outside startup -- so an Event whose
    // `canonicalBytesBase64` had been rewritten, with its selectors intact, passed the by-digest
    // read. Measured by root and closed here, for this scoped read only: no global replay runs.
    //
    // What that verifier proves: the payload parses, its `workspaceId` is this Workspace, the row
    // it names by digest exists, and the bytes it carries are byte-equal to that row's stored
    // canonical bytes. What it does NOT prove is the payload's own `missionId` and `completionId`,
    // which are redundant selectors -- so those are compared here against the verified record
    // rather than left unchecked.
    let payload_json: String = transaction
        .query_row(
            "SELECT payload_json FROM events
             WHERE workspace_id = ?1 AND kind = 'mission.completed'
               AND json_extract(payload_json, '$.missionId') = ?2",
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|error| unavailable(CompletionUnavailable::Store(error)))?;
    verify_completion_event(transaction, workspace_id, &payload_json)
        .map_err(|refusal| unavailable(CompletionUnavailable::CompletionUnverified(refusal)))?;
    let redundant: MissionCompletedPayload = serde_json::from_str(&payload_json)
        .map_err(|_| unavailable(CompletionUnavailable::DivergentEvidence))?;
    if redundant.mission_id != completion.mission_id
        || redundant.completion_id != completion.completion_id
        || redundant.digest_value != *digest_value
    {
        return Err(unavailable(CompletionUnavailable::DivergentEvidence));
    }

    Ok(Some(completion))
}

/// The C3 view, derived from an already-verified record. Counts are RECORDED counts: cited support
/// is counted, never read as verified supporting content.
fn project_completion_view(
    completion: &MissionCompletionV1,
) -> Result<CompletionView, MissionCompletionReadError> {
    let mut condition_results = Vec::with_capacity(completion.condition_results.len());
    for result in &completion.condition_results {
        condition_results.push(ConditionResultView {
            ordinal: result.ordinal,
            outcome: outcome_token(result.outcome).to_owned(),
            support_count: u32::try_from(result.support.len())
                .map_err(|_| unavailable(CompletionUnavailable::DivergentEvidence))?,
            note: result.note.clone(),
        });
    }
    let count = |value: usize| {
        u32::try_from(value).map_err(|_| unavailable(CompletionUnavailable::DivergentEvidence))
    };
    Ok(CompletionView {
        mission_id: completion.mission_id,
        completion_id: completion.completion_id,
        actor: actor_token(completion.actor).to_owned(),
        accountable_resident_id: completion.accountable_resident_id,
        accountable_resident_configuration_id: completion.accountable_resident_configuration_id,
        disposition: disposition_token(completion).to_owned(),
        condition_results,
        delivered_artifact_count: count(completion.delivered_artifacts.len())?,
        unfinished_work_count: count(completion.unfinished_work.len())?,
    })
}

/// The authenticated, Workspace-scoped read over recorded Mission completions.
///
/// It returns a verified summary or an honest absence, and nothing else: no connection, no
/// transaction, no path, and no way to record, prepare or publish anything.
pub struct MissionCompletionReadService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl MissionCompletionReadService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// One Deferred snapshot owns the Mission proof, the completion selection, the record's whole
    /// owner proof and the scoped Event/row reconciliation.
    pub fn get(
        &self,
        mission_id: Uuid,
    ) -> Result<Option<CompletionView>, MissionCompletionReadError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            match verified_completion_for_mission_within(
                &transaction,
                self.workspace_id,
                mission_id,
            ) {
                Ok(found) => {
                    transaction.commit()?;
                    Ok(Ok(found))
                }
                Err(refusal) => Ok::<_, OperatorAccountError>(Err(refusal)),
            }
        })?
    }
}

// ---------------------------------------------------------------------------------------------
// R68/H1: the recording Event, proved, for a by-digest read
// ---------------------------------------------------------------------------------------------

/// Why a completion did not prove itself WITH its recording Event.
#[derive(Debug)]
pub(crate) enum CompletionEventProof {
    Owner(CompletionReadRefusal),
    /// No `mission.completed` Event in this Workspace names this digest, or more than one does.
    RecordingEventUnaccounted,
    /// The Event exists, and the owning verifier or a redundant selector refuses it.
    DivergentEvidence,
}

impl CompletionEventProof {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Owner(_) => "Owner",
            Self::RecordingEventUnaccounted => "RecordingEventUnaccounted",
            Self::DivergentEvidence => "DivergentEvidence",
        }
    }
}

/// The verified completion's exact canonical bytes, WITH the Event that recorded them.
///
/// `verified_completion_within` finds the Event by `digestValue` and checks its sequence but "does
/// NOT open the payload" (`mission_completion_store.rs:1222-1226`); `verify_completion_event` is
/// the verifier that does. For a packet whose S4 calls itself independent owner evidence, the
/// weaker read is not enough (R68/H1), so this composes both in one snapshot.
pub(crate) fn verified_completion_with_event_bytes(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<Vec<u8>>, CompletionEventProof> {
    let completion = match verified_completion_within(connection, workspace_id, digest_value) {
        Ok(completion) => completion,
        Err(CompletionReadRefusal::Absent) => return Ok(None),
        Err(other) => return Err(CompletionEventProof::Owner(other)),
    };
    let bytes = completion
        .canonical_bytes()
        .map_err(|_| CompletionEventProof::DivergentEvidence)?;

    // The strongest scoped reader counts both directions by Mission before selecting the row.
    let mission =
        crate::mission::verified_mission_admission(connection, workspace_id, completion.mission_id)
            .map_err(|_| CompletionEventProof::DivergentEvidence)?;
    if mission.is_none() {
        return Err(CompletionEventProof::DivergentEvidence);
    }
    let row_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM mission_completions
             WHERE workspace_id = ?1 AND mission_id = ?2",
            rusqlite::params![workspace_id.to_string(), completion.mission_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|_| CompletionEventProof::DivergentEvidence)?;
    let event_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = 'mission.completed'
               AND json_extract(payload_json, '$.missionId') = ?2",
            rusqlite::params![workspace_id.to_string(), completion.mission_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|_| CompletionEventProof::DivergentEvidence)?;
    if row_count != 1 || event_count != 1 {
        return Err(CompletionEventProof::DivergentEvidence);
    }

    let mut statement = connection
        .prepare(
            "SELECT payload_json, occurred_at_ms FROM events
             WHERE workspace_id = ?1 AND kind = 'mission.completed'
               AND json_extract(payload_json, '$.digestValue') = ?2",
        )
        .map_err(|_| CompletionEventProof::RecordingEventUnaccounted)?;
    let payloads: Vec<(String, i64)> = statement
        .query_map(
            rusqlite::params![workspace_id.to_string(), digest_value],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|_| CompletionEventProof::RecordingEventUnaccounted)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CompletionEventProof::RecordingEventUnaccounted)?;
    let [(payload_json, occurred_at_ms)] = payloads.as_slice() else {
        return Err(CompletionEventProof::RecordingEventUnaccounted);
    };
    let occurred_at_ms =
        u64::try_from(*occurred_at_ms).map_err(|_| CompletionEventProof::DivergentEvidence)?;

    verify_completion_event(connection, workspace_id, payload_json)
        .map_err(|_| CompletionEventProof::DivergentEvidence)?;
    let redundant: MissionCompletedPayload =
        serde_json::from_str(payload_json).map_err(|_| CompletionEventProof::DivergentEvidence)?;
    if redundant.mission_id != completion.mission_id
        || redundant.completion_id != completion.completion_id
        || redundant.digest_value != *digest_value
        || redundant.canonical_bytes_base64 != data_encoding::BASE64.encode(&bytes)
        || occurred_at_ms != completion.completed_at_ms
    {
        return Err(CompletionEventProof::DivergentEvidence);
    }
    Ok(Some(bytes))
}
