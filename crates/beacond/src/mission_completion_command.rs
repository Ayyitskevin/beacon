//! The Operator's explicit Mission completion: preview, record, and recover its consistency token.
//!
//! **OWNER AND DTO ONLY.** There is no transport, no native command and no frontend in this slice,
//! and that is deliberate rather than unfinished: root measured a complete request at **66348 JSON
//! bytes** against the existing **64 KiB** frame (`c4-transport-measure/`), so the owner can hold a
//! record today's transport cannot carry. Widening the frame or dropping fields would both hide
//! that. A later explicit bounded transport slice connects these owners; until then **nothing here
//! is reachable from the desktop.**
//!
//! Nothing in this module verifies a citation. Delivered Artifacts, Receipts, Evaluations and
//! Rulings are stored **as cited**, exactly as the writer already treats them, and no count here is
//! a claim that anything was read. `concluded` is a disposition, not a success.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use data_encoding::HEXLOWER;
use rusqlite::TransactionBehavior;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mission_completion::{
    CompletionActor, CompletionDisposition, CompletionSupersedes, ConditionOutcome,
    ConditionResultRecord, MissionCompletionV1, NarrativeItem, SpendReference, SupportRef,
    UncertaintyKind, UnfinishedKind,
};
use crate::mission_completion_store::{
    CompletionReadRefusal, MissionCompletionReadError, declared_conditions_digest,
    record_completion_within, verified_full_completion_for_mission_within,
};
use crate::operator_account::{OperatorAccountError, OperatorAccountService};
use crate::proposal::sha256;

/// The one-NUL-terminated ASCII prefix the reviewed-intent token is computed over.
///
/// A token is not a record identity: prefixing the canonical bytes means an intent digest can never
/// be mistaken for, or collide with, a completion's own content digest over the same bytes.
pub const REVIEWED_INTENT_PREFIX: &[u8] = b"beacon.mission_completion.reviewed_intent.v1\0";

// ---------------------------------------------------------------------------------------------
// The request DTO. Strict: unknown fields, unknown tags and out-of-domain values all refuse.
// ---------------------------------------------------------------------------------------------

