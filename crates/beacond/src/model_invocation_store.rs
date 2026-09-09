//! Invocation storage (DP13 S1): the durable half of C1.
//!
//! One admitted Mission-plan Invocation, its exact prompt and request bytes, and the bounded
//! transport observations whose language [`crate::model_invocation`] already closes. This module
//! owns persistence and proof; it owns no grammar, and it re-implements none of C1's.
//!
//! # What this is not
//!
//! **It creates no authority and permits no request.** The admission carries a retained authority
//! reference and a *claimed* digest; nothing behind either is held here, hashed here or verified
//! here. There is no resolver, no adapter, no registry, no public Command and no provider. A row in
//! these tables is evidence that an admission was recorded, never that anything may run.
//!
//! # Two bounds that are not the same bound
//!
//! The ceilings below are **parser bounds owned by this module**, the shape every other owner in
//! this crate already uses (`artifact.rs`, `candidate.rs`, `evaluation.rs`, `local_transport.rs`).
//! They are not a request/response envelope: that is a reserved product decision, retained as
//! evidence inside the admission's own canonical bytes as [`RetainedLimits`] and never spelled as a
//! SQL literal. The ceiling constrains the envelope and never the reverse — an admission whose
//! recorded envelope exceeds what this process will parse is refused at admission time.
//!
//! **The allocation guarantee, exactly.** Every length is read with SQL `length()` and compared
//! before the value is selected, so an oversized blob or canonical record is never transferred into
//! a Rust-owned `Vec`/`String` by this module. It is **not** a total process or SQLite memory
//! ceiling: SQLite is in-process and may read, cache or cast values internally while evaluating
//! `length()`. The claim is about what crosses into owned Rust values, and no more.

use std::fmt;

use data_encoding::HEXLOWER;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event_chain;
use crate::model_invocation::{
    ADMISSION_CANONICALIZER, AppendRefusal, Appended, INVOCATION_DIGEST_ALGORITHM,
    InvocationAdmissionV1, InvocationObservationV1, MalformedInvocation, OBSERVATION_CANONICALIZER,
    ObservationKind, ReadCeiling, TransportHistory,
};
use crate::proposal::sha256;

// ---------------------------------------------------------------------------------------------
// Structural parser ceilings. PROPOSED: Kevin signs these with migration 0037 at the merge gate.
// ---------------------------------------------------------------------------------------------

/// An admission record is fixed-shape: magic, seven UUIDs, four bounded name tokens, four digests,
/// six `u32`s, one tag and a timestamp. Its maximum framing is measured by
/// `the_widest_admission_record_fits_the_proposed_ceiling`.
pub(crate) const MAX_ADMISSION_CANONICAL_BYTES: usize = 4096;
/// One retained prompt or request body. Artifact's accepted blob tier (`artifact.rs`).
pub(crate) const MAX_RETAINED_BLOB_BYTES: usize = 1_048_576;
/// One observation record: a retained body at the blob tier plus its framing.
pub(crate) const MAX_OBSERVATION_CANONICAL_BYTES: usize = 1_052_672;
/// One Invocation Event payload, in UTF-8 **bytes**. These Events carry identity, sequence and
/// digest — never a duplicated body — so this is generous for the shape they have.
pub(crate) const MAX_EVENT_PAYLOAD_BYTES: i64 = 4096;

fn admission_ceiling() -> ReadCeiling {
    ReadCeiling::new(MAX_ADMISSION_CANONICAL_BYTES, MAX_RETAINED_BLOB_BYTES)
}

fn observation_ceiling() -> ReadCeiling {
    ReadCeiling::new(MAX_OBSERVATION_CANONICAL_BYTES, MAX_RETAINED_BLOB_BYTES)
}

pub(crate) const ADMITTED_EVENT_KIND: &str = "invocation.admitted";
pub(crate) const OBSERVED_EVENT_KIND: &str = "invocation.observed";

// ---------------------------------------------------------------------------------------------
// Faults
// ---------------------------------------------------------------------------------------------

/// Why the Invocation store refused.
#[derive(Debug)]
pub enum InvocationStoreError {
    Account(crate::operator_account::OperatorAccountError),
    Store(rusqlite::Error),
    Json(serde_json::Error),
    /// C1 could not read the canonical bytes, or they exceed this caller's ceiling.
    Malformed(MalformedInvocation),
    /// C1's grammar refused the offered observation.
    Append(AppendRefusal),
    /// A projected column disagrees with the canonical bytes it is a projection of.
    ColumnDisagreement(&'static str),
    /// The bytes the CALLER offered do not match the digest and length the admission claims.
    OfferedContentMismatch(&'static str),
    /// The bytes the STORE holds do not match the digest and length the admission claims. Never
    /// repaired, never accepted, and never blamed on the caller.
    StoredContentMismatch(&'static str),
    /// This Invocation is already admitted with different canonical bytes.
    AdmissionConflict,
    /// Two different byte strings both satisfy one claimed digest and length. Refused.
    DigestCollision,
    /// The admission's own recorded envelope is larger than this process will parse.
    EnvelopeExceedsStorageCeiling(&'static str),
    /// A stored value is larger than this module's parser ceiling, refused before transfer.
    OverStorageCeiling(&'static str),
    /// The store contradicts itself: a row with no Event, an Event with no row, a bad projection.
    InvalidStore,
}

impl InvocationStoreError {
    /// The variant's NAME, for a one-line startup message.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
            Self::Malformed(_) => "Malformed",
            Self::Append(_) => "Append",
            Self::ColumnDisagreement(_) => "ColumnDisagreement",
            Self::OfferedContentMismatch(_) => "OfferedContentMismatch",
            Self::StoredContentMismatch(_) => "StoredContentMismatch",
            Self::AdmissionConflict => "AdmissionConflict",
            Self::DigestCollision => "DigestCollision",
            Self::EnvelopeExceedsStorageCeiling(_) => "EnvelopeExceedsStorageCeiling",
            Self::OverStorageCeiling(_) => "OverStorageCeiling",
            Self::InvalidStore => "InvalidStore",
        }
    }
}

impl fmt::Display for InvocationStoreError {
    /// ONE independent literal per top-level arm, and nothing else (R42, brief item 7).
    ///
    /// No nested `Display` is forwarded and no carried `&'static str` reaches this line: an outward
    /// refusal is a closed token a reader can enumerate, while the store's own text -- which column,
    /// which field, which SQLite or serde error -- stays in `Debug` and in `source()`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Account(_) => "Account",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
            Self::Malformed(_) => "Malformed",
            Self::Append(_) => "Append",
            Self::ColumnDisagreement(_) => "ColumnDisagreement",
            Self::OfferedContentMismatch(_) => "OfferedContentMismatch",
            Self::StoredContentMismatch(_) => "StoredContentMismatch",
            Self::AdmissionConflict => "AdmissionConflict",
            Self::DigestCollision => "DigestCollision",
            Self::EnvelopeExceedsStorageCeiling(_) => "EnvelopeExceedsStorageCeiling",
            Self::OverStorageCeiling(_) => "OverStorageCeiling",
            Self::InvalidStore => "InvalidStore",
        })
    }
}

impl std::error::Error for InvocationStoreError {
    /// The carried cause, where one is a real `Error`. This is where a diagnostician follows the
    /// detail the one-line projection deliberately does not carry.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Account(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Malformed(error) => Some(error),
            Self::Append(error) => Some(error),
            _ => None,
        }
    }
}

