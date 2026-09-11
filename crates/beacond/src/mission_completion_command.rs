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
/// A token is not a record identity. Prefixing the canonical bytes separates the two DOMAINS, so an
/// intent digest and a content digest are taken over different inputs and neither can be presented
/// as the other. No claim is made about SHA-256 itself.
pub const REVIEWED_INTENT_PREFIX: &[u8] = b"beacon.mission_completion.reviewed_intent.v1\0";

// ---------------------------------------------------------------------------------------------
// The request DTO. Strict: unknown fields, unknown tags and out-of-domain values all refuse.
// ---------------------------------------------------------------------------------------------

/// Require the KEY, permit the value to be `null`.
///
/// Serde's own missing-field path (`serde/src/private/de.rs:24-58`) hands a type a deserializer
/// that supports only `deserialize_option`, so ANY type that forwards to `Option` -- including a
/// transparent newtype over one -- accepts an omitted key. **Measured, not assumed:** both of those
/// shapes were tried here and both accepted an omitted `note`. A field-level `deserialize_with`
/// takes a different branch in the derive, which returns `missing field` instead, and that is the
/// only shape that tells "the Operator recorded no note" from "the client forgot the key".
fn required_explicit_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// One support reference, externally tagged as the C4 contract specifies.
///
/// `rename_all` renames the VARIANTS; `rename_all_fields` renames a struct variant's own fields,
/// which the first rule does not touch. Without the second, `Receipt` expects `run_id` and the
/// contract sends `runId` (root's checkpoint read, finding 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
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
    /// Explicit `null`, never an absent key: an omitted note and a recorded absence are different
    /// facts, and only one of them is something the Operator said.
    #[serde(deserialize_with = "required_explicit_null")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NarrativeItemDto {
    pub ordinal: u32,
    pub kind: String,
    /// The declared condition this relates to, as explicit `null` when there is none.
    #[serde(deserialize_with = "required_explicit_null")]
    pub condition_ordinal: Option<u32>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
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