/// One support reference, externally tagged as the C4 contract specifies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum SupportRefDto {
    Artifact(String),
    Receipt {
        run_id: Uuid,
        receipt_version: u32,
        receipt_bytes_sha256: String,
    },
    Evaluation(String),
    Ruling(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConditionResultDto {
    pub ordinal: u32,
    pub outcome: String,
    pub support: Vec<SupportRefDto>,
    /// Explicit `null`, never an absent key: an omitted note and an empty note are different facts.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NarrativeItemDto {
    pub ordinal: u32,
    pub kind: String,
    /// The declared condition this relates to, or explicit `null`.
    pub condition_ordinal: Option<u32>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum SpendDto {
    Recorded {
        spend_id: Uuid,
        spend_version: u32,
        spend_sha256: String,
    },
    NotMeasured(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionIntentRequest {
    pub mission_id: Uuid,
    /// Chosen by the Operator from the Mission's HISTORICAL admission roster. Never auto-selected,
    /// and never defaulted to a sole member.
    pub accountable_resident_id: Uuid,
    pub disposition: String,
    pub condition_results: Vec<ConditionResultDto>,
    pub delivered_artifacts: Vec<String>,
    pub unfinished_work: Vec<NarrativeItemDto>,
    pub uncertainty: Vec<NarrativeItemDto>,
    pub spend: SpendDto,
    pub summary: String,
}

/// What the Operator reviews: the owner's normalized request, its bindings, and the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionPreview {
    /// The owner's normalization of what was sent, so the review screen shows what will be
    /// submitted rather than what was typed.
    pub request: CompletionIntentRequest,
    pub intent_sha256: String,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub actor: String,
    /// The configuration on the SAME admission roster entry as the chosen Resident. Historical, and
    /// never that Resident's current configuration.
    pub accountable_resident_configuration_id: Uuid,
    pub declared_conditions_sha256: String,
    pub conditions: Vec<String>,
}

/// The token, alone. `record` and `get_intent` both answer with exactly this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewedIntent {
    pub intent_sha256: String,
}

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

/// Why a completion command refused.
///
/// The outward `Display` is ONE independent literal per arm. Driver text, paths and anything an
/// Operator typed stay in `Debug` and `source`, which is where R42 puts them.
#[derive(Debug)]
pub enum CompletionCommandError {
    Account(OperatorAccountError),
    /// The reviewed token does not match what this request now derives.
    IntentChanged,
    /// A completion is already recorded for this Mission. Nothing is overwritten.
    AlreadyRecorded,
    /// The chosen Resident is not on this Mission's admission roster.
    ResidentNotInAdmissionRoster,
    /// The condition results are not exactly the declared set, once each.
    ConditionCoverageIncomplete,
    /// A set carries more entries than a completion can record.
    SupportOverLimit,
    /// Some text exceeds the codec's own bound.
    TextOverLimit,
    /// `notMeasured` without a reason.
    SpendReasonRequired,
    /// A malformed identity, digest, tag or integer in the request.
    MalformedRequest(&'static str),
    /// A set contains the same entry twice.
    DuplicateSetEntry(&'static str),
    /// The Mission, or the stored completion, could not be verified.
    Unavailable(CompletionCommandUnavailable),
}

/// What actually went wrong. Never projected outward.
#[derive(Debug)]
pub enum CompletionCommandUnavailable {
    Store(rusqlite::Error),
    Read(MissionCompletionReadError),
    Write(CompletionReadRefusal),
    /// The Mission is not admitted in this Workspace. Indistinguishable outwardly from a Mission a
    /// caller may not see.
    MissionAbsent,
    /// The record the writer just stored does not read back as what was written.
    WriteReadBackDisagrees,
}

impl CompletionCommandError {
    /// The closed outward token.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::IntentChanged => "IntentChanged",
            Self::AlreadyRecorded => "AlreadyRecorded",
            Self::ResidentNotInAdmissionRoster => "ResidentNotInAdmissionRoster",
            Self::ConditionCoverageIncomplete => "ConditionCoverageIncomplete",
            Self::SupportOverLimit => "SupportOverLimit",
            Self::TextOverLimit => "TextOverLimit",
            Self::SpendReasonRequired => "SpendReasonRequired",
            Self::MalformedRequest(_) => "MalformedRequest",
            Self::DuplicateSetEntry(_) => "DuplicateSetEntry",
            Self::Unavailable(_) => "Unavailable",
        }
    }
}

impl fmt::Display for CompletionCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl std::error::Error for CompletionCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Account(error) => Some(error),
            _ => None,
        }
    }
}

impl From<OperatorAccountError> for CompletionCommandError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for CompletionCommandError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Unavailable(CompletionCommandUnavailable::Store(error))
    }
}

fn malformed(field: &'static str) -> CompletionCommandError {
    CompletionCommandError::MalformedRequest(field)
}

// ---------------------------------------------------------------------------------------------
// The token
// ---------------------------------------------------------------------------------------------

/// The reviewed-intent token of one derived record.
///
/// The three owner-assigned fields are cleared **on a temporary clone** and the EXISTING
/// `canonical_bytes` encoder does the rest: no subset encoder, no second grammar. The normalized
/// object is never stored, never returned as a completion, and never accepted as a real record.
///
/// It is a consistency token. It is not a record identity, an authorization, or evidence that any
/// screen was looked at.
pub(crate) fn reviewed_intent_token(
    record: &MissionCompletionV1,
) -> Result<String, CompletionCommandError> {
    let mut normalized = record.clone();
    normalized.completion_id = Uuid::nil();
    normalized.boundary_event_seq = 0;
    normalized.completed_at_ms = 0;
    let bytes = normalized
        .canonical_bytes()
        .map_err(|_| malformed("canonical_bytes"))?;
    let mut input = REVIEWED_INTENT_PREFIX.to_vec();
    input.extend_from_slice(&bytes);
    Ok(HEXLOWER.encode(&sha256(&input)))
}