impl From<crate::operator_account::OperatorAccountError> for InvocationStoreError {
    fn from(error: crate::operator_account::OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for InvocationStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for InvocationStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<MalformedInvocation> for InvocationStoreError {
    fn from(error: MalformedInvocation) -> Self {
        Self::Malformed(error)
    }
}

/// Which fault the Invocation startup pass hit, and which row hit it.
#[derive(Debug)]
pub(crate) enum InvocationStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One Invocation did not re-prove.
    Row {
        workspace_id: Uuid,
        invocation_id: Uuid,
        cause: InvocationStoreError,
    },
    /// A check about the store rather than one Invocation.
    Totals(crate::TotalsMismatch),
    /// A row whose identity columns are not UUIDs.
    Identity {
        workspace_id: String,
        invocation_id: String,
    },
}

// ---------------------------------------------------------------------------------------------
// Owner Event payloads: identity, sequence and digest. Never a duplicated body.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InvocationDigestPayload {
    pub(crate) algorithm: String,
    pub(crate) canonicalizer: String,
    pub(crate) value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdmittedEventRecord {
    id: Uuid,
    workspace_id: Uuid,
    digest: InvocationDigestPayload,
    admitted_at_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InvocationAdmittedPayload {
    invocation: AdmittedEventRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObservedEventRecord {
    invocation_id: Uuid,
    workspace_id: Uuid,
    sequence: u32,
    kind: String,
    digest: InvocationDigestPayload,
    /// The local instant this observation was recorded. Compared against the Event's own
    /// `occurred_at_ms` on every read, exactly as the admission's time is.
    recorded_at_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InvocationObservedPayload {
    observation: ObservedEventRecord,
}

fn admission_digest_payload(value: &str) -> InvocationDigestPayload {
    InvocationDigestPayload {
        algorithm: INVOCATION_DIGEST_ALGORITHM.to_owned(),
        canonicalizer: ADMISSION_CANONICALIZER.to_owned(),
        value: value.to_owned(),
    }
}

fn observation_digest_payload(value: &str) -> InvocationDigestPayload {
    InvocationDigestPayload {
        algorithm: INVOCATION_DIGEST_ALGORITHM.to_owned(),
        canonicalizer: OBSERVATION_CANONICALIZER.to_owned(),
        value: value.to_owned(),
    }
}

// ---------------------------------------------------------------------------------------------
// The verified value
// ---------------------------------------------------------------------------------------------

/// One stored Invocation, proved: the canonical bytes decode, every projected column repeats what
/// they say, both retained blobs hash and measure to what the admission claims, the owning Event
/// names this record, and the observations replay through C1's grammar.
///
/// No `Eq`: `TransportHistory` has none, and C1 is not edited by this slice. Callers compare the
/// parts they care about.
#[derive(Debug, Clone)]
pub struct VerifiedInvocation {
    admission: InvocationAdmissionV1,
    prompt: Vec<u8>,
    request: Vec<u8>,
    history: TransportHistory,
    creation_event_seq: i64,
}

impl VerifiedInvocation {
    pub fn admission(&self) -> &InvocationAdmissionV1 {
        &self.admission
    }

    pub fn prompt(&self) -> &[u8] {
        &self.prompt
    }

    pub fn request(&self) -> &[u8] {
        &self.request
    }

    pub fn history(&self) -> &TransportHistory {
        &self.history
    }

    pub fn creation_event_seq(&self) -> i64 {
        self.creation_event_seq
    }
}

// ---------------------------------------------------------------------------------------------
// Content proof
// ---------------------------------------------------------------------------------------------

/// Whether these bytes are what the admission claims: the digest AND the length, both.
fn content_matches(bytes: &[u8], claimed_digest: &[u8; 32], claimed_len: u32) -> bool {
    u32::try_from(bytes.len()) == Ok(claimed_len) && sha256(bytes) == *claimed_digest
}

fn require_envelope_within_ceiling(
    admission: &InvocationAdmissionV1,
) -> Result<(), InvocationStoreError> {
    let blob_tier = u32::try_from(MAX_RETAINED_BLOB_BYTES).unwrap_or(u32::MAX);
    if admission.limits.request_bytes > blob_tier {
        return Err(InvocationStoreError::EnvelopeExceedsStorageCeiling(
            "request_bytes",
        ));
    }
    if admission.limits.response_bytes > blob_tier {
        return Err(InvocationStoreError::EnvelopeExceedsStorageCeiling(
            "response_bytes",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Loading, bounded before transfer
// ---------------------------------------------------------------------------------------------

struct StoredInvocation {
    digest_value: String,
    canonical_bytes: Vec<u8>,
    projection: StoredProjection,
    prompt_inline: Vec<u8>,
    request_inline: Vec<u8>,
    creation_event_seq: i64,
}

struct StoredProjection {
    digest_algorithm: String,
    digest_canonicalizer: String,
    invocation_id: String,
    project_id: String,
    mission_id: String,
    resident_id: String,
    admitted_configuration_id: String,
    activation_id: String,
    admitted_at_ms: i64,
}

/// Read one stored Invocation, refusing any oversized value BEFORE it is transferred.
///
/// Phase one selects only `length()`s and the small projected columns; phase two selects the blobs.
fn load_stored(
    connection: &Transaction<'_>,
    workspace_id: Uuid,
    invocation_id: Uuid,
) -> Result<Option<StoredInvocation>, InvocationStoreError> {
    let lengths: Option<(String, i64, i64, i64)> = connection
        .query_row(
            "SELECT digest_value,
                    length(canonical_bytes), length(prompt_inline), length(request_inline)
             FROM model_invocations
             WHERE workspace_id = ?1 AND invocation_id = ?2",
            params![workspace_id.to_string(), invocation_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((digest_value, canonical_len, prompt_len, request_len)) = lengths else {
        return Ok(None);
    };
    if canonical_len > i64::try_from(MAX_ADMISSION_CANONICAL_BYTES).unwrap_or(i64::MAX) {
        return Err(InvocationStoreError::OverStorageCeiling("canonical_bytes"));
    }
    let blob_tier = i64::try_from(MAX_RETAINED_BLOB_BYTES).unwrap_or(i64::MAX);
    if prompt_len > blob_tier {
        return Err(InvocationStoreError::OverStorageCeiling("prompt_inline"));
    }
    if request_len > blob_tier {
        return Err(InvocationStoreError::OverStorageCeiling("request_inline"));
    }

    let stored = connection.query_row(
        "SELECT canonical_bytes, digest_algorithm, digest_canonicalizer, invocation_id,
                project_id, mission_id, resident_id, admitted_configuration_id, activation_id,
                admitted_at_ms, prompt_inline, request_inline, creation_event_seq
         FROM model_invocations
         WHERE workspace_id = ?1 AND invocation_id = ?2",
        params![workspace_id.to_string(), invocation_id.to_string()],
        |row| {
            Ok(StoredInvocation {
                digest_value: digest_value.clone(),
                canonical_bytes: row.get(0)?,
                projection: StoredProjection {
                    digest_algorithm: row.get(1)?,
                    digest_canonicalizer: row.get(2)?,
                    invocation_id: row.get(3)?,
                    project_id: row.get(4)?,
                    mission_id: row.get(5)?,
                    resident_id: row.get(6)?,
                    admitted_configuration_id: row.get(7)?,
                    activation_id: row.get(8)?,
                    admitted_at_ms: row.get(9)?,
                },
                prompt_inline: row.get(10)?,
                request_inline: row.get(11)?,
                creation_event_seq: row.get(12)?,
            })
        },
    )?;
    Ok(Some(stored))
}

/// Every projected column must repeat what the canonical bytes say.
fn projection_disagreement(
    stored: &StoredInvocation,
    admission: &InvocationAdmissionV1,
    digest_value: &str,
) -> Option<&'static str> {
    let projection = &stored.projection;
    if projection.digest_algorithm != INVOCATION_DIGEST_ALGORITHM {
        return Some("digest_algorithm");
    }
    if projection.digest_canonicalizer != ADMISSION_CANONICALIZER {
        return Some("digest_canonicalizer");
    }
    if stored.digest_value != digest_value {
        return Some("digest_value");
    }
    for (column, projected, expected) in [
        (
            "invocation_id",
            projection.invocation_id.as_str(),
            admission.invocation_id,
        ),
        (
            "project_id",
            projection.project_id.as_str(),
            admission.project_id,
        ),
        (
            "mission_id",
            projection.mission_id.as_str(),
            admission.mission_id,
        ),
        (
            "resident_id",
            projection.resident_id.as_str(),
            admission.resident_id,
        ),
        (
            "admitted_configuration_id",
            projection.admitted_configuration_id.as_str(),
            admission.admitted_configuration_id,
        ),
        (
            "activation_id",
            projection.activation_id.as_str(),
            admission.activation_id,
        ),
    ] {
        if projected != expected.to_string() {
            return Some(column);
        }
    }
    if projection.admitted_at_ms != admission.admitted_at_ms {
        return Some("admitted_at_ms");
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Owner Events, re-derived rather than trusted
// ---------------------------------------------------------------------------------------------

/// The Event that admitted this record, found by its own digest and proved against it.
fn paired_admission_event(
    connection: &Transaction<'_>,
    workspace_id: Uuid,
    admission: &InvocationAdmissionV1,
    digest_value: &str,
) -> Result<i64, InvocationStoreError> {
    // The payload is SUPPRESSED IN SQL before transfer, not measured beside it: selecting the
    // length next to the value still moves the value. This is the same guard the startup
    // enumeration applies, and it is repeated here because startup's CASE cannot protect a public
    // read that happens after startup. It covers every matching row, including duplicates.
    let mut statement = connection.prepare(
        "SELECT sequence, occurred_at_ms, length(CAST(payload_json AS BLOB)),
                CASE WHEN length(CAST(payload_json AS BLOB)) > ?4
                     THEN NULL ELSE payload_json END
         FROM events
         WHERE workspace_id = ?1 AND kind = ?2
           AND json_extract(payload_json, '$.invocation.digest.value') = ?3",
    )?;
    let rows: Vec<(i64, i64, i64, Option<String>)> = statement
        .query_map(
            params![
                workspace_id.to_string(),
                ADMITTED_EVENT_KIND,
                digest_value,
                MAX_EVENT_PAYLOAD_BYTES
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    // Any oversized match refuses, before the single-row shape is even considered.
    if rows.iter().any(|(_, _, _, payload)| payload.is_none()) {
        return Err(InvocationStoreError::OverStorageCeiling("payload_json"));
    }
    // Exactly one: none is a record the log does not account for, two would give one admission two
    // answers to "when".
    let [(sequence, occurred_at_ms, _bytes, payload_json)] = rows.as_slice() else {
        return Err(InvocationStoreError::InvalidStore);
    };
    let payload_json = payload_json
        .as_deref()
        .ok_or(InvocationStoreError::OverStorageCeiling("payload_json"))?;
    let payload: InvocationAdmittedPayload = serde_json::from_str(payload_json)?;
    let recorded = payload.invocation;
    if recorded.id != admission.invocation_id
        || recorded.workspace_id != workspace_id
        || recorded.digest != admission_digest_payload(digest_value)
        || i64::try_from(recorded.admitted_at_ms) != Ok(*occurred_at_ms)
        || recorded.admitted_at_ms != u64::try_from(admission.admitted_at_ms).unwrap_or(u64::MAX)
    {
        return Err(InvocationStoreError::InvalidStore);
    }
    Ok(*sequence)
}

/// The Event that recorded one observation, proved the same way.
fn paired_observation_event(
    connection: &Transaction<'_>,
    workspace_id: Uuid,
    record: &InvocationObservationV1,
    digest_value: &str,
) -> Result<i64, InvocationStoreError> {
    // Suppressed before transfer, for the same reason and covering duplicates the same way.
    let mut statement = connection.prepare(
        "SELECT sequence, occurred_at_ms,
                CASE WHEN length(CAST(payload_json AS BLOB)) > ?4
                     THEN NULL ELSE payload_json END
         FROM events
         WHERE workspace_id = ?1 AND kind = ?2
           AND json_extract(payload_json, '$.observation.digest.value') = ?3",
    )?;
    let rows: Vec<(i64, i64, Option<String>)> = statement
        .query_map(
            params![
                workspace_id.to_string(),
                OBSERVED_EVENT_KIND,
                digest_value,
                MAX_EVENT_PAYLOAD_BYTES
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.iter().any(|(_, _, payload)| payload.is_none()) {
        return Err(InvocationStoreError::OverStorageCeiling("payload_json"));
    }
    let [(sequence, occurred_at_ms, payload_json)] = rows.as_slice() else {
        return Err(InvocationStoreError::InvalidStore);
    };
    let payload_json = payload_json
        .as_deref()
        .ok_or(InvocationStoreError::OverStorageCeiling("payload_json"))?;
    let payload: InvocationObservedPayload = serde_json::from_str(payload_json)?;
    let recorded = payload.observation;
    if recorded.invocation_id != record.invocation_id
        || recorded.workspace_id != workspace_id
        || recorded.sequence != record.sequence
        || recorded.kind != record.kind().as_str()
        || recorded.digest != observation_digest_payload(digest_value)
        || i64::try_from(recorded.recorded_at_ms) != Ok(*occurred_at_ms)
    {
        return Err(InvocationStoreError::InvalidStore);
    }
    Ok(*sequence)
}

// ---------------------------------------------------------------------------------------------
// The history, replayed through C1
// ---------------------------------------------------------------------------------------------

/// Every stored observation, replayed through [`TransportHistory::append_record`].
///
/// A store that has drifted out of C1's language refuses HERE rather than being replayed into a
/// value. This is the only path stored rows take into a history.
fn load_history(
    connection: &Transaction<'_>,
    workspace_id: Uuid,
    admission: &InvocationAdmissionV1,
) -> Result<TransportHistory, InvocationStoreError> {
    // Count and length safety BEFORE any arbitrary row is collected: the grammar admits at most
    // four observation kinds per Invocation, so more rows than that is a store fault, not a history.
    let stored_rows: i64 = connection.query_row(
        "SELECT count(*) FROM model_invocation_observations
         WHERE workspace_id = ?1 AND invocation_id = ?2",
        params![
            workspace_id.to_string(),
            admission.invocation_id.to_string()
        ],
        |row| row.get(0),
    )?;
    if stored_rows > i64::try_from(ObservationKind::ALL.len()).unwrap_or(i64::MAX) {
        return Err(InvocationStoreError::InvalidStore);
    }
    let mut lengths = connection.prepare(
        "SELECT sequence, kind, digest_algorithm, digest_canonicalizer, digest_value,
                observed_event_seq, length(canonical_bytes)
         FROM model_invocation_observations
         WHERE workspace_id = ?1 AND invocation_id = ?2
         ORDER BY sequence",
    )?;
    let rows: Vec<(i64, String, String, String, String, i64, i64)> = lengths
        .query_map(
            params![
                workspace_id.to_string(),
                admission.invocation_id.to_string()
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    drop(lengths);

    let mut history = TransportHistory::admitted(admission);
    let mut previous_sequence = 0_i64;
    for (
        sequence,
        kind,
        digest_algorithm,
        digest_canonicalizer,
        digest_value,
        observed_event_seq,
        canonical_len,
    ) in rows
    {
        // Row order is explicit, not assumed from the ORDER BY: a store whose sequences repeat or
        // regress is a fault here rather than a surprise inside C1.
        if sequence <= previous_sequence {
            return Err(InvocationStoreError::ColumnDisagreement("sequence"));
        }
        previous_sequence = sequence;
        if digest_algorithm != INVOCATION_DIGEST_ALGORITHM {
            return Err(InvocationStoreError::ColumnDisagreement("digest_algorithm"));
        }
        if digest_canonicalizer != OBSERVATION_CANONICALIZER {
            return Err(InvocationStoreError::ColumnDisagreement(
                "digest_canonicalizer",
            ));
        }
        if canonical_len > i64::try_from(MAX_OBSERVATION_CANONICAL_BYTES).unwrap_or(i64::MAX) {
            return Err(InvocationStoreError::OverStorageCeiling("canonical_bytes"));
        }
        let canonical: Vec<u8> = connection.query_row(
            "SELECT canonical_bytes FROM model_invocation_observations
             WHERE workspace_id = ?1 AND invocation_id = ?2 AND sequence = ?3",
            params![
                workspace_id.to_string(),
                admission.invocation_id.to_string(),
                sequence
            ],
            |row| row.get(0),
        )?;
        if HEXLOWER.encode(&sha256(&canonical)) != digest_value {
            return Err(InvocationStoreError::ColumnDisagreement("digest_value"));
        }
        let record =
            InvocationObservationV1::from_canonical_bytes(&canonical, observation_ceiling())?;
        if record.invocation_id != admission.invocation_id {
            return Err(InvocationStoreError::ColumnDisagreement("invocation_id"));
        }
        if record.kind().as_str() != kind {
            return Err(InvocationStoreError::ColumnDisagreement("kind"));
        }
        if i64::from(record.sequence) != sequence {
            return Err(InvocationStoreError::ColumnDisagreement("sequence"));
        }
        let event_sequence =
            paired_observation_event(connection, workspace_id, &record, &digest_value)?;
        if event_sequence != observed_event_seq {
            return Err(InvocationStoreError::ColumnDisagreement(
                "observed_event_seq",
            ));
        }
        match history
            .append_record(record)
            .map_err(InvocationStoreError::Append)?
        {
            Appended::Recorded { .. } => {}
            // A stored duplicate identity is a store fault, not an idempotent re-offer.
            Appended::AlreadyPresent { .. } => return Err(InvocationStoreError::InvalidStore),
        }
    }
    Ok(history)
}

// ---------------------------------------------------------------------------------------------
// The verified read
// ---------------------------------------------------------------------------------------------

/// One stored Invocation, proved end to end inside the caller's snapshot.
pub(crate) fn verified_invocation_within(
    transaction: &Transaction<'_>,
    workspace_id: Uuid,
    invocation_id: Uuid,
) -> Result<Option<VerifiedInvocation>, InvocationStoreError> {
    let Some(stored) = load_stored(transaction, workspace_id, invocation_id)? else {
        // A retained owner Event is POSITIVE evidence that this Workspace admitted this
        // Invocation, so "absent" would be a statement the store itself contradicts. Startup's
        // totals cannot stand in for this: they run at boot, and a public read happens after.
        //
        // Scoped to this Workspace and this Invocation, so cross-Workspace opacity is unchanged --
        // a foreign Workspace matches no Event and still receives `Ok(None)` -- and no global pass
        // runs on a get. Counted through `json_extract`, so no payload is transferred.
        // Absence must be the absence of EVERY scoped owner trace, not of one kind. An authentic
        // `invocation.observed` Event naming this Invocation is positive evidence too, even when the
        // admission row and its Event are both gone -- and so is an orphaned observation row.
        let admitted = scoped_event_count(
            transaction,
            workspace_id,
            ADMITTED_EVENT_KIND,
            "$.invocation.id",
            invocation_id,
        )?;
        let observed = scoped_event_count(
            transaction,
            workspace_id,
            OBSERVED_EVENT_KIND,
            "$.observation.invocationId",
            invocation_id,
        )?;
        let orphan_rows: i64 = transaction.query_row(
            "SELECT count(*) FROM model_invocation_observations
             WHERE workspace_id = ?1 AND invocation_id = ?2",
            params![workspace_id.to_string(), invocation_id.to_string()],
            |row| row.get(0),
        )?;
        if admitted > 0 || observed > 0 || orphan_rows > 0 {
            return Err(InvocationStoreError::InvalidStore);
        }
        return Ok(None);
    };
    let digest_value = HEXLOWER.encode(&sha256(&stored.canonical_bytes));
    let admission =
        InvocationAdmissionV1::from_canonical_bytes(&stored.canonical_bytes, admission_ceiling())?;
    if admission.workspace_id != workspace_id {
        return Err(InvocationStoreError::ColumnDisagreement("workspace_id"));
    }
    if let Some(column) = projection_disagreement(&stored, &admission, &digest_value) {
        return Err(InvocationStoreError::ColumnDisagreement(column));
    }
    // The content proof, recomputed from the stored bytes on every read: never trusted from a
    // column, and both the digest and the length must agree.
    if !content_matches(
        &stored.prompt_inline,
        &admission.claimed_prompt_digest,
        admission.claimed_prompt_bytes_len,
    ) {
        return Err(InvocationStoreError::StoredContentMismatch("prompt"));
    }
    if !content_matches(
        &stored.request_inline,
        &admission.claimed_request_digest,
        admission.claimed_request_bytes_len,
    ) {
        return Err(InvocationStoreError::StoredContentMismatch("request"));
    }
    // EXACTLY ONE admitted Event for this Invocation, by IDENTITY -- not merely one that matches
    // the expected digest. A second Event naming this Invocation with different bytes is a second
    // answer to "what was admitted", and searching only for the digest we already hold would step
    // straight past it.
    if scoped_event_count(
        transaction,
        workspace_id,
        ADMITTED_EVENT_KIND,
        "$.invocation.id",
        invocation_id,
    )? != 1
    {
        return Err(InvocationStoreError::InvalidStore);
    }
    let event_sequence =
        paired_admission_event(transaction, workspace_id, &admission, &digest_value)?;
    if event_sequence != stored.creation_event_seq {
        return Err(InvocationStoreError::ColumnDisagreement(
            "creation_event_seq",
        ));
    }
    let history = load_history(transaction, workspace_id, &admission)?;
    // The same completeness question for observations, in the same snapshot: every retained
    // `invocation.observed` Event for THIS Invocation must have its row. A deleted row leaves a
    // history that C1's grammar still accepts -- a shorter legal word -- so the grammar cannot
    // stand in for this check.
    let observed_events = scoped_event_count(
        transaction,
        workspace_id,
        OBSERVED_EVENT_KIND,
        "$.observation.invocationId",
        invocation_id,
    )?;
    if observed_events != i64::try_from(history.len()).unwrap_or(i64::MAX) {
        return Err(InvocationStoreError::InvalidStore);
    }
    Ok(Some(VerifiedInvocation {
        admission,
        prompt: stored.prompt_inline,
        request: stored.request_inline,
        history,
        creation_event_seq: stored.creation_event_seq,
    }))
}

/// How many Events of one kind this Workspace retains for one Invocation.
///
/// `json_extract` on the identity only: the payload itself is never selected, so this bounds
/// nothing away and transfers nothing.
fn scoped_event_count(
    connection: &Transaction<'_>,
    workspace_id: Uuid,
    kind: &str,
    identity_path: &str,
    invocation_id: Uuid,
) -> Result<i64, InvocationStoreError> {
    Ok(connection.query_row(
        "SELECT count(*) FROM events
         WHERE workspace_id = ?1 AND kind = ?2
           AND json_extract(payload_json, ?3) = ?4",
        params![
            workspace_id.to_string(),
            kind,
            identity_path,
            invocation_id.to_string()
        ],
        |row| row.get(0),
    )?)
}

/// The rows the startup pass enumerates, in the order it enumerates them.
pub fn enumerate_invocations(connection: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "SELECT workspace_id, invocation_id FROM model_invocations
         ORDER BY workspace_id, invocation_id",
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------------------------
// Writers
//
// DEAD-CODE FRONTIER, declared narrowly and on purpose. `admit_invocation_within` and
// `append_observation_within` have no production caller yet: the admitted-work consumer is DP13's
// next slice, and the alternative -- a public constructor or a public Command minted only so a test
// can reach them -- would be a forged admission path, which this slice must not create. The
// `allow(dead_code)` is on exactly these two declarations, never on the module, and the writers are
// exercised by this module's own bars.
// ---------------------------------------------------------------------------------------------

/// Store one admitted Invocation, its exact prompt and request bytes, and its owning Event, in the
/// caller's one transaction.
///
/// Idempotence is adjudicated, not assumed: which side is wrong is decided before either is named.
#[allow(dead_code)]
pub(crate) fn admit_invocation_within(
    transaction: &Transaction<'_>,
    workspace_id: Uuid,
    admission: &InvocationAdmissionV1,
    prompt: &[u8],
    request: &[u8],
) -> Result<VerifiedInvocation, InvocationStoreError> {
    if admission.workspace_id != workspace_id {
        return Err(InvocationStoreError::ColumnDisagreement("workspace_id"));
    }
    require_envelope_within_ceiling(admission)?;
    let canonical_bytes = admission.canonical_bytes()?;
    if canonical_bytes.len() > MAX_ADMISSION_CANONICAL_BYTES {
        return Err(InvocationStoreError::OverStorageCeiling("canonical_bytes"));
    }
    let digest_value = HEXLOWER.encode(&sha256(&canonical_bytes));

    // Lookup by IDENTITY, not by digest, so "same Invocation, changed admission" stays its own
    // answer and no arm is made unreachable by the order of the checks.
    if let Some(stored) = load_stored(transaction, workspace_id, admission.invocation_id)? {
        if stored.canonical_bytes != canonical_bytes {
            return Err(InvocationStoreError::AdmissionConflict);
        }
        // The canonical bytes agree, so BOTH sides claim the same digest and length for each blob.
        // Which side is wrong is therefore decidable, and is decided before either is named.
        for (field, stored_bytes, offered_bytes, claimed_digest, claimed_len) in [
            (
                "prompt",
                &stored.prompt_inline,
                prompt,
                &admission.claimed_prompt_digest,
                admission.claimed_prompt_bytes_len,
            ),
            (
                "request",
                &stored.request_inline,
                request,
                &admission.claimed_request_digest,
                admission.claimed_request_bytes_len,
            ),
        ] {
            let stored_ok = content_matches(stored_bytes, claimed_digest, claimed_len);
            let offered_ok = content_matches(offered_bytes, claimed_digest, claimed_len);
            match (stored_ok, offered_ok) {
                (true, true) if stored_bytes.as_slice() != offered_bytes => {
                    return Err(InvocationStoreError::DigestCollision);
                }
                (true, true) => {}
                (true, false) => {
                    return Err(InvocationStoreError::OfferedContentMismatch(field));
                }
                // Store-side first when both fail: a caller cannot repair a broken row, and this is
                // the fault that must also stop startup.
                (false, _) => return Err(InvocationStoreError::StoredContentMismatch(field)),
            }
        }
        return verified_invocation_within(transaction, workspace_id, admission.invocation_id)?
            .ok_or(InvocationStoreError::InvalidStore);
    }

    // A NEW row: the offered bytes must be what this admission describes.
    if !content_matches(
        prompt,
        &admission.claimed_prompt_digest,
        admission.claimed_prompt_bytes_len,
    ) {
        return Err(InvocationStoreError::OfferedContentMismatch("prompt"));
    }
    if !content_matches(
        request,
        &admission.claimed_request_digest,
        admission.claimed_request_bytes_len,
    ) {
        return Err(InvocationStoreError::OfferedContentMismatch("request"));
    }

    let admitted_at_ms = u64::try_from(admission.admitted_at_ms)
        .map_err(|_| InvocationStoreError::Malformed(MalformedInvocation::NegativeTimestamp))?;
    let payload = InvocationAdmittedPayload {
        invocation: AdmittedEventRecord {
            id: admission.invocation_id,
            workspace_id,
            digest: admission_digest_payload(&digest_value),
            admitted_at_ms,
        },
    };
    let payload_json = serde_json::to_string(&payload)?;
    if i64::try_from(payload_json.len()).unwrap_or(i64::MAX) > MAX_EVENT_PAYLOAD_BYTES {
        return Err(InvocationStoreError::OverStorageCeiling("payload_json"));
    }
    let sequence = event_chain::append_event(
        transaction,
        &workspace_id.to_string(),
        ADMITTED_EVENT_KIND,
        &payload_json,
        admission.admitted_at_ms,
    )?;

    transaction.execute(
        "INSERT INTO model_invocations (
             workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
             invocation_id, project_id, mission_id, resident_id, admitted_configuration_id,
             activation_id, prompt_inline, request_inline, admitted_at_ms, creation_event_seq
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            workspace_id.to_string(),
            INVOCATION_DIGEST_ALGORITHM,
            ADMISSION_CANONICALIZER,
            digest_value,
            canonical_bytes,
            admission.invocation_id.to_string(),
            admission.project_id.to_string(),
            admission.mission_id.to_string(),
            admission.resident_id.to_string(),
            admission.admitted_configuration_id.to_string(),
            admission.activation_id.to_string(),
            prompt,
            request,
            admission.admitted_at_ms,
            sequence,
        ],
    )?;

    verified_invocation_within(transaction, workspace_id, admission.invocation_id)?
        .ok_or(InvocationStoreError::InvalidStore)
}

/// Append one observation, with its owning Event, in the caller's one transaction.
///
/// The stored history is rebuilt through C1 first, so only a legal next record is ever written.
///
/// `recorded_at_ms` is the caller's TRUSTED LOCAL recording time and is the Event's
/// `occurred_at_ms`. It is a parameter rather than a field read off the admission: reusing
/// `admitted_at_ms` would make every later observation's Event claim the instant the request was
/// admitted, so a header seen a minute later would be logged as having happened at admission. It is
/// also not a provider clock -- no observed remote time exists, and none is invented here.
#[allow(dead_code)]
pub(crate) fn append_observation_within(
    transaction: &Transaction<'_>,
    workspace_id: Uuid,
    record: InvocationObservationV1,
    recorded_at_ms: i64,
) -> Result<Appended, InvocationStoreError> {
    let existing = verified_invocation_within(transaction, workspace_id, record.invocation_id)?
        .ok_or(InvocationStoreError::InvalidStore)?;
    let mut history = existing.history;
    let canonical_bytes = record.canonical_bytes()?;
    if canonical_bytes.len() > MAX_OBSERVATION_CANONICAL_BYTES {
        return Err(InvocationStoreError::OverStorageCeiling("canonical_bytes"));
    }
    let digest_value = HEXLOWER.encode(&sha256(&canonical_bytes));
    let kind = record.kind();
    let sequence = record.sequence;

    // C1 decides. An exact re-offer writes no row and no Event.
    match history
        .append_record(record)
        .map_err(InvocationStoreError::Append)?
    {
        Appended::AlreadyPresent { sequence } => return Ok(Appended::AlreadyPresent { sequence }),
        Appended::Recorded { .. } => {}
    }

    let payload = InvocationObservedPayload {
        observation: ObservedEventRecord {
            invocation_id: existing.admission.invocation_id,
            workspace_id,
            sequence,
            kind: kind.as_str().to_owned(),
            digest: observation_digest_payload(&digest_value),
            recorded_at_ms: u64::try_from(recorded_at_ms).unwrap_or(u64::MAX),
        },
    };
    let payload_json = serde_json::to_string(&payload)?;
    if i64::try_from(payload_json.len()).unwrap_or(i64::MAX) > MAX_EVENT_PAYLOAD_BYTES {
        return Err(InvocationStoreError::OverStorageCeiling("payload_json"));
    }
    if recorded_at_ms < 0 {
        return Err(InvocationStoreError::Malformed(
            MalformedInvocation::NegativeTimestamp,
        ));
    }
    let event_sequence = event_chain::append_event(
        transaction,
        &workspace_id.to_string(),
        OBSERVED_EVENT_KIND,
        &payload_json,
        recorded_at_ms,
    )?;

    transaction.execute(
        "INSERT INTO model_invocation_observations (
             workspace_id, invocation_id, sequence, kind, digest_algorithm, digest_canonicalizer,
             digest_value, canonical_bytes, observed_event_seq
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            workspace_id.to_string(),
            existing.admission.invocation_id.to_string(),
            sequence,
            kind.as_str(),
            INVOCATION_DIGEST_ALGORITHM,
            OBSERVATION_CANONICALIZER,
            digest_value,
            canonical_bytes,
            event_sequence,
        ],
    )?;
    Ok(Appended::Recorded { sequence })
}

// ---------------------------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------------------------

fn count(connection: &Connection, sql: &str) -> Result<i64, rusqlite::Error> {
    connection.query_row(sql, [], |row| row.get(0))
}

/// Re-prove every stored Invocation, and account for the Events in both directions.
pub(crate) fn verify_invocation_state(
    transaction: &Transaction<'_>,
) -> Result<(), InvocationStateFault> {
    let rows = enumerate_invocations(transaction).map_err(InvocationStateFault::Store)?;

    if !rows.is_empty() {
        let owners = count(transaction, "SELECT count(*) FROM operator_accounts")
            .map_err(InvocationStateFault::Store)?;
        if owners == 0 {
            return Err(InvocationStateFault::Totals(crate::TotalsMismatch {
                table: "operator_accounts (an owner for the stored Invocations)",
                expected: 1,
                observed: 0,
            }));
        }
    }

    for (workspace_id, invocation_id) in &rows {
        let identity = || InvocationStateFault::Identity {
            workspace_id: workspace_id.clone(),
            invocation_id: invocation_id.clone(),
        };
        let workspace = Uuid::parse_str(workspace_id).map_err(|_| identity())?;
        let invocation = Uuid::parse_str(invocation_id).map_err(|_| identity())?;
        match verified_invocation_within(transaction, workspace, invocation) {
            // A row the enumeration saw and the reader cannot find is a contradiction, not absence.
            Ok(None) => {
                return Err(InvocationStateFault::Row {
                    workspace_id: workspace,
                    invocation_id: invocation,
                    cause: InvocationStoreError::InvalidStore,
                });
            }
            Ok(Some(_)) => {}
            Err(cause) => {
                return Err(InvocationStateFault::Row {
                    workspace_id: workspace,
                    invocation_id: invocation,
                    cause,
                });
            }
        }
    }

    // Both directions. Every row proved it has its Event above; these two catch an Event with no
    // row, which no per-row walk can see.
    for (kind, table, events_sql, rows_sql) in [
        (
            ADMITTED_EVENT_KIND,
            "model_invocations",
            "SELECT count(*) FROM events WHERE kind = 'invocation.admitted'",
            "SELECT count(*) FROM model_invocations",
        ),
        (
            OBSERVED_EVENT_KIND,
            "model_invocation_observations",
            "SELECT count(*) FROM events WHERE kind = 'invocation.observed'",
            "SELECT count(*) FROM model_invocation_observations",
        ),
    ] {
        let events = count(transaction, events_sql).map_err(InvocationStateFault::Store)?;
        let stored = count(transaction, rows_sql).map_err(InvocationStateFault::Store)?;
        if events != stored {
            let _ = kind;
            return Err(InvocationStateFault::Totals(crate::TotalsMismatch {
                table,
                expected: u64::try_from(events).unwrap_or(u64::MAX),
                observed: u64::try_from(stored).unwrap_or(u64::MAX),
            }));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The public read
// ---------------------------------------------------------------------------------------------

/// The authenticated, Workspace-scoped read over stored Invocations.
///
/// It returns a proved record and nothing else: no connection, no transaction, no path, no
/// authority, and no way to admit, observe or resend anything. There is no public constructor for a
/// stored record, so a caller cannot mint one.
pub struct InvocationStoreService {
    accounts: std::sync::Arc<crate::operator_account::OperatorAccountService>,
    workspace_id: Uuid,
}

impl InvocationStoreService {
    pub fn new(
        accounts: std::sync::Arc<crate::operator_account::OperatorAccountService>,
        workspace_id: Uuid,
    ) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// One stored Invocation, proved inside ONE snapshot this call owns.
    ///
    /// `Ok(None)` is a Workspace that admitted no such Invocation -- and a cross-Workspace id is the
    /// same answer, so scope stays opaque. A record that does not re-prove is an error, never
    /// absence.
    pub fn get(
        &self,
        invocation_id: Uuid,
    ) -> Result<Option<VerifiedInvocation>, InvocationStoreError> {
        // The inner refusal is carried OUT as a value rather than converted into an account error:
        // "this record does not re-prove" is not "you are not authenticated", and collapsing them
        // would lose which one happened.
        self.accounts.with_authenticated_store(|connection, _now| {
            let transaction =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
            match verified_invocation_within(&transaction, self.workspace_id, invocation_id) {
                Ok(found) => {
                    transaction.commit()?;
                    Ok(Ok(found))
                }
                Err(refusal) => {
                    Ok::<_, crate::operator_account::OperatorAccountError>(Err(refusal))
                }
            }
        })?
    }
}

#[cfg(test)]
mod tests {
    //! Real stores, real owning Commands, and raw SQL only to corrupt what an owner already minted.
    //!
    //! Every fixture is built through the product's own services, because the thing under test is a
    //! proof against real rows and a real Event chain. Expected values are held here; nothing
    //! asserts a field by reading it back out of the value under test.

    use std::fs;
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};

    use serde_json::json;

    use super::*;
    use crate::local_model_activation::LocalModelActivationService;
    use crate::mission::{MissionDraft, MissionMemberDraft, MissionService};
    use crate::model_invocation::{
        IncompleteCause, OwnerAuthorizationBasis, RetainedAuthority, RetainedBinding,
        RetainedHeader, RetainedLimits, TransportObservation, TransportStatus,
    };
    use crate::operator_account::OperatorAccountService;
    use crate::project::{AutonomyProfile, ProjectService};
    use crate::resident_configuration::ResidentConfigurationService;
    use crate::resident_lifecycle::ResidentLifecycleService;
    use crate::resource_discovery::LocalModelEndpoint;
    use crate::starter_organization::{
        StarterOrganizationService, StarterResidentDraft, StarterRole,
    };
    use crate::team::{TeamDraft, TeamMemberDraft, TeamService};
    use crate::{
        BeaconCapability, ProjectCharterDraft, ProjectDraft, ProviderFeature, SecretString,
        StartupConfig, Supervisor,
    };

    const MODEL_NAME: &str = "qwen3.8:27b";
    const MODEL_DIGEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

    const AUTHORITY_REFERENCE: &str = "beacon.invocation_authority.resolved.v1";
    const BINDING_REFERENCE: &str = "beacon.local_model_activation.binding.v1";
    const PROMPT_PROFILE: &str = "beacon.mission-plan.prompt.v1";
    const ADAPTER_PROFILE: &str = "beacon.model_invocation.adapter.bounded-json.v1";
    const PROMPT: &[u8] = b"Draft a plan for the admitted Mission.";
    const REQUEST: &[u8] = b"{\"prompt\":\"Draft a plan for the admitted Mission.\"}";
    const ADMITTED_AT_MS: i64 = 1_788_372_724_000;
    /// Two DIFFERENT instants, both later than the admission: an observation Event that reused the
    /// admission time, or one observation's time for another, is visible against these literals.
    const HEADERS_AT_MS: i64 = 1_788_372_725_500;
    const PREFIX_AT_MS: i64 = 1_788_372_726_750;

    struct TestStore {
        path: PathBuf,
        workspace_id: Uuid,
    }

    impl TestStore {
        fn new(label: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("beacond-c2-{label}-{}.sqlite3", Uuid::new_v4()));
            let report = Supervisor::bootstrap(StartupConfig {
                store_path: path.clone(),
                initial_workspace_name: "Personal".to_owned(),
            })
            .unwrap();
            Self {
                path,
                workspace_id: report.workspace.id,
            }
        }

        fn open(&self) -> Connection {
            Connection::open(&self.path).unwrap()
        }

        fn reopen(&self) -> Result<(), crate::SupervisorError> {
            Supervisor::bootstrap(StartupConfig {
                store_path: self.path.clone(),
                initial_workspace_name: "Personal".to_owned(),
            })
            .map(|_| ())
        }
    }

    impl Drop for TestStore {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = fs::remove_file(format!("{}{suffix}", self.path.display()));
            }
        }
    }

    fn enrolled_accounts(store: &TestStore) -> Arc<OperatorAccountService> {
        let accounts = Arc::new(OperatorAccountService::open(&store.path).unwrap());
        accounts
            .begin_password_setup(
                SecretString::new("a long local passphrase".to_owned()),
                SecretString::new("a long local passphrase".to_owned()),
            )
            .unwrap();
        accounts.complete_operator_setup(true).unwrap();
        accounts
    }

    fn read_http_request(stream: &mut impl Read) -> String {
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let size = stream.read(&mut chunk).unwrap();
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                return String::from_utf8(request).unwrap();
            }
        }
    }

    /// The model host's inspection endpoints, stood in locally. No model is called: the fixture
    /// cannot mint a Configuration without an activation, and an activation needs one inspection.
    fn serve_inspections(count: usize) -> (SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for _ in 0..count {
                for expected in ["GET /api/tags", "POST /api/show", "GET /api/tags"] {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_http_request(&mut stream);
                    assert!(
                        request.starts_with(expected),
                        "unexpected request: {request}"
                    );
                    let body = if expected.starts_with("POST") {
                        json!({ "capabilities": ["completion", "tools"] }).to_string()
                    } else {
                        json!({"models": [{
                            "name": MODEL_NAME, "model": MODEL_NAME,
                            "modified_at": "2026-08-14T14:38:20.13373148-04:00",
                            "size": 17_741_872_154_u64, "digest": MODEL_DIGEST,
                            "details": {"format": "gguf", "family": "qwen35",
                                "families": ["qwen35"], "parameter_size": "27.3B",
                                "quantization_level": "Q4_K_M"}
                        }]})
                        .to_string()
                    };
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .unwrap();
                }
            }
        });
        (address, handle)
    }

    /// A real Workspace with one admitted Mission, and an admission record that references it.
    struct Fixture {
        store: TestStore,
        accounts: Arc<OperatorAccountService>,
        admission: InvocationAdmissionV1,
    }

    fn fixture(label: &str) -> Fixture {
        let store = TestStore::new(label);
        let accounts = enrolled_accounts(&store);
        let project = ProjectService::new(Arc::clone(&accounts), store.workspace_id)
            .create(ProjectDraft {
                name: "Beacon Alpha".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Prove the local agent office.".to_owned(),
                    desired_outcomes: vec!["Complete one attributable Mission".to_owned()],
                    constraints: vec!["Keep authority explicit".to_owned()],
                    prohibitions: vec!["Do not deploy".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .unwrap();
        let residents = StarterOrganizationService::new(Arc::clone(&accounts), store.workspace_id)
            .accept(
                1,
                vec![StarterResidentDraft {
                    role: StarterRole::Lead,
                    display_name: None,
                }],
            )
            .unwrap()
            .residents;
        let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
        for resident in &residents {
            lifecycle.activate(resident.id, None).unwrap();
        }
        let (address, host) = serve_inspections(2);
        let models = LocalModelActivationService::new(
            Arc::clone(&accounts),
            store.workspace_id,
            LocalModelEndpoint::new(address).unwrap(),
        );
        let review = models.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
        let activation = models
            .activate(review, vec![ProviderFeature::Completion])
            .unwrap();
        host.join().unwrap();
        let configuration =
            ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id)
                .configure(
                    residents[0].id,
                    None,
                    activation.id,
                    vec![BeaconCapability::WorkCreateMission],
                )
                .unwrap();
        let team = TeamService::new(Arc::clone(&accounts), store.workspace_id)
            .form(TeamDraft {
                project_id: project.id,
                name: "Mission Team".to_owned(),
                members: vec![TeamMemberDraft {
                    resident_id: configuration.resident_id,
                    expected_configuration_id: configuration.id,
                }],
            })
            .unwrap();
        let mission = MissionService::new(Arc::clone(&accounts), store.workspace_id)
            .admit(MissionDraft {
                project_id: project.id,
                expected_charter_revision: project.charter.revision,
                team_id: team.id,
                desired_outcome: "Ship one reviewed Beacon feature.".to_owned(),
                success_conditions: vec!["Relevant gates pass".to_owned()],
                constraints: vec!["Keep the change reversible".to_owned()],
                prohibitions: vec!["Do not deploy".to_owned()],
                members: vec![MissionMemberDraft {
                    resident_id: configuration.resident_id,
                    expected_configuration_id: configuration.id,
                }],
            })
            .unwrap();

        let admission = InvocationAdmissionV1 {
            invocation_id: Uuid::new_v4(),
            workspace_id: store.workspace_id,
            project_id: project.id,
            mission_id: mission.id,
            resident_id: configuration.resident_id,
            admitted_configuration_id: configuration.id,
            activation_id: activation.id,
            authority: RetainedAuthority {
                reference: AUTHORITY_REFERENCE.to_owned(),
                claimed_digest: [0x11; 32],
            },
            binding: RetainedBinding {
                reference: BINDING_REFERENCE.to_owned(),
                claimed_digest: [0x33; 32],
            },
            prompt_profile: PROMPT_PROFILE.to_owned(),
            claimed_prompt_digest: sha256(PROMPT),
            claimed_prompt_bytes_len: u32::try_from(PROMPT.len()).unwrap(),
            adapter_profile: ADAPTER_PROFILE.to_owned(),
            claimed_request_digest: sha256(REQUEST),
            claimed_request_bytes_len: u32::try_from(REQUEST.len()).unwrap(),
            limits: RetainedLimits {
                request_bytes: 65_536,
                response_bytes: 262_144,
                prefix_bytes: 4_096,
            },
            owner_basis: OwnerAuthorizationBasis::Owner,
            admitted_at_ms: ADMITTED_AT_MS,
        };
        Fixture {
            store,
            accounts,
            admission,
        }
    }

    impl Fixture {
        /// Run one owner write inside the authenticated store, in one transaction.
        fn write<T>(
            &self,
            action: impl FnOnce(&Transaction<'_>) -> Result<T, InvocationStoreError>,
        ) -> Result<T, InvocationStoreError> {
            self.accounts
                .with_authenticated_store(|connection, _now| {
                    let transaction = connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                    match action(&transaction) {
                        Ok(value) => {
                            transaction.commit()?;
                            Ok(Ok(value))
                        }
                        Err(refusal) => {
                            Ok::<_, crate::operator_account::OperatorAccountError>(Err(refusal))
                        }
                    }
                })
                .unwrap()
        }

        fn admit(&self) -> Result<VerifiedInvocation, InvocationStoreError> {
            self.write(|transaction| {
                admit_invocation_within(
                    transaction,
                    self.store.workspace_id,
                    &self.admission,
                    PROMPT,
                    REQUEST,
                )
            })
        }

        fn get(&self) -> Result<Option<VerifiedInvocation>, InvocationStoreError> {
            InvocationStoreService::new(Arc::clone(&self.accounts), self.store.workspace_id)
                .get(self.admission.invocation_id)
        }
    }

    /// Corrupt through an immutability trigger the only way a test honestly can: drop it, write,
    /// and recreate it **byte-identically** to the migration's own text, so the schema signature
    /// startup verifies is unchanged. `PRAGMA writable_schema` was tried first and is wrong: it
    /// moves the signature, and startup then refuses with `SchemaMismatch` before the Invocation
    /// pass is ever reached -- a refusal that proves nothing about this owner.
    fn without_trigger(connection: &Connection, name: &str, create: &str, write: &str) {
        connection
            .execute_batch(&format!("DROP TRIGGER {name};"))
            .unwrap();
        connection.execute_batch(write).unwrap();
        connection.execute_batch(create).unwrap();
    }

    const NO_UPDATE_TRIGGER: &str = "CREATE TRIGGER model_invocations_no_update\nBEFORE UPDATE ON model_invocations\nBEGIN\n    SELECT RAISE(ABORT, 'Model Invocations are immutable');\nEND;";
    const NO_DELETE_TRIGGER: &str = "CREATE TRIGGER model_invocations_no_delete\nBEFORE DELETE ON model_invocations\nBEGIN\n    SELECT RAISE(ABORT, 'Model Invocations are immutable');\nEND;";

    // -----------------------------------------------------------------------------------------
    // Admission
    // -----------------------------------------------------------------------------------------

    #[test]
    fn a_stored_admission_reopens_with_its_exact_prompt_and_request_bytes() {
        let fixture = fixture("reopen");
        let stored = fixture.admit().expect("C2.ADMIT.STORES");
        assert_eq!(stored.prompt(), PROMPT);
        assert_eq!(stored.request(), REQUEST);

        fixture
            .store
            .reopen()
            .expect("C2.STARTUP.READY_AFTER_ADMIT");
        let reopened = fixture
            .get()
            .expect("C2.GET.VERIFIES")
            .expect("C2.GET.PRESENT");
        assert_eq!(reopened.prompt(), PROMPT, "C2.GET.EXACT_PROMPT_BYTES");
        assert_eq!(reopened.request(), REQUEST, "C2.GET.EXACT_REQUEST_BYTES");
        assert_eq!(
            reopened.admission().invocation_id,
            fixture.admission.invocation_id
        );
        assert_eq!(
            reopened.admission().authority.reference,
            AUTHORITY_REFERENCE
        );
        assert!(reopened.history().is_empty(), "C2.GET.NO_OBSERVATIONS_YET");
    }

    #[test]
    fn the_public_get_is_workspace_scoped_and_absent_is_not_broken() {
        let fixture = fixture("scope");
        fixture.admit().expect("C2.ADMIT.STORES");
        let missing =
            InvocationStoreService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
                .get(Uuid::new_v4())
                .expect("C2.GET.ABSENT_IS_OK");
        assert!(missing.is_none(), "C2.GET.ABSENT_IS_NONE");

        let foreign = InvocationStoreService::new(Arc::clone(&fixture.accounts), Uuid::new_v4())
            .get(fixture.admission.invocation_id)
            .expect("C2.GET.FOREIGN_IS_OK");
        assert!(foreign.is_none(), "C2.GET.FOREIGN_IS_ABSENT_NOT_BROKEN");
    }

    #[test]
    fn re_admitting_the_identical_admission_and_bytes_adds_no_row_and_no_event() {
        let fixture = fixture("idempotent");
        fixture.admit().expect("C2.ADMIT.STORES");
        let before = fixture.counts();
        fixture.admit().expect("C2.ADMIT.IDEMPOTENT");
        assert_eq!(
            fixture.counts(),
            before,
            "C2.ADMIT.IDEMPOTENT_WRITES_NOTHING"
        );
    }

    #[test]
    fn the_same_invocation_with_changed_canonical_admission_is_an_admission_conflict() {
        let fixture = fixture("conflict");
        fixture.admit().expect("C2.ADMIT.STORES");
        let mut moved = fixture.admission.clone();
        moved.adapter_profile = "beacon.model_invocation.adapter.bounded-json.v2".to_owned();
        let refusal = fixture
            .write(|transaction| {
                admit_invocation_within(
                    transaction,
                    fixture.store.workspace_id,
                    &moved,
                    PROMPT,
                    REQUEST,
                )
            })
            .expect_err("C2.ADMIT.ADMISSION_CONFLICT");
        assert_eq!(
            refusal.name(),
            "AdmissionConflict",
            "C2.ADMIT.ADMISSION_CONFLICT"
        );
    }

    #[test]
    fn offered_bytes_that_do_not_match_the_claim_never_blame_the_store() {
        let fixture = fixture("offered");
        fixture.admit().expect("C2.ADMIT.STORES");
        let refusal = fixture
            .write(|transaction| {
                admit_invocation_within(
                    transaction,
                    fixture.store.workspace_id,
                    &fixture.admission,
                    b"a different prompt",
                    REQUEST,
                )
            })
            .expect_err("C2.ADMIT.OFFERED_CONTENT_MISMATCH");
        assert_eq!(
            refusal.name(),
            "OfferedContentMismatch",
            "C2.ADMIT.OFFERED_IS_NOT_STORED_CORRUPTION: {refusal}"
        );
    }

    #[test]
    fn a_tampered_stored_blob_refuses_at_get_and_at_startup_as_stored_corruption() {
        let fixture = fixture("stored-corrupt");
        fixture.admit().expect("C2.ADMIT.STORES");
        // INTENTIONAL CORRUPTION, after the owner minted the row.
        // A DIFFERENT prompt of exactly the SAME length, held here independently. The previous
        // 21-byte string let this bar pass on its length check alone, so a mutant that dropped only
        // the SHA-256 comparison would have survived it -- the bar could not tell digest
        // verification from length verification (root's measurement).
        const SAME_LENGTH_PROMPT: &[u8] = b"Draft a PLAN for the admitted Mission.";
        assert_eq!(
            SAME_LENGTH_PROMPT.len(),
            PROMPT.len(),
            "C2.STORED_CORRUPTION.SAME_LENGTH"
        );
        assert_ne!(
            SAME_LENGTH_PROMPT, PROMPT,
            "C2.STORED_CORRUPTION.DIFFERENT_BYTES"
        );
        let connection = fixture.store.open();
        without_trigger(
            &connection,
            "model_invocations_no_update",
            NO_UPDATE_TRIGGER,
            "UPDATE model_invocations SET prompt_inline = CAST('Draft a PLAN for the admitted Mission.' AS BLOB);",
        );
        drop(connection);

        let refusal = fixture.get().expect_err("C2.GET.STORED_CONTENT_MISMATCH");
        assert_eq!(
            refusal.name(),
            "StoredContentMismatch",
            "C2.GET.STORED_CONTENT_MISMATCH: {refusal}"
        );
        let startup = fixture.store.reopen().expect_err("C2.STARTUP.REFUSES");
        // The WHOLE line, against the fixture's own held identities -- not a substring, and not an
        // expectation read back out of the refusal.
        assert_eq!(
            startup.to_string(),
            format!(
                "Invocation {} in Workspace {} does not re-prove: StoredContentMismatch",
                fixture.admission.invocation_id, fixture.store.workspace_id
            ),
            "C2.STARTUP.NAMES_THE_ROW_AND_THE_CAUSE"
        );
    }

    #[test]
    fn an_envelope_larger_than_the_storage_ceiling_is_refused_at_admission() {
        let fixture = fixture("envelope");
        let mut wide = fixture.admission.clone();
        wide.limits.response_bytes = u32::try_from(MAX_RETAINED_BLOB_BYTES).unwrap() + 1;
        let refusal = fixture
            .write(|transaction| {
                admit_invocation_within(
                    transaction,
                    fixture.store.workspace_id,
                    &wide,
                    PROMPT,
                    REQUEST,
                )
            })
            .expect_err("C2.ADMIT.ENVELOPE_WITHIN_CEILING");
        assert_eq!(
            refusal.name(),
            "EnvelopeExceedsStorageCeiling",
            "C2.ADMIT.ENVELOPE_WITHIN_CEILING: {refusal}"
        );
    }

    // -----------------------------------------------------------------------------------------
    // Observations
    // -----------------------------------------------------------------------------------------

    #[test]
    fn observations_persist_only_through_the_grammar_and_replay_on_reopen() {
        let fixture = fixture("observations");
        fixture.admit().expect("C2.ADMIT.STORES");
        let headers = InvocationObservationV1 {
            invocation_id: fixture.admission.invocation_id,
            sequence: 1,
            observation: TransportObservation::Headers {
                status: 200,
                headers: vec![RetainedHeader {
                    name: "content-type".to_owned(),
                    value: "application/json".to_owned(),
                }],
            },
        };
        fixture
            .write(|transaction| {
                append_observation_within(
                    transaction,
                    fixture.store.workspace_id,
                    headers.clone(),
                    HEADERS_AT_MS,
                )
            })
            .expect("C2.OBSERVE.HEADERS");

        // A prefix requires headers, which are present; a complete body must extend it.
        let prefix = InvocationObservationV1 {
            invocation_id: fixture.admission.invocation_id,
            sequence: 2,
            observation: TransportObservation::BodyPrefix {
                bytes: b"{\"plan\":".to_vec(),
            },
        };
        fixture
            .write(|transaction| {
                append_observation_within(
                    transaction,
                    fixture.store.workspace_id,
                    prefix,
                    PREFIX_AT_MS,
                )
            })
            .expect("C2.OBSERVE.PREFIX");

        let incoherent = InvocationObservationV1 {
            invocation_id: fixture.admission.invocation_id,
            sequence: 3,
            observation: TransportObservation::BodyComplete {
                bytes: b"a different response".to_vec(),
                elapsed_ms: 4,
            },
        };
        let refusal = fixture
            .write(|transaction| {
                append_observation_within(
                    transaction,
                    fixture.store.workspace_id,
                    incoherent,
                    PREFIX_AT_MS,
                )
            })
            .expect_err("C2.OBSERVE.GRAMMAR_OWNS_COHERENCE");
        assert_eq!(
            refusal.name(),
            "Append",
            "C2.OBSERVE.GRAMMAR_OWNS_COHERENCE"
        );

        // The exact re-offer of a stored record writes nothing.
        let before = fixture.counts();
        let again = fixture
            .write(|transaction| {
                append_observation_within(
                    transaction,
                    fixture.store.workspace_id,
                    headers,
                    HEADERS_AT_MS,
                )
            })
            .expect("C2.OBSERVE.IDEMPOTENT");
        assert!(
            matches!(again, Appended::AlreadyPresent { sequence: 1 }),
            "C2.OBSERVE.IDEMPOTENT"
        );
        assert_eq!(
            fixture.counts(),
            before,
            "C2.OBSERVE.IDEMPOTENT_WRITES_NOTHING"
        );

        fixture
            .store
            .reopen()
            .expect("C2.STARTUP.READY_WITH_HISTORY");
        let reopened = fixture.get().expect("C2.GET.VERIFIES").expect("present");
        assert_eq!(reopened.history().word(), "HP", "C2.GET.HISTORY_REPLAYS");
        assert_eq!(
            reopened.history().status(),
            TransportStatus::ResponsePartial {
                status: 200,
                prefix: b"{\"plan\":".to_vec(),
            },
            "C2.GET.HISTORY_STATUS"
        );
    }

    #[test]
    fn an_incomplete_terminal_before_any_headers_is_stored_and_replays() {
        let fixture = fixture("incomplete-first");
        fixture.admit().expect("C2.ADMIT.STORES");
        fixture
            .write(|transaction| {
                append_observation_within(
                    transaction,
                    fixture.store.workspace_id,
                    InvocationObservationV1 {
                        invocation_id: fixture.admission.invocation_id,
                        sequence: 1,
                        observation: TransportObservation::BodyIncomplete {
                            prefix: Vec::new(),
                            cause: IncompleteCause::TransportRefused,
                        },
                    },
                    HEADERS_AT_MS,
                )
            })
            .expect("C2.OBSERVE.TERMINAL_WITHOUT_HEADERS");
        fixture.store.reopen().expect("C2.STARTUP.READY");
        let reopened = fixture.get().expect("C2.GET.VERIFIES").expect("present");
        assert_eq!(
            reopened.history().status(),
            TransportStatus::Incomplete {
                status: None,
                cause: IncompleteCause::TransportRefused,
                prefix: Vec::new(),
            },
            "C2.GET.LOCAL_TERMINAL_IS_NOT_A_PROVIDER_STOP"
        );
    }

    #[test]
    fn each_observation_event_carries_its_own_recording_time_not_the_admissions() {
        let fixture = fixture("observation-clock");
        fixture.admit().expect("C2.ADMIT.STORES");
        for (sequence, observation, at_ms) in [
            (
                1_u32,
                TransportObservation::Headers {
                    status: 200,
                    headers: Vec::new(),
                },
                HEADERS_AT_MS,
            ),
            (
                2,
                TransportObservation::BodyPrefix {
                    bytes: b"{\"plan\":".to_vec(),
                },
                PREFIX_AT_MS,
            ),
        ] {
            fixture
                .write(|transaction| {
                    append_observation_within(
                        transaction,
                        fixture.store.workspace_id,
                        InvocationObservationV1 {
                            invocation_id: fixture.admission.invocation_id,
                            sequence,
                            observation,
                        },
                        at_ms,
                    )
                })
                .expect("C2.OBSERVE.STORES");
        }

        let connection = fixture.store.open();
        let mut statement = connection
            .prepare(
                "SELECT occurred_at_ms,
                        json_extract(payload_json, '$.observation.recordedAtMs')
                 FROM events WHERE kind = 'invocation.observed' ORDER BY sequence",
            )
            .unwrap();
        let times: Vec<(i64, i64)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        drop(statement);
        let admitted: i64 = connection
            .query_row(
                "SELECT occurred_at_ms FROM events WHERE kind = 'invocation.admitted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(connection);

        // Held literals, not values read back out of the record under test.
        assert_eq!(
            times,
            vec![(HEADERS_AT_MS, HEADERS_AT_MS), (PREFIX_AT_MS, PREFIX_AT_MS)],
            "C2.OBSERVE.EVENT_TIME_IS_ITS_OWN"
        );
        assert_eq!(
            admitted, ADMITTED_AT_MS,
            "C2.ADMIT.EVENT_TIME_IS_THE_ADMISSION"
        );
        assert!(
            times.iter().all(|(occurred, _)| *occurred != admitted),
            "C2.OBSERVE.EVENT_TIME_IS_NOT_THE_ADMISSIONS"
        );
        assert_ne!(
            times[0].0, times[1].0,
            "C2.OBSERVE.TWO_INSTANTS_ARE_DISTINCT"
        );

        // And the reader agrees: it compares the payload's recorded time to the Event's own.
        fixture.store.reopen().expect("C2.STARTUP.READY");
        assert!(fixture.get().expect("C2.GET.VERIFIES").is_some());
    }

    // -----------------------------------------------------------------------------------------
    // Events, both directions
    // -----------------------------------------------------------------------------------------

    #[test]
    fn a_removed_owner_event_refuses_at_get_and_at_startup() {
        let fixture = fixture("no-event");
        fixture.admit().expect("C2.ADMIT.STORES");
        // INTENTIONAL CORRUPTION after a real owner mint.
        let connection = fixture.store.open();
        assert_eq!(
            connection
                .execute("DELETE FROM events WHERE kind = 'invocation.admitted'", [],)
                .unwrap(),
            1
        );
        drop(connection);

        let refusal = fixture.get().expect_err("C2.GET.ROW_WITHOUT_EVENT");
        assert_eq!(refusal.name(), "InvalidStore", "C2.GET.ROW_WITHOUT_EVENT");
        assert!(
            fixture.store.reopen().is_err(),
            "C2.STARTUP.ROW_WITHOUT_EVENT"
        );
    }

    #[test]
    fn an_event_with_no_row_is_a_totals_mismatch_at_startup() {
        let fixture = fixture("event-without-row");
        fixture.admit().expect("C2.ADMIT.STORES");
        // INTENTIONAL CORRUPTION: the row goes, the Event stays. No per-row walk can see this.
        let connection = fixture.store.open();
        without_trigger(
            &connection,
            "model_invocations_no_delete",
            NO_DELETE_TRIGGER,
            "DELETE FROM model_invocations;",
        );
        drop(connection);

        let startup = fixture
            .store
            .reopen()
            .expect_err("C2.STARTUP.EVENT_WITHOUT_ROW");
        assert!(
            startup.to_string().contains("do not agree with the log"),
            "C2.STARTUP.EVENT_WITHOUT_ROW: {startup}"
        );
    }

    #[test]
    fn a_row_whose_invocation_id_is_not_a_uuid_refuses_through_the_closed_identity_root() {
        let fixture = fixture("bad-identity");
        fixture.admit().expect("C2.ADMIT.STORES");
        // INTENTIONAL CORRUPTION after a real owner mint: a SECOND row whose `invocation_id` is 36
        // characters -- so the column CHECK admits it -- and not a UUID. Inserting is the reachable
        // shape: the immutability triggers guard UPDATE and DELETE, not INSERT.
        let connection = fixture.store.open();
        let inserted = connection
            .execute(
                "INSERT INTO model_invocations
                 SELECT workspace_id, digest_algorithm, digest_canonicalizer,
                        'ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff', canonical_bytes,
                        'not-a-uuid-but-exactly-36-characters', project_id, mission_id,
                        resident_id, admitted_configuration_id, activation_id, prompt_inline,
                        request_inline, admitted_at_ms, creation_event_seq
                 FROM model_invocations",
                [],
            )
            .unwrap();
        assert_eq!(inserted, 1, "C2.IDENTITY.FIXTURE_ROW_INSERTED");
        drop(connection);

        let startup = fixture
            .store
            .reopen()
            .expect_err("C2.STARTUP.UNREADABLE_IDENTITY");
        let line = startup.to_string();
        assert_eq!(
            line, "an Invocation row's identity is not readable: StoredIdentityUnreadable",
            "C2.IDENTITY.CLOSED_LABEL_ONLY"
        );
        // The raw store text survives in Debug, and never in the one-line projection.
        let debug = format!("{startup:?}");
        assert!(
            debug.contains("not-a-uuid-but-exactly-36-characters"),
            "C2.IDENTITY.RAW_SURVIVES_IN_DEBUG: {debug}"
        );
        assert!(
            !line.contains("not-a-uuid-but-exactly-36-characters"),
            "C2.IDENTITY.RAW_NEVER_IN_THE_LINE: {line}"
        );
    }

    // -----------------------------------------------------------------------------------------
    // The ceilings
    // -----------------------------------------------------------------------------------------

    #[test]
    fn the_widest_admission_record_fits_the_proposed_canonical_ceiling() {
        let widest = InvocationAdmissionV1 {
            authority: RetainedAuthority {
                reference: "r".repeat(crate::model_invocation::MAX_AUTHORITY_REFERENCE_BYTES),
                claimed_digest: [0xff; 32],
            },
            binding: RetainedBinding {
                reference: "b".repeat(crate::model_invocation::MAX_BINDING_REFERENCE_BYTES),
                claimed_digest: [0xff; 32],
            },
            prompt_profile: "p".repeat(crate::model_invocation::MAX_PROMPT_PROFILE_BYTES),
            adapter_profile: "a".repeat(crate::model_invocation::MAX_ADAPTER_PROFILE_BYTES),
            ..fixture_shaped_admission()
        };
        let bytes = widest.canonical_bytes().expect("the widest record encodes");
        assert!(
            bytes.len() <= MAX_ADMISSION_CANONICAL_BYTES,
            "C2.CEILING.WIDEST_ADMISSION_FITS: {} > {MAX_ADMISSION_CANONICAL_BYTES}",
            bytes.len()
        );
        // And the migration's own floor admits it.
        assert!(
            bytes.len() >= 300,
            "C2.CEILING.MIGRATION_FLOOR: {}",
            bytes.len()
        );
    }

    /// A record with no store behind it, for pure framing measurement.
    fn fixture_shaped_admission() -> InvocationAdmissionV1 {
        InvocationAdmissionV1 {
            invocation_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            mission_id: Uuid::new_v4(),
            resident_id: Uuid::new_v4(),
            admitted_configuration_id: Uuid::new_v4(),
            activation_id: Uuid::new_v4(),
            authority: RetainedAuthority {
                reference: AUTHORITY_REFERENCE.to_owned(),
                claimed_digest: [0x11; 32],
            },
            binding: RetainedBinding {
                reference: BINDING_REFERENCE.to_owned(),
                claimed_digest: [0x33; 32],
            },
            prompt_profile: PROMPT_PROFILE.to_owned(),
            claimed_prompt_digest: sha256(PROMPT),
            claimed_prompt_bytes_len: u32::try_from(PROMPT.len()).unwrap(),
            adapter_profile: ADAPTER_PROFILE.to_owned(),
            claimed_request_digest: sha256(REQUEST),
            claimed_request_bytes_len: u32::try_from(REQUEST.len()).unwrap(),
            limits: RetainedLimits {
                request_bytes: 65_536,
                response_bytes: 262_144,
                prefix_bytes: 4_096,
            },
            owner_basis: OwnerAuthorizationBasis::Owner,
            admitted_at_ms: ADMITTED_AT_MS,
        }
    }

    #[test]
    fn an_event_payload_over_the_byte_ceiling_refuses_before_it_is_transferred() {
        let fixture = fixture("payload-ceiling");
        fixture.admit().expect("C2.ADMIT.STORES");
        // INTENTIONAL CORRUPTION: multi-byte scalars, so the CHARACTER count stays under the
        // ceiling while the UTF-8 BYTE count passes it. A character-counting guard would let this
        // through, which is why the expression casts to BLOB.
        let connection = fixture.store.open();
        let padding = "é".repeat(2_400);
        connection
            .execute(
                "UPDATE events SET payload_json = json_set(payload_json, '$.padding', ?1)
                 WHERE kind = 'invocation.admitted'",
                params![padding],
            )
            .unwrap();
        let (characters, bytes): (i64, i64) = connection
            .query_row(
                "SELECT length(payload_json), length(CAST(payload_json AS BLOB)) FROM events
                 WHERE kind = 'invocation.admitted'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        drop(connection);
        assert!(
            characters <= MAX_EVENT_PAYLOAD_BYTES && bytes > MAX_EVENT_PAYLOAD_BYTES,
            "C2.CEILING.BYTES_NOT_CHARACTERS premise: {characters} chars, {bytes} bytes"
        );

        let refusal = fixture
            .get()
            .expect_err("C2.GET.EVENT_PAYLOAD_OVER_CEILING");
        assert_eq!(
            refusal.name(),
            "OverStorageCeiling",
            "C2.GET.EVENT_PAYLOAD_OVER_CEILING: {refusal}"
        );
        let startup = fixture
            .store
            .reopen()
            .expect_err("C2.STARTUP.EVENT_PAYLOAD_OVER_CEILING");
        assert!(
            startup
                .to_string()
                .contains("past the Invocation store's ceiling"),
            "C2.STARTUP.GENERIC_ENUMERATION_GUARDS_IT: {startup}"
        );
    }

    // ---------------------------------------------------------------------------------------------
    // R42 outward-projection witness for the PUBLIC `InvocationStoreError` (opus2, non-author).
    //
    // Ready to insert into `model_invocation_store.rs`'s own `#[cfg(test)] mod tests`. No product API
    // is added for it, no dependency, no crate-wide change.
    //
    // Every expected token below is a LITERAL written here. None is read from `name()`: an expectation
    // derived from the implementation cannot fail when the implementation is what moved.
    // ---------------------------------------------------------------------------------------------

    /// A label an operator never chose, carrying CR/LF. A fault code that forwards this can forge a
    /// log line, which is why the newline assertion is separate from the needle assertion.
    const O2_STATIC_NEEDLE: &str = "o2-needle-static\r\nFORGED-LOG-LINE";
    const O2_JSON_NEEDLE: &str = "o2-needle-json";
    const O2_DRIVER_NEEDLE: &str = "o2-needle-driver";

    fn o2_driver_error() -> rusqlite::Error {
        // Driver detail an operator never authored. `InvalidParameterName` carries its argument
        // verbatim into `rusqlite::Error`'s own Display.
        rusqlite::Error::InvalidParameterName(O2_DRIVER_NEEDLE.to_owned())
    }

    fn o2_json_error() -> serde_json::Error {
        // serde echoes the offending input into its message: `invalid type: string "o2-needle-json"`.
        serde_json::from_str::<i32>(&format!("\"{O2_JSON_NEEDLE}\"")).unwrap_err()
    }

    /// (independent literal token, value, needle that must NOT reach Display)
    fn o2_projection_table() -> Vec<(&'static str, InvocationStoreError, Option<&'static str>)> {
        vec![
            (
                "Account",
                InvocationStoreError::Account(
                    crate::operator_account::OperatorAccountError::Store(o2_driver_error()),
                ),
                Some(O2_DRIVER_NEEDLE),
            ),
            (
                "Store",
                InvocationStoreError::Store(o2_driver_error()),
                Some(O2_DRIVER_NEEDLE),
            ),
            (
                "Json",
                InvocationStoreError::Json(o2_json_error()),
                Some(O2_JSON_NEEDLE),
            ),
            // Closed child, no arbitrary payload: named rather than given an invented needle.
            (
                "Malformed",
                InvocationStoreError::Malformed(MalformedInvocation::UnknownObservationTag),
                None,
            ),
            // Closed child with structured fields only (digests, kinds, integers): no free text.
            (
                "Append",
                InvocationStoreError::Append(AppendRefusal::Transition(
                    crate::model_invocation::TransitionRefusal::PrefixWithoutHeaders,
                )),
                None,
            ),
            (
                "ColumnDisagreement",
                InvocationStoreError::ColumnDisagreement(O2_STATIC_NEEDLE),
                Some(O2_STATIC_NEEDLE),
            ),
            (
                "OfferedContentMismatch",
                InvocationStoreError::OfferedContentMismatch(O2_STATIC_NEEDLE),
                Some(O2_STATIC_NEEDLE),
            ),
            (
                "StoredContentMismatch",
                InvocationStoreError::StoredContentMismatch(O2_STATIC_NEEDLE),
                Some(O2_STATIC_NEEDLE),
            ),
            (
                "AdmissionConflict",
                InvocationStoreError::AdmissionConflict,
                None,
            ),
            (
                "DigestCollision",
                InvocationStoreError::DigestCollision,
                None,
            ),
            (
                "EnvelopeExceedsStorageCeiling",
                InvocationStoreError::EnvelopeExceedsStorageCeiling(O2_STATIC_NEEDLE),
                Some(O2_STATIC_NEEDLE),
            ),
            (
                "OverStorageCeiling",
                InvocationStoreError::OverStorageCeiling(O2_STATIC_NEEDLE),
                Some(O2_STATIC_NEEDLE),
            ),
            ("InvalidStore", InvocationStoreError::InvalidStore, None),
        ]
    }

    #[test]
    fn o2_the_projection_table_covers_every_arm_exhaustively() {
        // The compiler owns this count. Adding an arm without adding a row fails to build here, which
        // is the only check that cannot rot.
        fn exhaustive(value: &InvocationStoreError) -> &'static str {
            match value {
                InvocationStoreError::Account(_) => "Account",
                InvocationStoreError::Store(_) => "Store",
                InvocationStoreError::Json(_) => "Json",
                InvocationStoreError::Malformed(_) => "Malformed",
                InvocationStoreError::Append(_) => "Append",
                InvocationStoreError::ColumnDisagreement(_) => "ColumnDisagreement",
                InvocationStoreError::OfferedContentMismatch(_) => "OfferedContentMismatch",
                InvocationStoreError::StoredContentMismatch(_) => "StoredContentMismatch",
                InvocationStoreError::AdmissionConflict => "AdmissionConflict",
                InvocationStoreError::DigestCollision => "DigestCollision",
                InvocationStoreError::EnvelopeExceedsStorageCeiling(_) => {
                    "EnvelopeExceedsStorageCeiling"
                }
                InvocationStoreError::OverStorageCeiling(_) => "OverStorageCeiling",
                InvocationStoreError::InvalidStore => "InvalidStore",
            }
        }
        let table = o2_projection_table();
        assert_eq!(table.len(), 13, "C2.PUBLIC_ERROR.TABLE_CARDINALITY");
        let mut seen: Vec<&'static str> = table.iter().map(|(token, _, _)| *token).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 13, "C2.PUBLIC_ERROR.TOKENS_ARE_DISTINCT");
        for (token, value, _) in &table {
            assert_eq!(
                exhaustive(value),
                *token,
                "C2.PUBLIC_ERROR.ARM_MATCHES_ITS_LITERAL_TOKEN"
            );
        }
    }

    #[test]
    fn o2_the_outward_projection_is_exactly_the_arm_name() {
        for (token, value, needle) in o2_projection_table() {
            let shown = value.to_string();
            // Needle first, then exactness. Both must hold; asserting the needle first means a
            // forwarded payload is diagnosed as a forwarded payload rather than as a length mismatch.
            if let Some(needle) = needle {
                assert!(
                    !shown.contains(needle),
                    "C2.PUBLIC_ERROR.NO_PAYLOAD_OUTWARD: {token} forwarded {needle:?} in {shown:?}"
                );
            }
            assert!(
                !shown.contains('\n') && !shown.contains('\r'),
                "C2.PUBLIC_ERROR.OUTWARD_IS_ONE_LINE: {token} produced {shown:?}"
            );
            // The outward line is the NAME and nothing else -- byte for byte. A suffix, a separator or
            // a forwarded nested Display dies here even when it happens to carry no needle of ours,
            // which `starts_with` would have let through.
            assert_eq!(
                shown, token,
                "C2.PUBLIC_ERROR.OUTWARD_EXACT: {token} projected {shown:?}"
            );
        }
    }

    #[test]
    fn o2_the_operator_side_debug_retains_what_the_outward_projection_drops() {
        // Suppressing a payload outward is only acceptable if the operator's own record keeps it.
        for (token, value, needle) in o2_projection_table() {
            let Some(needle) = needle else { continue };
            let debug = format!("{value:?}");
            // Derived `Debug` ESCAPES control characters, so the raw CR/LF marker can never appear
            // literally and searching for it would be a false RED. The escaped form is what retention
            // means here, and raw CR/LF in Debug is NOT required -- it would be the same forging
            // hazard the outward assertion refuses.
            let retained = needle.escape_debug().to_string();
            assert!(
                debug.contains(&retained),
                "C2.PUBLIC_ERROR.DEBUG_RETAINS_DIAGNOSTIC: {token} lost {retained:?} from {debug:?}"
            );
            // The ASCII half of the marker is independently attributable and must survive whole, so a
            // truncating Debug cannot pass by keeping a prefix.
            assert!(
                debug.contains("FORGED-LOG-LINE") || !needle.contains("FORGED-LOG-LINE"),
                "C2.PUBLIC_ERROR.DEBUG_RETAINS_WHOLE_MARKER: {token} truncated {debug:?}"
            );
        }
    }

    // Astra-authored probes, added verbatim as committed regressions.
    #[test]
    fn astra_c2_get_refuses_an_admission_event_without_its_row() {
        let fixture = fixture("astra-admission-orphan");
        fixture.admit().expect("C2.GET.ORPHAN_FIXTURE_ADMITTED");
        let connection = fixture.store.open();
        without_trigger(
            &connection,
            "model_invocations_no_delete",
            NO_DELETE_TRIGGER,
            "DELETE FROM model_invocations;",
        );
        let retained: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind = 'invocation.admitted'",
                [],
                |row| row.get(0),
            )
            .expect("C2.GET.ORPHAN_FIXTURE_EVENT_QUERY");
        assert_eq!(retained, 1, "C2.GET.ORPHAN_FIXTURE_EVENT_RETAINED");
        drop(connection);
        let refusal = fixture
            .get()
            .expect_err("C2.GET.ADMISSION_EVENT_WITHOUT_ROW");
        assert!(
            matches!(refusal, InvocationStoreError::InvalidStore),
            "C2.GET.ADMISSION_EVENT_WITHOUT_ROW: {refusal:?}"
        );
    }

    #[test]
    fn astra_c2_get_refuses_an_observation_event_without_its_row() {
        let fixture = fixture("astra-observation-orphan");
        fixture.admit().expect("C2.GET.ORPHAN_FIXTURE_ADMITTED");
        fixture
            .write(|transaction| {
                append_observation_within(
                    transaction,
                    fixture.store.workspace_id,
                    InvocationObservationV1 {
                        invocation_id: fixture.admission.invocation_id,
                        sequence: 1,
                        observation: TransportObservation::BodyIncomplete {
                            prefix: Vec::new(),
                            cause: IncompleteCause::TransportRefused,
                        },
                    },
                    HEADERS_AT_MS,
                )
            })
            .expect("C2.GET.ORPHAN_FIXTURE_OBSERVED");
        let before = fixture
            .get()
            .expect("C2.GET.ORPHAN_CONTROL_VERIFIED")
            .expect("C2.GET.ORPHAN_CONTROL_PRESENT");
        assert_eq!(
            before.history().status(),
            TransportStatus::Incomplete {
                status: None,
                cause: IncompleteCause::TransportRefused,
                prefix: Vec::new(),
            },
            "C2.GET.ORPHAN_CONTROL_TERMINAL"
        );
        let connection = fixture.store.open();
        without_trigger(
            &connection,
            "model_invocation_observations_no_delete",
            "CREATE TRIGGER model_invocation_observations_no_delete\nBEFORE DELETE ON model_invocation_observations\nBEGIN\n    SELECT RAISE(ABORT, 'Model Invocation observations are immutable');\nEND;",
            "DELETE FROM model_invocation_observations;",
        );
        let retained: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind = 'invocation.observed'",
                [],
                |row| row.get(0),
            )
            .expect("C2.GET.ORPHAN_FIXTURE_OBSERVATION_EVENT_QUERY");
        assert_eq!(
            retained, 1,
            "C2.GET.ORPHAN_FIXTURE_OBSERVATION_EVENT_RETAINED"
        );
        drop(connection);
        let refusal = fixture
            .get()
            .expect_err("C2.GET.OBSERVATION_EVENT_WITHOUT_ROW");
        assert!(
            matches!(refusal, InvocationStoreError::InvalidStore),
            "C2.GET.OBSERVATION_EVENT_WITHOUT_ROW: {refusal:?}"
        );
    }

    // ---------------------------------------------------------------------------------------------
    // Populated 36 -> 37 migration regression (opus2, non-author). C2 brief requirement 6.
    //
    // Ready to insert into `model_invocation_store.rs`'s own `#[cfg(test)] mod tests`. It reuses the
    // existing `TestStore`/`fixture` owners and adds no public planting API: every retained row was
    // minted by a real owner (Account, Workspace, activation, Resident, configuration, Team, Mission)
    // BEFORE `admit()`, so both Invocation tables are empty and the rebuild is the only thing under
    // test.
    //
    // The schema oracle proves SHAPE. This proves the DATA survives the rebuild.
    // ---------------------------------------------------------------------------------------------

    /// The old `events` definition and its two dependent triggers, taken from an in-memory store built
    /// by the OLD corpus. Independent of the migration under test by construction: nothing here reads
    /// `MIGRATION_V37`.
    fn o2_schema_36_objects() -> (String, Vec<(String, String)>) {
        let memory = Connection::open_in_memory().unwrap();
        crate::apply_migrations_through(&memory, 36).unwrap();
        let events: String = memory
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut statement = memory
            .prepare(
                "SELECT name, sql FROM sqlite_schema WHERE type = 'trigger'
                   AND name IN ('team_formation_members_no_insert_after_formation',
                                'mission_admission_members_no_insert_after_admission')
                 ORDER BY name",
            )
            .unwrap();
        let triggers: Vec<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            triggers.len(),
            2,
            "C2.MIGRATION.OLD_DEPENDENT_TRIGGERS_FOUND"
        );
        (events, triggers)
    }

    /// Every schema object, as this test sees it. Written here rather than reusing the product's own
    /// signature helper, so "the store is at 36" is an independent statement.
    fn o2_signature(connection: &Connection) -> Vec<(String, String, String)> {
        let mut statement = connection
            .prepare(
                "SELECT type, name, COALESCE(sql, '') FROM sqlite_schema
                 WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY type, name",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    /// One `events` row, exactly as the rebuild must carry it across. Named because the tuple is
    /// eight columns wide and clippy is right that an unnamed one is unreadable; the shape is
    /// Opus2's, unchanged.
    type O2EventRow = (
        i64,
        String,
        String,
        String,
        i64,
        String,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    );

    fn o2_events_rows(connection: &Connection) -> Vec<O2EventRow> {
        let mut statement = connection
            .prepare(
                "SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms,
                        predecessor_kind, predecessor_digest, link_digest
                 FROM events ORDER BY sequence",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn o2_chain_state(connection: &Connection) -> (i64, String, Option<i64>, Option<Vec<u8>>, i64) {
        connection
            .query_row(
                "SELECT id, genesis_kind, migrated_through_sequence, head_digest, head_sequence
                 FROM event_chain_state WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap()
    }

    fn o2_team_members(connection: &Connection) -> Vec<(String, String, i64, String, String)> {
        let mut s = connection
            .prepare(
                "SELECT workspace_id, team_id, position, resident_id, configuration_id
                 FROM team_formation_members ORDER BY workspace_id, team_id, position",
            )
            .unwrap();
        s.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    fn o2_mission_members(
        connection: &Connection,
    ) -> Vec<(String, String, String, i64, String, String)> {
        let mut s = connection
            .prepare(
                "SELECT workspace_id, mission_id, team_id, position, resident_id, configuration_id
                 FROM mission_admission_members ORDER BY workspace_id, mission_id, position",
            )
            .unwrap();
        s.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    /// Roll ONLY 0037 back in this disposable owner-minted store, so the next bootstrap performs the
    /// real 36 -> 37 ladder step over populated data.
    fn o2_roll_back_to_36(
        connection: &Connection,
        events_sql: &str,
        triggers: &[(String, String)],
    ) {
        let enabled_before: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            enabled_before, 1,
            "C2.MIGRATION.FOREIGN_KEYS_ON_BEFORE_ROLLBACK"
        );
        connection
            .execute_batch("PRAGMA foreign_keys = OFF;")
            .unwrap();
        connection
            .execute_batch(
                "DROP TABLE IF EXISTS model_invocation_observations;
                 DROP TABLE IF EXISTS model_invocations;
                 DROP TRIGGER IF EXISTS team_formation_members_no_insert_after_formation;
                 DROP TRIGGER IF EXISTS mission_admission_members_no_insert_after_admission;
                 ALTER TABLE events RENAME TO events_37;",
            )
            .unwrap();
        connection.execute_batch(events_sql).unwrap();
        // All eight columns, explicitly. A `SELECT *` here would silently reorder if either definition
        // ever changes, and this test exists to notice exactly that class of change.
        connection
            .execute_batch(
                "INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms,
                                     predecessor_kind, predecessor_digest, link_digest)
                   SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms,
                          predecessor_kind, predecessor_digest, link_digest
                   FROM events_37 ORDER BY sequence;
                 DROP TABLE events_37;",
            )
            .unwrap();
        for (_, sql) in triggers {
            connection.execute_batch(sql).unwrap();
        }
        connection
            .execute(
                "UPDATE schema_meta SET version = 36 WHERE singleton = 1",
                [],
            )
            .unwrap();
        // Enforcement is a SEPARATE property from `pragma_foreign_key_check` finding no violations:
        // one says the data is consistent, the other says the connection would refuse making it
        // inconsistent. Restored and read back, never assumed.
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        let enabled_after: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(enabled_after, 1, "C2.MIGRATION.FOREIGN_KEYS_RESTORED_ON");
    }

    /// A trigger refusal, pinned by REASON. `is_err()` alone is a false witness here:
    /// `team_formation_members` carries `UNIQUE (workspace_id, team_id, resident_id)`, so an insert
    /// that reuses an existing member is refused by that UNIQUE whether or not the closing trigger
    /// exists. Only the extended code and the trigger's own literal message separate the two.
    fn o2_assert_trigger_refusal(
        result: rusqlite::Result<usize>,
        expected_message: &str,
        label: &str,
    ) {
        match result {
            Ok(rows) => panic!("{label}: the insert was accepted, {rows} row(s)"),
            Err(rusqlite::Error::SqliteFailure(error, Some(message))) => {
                assert_eq!(
                    error.extended_code,
                    rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER,
                    "{label}: refused with extended code {} and message {message:?}, not by the trigger",
                    error.extended_code
                );
                assert_eq!(message, expected_message, "{label}: wrong trigger spoke");
            }
            Err(other) => panic!("{label}: refused by {other:?}, not by a trigger"),
        }
    }

    #[test]
    fn o2_a_populated_store_survives_the_thirty_six_to_thirty_seven_rebuild() {
        let fixture = fixture("c2-populated-migration");
        // Deliberately BEFORE admit(): both Invocation tables are empty, so nothing this migration
        // creates is being carried, and only pre-existing data is under test.
        let connection = fixture.store.open();
        let empty_invocations: i64 = connection
            .query_row("SELECT count(*) FROM model_invocations", [], |r| r.get(0))
            .unwrap();
        let empty_observations: i64 = connection
            .query_row(
                "SELECT count(*) FROM model_invocation_observations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            (empty_invocations, empty_observations),
            (0, 0),
            "C2.MIGRATION.INVOCATION_TABLES_EMPTY"
        );

        let (events_sql, triggers) = o2_schema_36_objects();
        o2_roll_back_to_36(&connection, &events_sql, &triggers);

        // The store really is a 36 store, not a 37 store with a mis-stamped version row.
        // The product's OWN check. `verify_schema` is private in `lib.rs`, and a descendant module of
        // the crate root can call it -- no visibility widening and no product API for a test.
        crate::verify_schema(&connection, 36).expect("C2.MIGRATION.PRODUCT_VERIFIES_36");
        // Kept beside it: an independently built expectation, so this does not rest on one helper.
        let memory = Connection::open_in_memory().unwrap();
        crate::apply_migrations_through(&memory, 36).unwrap();
        assert_eq!(
            o2_signature(&connection),
            o2_signature(&memory),
            "C2.MIGRATION.ROLLED_BACK_STORE_IS_STRUCTURALLY_36"
        );
        let violations: i64 = connection
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            violations, 0,
            "C2.MIGRATION.NO_FOREIGN_KEY_VIOLATIONS_BEFORE"
        );

        // Held independently, before the migration under test runs.
        let events_before = o2_events_rows(&connection);
        let chain_before = o2_chain_state(&connection);
        let teams_before = o2_team_members(&connection);
        let missions_before = o2_mission_members(&connection);
        assert!(!events_before.is_empty(), "C2.MIGRATION.EVENTS_NONEMPTY");
        assert!(
            !teams_before.is_empty(),
            "C2.MIGRATION.TEAM_MEMBERS_NONEMPTY"
        );
        assert!(
            !missions_before.is_empty(),
            "C2.MIGRATION.MISSION_MEMBERS_NONEMPTY"
        );
        let triggers_before: Vec<(String, String)> = triggers.clone();
        drop(connection);

        // The real ladder step, through the real startup. The Result is HELD, not unwrapped: startup
        // commits the migration before it verifies, so a copy that mutated a column can be read and
        // named here even when verification then refuses Ready. Unwrapping first would let a chain or
        // owner refusal mask the copy assertion this packet advertises.
        let ready = fixture.store.reopen();
        let after = fixture.store.open();
        // The copied data first, so the mutant this packet targets names the column it mutated.
        assert_eq!(
            o2_events_rows(&after),
            events_before,
            "C2.MIGRATION.EVENT_ROWS_PRESERVED"
        );
        // Then, and only then, that the real startup reached a Ready 37.
        ready.expect("C2.MIGRATION.BOOTSTRAP_REACHES_37");
        let version: i64 = after
            .query_row(
                "SELECT version FROM schema_meta WHERE singleton = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 37, "C2.MIGRATION.STAMPED_37");
        assert_eq!(
            o2_chain_state(&after),
            chain_before,
            "C2.MIGRATION.CHAIN_STATE_PRESERVED"
        );
        assert_eq!(
            o2_team_members(&after),
            teams_before,
            "C2.MIGRATION.TEAM_MEMBERS_PRESERVED"
        );
        assert_eq!(
            o2_mission_members(&after),
            missions_before,
            "C2.MIGRATION.MISSION_MEMBERS_PRESERVED"
        );
        for (name, sql) in &triggers_before {
            let now: String = after
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| {
                    panic!("C2.MIGRATION.DEPENDENT_TRIGGER_RESTORED: {name} is gone")
                });
            assert_eq!(
                &now, sql,
                "C2.MIGRATION.DEPENDENT_TRIGGER_BODY_UNCHANGED: {name}"
            );
        }

        // The restored triggers still REFUSE, and refuse FOR THE RIGHT REASON, which a name-and-body
        // comparison cannot show and `is_err()` cannot distinguish.
        //
        // LIMITATION, stated rather than papered over: this fixture has one Resident and one
        // configuration, so a genuinely NEW valid member cannot be prepared without minting Residents
        // that no owner admitted -- which would be inventing a product fact. The insert below therefore
        // reuses the existing member and WOULD ALSO violate
        // `UNIQUE (workspace_id, team_id, resident_id)`. That is exactly why the assertion pins
        // `SQLITE_CONSTRAINT_TRIGGER` and the trigger's own literal message: a UNIQUE refusal reports
        // `SQLITE_CONSTRAINT_UNIQUE` and a different message, and fails here. **No claim is made that
        // the input excluded duplicate or FK errors** -- the reason pin is what excludes them.
        let closed_team = &teams_before[0];
        o2_assert_trigger_refusal(
            after.execute(
                "INSERT INTO team_formation_members (workspace_id, team_id, position, resident_id,
                                                     configuration_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    closed_team.0,
                    closed_team.1,
                    closed_team.2 + 1_000,
                    closed_team.3,
                    closed_team.4
                ],
            ),
            "Team formation roster is closed",
            "C2.MIGRATION.CLOSED_TEAM_STILL_REFUSES_A_MEMBER",
        );
        let closed_mission = &missions_before[0];
        o2_assert_trigger_refusal(
            after.execute(
                "INSERT INTO mission_admission_members (workspace_id, mission_id, team_id, position,
                                                        resident_id, configuration_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    closed_mission.0,
                    closed_mission.1,
                    closed_mission.2,
                    closed_mission.3 + 1_000,
                    closed_mission.4,
                    closed_mission.5
                ],
            ),
            "Mission admission roster is closed",
            "C2.MIGRATION.CLOSED_MISSION_STILL_REFUSES_A_MEMBER",
        );
    }

    #[test]
    fn o2_reopening_the_migrated_store_adds_nothing() {
        let fixture = fixture("c2-populated-idempotent");
        let connection = fixture.store.open();
        let (events_sql, triggers) = o2_schema_36_objects();
        o2_roll_back_to_36(&connection, &events_sql, &triggers);
        drop(connection);

        fixture
            .store
            .reopen()
            .expect("C2.MIGRATION.FIRST_BOOTSTRAP");
        let first = fixture.store.open();
        let events_once = o2_events_rows(&first);
        let chain_once = o2_chain_state(&first);
        let invocations_once: i64 = first
            .query_row("SELECT count(*) FROM model_invocations", [], |r| r.get(0))
            .unwrap();
        let observations_once: i64 = first
            .query_row(
                "SELECT count(*) FROM model_invocation_observations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(first);

        // A SECOND real startup, not a second execution of the bare 0037 script.
        fixture
            .store
            .reopen()
            .expect("C2.MIGRATION.SECOND_BOOTSTRAP");
        let second = fixture.store.open();
        assert_eq!(
            o2_events_rows(&second),
            events_once,
            "C2.MIGRATION.REOPEN_ADDS_NO_EVENT"
        );
        assert_eq!(
            o2_chain_state(&second),
            chain_once,
            "C2.MIGRATION.REOPEN_KEEPS_CHAIN_STATE"
        );
        let invocations_twice: i64 = second
            .query_row("SELECT count(*) FROM model_invocations", [], |r| r.get(0))
            .unwrap();
        let observations_twice: i64 = second
            .query_row(
                "SELECT count(*) FROM model_invocation_observations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            invocations_twice, invocations_once,
            "C2.MIGRATION.REOPEN_ADDS_NO_INVOCATION"
        );
        assert_eq!(
            observations_twice, observations_once,
            "C2.MIGRATION.REOPEN_ADDS_NO_OBSERVATION"
        );
    }

    // Astra-authored completeness corners, verbatim.
    #[test]
    fn astra_c2_get_refuses_a_second_admission_event_for_the_same_invocation() {
        let fixture = fixture("astra-duplicate-admission-event");
        fixture.admit().expect("C2.GET.DUPLICATE_FIXTURE_ADMITTED");
        fixture
            .write(|transaction| {
                let raw: String = transaction.query_row(
                    "SELECT payload_json FROM events WHERE kind = 'invocation.admitted'",
                    [],
                    |row| row.get(0),
                )?;
                let mut payload: serde_json::Value = serde_json::from_str(&raw)?;
                // Intentional corruption: another well-formed Event names this same Invocation but a
                // DIFFERENT digest, so looking up only the row's digest cannot see it.
                payload["invocation"]["digest"]["value"] = serde_json::json!("0".repeat(64));
                let raw = serde_json::to_string(&payload)?;
                event_chain::append_event(
                    transaction,
                    &fixture.store.workspace_id.to_string(),
                    ADMITTED_EVENT_KIND,
                    &raw,
                    ADMITTED_AT_MS,
                )?;
                Ok(())
            })
            .expect("C2.GET.DUPLICATE_FIXTURE_EVENT_APPENDED");
        let refusal = fixture.get().expect_err("C2.GET.SECOND_ADMISSION_EVENT");
        assert!(
            matches!(refusal, InvocationStoreError::InvalidStore),
            "C2.GET.SECOND_ADMISSION_EVENT: {refusal:?}"
        );
    }

    #[test]
    fn astra_c2_get_refuses_retained_observation_evidence_when_admission_is_gone() {
        let fixture = fixture("astra-only-observation-event");
        fixture.admit().expect("C2.GET.ORPHAN_FIXTURE_ADMITTED");
        fixture
            .write(|transaction| {
                append_observation_within(
                    transaction,
                    fixture.store.workspace_id,
                    InvocationObservationV1 {
                        invocation_id: fixture.admission.invocation_id,
                        sequence: 1,
                        observation: TransportObservation::BodyIncomplete {
                            prefix: Vec::new(),
                            cause: IncompleteCause::TransportRefused,
                        },
                    },
                    HEADERS_AT_MS,
                )
            })
            .expect("C2.GET.ORPHAN_FIXTURE_OBSERVED");
        let connection = fixture.store.open();
        // Intentional corruption, newest row first; its authentic observation Event remains.
        without_trigger(
            &connection,
            "model_invocation_observations_no_delete",
            "CREATE TRIGGER model_invocation_observations_no_delete\nBEFORE DELETE ON model_invocation_observations\nBEGIN\n    SELECT RAISE(ABORT, 'Model Invocation observations are immutable');\nEND;",
            "DELETE FROM model_invocation_observations;",
        );
        without_trigger(
            &connection,
            "model_invocations_no_delete",
            NO_DELETE_TRIGGER,
            "DELETE FROM model_invocations;",
        );
        assert_eq!(
            connection
                .execute("DELETE FROM events WHERE kind = 'invocation.admitted'", [],)
                .expect("C2.GET.ORPHAN_FIXTURE_ADMISSION_EVENT_REMOVED"),
            1
        );
        let retained: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind = 'invocation.observed'",
                [],
                |row| row.get(0),
            )
            .expect("C2.GET.ORPHAN_FIXTURE_OBSERVATION_EVENT_QUERY");
        assert_eq!(
            retained, 1,
            "C2.GET.ORPHAN_FIXTURE_OBSERVATION_EVENT_RETAINED"
        );
        drop(connection);
        let refusal = fixture
            .get()
            .expect_err("C2.GET.RETAINED_OBSERVATION_IS_NOT_ABSENCE");
        assert!(
            matches!(refusal, InvocationStoreError::InvalidStore),
            "C2.GET.RETAINED_OBSERVATION_IS_NOT_ABSENCE: {refusal:?}"
        );
    }

    impl Fixture {
        /// Row and Event counts, for the idempotence bars.
        fn counts(&self) -> (i64, i64, i64, i64) {
            let connection = self.store.open();
            let one = |sql: &str| {
                connection
                    .query_row(sql, [], |row| row.get::<_, i64>(0))
                    .unwrap()
            };
            (
                one("SELECT count(*) FROM model_invocations"),
                one("SELECT count(*) FROM model_invocation_observations"),
                one("SELECT count(*) FROM events WHERE kind = 'invocation.admitted'"),
                one("SELECT count(*) FROM events WHERE kind = 'invocation.observed'"),
            )
        }
    }
}
