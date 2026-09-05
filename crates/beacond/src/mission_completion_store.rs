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

use std::collections::BTreeSet;

use data_encoding::HEXLOWER;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mission_completion::{CompletionSupersedes, ConditionOutcome, MissionCompletionV1};
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

/// Startup: every stored completion re-proves itself, and every `mission.completed` Event names one.
pub(crate) fn verify_completion_state(
    connection: &Connection,
) -> Result<(), CompletionReadRefusal> {
    let mut statement = connection
        .prepare("SELECT workspace_id, digest_value FROM mission_completions ORDER BY digest_value")
        .map_err(store)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(store)?;
    for (workspace_id, digest_value) in &rows {
        let workspace_id =
            Uuid::parse_str(workspace_id).map_err(|_| CompletionReadRefusal::BytesDisagree)?;
        verified_completion_within(connection, workspace_id, digest_value)?;
    }
    // No orphan Event: one `mission.completed` per stored completion, and none naming a completion
    // that is not there.
    let events: i64 = connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'mission.completed'",
            [],
            |row| row.get(0),
        )
        .map_err(store)?;
    if usize::try_from(events) != Ok(rows.len()) {
        return Err(CompletionReadRefusal::EventCount(events));
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
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(store)?;
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
    transaction
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'mission.completed', ?2, ?3)",
            params![
                record.workspace_id.to_string(),
                payload,
                i64::try_from(record.completed_at_ms).unwrap_or(i64::MAX),
            ],
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
    let stored = verified_completion_within(&transaction, record.workspace_id, &digest.value)?;
    if &stored != record {
        return Err(CompletionReadRefusal::BytesDisagree);
    }
    transaction.commit().map_err(store)?;
    Ok(digest.value)
}