// ---------------------------------------------------------------------------------------------
// DTO -> record
// ---------------------------------------------------------------------------------------------

fn digest32(value: &str, field: &'static str) -> Result<[u8; 32], CompletionCommandError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(malformed(field));
    }
    if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(malformed(field));
    }
    let raw = HEXLOWER.decode(value.as_bytes()).map_err(|_| malformed(field))?;
    let mut out = [0_u8; 32];
    out.copy_from_slice(&raw);
    Ok(out)
}

fn outcome_from(token: &str) -> Result<ConditionOutcome, CompletionCommandError> {
    match token {
        "met" => Ok(ConditionOutcome::Met),
        "not_met" => Ok(ConditionOutcome::NotMet),
        "not_assessed" => Ok(ConditionOutcome::NotAssessed),
        "assessment_inconclusive" => Ok(ConditionOutcome::AssessmentInconclusive),
        _ => Err(malformed("outcome")),
    }
}

fn disposition_from(token: &str) -> Result<CompletionDisposition, CompletionCommandError> {
    match token {
        "concluded" => Ok(CompletionDisposition::Concluded),
        "abandoned" => Ok(CompletionDisposition::Abandoned),
        _ => Err(malformed("disposition")),
    }
}

fn unfinished_from(token: &str) -> Result<UnfinishedKind, CompletionCommandError> {
    match token {
        "condition_not_met" => Ok(UnfinishedKind::ConditionNotMet),
        "deferred_scope" => Ok(UnfinishedKind::DeferredScope),
        "known_defect" => Ok(UnfinishedKind::KnownDefect),
        "blocked_dependency" => Ok(UnfinishedKind::BlockedDependency),
        _ => Err(malformed("unfinishedWork.kind")),
    }
}

fn uncertainty_from(token: &str) -> Result<UncertaintyKind, CompletionCommandError> {
    match token {
        "unverified_claim" => Ok(UncertaintyKind::UnverifiedClaim),
        "unmeasured_quantity" => Ok(UncertaintyKind::UnmeasuredQuantity),
        "conflicting_evidence" => Ok(UncertaintyKind::ConflictingEvidence),
        "outside_scope_of_checks" => Ok(UncertaintyKind::OutsideScopeOfChecks),
        _ => Err(malformed("uncertainty.kind")),
    }
}

fn support_from(dto: &SupportRefDto) -> Result<SupportRef, CompletionCommandError> {
    Ok(match dto {
        SupportRefDto::Artifact(digest) => SupportRef::Artifact(digest32(digest, "support.artifact")?),
        SupportRefDto::Evaluation(digest) => {
            SupportRef::Evaluation(digest32(digest, "support.evaluation")?)
        }
        SupportRefDto::Ruling(digest) => SupportRef::Ruling(digest32(digest, "support.ruling")?),
        SupportRefDto::Receipt {
            run_id,
            receipt_version,
            receipt_bytes_sha256,
        } => SupportRef::Receipt {
            run_id: *run_id,
            receipt_version: *receipt_version,
            receipt_bytes_sha256: digest32(receipt_bytes_sha256, "support.receipt")?,
        },
    })
}

/// Sort one support SET into the codec's own total order and refuse a repeat.
///
/// The key is the codec's `(kind, order_key)`, not a second spelling of it: a permutation of the
/// same support must not be able to produce a second identity for one completion.
fn canonical_support(
    mut support: Vec<SupportRef>,
) -> Result<Vec<SupportRef>, CompletionCommandError> {
    support.sort_by(|left, right| {
        (left.kind(), left.order_key()).cmp(&(right.kind(), right.order_key()))
    });
    for window in support.windows(2) {
        let left = (window[0].kind(), window[0].order_key());
        let right = (window[1].kind(), window[1].order_key());
        if left.cmp(&right) != Ordering::Less {
            return Err(CompletionCommandError::DuplicateSetEntry("support"));
        }
    }
    Ok(support)
}