/// The codec's refusal, projected into the small truthful set this owner already has.
///
/// No new policy and no second validator: every arm is the codec's own class, and only the
/// genuinely length-shaped ones reach a length-shaped caller line.
fn codec_refusal(
    refusal: crate::mission_completion::MalformedCompletion,
) -> CompletionCommandError {
    use crate::mission_completion::MalformedCompletion as Codec;
    match refusal {
        // Actually about length.
        Codec::TextTooLong => CompletionCommandError::TextOverLimit,
        // NOT SupportOverLimit. `OverLimitLength` covers a 33rd narrative item or delivered
        // Artifact as well as a support set, and `RecordTooLarge` is the whole envelope -- a caller
        // told "this condition cites more support than a completion can record" would be reading a
        // false sentence about a record with no support at all. `SupportOverLimit` is reached only
        // by the explicit per-condition check in `derive_record`, where the count is the actual
        // failing input (root's finding 4, second pass).
        Codec::OverLimitLength => CompletionCommandError::MalformedRequest("list.overLimit"),
        Codec::RecordTooLarge => CompletionCommandError::MalformedRequest("record.tooLarge"),
        // Actually about a set.
        Codec::SupportOrder => CompletionCommandError::DuplicateSetEntry("support"),
        Codec::DeliveredArtifactOrder => {
            CompletionCommandError::DuplicateSetEntry("deliveredArtifacts")
        }
        // Actually about the declared condition set.
        Codec::NoConditionResults | Codec::ConditionOrdinals => {
            CompletionCommandError::ConditionCoverageIncomplete
        }
        // Everything else is a malformed request, named by the field class it came from.
        Codec::NarrativeOrdinals => CompletionCommandError::MalformedRequest("narrative.ordinal"),
        Codec::ControlCharacter => CompletionCommandError::MalformedRequest("text.control"),
        Codec::EmptyText => CompletionCommandError::MalformedRequest("text.empty"),
        Codec::UntrimmedText => CompletionCommandError::MalformedRequest("text.untrimmed"),
        Codec::Magic
        | Codec::Truncated
        | Codec::TrailingBytes
        | Codec::UnknownTag
        | Codec::InvalidUtf8 => CompletionCommandError::MalformedRequest("canonical_bytes"),
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
/// The prefix is DOMAIN SEPARATION: an intent token and a completion's own content digest are
/// computed over different input grammars, so one cannot be presented as the other. That is a claim
/// about the inputs, not a cryptographic promise about SHA-256.
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
    let raw = HEXLOWER
        .decode(value.as_bytes())
        .map_err(|_| malformed(field))?;
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
        SupportRefDto::Artifact(digest) => {
            SupportRef::Artifact(digest32(digest, "support.artifact")?)
        }
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
            return Err(CompletionCommandError::DuplicateSetEntry(
                "deliveredArtifacts",
            ));
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
    for (result, source) in record
        .condition_results
        .iter()
        .zip(&request.condition_results)
    {
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
        let index =
            usize::try_from(result.ordinal).map_err(|_| malformed("conditionResults.ordinal"))?;
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
    // NOT sorted. Only the two SETS normalize; a submitted condition SEQUENCE is the Operator's,
    // and repairing [1,0] into [0,1] would silently accept a record about different conditions than
    // the one that was sent. The codec's own dense-ascending rule refuses it (root's finding 3).

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
        unfinished_work: narrative_from(&request.unfinished_work, unfinished_from)?,
        uncertainty: narrative_from(&request.uncertainty, uncertainty_from)?,
        spend,
        supersedes: CompletionSupersedes::None,
        summary: request.summary.clone(),
        completed_at_ms,
    };
    // The codec's own grammar is the validator: every bound, every text class and the envelope
    // ceiling are its rules, and a second copy here would be a second thing to keep true. Its
    // classes are PROJECTED, not flattened -- an invalid narrative ordinal or a control character
    // is not "text too long", and a caller told so would go looking for the wrong thing.
    record.canonical_bytes().map_err(codec_refusal)?;
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
                let completed_at_ms =
                    u64::try_from(now).map_err(|_| malformed("completed_at_ms"))?;
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
                    CompletionCommandError::Unavailable(CompletionCommandUnavailable::Write(
                        refusal,
                    ))
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

#[cfg(test)]
mod tests {
    //! Codec-level probes with no store, no account and no Mission.

    use super::*;

    /// Placeholder bindings, for SIZE and REFUSAL measurement only. Never a derived completion:
    /// the bindings are invented, so nothing here is storable and nothing proves an admission.
    fn size_reference_record(
        request: &CompletionIntentRequest,
    ) -> Result<MissionCompletionV1, CompletionCommandError> {
        let declared: Vec<String> = (0..request.condition_results.len())
            .map(|ordinal| format!("Declared condition {ordinal}"))
            .collect();
        let bindings = OwnerBindings {
            workspace_id: Uuid::nil(),
            project_id: Uuid::nil(),
            accountable_resident_configuration_id: Uuid::nil(),
            declared_conditions_sha256: declared_conditions_digest(&declared),
            declared_conditions: declared,
        };
        derive_record(&bindings, request, Uuid::nil(), 0, 0)
    }

    /// C4.OWNER.FINGERPRINT_REFERENCE, size half: the ACTUAL codec on root's actual fixture.
    ///
    /// Root CALCULATED 52 525 canonical bytes with a Python re-implementation and measured the file
    /// at 66 348 JSON bytes; neither was an owner encode. This is the encoder itself saying so.
    /// The owner-assigned fields are fixed width, so the canonical length does not depend on which
    /// Workspace, Project or instant the record would belong to.
    #[test]
    fn the_root_complete_request_encodes_to_the_calculated_canonical_size() {
        let raw = include_str!("../tests/fixtures/c4-complete-request.json");
        assert_eq!(
            raw.len(),
            66_348,
            "C4.OWNER.FINGERPRINT_REFERENCE: root's JSON byte count"
        );
        #[derive(Deserialize)]
        struct Arguments {
            request: CompletionIntentRequest,
        }
        let request = serde_json::from_str::<Arguments>(raw)
            .expect("C4.OWNER.FINGERPRINT_REFERENCE: a contract-shaped request")
            .request;
        let record = size_reference_record(&request)
            .expect("C4.OWNER.FINGERPRINT_REFERENCE: the owner grammar accepts it");
        let canonical = record
            .canonical_bytes()
            .expect("C4.OWNER.FINGERPRINT_REFERENCE: the codec encodes it");
        assert_eq!(
            canonical.len(),
            52_525,
            "C4.OWNER.FINGERPRINT_REFERENCE: the codec disagrees with the calculated size"
        );
        // The seam the transport design rests on, from the codec rather than from arithmetic.
        assert!(
            raw.len() > 65_536,
            "C4.OWNER.FINGERPRINT_REFERENCE: over the frame"
        );
        assert!(
            canonical.len() < 65_536,
            "C4.OWNER.FINGERPRINT_REFERENCE: the owner accepts what the frame cannot carry"
        );
    }

    /// C4.OWNER.INPUT_DOMAINS, truthful refusals: a length-shaped caller line only for length.
    #[test]
    fn a_generic_grammar_refusal_is_never_a_support_count_sentence() {
        let raw = include_str!("../tests/fixtures/c4-complete-request.json");
        #[derive(Deserialize)]
        struct Arguments {
            request: CompletionIntentRequest,
        }
        let base = serde_json::from_str::<Arguments>(raw).unwrap().request;

        // A 33rd narrative item, on a record whose conditions cite NO support at all. The old
        // mapping called this "this condition cites more support than a completion can record".
        let mut narrative = base.clone();
        for result in &mut narrative.condition_results {
            result.support.clear();
        }
        narrative.unfinished_work = (0..33_u32)
            .map(|ordinal| NarrativeItemDto {
                ordinal,
                kind: "deferred_scope".to_owned(),
                condition_ordinal: None,
                summary: "Deferred.".to_owned(),
            })
            .collect();
        let refusal = size_reference_record(&narrative)
            .expect_err("C4.OWNER.INPUT_DOMAINS: 33 narrative items must refuse");
        assert_eq!(
            refusal.name(),
            "MalformedRequest",
            "C4.OWNER.INPUT_DOMAINS: a narrative list length is not a support count: {refusal:?}"
        );

        // Actual over-length text keeps the length-shaped projection.
        let mut long_text = base.clone();
        long_text.summary = "x".repeat(2_001);
        let refusal = size_reference_record(&long_text)
            .expect_err("C4.OWNER.INPUT_DOMAINS: an over-length summary must refuse");
        assert_eq!(
            refusal.name(),
            "TextOverLimit",
            "C4.OWNER.INPUT_DOMAINS: over-length text keeps its own projection: {refusal:?}"
        );

        // A control character is malformed, not long.
        let mut control = base.clone();
        control.summary = "before\u{7}after".to_owned();
        let refusal = size_reference_record(&control)
            .expect_err("C4.OWNER.INPUT_DOMAINS: a control character must refuse");
        assert_eq!(
            refusal.name(),
            "MalformedRequest",
            "C4.OWNER.INPUT_DOMAINS: a control character is not over-length text: {refusal:?}"
        );
    }

    // -----------------------------------------------------------------------------------------
    // C4.OWNER.FINGERPRINT_REFERENCE — the token half.
    //
    // Blue captured four real canonical vectors from the EXISTING golden test's own encoder and
    // published their tokens from an independent Python calculator
    // (`beacon-team/tools/c4/blue/reviewed_intent_reference.py`, and
    // `reference-status.json` for the expected values). Every expectation below is that file's,
    // held here as a literal; none of them is computed from this owner.
    // -----------------------------------------------------------------------------------------

    const GOLDEN: &[u8] = include_bytes!("../tests/fixtures/c4-vector-canonical-v1.bin");
    const SAME_TOKEN: &[u8] = include_bytes!("../tests/fixtures/c4-vector-same-token.bin");
    const OTHER_CONTEXT: &[u8] =
        include_bytes!("../tests/fixtures/c4-vector-different-context.bin");
    const OTHER_CONTENT: &[u8] =
        include_bytes!("../tests/fixtures/c4-vector-different-content.bin");

    const GOLDEN_TOKEN: &str = "02d3b998eaf4d5524374ee9a24d7e94d6329e7b69b578d1aa6195e68f8adce62";
    const OTHER_CONTEXT_TOKEN: &str =
        "b83d871b64dd6ee754fa4b56e7bfa59f0c56cb95ed59c08f7a99884c538d6de9";
    const OTHER_CONTENT_TOKEN: &str =
        "5a3a24a426da9b1d1e7f107d7ab231ff633837b93c7c9bfa7bd422976267a0f5";

    fn vector(bytes: &[u8], expected_sha256: &str, label: &str) -> MissionCompletionV1 {
        assert_eq!(
            bytes.len(),
            707,
            "C4.OWNER.FINGERPRINT_REFERENCE: {label} length"
        );
        assert_eq!(
            HEXLOWER.encode(&sha256(bytes)),
            expected_sha256,
            "C4.OWNER.FINGERPRINT_REFERENCE: {label} is not the captured vector"
        );
        MissionCompletionV1::from_canonical_bytes(bytes).unwrap_or_else(|refusal| {
            panic!("C4.OWNER.FINGERPRINT_REFERENCE: {label}: {refusal:?}")
        })
    }

    #[test]
    fn the_captured_vectors_agree_with_the_independent_reference_token() {
        let golden = vector(
            GOLDEN,
            "705034373a787f7936d2bc2d086f5ef0295bbea5aa02ef3ccc66fb8deb951dfe",
            "the golden vector",
        );
        assert_eq!(
            reviewed_intent_token(&golden).expect("C4.OWNER.FINGERPRINT_REFERENCE"),
            GOLDEN_TOKEN,
            "C4.OWNER.FINGERPRINT_REFERENCE: this owner disagrees with the independent calculator"
        );

        // ONLY the three ephemerals differ, and the token does not move.
        let same = vector(
            SAME_TOKEN,
            "68c9ea95ea71180794c0f871e5ef1ae6330fcc3fdb46c9bb683d73f1fca3ba5b",
            "the same-token control",
        );
        assert_ne!(
            (
                same.completion_id,
                same.boundary_event_seq,
                same.completed_at_ms
            ),
            (
                golden.completion_id,
                golden.boundary_event_seq,
                golden.completed_at_ms
            ),
            "C4.OWNER.FINGERPRINT_REFERENCE: the control must actually differ"
        );
        assert_eq!(
            MissionCompletionV1 {
                completion_id: golden.completion_id,
                boundary_event_seq: golden.boundary_event_seq,
                completed_at_ms: golden.completed_at_ms,
                ..same.clone()
            },
            golden,
            "C4.OWNER.FINGERPRINT_REFERENCE: and must differ in NOTHING else"
        );
        assert_eq!(
            reviewed_intent_token(&same).expect("C4.OWNER.FINGERPRINT_REFERENCE"),
            GOLDEN_TOKEN,
            "C4.OWNER.FINGERPRINT_REFERENCE: identity, boundary and time are not the intent"
        );

        // A different stable context is a different intent, at the same length.
        let context = vector(
            OTHER_CONTEXT,
            "95593b761810d435f8b50b26461173d6d99655526699a22f7eeabb8d227ad6d2",
            "the stable-context control",
        );
        assert_ne!(
            context.workspace_id, golden.workspace_id,
            "C4.OWNER.FINGERPRINT_REFERENCE"
        );
        assert_eq!(
            reviewed_intent_token(&context).expect("C4.OWNER.FINGERPRINT_REFERENCE"),
            OTHER_CONTEXT_TOKEN,
            "C4.OWNER.FINGERPRINT_REFERENCE"
        );
        assert_ne!(
            OTHER_CONTEXT_TOKEN, GOLDEN_TOKEN,
            "C4.OWNER.FINGERPRINT_REFERENCE: a different Workspace is a different intent"
        );

        // So is a different authored character, at the same length.
        let content = vector(
            OTHER_CONTENT,
            "728208fa64df16e4912918a845b089582ea07556205c3a49b5f96e5bffbe9b28",
            "the authored-content control",
        );
        assert_eq!(
            reviewed_intent_token(&content).expect("C4.OWNER.FINGERPRINT_REFERENCE"),
            OTHER_CONTENT_TOKEN,
            "C4.OWNER.FINGERPRINT_REFERENCE"
        );
        assert_ne!(
            OTHER_CONTENT_TOKEN, GOLDEN_TOKEN,
            "C4.OWNER.FINGERPRINT_REFERENCE: one changed character is a different intent"
        );

        // The prefix is what the recipe says it is: one trailing NUL, no literal backslash-zero.
        assert_eq!(
            REVIEWED_INTENT_PREFIX.len(),
            45,
            "C4.OWNER.FINGERPRINT_REFERENCE"
        );
        assert_eq!(
            REVIEWED_INTENT_PREFIX[44], 0,
            "C4.OWNER.FINGERPRINT_REFERENCE"
        );
        assert_eq!(
            REVIEWED_INTENT_PREFIX
                .iter()
                .filter(|byte| **byte == 0)
                .count(),
            1,
            "C4.OWNER.FINGERPRINT_REFERENCE: exactly one NUL"
        );
        assert!(
            !REVIEWED_INTENT_PREFIX.windows(2).any(|pair| pair == b"\\0"),
            "C4.OWNER.FINGERPRINT_REFERENCE: no literal backslash-zero"
        );
        // And a token is not a content digest: the same bytes under the two domains differ.
        assert_ne!(
            reviewed_intent_token(&golden).expect("C4.OWNER.FINGERPRINT_REFERENCE"),
            HEXLOWER.encode(&sha256(GOLDEN)),
            "C4.OWNER.FINGERPRINT_REFERENCE: the prefix separates the two domains"
        );
    }

    // -----------------------------------------------------------------------------------------
    // C4.OWNER.INPUT_DOMAINS — the numeric and text domains, and the exact envelope ceiling.
    // -----------------------------------------------------------------------------------------

    /// A request with `conditions` declared conditions, `astral_notes` of them carrying a
    /// 500-SCALAR astral note, the rest a 500-character ASCII one, both narrative lists full, and
    /// `summary` ASCII characters of summary.
    fn ceiling_request(astral_notes: usize, summary: usize) -> CompletionIntentRequest {
        let digest = |seed: usize| format!("{:064x}", seed);
        CompletionIntentRequest {
            mission_id: Uuid::nil(),
            accountable_resident_id: Uuid::nil(),
            disposition: "concluded".to_owned(),
            condition_results: (0..20)
                .map(|ordinal| ConditionResultDto {
                    ordinal: u32::try_from(ordinal).unwrap(),
                    outcome: "met".to_owned(),
                    support: (0..8)
                        .map(|slot| SupportRefDto::Artifact(digest(ordinal * 8 + slot + 1)))
                        .collect(),
                    note: Some(if ordinal < astral_notes {
                        "\u{1f9ed}".repeat(500)
                    } else {
                        "n".repeat(500)
                    }),
                })
                .collect(),
            delivered_artifacts: (0..32).map(|slot| digest(1_000 + slot)).collect(),
            unfinished_work: (0..32)
                .map(|ordinal| NarrativeItemDto {
                    ordinal: u32::try_from(ordinal).unwrap(),
                    kind: "deferred_scope".to_owned(),
                    condition_ordinal: None,
                    summary: "u".repeat(500),
                })
                .collect(),
            uncertainty: (0..32)
                .map(|ordinal| NarrativeItemDto {
                    ordinal: u32::try_from(ordinal).unwrap(),
                    kind: "unmeasured_quantity".to_owned(),
                    condition_ordinal: None,
                    summary: "q".repeat(500),
                })
                .collect(),
            spend: SpendDto::NotMeasured("no spend record exists".to_owned()),
            summary: "s".repeat(summary),
        }
    }

    fn ceiling_bytes(astral_notes: usize, summary: usize) -> Result<usize, CompletionCommandError> {
        Ok(
            size_reference_record(&ceiling_request(astral_notes, summary))?
                .canonical_bytes()
                .expect("a record that derived encodes")
                .len(),
        )
    }

    /// The exact 65 536-byte envelope boundary, found by measurement rather than asserted.
    #[test]
    fn the_canonical_envelope_ceiling_is_exact() {
        // The search is well-founded only if the two ends actually straddle the ceiling.
        assert!(
            ceiling_bytes(0, 1).expect("C4.OWNER.INPUT_DOMAINS") <= 65_536,
            "C4.OWNER.INPUT_DOMAINS: the all-ASCII floor must fit"
        );
        assert!(
            ceiling_bytes(20, 1).is_err(),
            "C4.OWNER.INPUT_DOMAINS: the all-astral ceiling must not fit"
        );
        // The largest astral count that still fits with a one-character summary. Each astral note
        // costs 1 500 bytes more than its ASCII twin, so the remaining gap is always inside the
        // 2 000-scalar summary budget -- which is what makes the exact byte reachable at all.
        let (notes, floor) = (0..20)
            .rev()
            .find_map(|notes| ceiling_bytes(notes, 1).ok().map(|size| (notes, size)))
            .expect("C4.OWNER.INPUT_DOMAINS: some astral count fits");
        let summary = 1 + (65_536 - floor);
        assert!(
            summary <= 2_000,
            "C4.OWNER.INPUT_DOMAINS: the ceiling is not reachable from this fixture ({summary})"
        );

        assert_eq!(
            ceiling_bytes(notes, summary).expect("C4.OWNER.INPUT_DOMAINS"),
            65_536,
            "C4.OWNER.INPUT_DOMAINS: exactly at the ceiling"
        );
        let refusal = size_reference_record(&ceiling_request(notes, summary + 1))
            .expect_err("C4.OWNER.INPUT_DOMAINS: one byte over the ceiling must refuse");
        assert_eq!(
            refusal.name(),
            "MalformedRequest",
            "C4.OWNER.INPUT_DOMAINS: an oversized ENVELOPE is not a support count: {refusal:?}"
        );
    }

    /// Every text class the codec knows, and both ends of `u32`.
    #[test]
    fn the_text_and_number_domains_are_the_codecs_own() {
        let base = || {
            let mut request = ceiling_request(0, 10);
            request.condition_results.truncate(1);
            request.condition_results[0].support.clear();
            request.delivered_artifacts.clear();
            request.unfinished_work.truncate(1);
            request.uncertainty.clear();
            request
        };

        // 500 scalars pass and 501 refuse -- in ASCII and in ASTRAL scalars alike, because the
        // bound is scalars, not bytes.
        for (label, text) in [
            ("ascii", "n".repeat(500)),
            ("astral", "\u{1f9ed}".repeat(500)),
        ] {
            let mut request = base();
            request.condition_results[0].note = Some(text.clone());
            size_reference_record(&request).unwrap_or_else(|refusal| {
                panic!("C4.OWNER.INPUT_DOMAINS: 500 {label}: {refusal:?}")
            });

            let mut request = base();
            request.condition_results[0].note =
                Some(format!("{text}{}", &text[..text.len() / 500]));
            let refusal = size_reference_record(&request)
                .expect_err("C4.OWNER.INPUT_DOMAINS: 501 scalars must refuse");
            assert_eq!(
                refusal.name(),
                "TextOverLimit",
                "C4.OWNER.INPUT_DOMAINS: 501 {label}: {refusal:?}"
            );
        }

        // The summary has its own, larger bound.
        let mut request = base();
        request.summary = "s".repeat(2_000);
        size_reference_record(&request).expect("C4.OWNER.INPUT_DOMAINS: 2000 summary scalars");
        let mut request = base();
        request.summary = "s".repeat(2_001);
        assert_eq!(
            size_reference_record(&request)
                .expect_err("C4.OWNER.INPUT_DOMAINS: 2001 must refuse")
                .name(),
            "TextOverLimit",
            "C4.OWNER.INPUT_DOMAINS"
        );

        // Empty, untrimmed and control-bearing text are each their own refusal, never a length.
        for (text, field) in [
            (String::new(), "text.empty"),
            (" untrimmed".to_owned(), "text.untrimmed"),
            ("a\u{7}bell".to_owned(), "text.control"),
        ] {
            let mut request = base();
            request.summary = text.clone();
            let refusal = size_reference_record(&request)
                .expect_err("C4.OWNER.INPUT_DOMAINS: a malformed text class must refuse");
            assert!(
                matches!(refusal, CompletionCommandError::MalformedRequest(named) if named == field),
                "C4.OWNER.INPUT_DOMAINS: {field}: {refusal:?}"
            );
        }

        // Both ends of u32, on both versioned references.
        for version in [0_u32, u32::MAX] {
            let mut request = base();
            request.condition_results[0].support = vec![SupportRefDto::Receipt {
                run_id: Uuid::nil(),
                receipt_version: version,
                receipt_bytes_sha256: format!("{:064x}", 1),
            }];
            request.spend = SpendDto::Recorded {
                spend_id: Uuid::nil(),
                spend_version: version,
                spend_sha256: format!("{:064x}", 2),
            };
            let record = size_reference_record(&request).unwrap_or_else(|refusal| {
                panic!("C4.OWNER.INPUT_DOMAINS: u32 {version}: {refusal:?}")
            });
            let round_tripped = MissionCompletionV1::from_canonical_bytes(
                &record.canonical_bytes().expect("C4.OWNER.INPUT_DOMAINS"),
            )
            .expect("C4.OWNER.INPUT_DOMAINS");
            assert_eq!(
                round_tripped, record,
                "C4.OWNER.INPUT_DOMAINS: u32 {version} is lossless"
            );
        }

        // A 9-entry support set is the ONE place a support-count sentence is true.
        let mut request = base();
        request.condition_results[0].support = (0..9)
            .map(|slot| SupportRefDto::Artifact(format!("{:064x}", slot + 1)))
            .collect();
        let refusal = size_reference_record(&request)
            .expect_err("C4.OWNER.INPUT_DOMAINS: 9 citations must refuse");
        assert_eq!(
            refusal.name(),
            "SupportOverLimit",
            "C4.OWNER.INPUT_DOMAINS: {refusal:?}"
        );

        // Needles survive Debug and never reach Display. MEASURED, not assumed: the derived Debug
        // ESCAPES a CR/LF rather than emitting it, so the needle survives as `\\r\\n` -- readable
        // and greppable, and it cannot forge a line break in a log.
        let needle = CompletionCommandError::MalformedRequest("crlf\r\nneedle");
        assert_eq!(
            format!("{needle:?}"),
            r#"MalformedRequest("crlf\r\nneedle")"#,
            "C4.OWNER.INPUT_DOMAINS: Debug keeps the detail, escaped"
        );
        assert!(
            !format!("{needle:?}").contains('\n'),
            "C4.OWNER.INPUT_DOMAINS: and cannot forge a line break"
        );
        assert_eq!(
            needle.to_string(),
            "MalformedRequest",
            "C4.OWNER.INPUT_DOMAINS: Display is one independent literal"
        );
    }
}