fn canonical_artifacts(
    mut digests: Vec<[u8; 32]>,
) -> Result<Vec<[u8; 32]>, CompletionCommandError> {
    digests.sort();
    for window in digests.windows(2) {
        if window[0].cmp(&window[1]) != Ordering::Less {
            return Err(CompletionCommandError::DuplicateSetEntry("deliveredArtifacts"));
        }
    }
    Ok(digests)
}

fn narrative_from<K>(
    items: &[NarrativeItemDto],
    parse: impl Fn(&str) -> Result<K, CompletionCommandError>,
) -> Result<Vec<NarrativeItem<K>>, CompletionCommandError> {
    items
        .iter()
        .map(|item| {
            Ok(NarrativeItem {
                ordinal: item.ordinal,
                kind: parse(&item.kind)?,
                condition_ordinal: item.condition_ordinal,
                summary: item.summary.clone(),
            })
        })
        .collect()
}

/// The owner's normalization of one request, echoed back so the review screen shows what will be
/// submitted. Only the two SETS move; every sequence and every character is preserved.
fn normalized_request(
    request: &CompletionIntentRequest,
    record: &MissionCompletionV1,
) -> Result<CompletionIntentRequest, CompletionCommandError> {
    let mut condition_results = Vec::with_capacity(record.condition_results.len());
    for (result, source) in record.condition_results.iter().zip(&request.condition_results) {
        let mut support = Vec::with_capacity(result.support.len());
        for reference in &result.support {
            support.push(match reference {
                SupportRef::Artifact(digest) => SupportRefDto::Artifact(HEXLOWER.encode(digest)),
                SupportRef::Evaluation(digest) => {
                    SupportRefDto::Evaluation(HEXLOWER.encode(digest))
                }
                SupportRef::Ruling(digest) => SupportRefDto::Ruling(HEXLOWER.encode(digest)),
                SupportRef::Receipt {
                    run_id,
                    receipt_version,
                    receipt_bytes_sha256,
                } => SupportRefDto::Receipt {
                    run_id: *run_id,
                    receipt_version: *receipt_version,
                    receipt_bytes_sha256: HEXLOWER.encode(receipt_bytes_sha256),
                },
            });
        }
        let _ = source;
        condition_results.push(ConditionResultDto {
            ordinal: result.ordinal,
            outcome: crate::mission_completion_store::outcome_token(result.outcome).to_owned(),
            support,
            note: result.note.clone(),
        });
    }
    Ok(CompletionIntentRequest {
        mission_id: record.mission_id,
        accountable_resident_id: record.accountable_resident_id,
        disposition: request.disposition.clone(),
        condition_results,
        delivered_artifacts: record
            .delivered_artifacts
            .iter()
            .map(|digest| HEXLOWER.encode(digest))
            .collect(),
        unfinished_work: request.unfinished_work.clone(),
        uncertainty: request.uncertainty.clone(),
        spend: request.spend.clone(),
        summary: request.summary.clone(),
    })
}

// ---------------------------------------------------------------------------------------------
// Derivation
// ---------------------------------------------------------------------------------------------

/// What the owner assigns, versus what the Operator chose.
struct OwnerBindings {
    workspace_id: Uuid,
    project_id: Uuid,
    accountable_resident_configuration_id: Uuid,
    declared_conditions: Vec<String>,
    declared_conditions_sha256: [u8; 32],
}

/// Derive the immutable bindings from the Mission's OWN verified admission.
///
/// The Project, the configuration and the declared-conditions digest all come from the verified
/// record, never from the request: a caller chooses a Mission and a Resident and nothing else.
fn owner_bindings(
    transaction: &rusqlite::Transaction<'_>,
    workspace_id: Uuid,
    request: &CompletionIntentRequest,
) -> Result<OwnerBindings, CompletionCommandError> {
    let mission =
        crate::mission::verified_mission_admission(transaction, workspace_id, request.mission_id)
            .map_err(|error| {
                CompletionCommandError::Unavailable(CompletionCommandUnavailable::Read(
                    MissionCompletionReadError::Unavailable(
                        crate::mission_completion_store::CompletionUnavailable::MissionUnverified(
                            error,
                        ),
                    ),
                ))
            })?
            .ok_or(CompletionCommandError::Unavailable(
                CompletionCommandUnavailable::MissionAbsent,
            ))?;
    // The chosen Resident must be ON the admission roster, and its configuration is that roster
    // entry's -- the historical binding, which a later reconfiguration or suspension cannot move.
    let member = mission
        .members
        .iter()
        .find(|member| member.resident_id == request.accountable_resident_id)
        .ok_or(CompletionCommandError::ResidentNotInAdmissionRoster)?;
    Ok(OwnerBindings {
        workspace_id,
        project_id: mission.project_id,
        accountable_resident_configuration_id: member.admission_configuration_id,
        declared_conditions_sha256: declared_conditions_digest(&mission.success_conditions),
        declared_conditions: mission.success_conditions,
    })
}

/// Assemble the full record the write would store.
///
/// `completion_id`, `boundary_event_seq` and `completed_at_ms` are the caller's to pass in because
/// preview and record supply different values for them: preview passes the sentinels the token is
/// computed over, and record passes the real ones it assigned in its own transaction.
fn derive_record(
    bindings: &OwnerBindings,
    request: &CompletionIntentRequest,
    completion_id: Uuid,
    boundary_event_seq: u64,
    completed_at_ms: u64,
) -> Result<MissionCompletionV1, CompletionCommandError> {
    // Exactly the declared set, once each. No inferred outcome, and no default: an Operator who
    // has not assessed a condition has not completed the Mission.
    if request.condition_results.len() != bindings.declared_conditions.len() {
        return Err(CompletionCommandError::ConditionCoverageIncomplete);
    }
    let mut seen = vec![false; bindings.declared_conditions.len()];
    for result in &request.condition_results {
        let index = usize::try_from(result.ordinal)
            .map_err(|_| malformed("conditionResults.ordinal"))?;
        let slot = seen
            .get_mut(index)
            .ok_or(CompletionCommandError::ConditionCoverageIncomplete)?;
        if *slot {
            return Err(CompletionCommandError::ConditionCoverageIncomplete);
        }
        *slot = true;
    }

    let mut condition_results = Vec::with_capacity(request.condition_results.len());
    for result in &request.condition_results {
        if result.support.len() > crate::mission_completion::MAX_SUPPORT_PER_CONDITION {
            return Err(CompletionCommandError::SupportOverLimit);
        }
        let support = result
            .support
            .iter()
            .map(support_from)
            .collect::<Result<Vec<_>, _>>()?;
        condition_results.push(ConditionResultRecord {
            ordinal: result.ordinal,
            outcome: outcome_from(&result.outcome)?,
            support: canonical_support(support)?,
            note: result.note.clone(),
        });
    }
    // The codec keys conditions by ordinal, so the stored sequence is the declaration order.
    condition_results.sort_by_key(|result| result.ordinal);

    let delivered_artifacts = canonical_artifacts(
        request
            .delivered_artifacts
            .iter()
            .map(|digest| digest32(digest, "deliveredArtifacts"))
            .collect::<Result<Vec<_>, _>>()?,
    )?;

    let spend = match &request.spend {
        SpendDto::NotMeasured(reason) if reason.is_empty() => {
            return Err(CompletionCommandError::SpendReasonRequired);
        }
        SpendDto::NotMeasured(reason) => SpendReference::NotMeasured(reason.clone()),
        SpendDto::Recorded {
            spend_id,
            spend_version,
            spend_sha256,
        } => SpendReference::Recorded {
            spend_id: *spend_id,
            spend_version: *spend_version,
            spend_sha256: digest32(spend_sha256, "spend.spendSha256")?,
        },
    };

    let record = MissionCompletionV1 {
        workspace_id: bindings.workspace_id,
        project_id: bindings.project_id,
        mission_id: request.mission_id,
        completion_id,
        actor: CompletionActor::Operator,
        accountable_resident_id: request.accountable_resident_id,
        accountable_resident_configuration_id: bindings.accountable_resident_configuration_id,
        boundary_event_seq,
        disposition: disposition_from(&request.disposition)?,
        declared_conditions_sha256: bindings.declared_conditions_sha256,
        condition_results,
        delivered_artifacts,
        unfinished_work: narrative_from(&request.unfinished_work, |token| {
            unfinished_from(token)
        })?,
        uncertainty: narrative_from(&request.uncertainty, |token| uncertainty_from(token))?,
        spend,
        supersedes: CompletionSupersedes::None,
        summary: request.summary.clone(),
    };
    // The codec's own grammar is the validator: every bound, every text class and the envelope
    // ceiling are its rules, and a second copy here would be a second thing to keep true.
    record
        .canonical_bytes()
        .map_err(|_| CompletionCommandError::TextOverLimit)?;
    Ok(record)
}

// ---------------------------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------------------------

/// The authenticated Operator completion owner.
///
/// The Workspace comes from this service, never from client JSON. No method verifies a citation,
/// starts a provider, prepares, publishes or classifies anything.
pub struct MissionCompletionCommandService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl MissionCompletionCommandService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Derive the whole intended record and its token, and write **nothing**.
    ///
    /// One Deferred snapshot. The token is computed over the sentinel-normalized record, so it is
    /// identical to the one `record` will compare against even though identity, time and boundary
    /// are not assigned yet.
    pub fn preview(
        &self,
        request: CompletionIntentRequest,
    ) -> Result<CompletionPreview, CompletionCommandError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let outcome = (|| {
                let bindings = owner_bindings(&transaction, self.workspace_id, &request)?;
                // Sentinels, not guesses: the same three fields the token clears.
                let record = derive_record(&bindings, &request, Uuid::nil(), 0, 0)?;
                Ok(CompletionPreview {
                    request: normalized_request(&request, &record)?,
                    intent_sha256: reviewed_intent_token(&record)?,
                    workspace_id: bindings.workspace_id,
                    project_id: bindings.project_id,
                    actor: "operator".to_owned(),
                    accountable_resident_configuration_id: bindings
                        .accountable_resident_configuration_id,
                    declared_conditions_sha256: HEXLOWER
                        .encode(&bindings.declared_conditions_sha256),
                    conditions: bindings.declared_conditions,
                })
            })();
            match outcome {
                Ok(preview) => {
                    transaction.commit()?;
                    Ok(Ok(preview))
                }
                Err(refusal) => Ok::<_, OperatorAccountError>(Err(refusal)),
            }
        })?
    }

    /// Record the reviewed completion, atomically, in ONE Immediate transaction.
    ///
    /// Everything is recomputed before any insert: the bindings, the current boundary, the identity
    /// and time, and the token. A stale token or an existing completion refuses **without
    /// writing**. There is no retry, no update, no delete and no hidden idempotency column.
    pub fn record(
        &self,
        request: CompletionIntentRequest,
        reviewed_intent_sha256: &str,
    ) -> Result<ReviewedIntent, CompletionCommandError> {
        let reviewed = reviewed_intent_sha256.to_owned();
        self.accounts.with_authenticated_store(|connection, now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let outcome = (|| {
                let bindings = owner_bindings(&transaction, self.workspace_id, &request)?;

                // The token is compared over the SENTINEL-normalized record, so a transient change
                // in identity, time or boundary cannot invalidate a review.
                let reviewable = derive_record(&bindings, &request, Uuid::nil(), 0, 0)?;
                if reviewed_intent_token(&reviewable)? != reviewed {
                    return Err(CompletionCommandError::IntentChanged);
                }

                // Refuse an existing completion before assigning anything, so a duplicate write
                // never reaches the writer.
                if verified_full_completion_for_mission_within(
                    &transaction,
                    self.workspace_id,
                    request.mission_id,
                )
                .map_err(|error| {
                    CompletionCommandError::Unavailable(CompletionCommandUnavailable::Read(error))
                })?
                .is_some()
                {
                    return Err(CompletionCommandError::AlreadyRecorded);
                }

                // The owner's three fields, assigned here and nowhere else. The boundary is the
                // chain head read in this same transaction: the cut this record reports at.
                let boundary: i64 = transaction.query_row(
                    "SELECT coalesce(max(sequence), 0) FROM events WHERE workspace_id = ?1",
                    rusqlite::params![self.workspace_id.to_string()],
                    |row| row.get(0),
                )?;
                let boundary =
                    u64::try_from(boundary).map_err(|_| malformed("boundary_event_seq"))?;
                let completed_at_ms = u64::try_from(now).map_err(|_| malformed("completed_at_ms"))?;
                let record = derive_record(
                    &bindings,
                    &request,
                    Uuid::new_v4(),
                    boundary,
                    completed_at_ms,
                )?;

                // The existing writer, in THIS transaction. It writes the row, its condition
                // projections and the Event together, and reads the record back before returning.
                record_completion_within(&transaction, &record).map_err(|refusal| {
                    CompletionCommandError::Unavailable(CompletionCommandUnavailable::Write(refusal))
                })?;

                // And the whole thing proves itself through C3's reader before the commit.
                let stored = verified_full_completion_for_mission_within(
                    &transaction,
                    self.workspace_id,
                    request.mission_id,
                )
                .map_err(|error| {
                    CompletionCommandError::Unavailable(CompletionCommandUnavailable::Read(error))
                })?
                .ok_or(CompletionCommandError::Unavailable(
                    CompletionCommandUnavailable::WriteReadBackDisagrees,
                ))?;
                if stored != record {
                    return Err(CompletionCommandError::Unavailable(
                        CompletionCommandUnavailable::WriteReadBackDisagrees,
                    ));
                }
                Ok(ReviewedIntent {
                    intent_sha256: reviewed_intent_token(&stored)?,
                })
            })();
            match outcome {
                Ok(intent) => {
                    transaction.commit()?;
                    Ok(Ok(intent))
                }
                // The transaction is dropped unwritten: a refusal writes nothing.
                Err(refusal) => Ok::<_, OperatorAccountError>(Err(refusal)),
            }
        })?
    }

    /// The token of the ACTUAL stored completion, or an honest absence.
    ///
    /// One Deferred snapshot, through C3's scoped both-direction Event/record verification. The
    /// token is computed from the full verified record, never reconstructed from counts and never
    /// taken from a by-digest read alone.
    ///
    /// `Ok(None)` is "no completion is visible in THIS snapshot" and is byte-identical for a Mission
    /// this Workspace never admitted. An equal token means the stored content equals the reviewed
    /// intent; it does not attribute the write. Corrupt or orphaned evidence is unavailable, never
    /// null.
    pub fn get_intent(
        &self,
        mission_id: Uuid,
    ) -> Result<Option<ReviewedIntent>, CompletionCommandError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let outcome = (|| {
                let found = verified_full_completion_for_mission_within(
                    &transaction,
                    self.workspace_id,
                    mission_id,
                )
                .map_err(|error| {
                    CompletionCommandError::Unavailable(CompletionCommandUnavailable::Read(error))
                })?;
                match found {
                    Some(completion) => Ok(Some(ReviewedIntent {
                        intent_sha256: reviewed_intent_token(&completion)?,
                    })),
                    None => Ok(None),
                }
            })();
            match outcome {
                Ok(found) => {
                    transaction.commit()?;
                    Ok(Ok(found))
                }
                Err(refusal) => Ok::<_, OperatorAccountError>(Err(refusal)),
            }
        })?
    }
}
