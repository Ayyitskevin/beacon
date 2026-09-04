//! `MissionCompletionV1` — the record that one Mission finished (ADR 0041's follow-on).
//!
//! This module is the **record only**: canonical bytes, the closed vocabularies, and the domain
//! checks the bytes alone can settle. There is no store, no Event, no Command and no verified
//! reader here — those are the next slice, together with every check that compares this record
//! against the Mission it names.
//!
//! # What this record is, and is not
//!
//! It is the Work-owned, immutable, digest-identified record that a Mission finished: at which
//! event boundary, with whom accountable, what each declared success condition came to, what was
//! delivered, what was left, what is uncertain, and where the measured spend is recorded.
//!
//! It is **not** a success verdict. There is deliberately no aggregate — no `succeeded` boolean, no
//! `all_met` flag, no count of met conditions. A reader that wants "did this Mission succeed" reads
//! the conditions and decides; nothing is stored for a module to branch on. It is not an Outcome
//! Document, not an authority surface, not a spend measurement, and not an Artifact.
//!
//! # The exact canonical bytes
//!
//! The ADR 0034/0037 discipline: versioned magic, positional fixed-order fields, integers unsigned
//! big-endian at the stated width, strings valid UTF-8 preceded by a `u64` big-endian byte length,
//! UUIDs their 16 raw bytes, SHA-256 values their 32 raw bytes, closed one-byte tags never
//! renumbered, lists a `u32` big-endian count proved against the remaining input before anything is
//! allocated. The decoder consumes the whole input and refuses unknown tags, trailing bytes,
//! over-limit lengths, invalid UTF-8 and non-canonical ordering. Decode, re-encode, and the bytes
//! must be equal.
//!
//! | # | field | encoding | bound / rule |
//! |---|---|---|---|
//! | 1 | magic | 29 literal bytes `beacon.mission_completion.v1\0` | fixed; not length-framed |
//! | 2 | `workspace_id` | uuid | — |
//! | 3 | `project_id` | uuid | copied from the Mission; the comparison is the next slice's |
//! | 4 | `mission_id` | uuid | — |
//! | 5 | `completion_id` | uuid | the record's **reference** identity (R15) |
//! | 6 | `actor` | tag | `operator` `0x01`; the only assigned value in v1 |
//! | 7 | `accountable_resident_id` | uuid | roster membership is the next slice's |
//! | 8 | `accountable_resident_configuration_id` | uuid | — |
//! | 9 | `boundary_event_seq` | `u64` | ≥ the Mission's admission sequence — next slice's |
//! | 10 | `disposition` | tag | `concluded` `0x01`, `abandoned` `0x02` |
//! | 11 | `declared_conditions_sha256` | 32 raw bytes | over the Mission's ordered condition list |
//! | 12 | `condition_results` | `u32` count | 1..=20; ascending, dense from 0 |
//! | 13 | └ `ordinal` | `u32` | dense from 0, ascending |
//! | 14 | └ `outcome` | tag | `met` `0x01`, `not_met` `0x02`, `not_assessed` `0x03`, `assessment_inconclusive` `0x04` |
//! | 15 | └ `support` | `u32` count | ≤ 8; a set, ascending by (kind tag, identity bytes) |
//! | 16 | &nbsp;&nbsp;└ `kind` | tag | `artifact` `0x01`, `receipt` `0x02`, `evaluation` `0x03`, `ruling` `0x04` |
//! | 17 | &nbsp;&nbsp;└ `identity` | per kind | artifact/evaluation/ruling: 32 raw bytes. receipt: uuid + `u32` + 32 raw bytes |
//! | 18 | └ `note` | tag | `none` `0x00`; `text` `0x01` + string ≤ 500 scalars |
//! | 19 | `delivered_artifacts` | `u32` count | ≤ 32; a set, ascending by digest bytes |
//! | 20 | └ `value` | 32 raw bytes | the **Artifact record's** digest, not its content digest (R16) |
//! | 21 | `unfinished_work` | `u32` count | ≤ 32; a sequence, order is semantic |
//! | 22 | └ `ordinal` | `u32` | dense from 0, ascending |
//! | 23 | └ `kind` | tag | `condition_not_met` `0x01`, `deferred_scope` `0x02`, `known_defect` `0x03`, `blocked_dependency` `0x04` |
//! | 24 | └ `condition_ordinal` | tag | `none` `0x00`; `some` `0x01` + `u32` |
//! | 25 | └ `summary` | string | 1..=500 scalars; LF and TAB kept, other control characters refused |
//! | 26 | `uncertainty` | `u32` count | ≤ 32; a sequence, order is semantic |
//! | 27 | └ `ordinal` | `u32` | dense from 0, ascending |
//! | 28 | └ `kind` | tag | `unverified_claim` `0x01`, `unmeasured_quantity` `0x02`, `conflicting_evidence` `0x03`, `outside_scope_of_checks` `0x04` |
//! | 29 | └ `condition_ordinal` | tag | `none` `0x00`; `some` `0x01` + `u32` |
//! | 30 | └ `summary` | string | as field 25 |
//! | 31 | `spend` | tag | `recorded` `0x01` + uuid + `u32` + 32 raw bytes; `not_measured` `0x02` + string ≤ 500 scalars |
//! | 32 | `supersedes` | tag | `none` `0x00`; `digest` `0x01` + 32 raw bytes |
//! | 33 | `summary` | string | mandatory, trimmed, 1..=2,000 scalars; LF and TAB kept |
//! | 34 | `completed_at_ms` | `u64` | service clock, never caller input |
//!
//! Every string is a `u64` length prefix then bytes, matching `beacon.proposal.v1`. The record's own
//! total bound is 65,536 bytes: the per-field limits alone admit roughly 188,000, so a grammar with
//! only per-field bounds is not total, and the encoder refuses over the bound before anything
//! reaches a store.
//!
//! # The per-condition vocabulary, and the fifth state
//!
//! Four assigned tags — `met`, `not_met`, `not_assessed`, `assessment_inconclusive` — and two more
//! states that stay distinct **without** a tag:
//!
//! * **absent** is unrepresentable rather than unassigned. A condition with no result cannot be
//!   encoded, because the ordinals must be dense from zero and the store compares the count against
//!   the Mission's declaration. A fifth literal for it would be a tag no writer could emit.
//! * **`unsupported_reference` is reserved by name at `0x05` and deliberately unassigned.** Every
//!   referent this record can cite is immutable and undeletable today, so a dangling support
//!   reference means the store is corrupt rather than that a condition's evidence went missing —
//!   and an assigned literal for it would be one no test could reach. It becomes real when a
//!   referent can legitimately dangle. Reserving the number, rather than borrowing an unassigned
//!   one later, is R13 item 8's rule applied before the fact.
//!
//! `not_assessed` and `assessment_inconclusive` are the pair most likely to be collapsed by a later
//! edit and the pair that must not be: "nobody looked" and "somebody looked and could not say" are
//! different facts about the Team, and only the second is evidence of anything.
//!
//! This vocabulary is **exported**: the Outcome Document imports it (R13 item 3), which widens its
//! slice-1 two-arm `met`/`not_met` to these four. Nothing here may be renumbered afterwards, and an
//! importing module must not render `not_assessed` or `assessment_inconclusive` as `not_met`.
//!
//! # Published identity vector
//!
//! The fixture in `tests/mission_completion.rs` -- which exercises every arm of every tag -- encodes
//! to **707 bytes** with SHA-256
//! `705034373a787f7936d2bc2d086f5ef0295bbea5aa02ef3ccc66fb8deb951dfe`.
//!
//! That pair is pinned by `the_canonical_vector_is_exactly_these_bytes_and_a_second_encoder_reproduces_them`,
//! which also assembles the same bytes a second time from the table above **without calling the
//! encoder**. The two assemblers share an author, which is weaker than ADR 0038's vector had: a
//! non-author reproduction from this table alone is the strongest thing this record is still
//! missing, and it is the right thing to ask of the review. The table is here rather than in an ADR
//! because this slice has no store; the ADR is owed when the store lands, and this table moves into
//! it verbatim.

use std::cmp::Ordering;
use std::fmt;

use data_encoding::HEXLOWER;
use uuid::Uuid;

use crate::proposal::sha256;

/// The canonicalizer this record's digest names.
pub const COMPLETION_CANONICALIZER: &str = "beacon.mission_completion.v1";

const MAGIC: &[u8] = b"beacon.mission_completion.v1\0";
const DIGEST_ALGORITHM: &str = "sha256";

/// The record's own total bound. The per-field limits admit roughly 188,000 bytes, so without this
/// a record every field accepts could not be stored -- the gap ADR 0038 closed for Evaluation.
const MAX_CANONICAL_BYTES: usize = 65_536;

/// `mission::MAX_MISSION_ITEMS`, repeated because this grammar owns its own bounds and a bound
/// borrowed from another module drifts when that module's changes.
const MAX_CONDITIONS: usize = 20;
const MAX_SUPPORT_PER_CONDITION: usize = 8;
const MAX_DELIVERED_ARTIFACTS: usize = 32;
const MAX_NARRATIVE_ITEMS: usize = 32;
const MAX_SHORT_TEXT_SCALARS: usize = 500;
const MAX_SUMMARY_SCALARS: usize = 2_000;

/// The smallest number of bytes one entry of each list can occupy: every fixed-width field and every
/// length prefix, with zero-length strings. A declared count is proved against the remaining input
/// at this size before the first entry is read and before anything reserves capacity.
const MIN_CONDITION_BYTES: usize = 10;
const MIN_SUPPORT_BYTES: usize = 33;
const MIN_ARTIFACT_BYTES: usize = 32;
const MIN_NARRATIVE_BYTES: usize = 14;

const TAG_ACTOR_OPERATOR: u8 = 0x01;

const TAG_DISPOSITION_CONCLUDED: u8 = 0x01;
const TAG_DISPOSITION_ABANDONED: u8 = 0x02;

const TAG_OUTCOME_MET: u8 = 0x01;
const TAG_OUTCOME_NOT_MET: u8 = 0x02;
const TAG_OUTCOME_NOT_ASSESSED: u8 = 0x03;
const TAG_OUTCOME_ASSESSMENT_INCONCLUSIVE: u8 = 0x04;
/// Reserved by name for `unsupported_reference` and deliberately never assigned in v1. Named here so
/// no second implementation picks it for something else.
const TAG_OUTCOME_RESERVED_UNSUPPORTED_REFERENCE: u8 = 0x05;

const TAG_SUPPORT_ARTIFACT: u8 = 0x01;
const TAG_SUPPORT_RECEIPT: u8 = 0x02;
const TAG_SUPPORT_EVALUATION: u8 = 0x03;
const TAG_SUPPORT_RULING: u8 = 0x04;

const TAG_UNFINISHED_CONDITION_NOT_MET: u8 = 0x01;
const TAG_UNFINISHED_DEFERRED_SCOPE: u8 = 0x02;
const TAG_UNFINISHED_KNOWN_DEFECT: u8 = 0x03;
const TAG_UNFINISHED_BLOCKED_DEPENDENCY: u8 = 0x04;

const TAG_UNCERTAINTY_UNVERIFIED_CLAIM: u8 = 0x01;
const TAG_UNCERTAINTY_UNMEASURED_QUANTITY: u8 = 0x02;
const TAG_UNCERTAINTY_CONFLICTING_EVIDENCE: u8 = 0x03;
const TAG_UNCERTAINTY_OUTSIDE_SCOPE_OF_CHECKS: u8 = 0x04;

const TAG_NONE: u8 = 0x00;
const TAG_SOME: u8 = 0x01;

const TAG_SPEND_RECORDED: u8 = 0x01;
const TAG_SPEND_NOT_MEASURED: u8 = 0x02;

// ---------------------------------------------------------------------------------------------
// The vocabularies
// ---------------------------------------------------------------------------------------------

/// The authenticated command actor. One closed arm, because there is no authenticated Resident
/// command path: the Operator writes completion, and the record names the accountable Resident as a
/// fact about the work rather than as its author.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionActor {
    Operator,
}

/// How the Mission finished. Neither arm is a success verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionDisposition {
    /// The Team finished the work and reported per-condition results.
    Concluded,
    /// The Mission was stopped. Conditions may be `NotAssessed`, and that is a decision rather than
    /// neglect.
    Abandoned,
}

/// What one declared success condition came to. See the module docs for the fifth and sixth states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionOutcome {
    /// Assessed, and it holds.
    Met,
    /// Assessed, and it does not hold. Not "the assessment failed" -- that is
    /// [`Self::AssessmentInconclusive`].
    NotMet,
    /// Nobody looked. Not a negative.
    NotAssessed,
    /// Somebody looked and could not establish an answer.
    AssessmentInconclusive,
}

/// What kind of record supports a condition's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SupportKind {
    Artifact,
    Receipt,
    Evaluation,
    Ruling,
}

/// One piece of support, identified as its owning module identifies it.
///
/// A Receipt is the odd one and deliberately so: Receipts in this store are not digest-identified
/// (ADR 0039), so the reference is `(run, receipt version)` plus a checksum over the exact stored
/// `receipt_json` -- a completion verification checksum, **never** a retroactive Receipt domain
/// identity. Everything else already has a canonical digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportRef {
    /// The Artifact **record** digest (R16), not its content digest.
    Artifact([u8; 32]),
    Receipt {
        run_id: Uuid,
        receipt_version: u32,
        receipt_bytes_sha256: [u8; 32],
    },
    Evaluation([u8; 32]),
    Ruling([u8; 32]),
}

impl SupportRef {
    fn kind(&self) -> SupportKind {
        match self {
            Self::Artifact(_) => SupportKind::Artifact,
            Self::Receipt { .. } => SupportKind::Receipt,
            Self::Evaluation(_) => SupportKind::Evaluation,
            Self::Ruling(_) => SupportKind::Ruling,
        }
    }

    fn tag(&self) -> u8 {
        match self {
            Self::Artifact(_) => TAG_SUPPORT_ARTIFACT,
            Self::Receipt { .. } => TAG_SUPPORT_RECEIPT,
            Self::Evaluation(_) => TAG_SUPPORT_EVALUATION,
            Self::Ruling(_) => TAG_SUPPORT_RULING,
        }
    }

    /// The bytes this reference sorts by, after its kind tag. A set needs one total order, and
    /// comparing the identity's own bytes is the only order that does not depend on a field name.
    fn order_key(&self) -> Vec<u8> {
        match self {
            Self::Artifact(digest) | Self::Evaluation(digest) | Self::Ruling(digest) => {
                digest.to_vec()
            }
            Self::Receipt {
                run_id,
                receipt_version,
                receipt_bytes_sha256,
            } => {
                let mut key = run_id.as_bytes().to_vec();
                key.extend_from_slice(&receipt_version.to_be_bytes());
                key.extend_from_slice(receipt_bytes_sha256);
                key
            }
        }
    }
}

/// One declared condition's result, keyed on the condition identity migration 0024 gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionResultRecord {
    /// The condition's identity: its 0-based index in the Mission's declaration order.
    pub ordinal: u32,
    pub outcome: ConditionOutcome,
    /// A **set**: a permutation of the same support cannot produce a second completion identity.
    pub support: Vec<SupportRef>,
    /// Absence is a value, encoded as one.
    pub note: Option<String>,
}

/// Why something declared is not finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnfinishedKind {
    ConditionNotMet,
    DeferredScope,
    KnownDefect,
    BlockedDependency,
}

impl UnfinishedKind {
    /// The lowercase token this kind renders as -- the one in the record's own field table.
    ///
    /// It lives here rather than in a consumer because the vocabulary is this record's (R13.3): a
    /// second module spelling these four words is how two spellings of one kind begin.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConditionNotMet => "condition_not_met",
            Self::DeferredScope => "deferred_scope",
            Self::KnownDefect => "known_defect",
            Self::BlockedDependency => "blocked_dependency",
        }
    }
}

/// What the Team could not establish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncertaintyKind {
    UnverifiedClaim,
    UnmeasuredQuantity,
    ConflictingEvidence,
    OutsideScopeOfChecks,
}

impl UncertaintyKind {
    /// The lowercase token this kind renders as -- the one in the record's own field table.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnverifiedClaim => "unverified_claim",
            Self::UnmeasuredQuantity => "unmeasured_quantity",
            Self::ConflictingEvidence => "conflicting_evidence",
            Self::OutsideScopeOfChecks => "outside_scope_of_checks",
        }
    }
}

/// One narrative item. The `kind` is the machine-readable part; the `summary` is **never parsed**.
/// A consumer that needs a machine fact needs a new kind tag, and adding one is a new canonicalizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarrativeItem<K> {
    /// This item's stable identity within the completion. Dense from 0, ascending.
    pub ordinal: u32,
    pub kind: K,
    /// The condition this relates to, when it relates to one. Typed, never a sentinel.
    pub condition_ordinal: Option<u32>,
    pub summary: String,
}

/// Where the measured spend is recorded, or why it is not.
///
/// Typed absent is never zero -- the crate's own `Recorded<T>` precedent in canonical-bytes form.
/// The completion carries no amount, no currency and no coverage window, so no consumer can do
/// arithmetic on it: those belong to `MeasuredSpendV1` (ADR 0042).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendReference {
    Recorded {
        /// The spend record's reference identity (R15).
        spend_id: Uuid,
        spend_version: u32,
        spend_sha256: [u8; 32],
    },
    NotMeasured(String),
}

/// Whether this record replaces an earlier completion of the same Mission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionSupersedes {
    None,
    Record([u8; 32]),
}

/// The immutable v1 payload, exactly as its canonical bytes decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionCompletionV1 {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub mission_id: Uuid,
    /// The record's **reference** identity: which record a citation names (R15). The digest is its
    /// **content** identity: what the bytes are.
    pub completion_id: Uuid,
    pub actor: CompletionActor,
    pub accountable_resident_id: Uuid,
    pub accountable_resident_configuration_id: Uuid,
    /// The cut this record reports at. Everything it says was established at or before this event;
    /// append-only records after it do not alter this completion and are not claimed by it.
    pub boundary_event_seq: u64,
    pub disposition: CompletionDisposition,
    /// Over the Mission's ordered condition list. The comparison against the Mission is the next
    /// slice's; the field is here because a later reader must be able to make it.
    pub declared_conditions_sha256: [u8; 32],
    pub condition_results: Vec<ConditionResultRecord>,
    /// The Artifact **record** digests this Mission delivered. A set.
    pub delivered_artifacts: Vec<[u8; 32]>,
    /// A sequence: the writer put these in an order and re-sorting would be the record deciding
    /// what mattered.
    pub unfinished_work: Vec<NarrativeItem<UnfinishedKind>>,
    pub uncertainty: Vec<NarrativeItem<UncertaintyKind>>,
    pub spend: SpendReference,
    pub supersedes: CompletionSupersedes,
    pub summary: String,
    /// Service clock, never caller input.
    pub completed_at_ms: u64,
}

/// A digest that carries its algorithm and canonicalizer identity, never a bare hex string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionDigest {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

// ---------------------------------------------------------------------------------------------
// Refusals -- each names the rule it broke
// ---------------------------------------------------------------------------------------------

/// Bytes or field values that cannot be a v1 mission-completion record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedCompletion {
    Magic,
    Truncated,
    TrailingBytes,
    UnknownTag,
    InvalidUtf8,
    /// The whole record is larger than the envelope a store will hold.
    RecordTooLarge,
    /// A declared length or count larger than its field's limit.
    OverLimitLength,
    /// No condition result at all. A Mission declares at least one condition, so a completion that
    /// reports on none reports on nothing.
    NoConditionResults,
    /// Ordinals that are not dense from zero and ascending. The ordinal IS the condition's
    /// identity, so a gap, a duplicate or a reordering is a different record about different
    /// conditions.
    ConditionOrdinals,
    /// Narrative item ordinals that are not dense from zero and ascending.
    NarrativeOrdinals,
    /// Two support references naming one object, or support not in canonical order.
    SupportOrder,
    /// Two delivered Artifacts naming one digest, or artifacts not in canonical order.
    DeliveredArtifactOrder,
    EmptyText,
    UntrimmedText,
    TextTooLong,
    /// A control character other than LF or TAB (R13 item 5, applied to a record).
    ControlCharacter,
}

impl MalformedCompletion {
    /// A stable lowercase reason token. Refusals are named, never spelled differently by two
    /// callers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Magic => "magic",
            Self::Truncated => "truncated",
            Self::TrailingBytes => "trailing_bytes",
            Self::UnknownTag => "unknown_tag",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::RecordTooLarge => "record_too_large",
            Self::OverLimitLength => "over_limit_length",
            Self::NoConditionResults => "no_condition_results",
            Self::ConditionOrdinals => "condition_ordinals",
            Self::NarrativeOrdinals => "narrative_ordinals",
            Self::SupportOrder => "support_order",
            Self::DeliveredArtifactOrder => "delivered_artifact_order",
            Self::EmptyText => "empty_text",
            Self::UntrimmedText => "untrimmed_text",
            Self::TextTooLong => "text_too_long",
            Self::ControlCharacter => "control_character",
        }
    }
}

impl fmt::Display for MalformedCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for MalformedCompletion {}

// ---------------------------------------------------------------------------------------------
// Domain checks the bytes alone can settle
// ---------------------------------------------------------------------------------------------

/// Human text a record may carry: LF and TAB are content, every other control character is refused
/// by name (R13 item 5).
fn require_text(value: &str, max_scalars: usize) -> Result<(), MalformedCompletion> {
    if value.is_empty() {
        return Err(MalformedCompletion::EmptyText);
    }
    if value.trim() != value {
        return Err(MalformedCompletion::UntrimmedText);
    }
    if value.chars().count() > max_scalars {
        return Err(MalformedCompletion::TextTooLong);
    }
    if value
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(MalformedCompletion::ControlCharacter);
    }
    Ok(())
}

/// Ordinals must be dense from zero and ascending: they are identities, not decoration.
fn require_dense_ordinals(
    ordinals: impl Iterator<Item = u32>,
    refusal: MalformedCompletion,
) -> Result<(), MalformedCompletion> {
    for (expected, actual) in ordinals.enumerate() {
        if u32::try_from(expected) != Ok(actual) {
            return Err(refusal);
        }
    }
    Ok(())
}

fn require_condition_results(results: &[ConditionResultRecord]) -> Result<(), MalformedCompletion> {
    if results.is_empty() {
        return Err(MalformedCompletion::NoConditionResults);
    }
    if results.len() > MAX_CONDITIONS {
        return Err(MalformedCompletion::OverLimitLength);
    }
    require_dense_ordinals(
        results.iter().map(|result| result.ordinal),
        MalformedCompletion::ConditionOrdinals,
    )?;
    for result in results {
        if result.support.len() > MAX_SUPPORT_PER_CONDITION {
            return Err(MalformedCompletion::OverLimitLength);
        }
        // A set: one total order, strictly ascending, so a permutation of the same support cannot
        // produce a second identity for one completion.
        for window in result.support.windows(2) {
            let left = (window[0].kind(), window[0].order_key());
            let right = (window[1].kind(), window[1].order_key());
            if left.cmp(&right) != Ordering::Less {
                return Err(MalformedCompletion::SupportOrder);
            }
        }
        if let Some(note) = &result.note {
            require_text(note, MAX_SHORT_TEXT_SCALARS)?;
        }
    }
    Ok(())
}

fn require_delivered_artifacts(artifacts: &[[u8; 32]]) -> Result<(), MalformedCompletion> {
    if artifacts.len() > MAX_DELIVERED_ARTIFACTS {
        return Err(MalformedCompletion::OverLimitLength);
    }
    for window in artifacts.windows(2) {
        if window[0].cmp(&window[1]) != Ordering::Less {
            return Err(MalformedCompletion::DeliveredArtifactOrder);
        }
    }
    Ok(())
}

fn require_narrative<K>(items: &[NarrativeItem<K>]) -> Result<(), MalformedCompletion> {
    if items.len() > MAX_NARRATIVE_ITEMS {
        return Err(MalformedCompletion::OverLimitLength);
    }
    require_dense_ordinals(
        items.iter().map(|item| item.ordinal),
        MalformedCompletion::NarrativeOrdinals,
    )?;
    for item in items {
        require_text(&item.summary, MAX_SHORT_TEXT_SCALARS)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The canonical bytes
// ---------------------------------------------------------------------------------------------

struct Encoder {
    out: Vec<u8>,
}

impl Encoder {
    fn tag(&mut self, value: u8) {
        self.out.push(value);
    }
    fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }
    fn uuid(&mut self, value: &Uuid) {
        self.out.extend_from_slice(value.as_bytes());
    }
    fn sha256(&mut self, value: &[u8; 32]) {
        self.out.extend_from_slice(value);
    }
    fn string(&mut self, value: &str, max_scalars: usize) -> Result<(), MalformedCompletion> {
        require_text(value, max_scalars)?;
        let length =
            u64::try_from(value.len()).map_err(|_| MalformedCompletion::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value.as_bytes());
        Ok(())
    }
    /// A declared count, refused here as well as in the decoder so the two cannot disagree.
    fn count(&mut self, len: usize, limit: usize) -> Result<(), MalformedCompletion> {
        if len > limit {
            return Err(MalformedCompletion::OverLimitLength);
        }
        self.u32(u32::try_from(len).map_err(|_| MalformedCompletion::OverLimitLength)?);
        Ok(())
    }
}

struct Decoder<'a> {
    input: &'a [u8],
    at: usize,
}

impl<'a> Decoder<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, at: 0 }
    }
    fn remaining(&self) -> usize {
        self.input.len() - self.at
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8], MalformedCompletion> {
        if self.remaining() < length {
            return Err(MalformedCompletion::Truncated);
        }
        let slice = &self.input[self.at..self.at + length];
        self.at += length;
        Ok(slice)
    }
    fn magic(&mut self) -> Result<(), MalformedCompletion> {
        if self.take(MAGIC.len())? == MAGIC {
            Ok(())
        } else {
            Err(MalformedCompletion::Magic)
        }
    }
    fn tag(&mut self) -> Result<u8, MalformedCompletion> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, MalformedCompletion> {
        let mut value = [0_u8; 4];
        value.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(value))
    }
    fn u64(&mut self) -> Result<u64, MalformedCompletion> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }
    fn uuid(&mut self) -> Result<Uuid, MalformedCompletion> {
        let mut value = [0_u8; 16];
        value.copy_from_slice(self.take(16)?);
        Ok(Uuid::from_bytes(value))
    }
    fn sha256(&mut self) -> Result<[u8; 32], MalformedCompletion> {
        let mut value = [0_u8; 32];
        value.copy_from_slice(self.take(32)?);
        Ok(value)
    }
    fn string(&mut self, max_scalars: usize) -> Result<String, MalformedCompletion> {
        let length = self.u64()?;
        // Four bytes is the widest a scalar encodes to, so this bounds the read without deciding
        // the scalar count -- which `require_text` settles afterwards, on the decoded value.
        let limit =
            u64::try_from(max_scalars * 4).map_err(|_| MalformedCompletion::OverLimitLength)?;
        if length > limit {
            return Err(MalformedCompletion::OverLimitLength);
        }
        let length = usize::try_from(length).map_err(|_| MalformedCompletion::Truncated)?;
        let bytes = self.take(length)?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| MalformedCompletion::InvalidUtf8)?
            .to_owned();
        require_text(&text, max_scalars)?;
        Ok(text)
    }
    /// Reads a declared count and proves the input could hold that many entries, before the first
    /// entry is read and before any `Vec` reserves for it.
    fn count(&mut self, limit: usize, entry_bytes: usize) -> Result<usize, MalformedCompletion> {
        let count =
            usize::try_from(self.u32()?).map_err(|_| MalformedCompletion::OverLimitLength)?;
        if count > limit {
            return Err(MalformedCompletion::OverLimitLength);
        }
        let smallest = count
            .checked_mul(entry_bytes)
            .ok_or(MalformedCompletion::Truncated)?;
        if smallest > self.remaining() {
            return Err(MalformedCompletion::Truncated);
        }
        Ok(count)
    }
    fn finish(self) -> Result<(), MalformedCompletion> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MalformedCompletion::TrailingBytes)
        }
    }
}

fn outcome_tag(outcome: ConditionOutcome) -> u8 {
    match outcome {
        ConditionOutcome::Met => TAG_OUTCOME_MET,
        ConditionOutcome::NotMet => TAG_OUTCOME_NOT_MET,
        ConditionOutcome::NotAssessed => TAG_OUTCOME_NOT_ASSESSED,
        ConditionOutcome::AssessmentInconclusive => TAG_OUTCOME_ASSESSMENT_INCONCLUSIVE,
    }
}

fn parse_outcome(tag: u8) -> Result<ConditionOutcome, MalformedCompletion> {
    match tag {
        TAG_OUTCOME_MET => Ok(ConditionOutcome::Met),
        TAG_OUTCOME_NOT_MET => Ok(ConditionOutcome::NotMet),
        TAG_OUTCOME_NOT_ASSESSED => Ok(ConditionOutcome::NotAssessed),
        TAG_OUTCOME_ASSESSMENT_INCONCLUSIVE => Ok(ConditionOutcome::AssessmentInconclusive),
        // `0x05` is reserved by name for `unsupported_reference` and never assigned in v1, so a
        // record carrying it is refused like any other unassigned tag. Named rather than folded
        // into the wildcard so a reader of this match sees the number is spoken for.
        TAG_OUTCOME_RESERVED_UNSUPPORTED_REFERENCE => Err(MalformedCompletion::UnknownTag),
        _ => Err(MalformedCompletion::UnknownTag),
    }
}

fn unfinished_tag(kind: UnfinishedKind) -> u8 {
    match kind {
        UnfinishedKind::ConditionNotMet => TAG_UNFINISHED_CONDITION_NOT_MET,
        UnfinishedKind::DeferredScope => TAG_UNFINISHED_DEFERRED_SCOPE,
        UnfinishedKind::KnownDefect => TAG_UNFINISHED_KNOWN_DEFECT,
        UnfinishedKind::BlockedDependency => TAG_UNFINISHED_BLOCKED_DEPENDENCY,
    }
}

fn parse_unfinished(tag: u8) -> Result<UnfinishedKind, MalformedCompletion> {
    match tag {
        TAG_UNFINISHED_CONDITION_NOT_MET => Ok(UnfinishedKind::ConditionNotMet),
        TAG_UNFINISHED_DEFERRED_SCOPE => Ok(UnfinishedKind::DeferredScope),
        TAG_UNFINISHED_KNOWN_DEFECT => Ok(UnfinishedKind::KnownDefect),
        TAG_UNFINISHED_BLOCKED_DEPENDENCY => Ok(UnfinishedKind::BlockedDependency),
        _ => Err(MalformedCompletion::UnknownTag),
    }
}

fn uncertainty_tag(kind: UncertaintyKind) -> u8 {
    match kind {
        UncertaintyKind::UnverifiedClaim => TAG_UNCERTAINTY_UNVERIFIED_CLAIM,
        UncertaintyKind::UnmeasuredQuantity => TAG_UNCERTAINTY_UNMEASURED_QUANTITY,
        UncertaintyKind::ConflictingEvidence => TAG_UNCERTAINTY_CONFLICTING_EVIDENCE,
        UncertaintyKind::OutsideScopeOfChecks => TAG_UNCERTAINTY_OUTSIDE_SCOPE_OF_CHECKS,
    }
}

fn parse_uncertainty(tag: u8) -> Result<UncertaintyKind, MalformedCompletion> {
    match tag {
        TAG_UNCERTAINTY_UNVERIFIED_CLAIM => Ok(UncertaintyKind::UnverifiedClaim),
        TAG_UNCERTAINTY_UNMEASURED_QUANTITY => Ok(UncertaintyKind::UnmeasuredQuantity),
        TAG_UNCERTAINTY_CONFLICTING_EVIDENCE => Ok(UncertaintyKind::ConflictingEvidence),
        TAG_UNCERTAINTY_OUTSIDE_SCOPE_OF_CHECKS => Ok(UncertaintyKind::OutsideScopeOfChecks),
        _ => Err(MalformedCompletion::UnknownTag),
    }
}

fn encode_narrative<K: Copy>(
    encoder: &mut Encoder,
    items: &[NarrativeItem<K>],
    tag_of: impl Fn(K) -> u8,
) -> Result<(), MalformedCompletion> {
    require_narrative(items)?;
    encoder.count(items.len(), MAX_NARRATIVE_ITEMS)?;
    for item in items {
        encoder.u32(item.ordinal);
        encoder.tag(tag_of(item.kind));
        match item.condition_ordinal {
            None => encoder.tag(TAG_NONE),
            Some(ordinal) => {
                encoder.tag(TAG_SOME);
                encoder.u32(ordinal);
            }
        }
        encoder.string(&item.summary, MAX_SHORT_TEXT_SCALARS)?;
    }
    Ok(())
}

fn decode_narrative<K>(
    decoder: &mut Decoder<'_>,
    parse: impl Fn(u8) -> Result<K, MalformedCompletion>,
) -> Result<Vec<NarrativeItem<K>>, MalformedCompletion> {
    let count = decoder.count(MAX_NARRATIVE_ITEMS, MIN_NARRATIVE_BYTES)?;
    let mut items = Vec::new();
    for _ in 0..count {
        let ordinal = decoder.u32()?;
        let kind = parse(decoder.tag()?)?;
        let condition_ordinal = match decoder.tag()? {
            TAG_NONE => None,
            TAG_SOME => Some(decoder.u32()?),
            _ => return Err(MalformedCompletion::UnknownTag),
        };
        let summary = decoder.string(MAX_SHORT_TEXT_SCALARS)?;
        items.push(NarrativeItem {
            ordinal,
            kind,
            condition_ordinal,
            summary,
        });
    }
    require_narrative(&items)?;
    Ok(items)
}

fn encode(record: &MissionCompletionV1) -> Result<Vec<u8>, MalformedCompletion> {
    require_condition_results(&record.condition_results)?;
    require_delivered_artifacts(&record.delivered_artifacts)?;

    let mut encoder = Encoder {
        out: MAGIC.to_vec(),
    };
    encoder.uuid(&record.workspace_id);
    encoder.uuid(&record.project_id);
    encoder.uuid(&record.mission_id);
    encoder.uuid(&record.completion_id);
    encoder.tag(match record.actor {
        CompletionActor::Operator => TAG_ACTOR_OPERATOR,
    });
    encoder.uuid(&record.accountable_resident_id);
    encoder.uuid(&record.accountable_resident_configuration_id);
    encoder.u64(record.boundary_event_seq);
    encoder.tag(match record.disposition {
        CompletionDisposition::Concluded => TAG_DISPOSITION_CONCLUDED,
        CompletionDisposition::Abandoned => TAG_DISPOSITION_ABANDONED,
    });
    encoder.sha256(&record.declared_conditions_sha256);

    encoder.count(record.condition_results.len(), MAX_CONDITIONS)?;
    for result in &record.condition_results {
        encoder.u32(result.ordinal);
        encoder.tag(outcome_tag(result.outcome));
        encoder.count(result.support.len(), MAX_SUPPORT_PER_CONDITION)?;
        for support in &result.support {
            encoder.tag(support.tag());
            match support {
                SupportRef::Artifact(digest)
                | SupportRef::Evaluation(digest)
                | SupportRef::Ruling(digest) => encoder.sha256(digest),
                SupportRef::Receipt {
                    run_id,
                    receipt_version,
                    receipt_bytes_sha256,
                } => {
                    encoder.uuid(run_id);
                    encoder.u32(*receipt_version);
                    encoder.sha256(receipt_bytes_sha256);
                }
            }
        }
        match &result.note {
            None => encoder.tag(TAG_NONE),
            Some(note) => {
                encoder.tag(TAG_SOME);
                encoder.string(note, MAX_SHORT_TEXT_SCALARS)?;
            }
        }
    }

    encoder.count(record.delivered_artifacts.len(), MAX_DELIVERED_ARTIFACTS)?;
    for digest in &record.delivered_artifacts {
        encoder.sha256(digest);
    }

    encode_narrative(&mut encoder, &record.unfinished_work, unfinished_tag)?;
    encode_narrative(&mut encoder, &record.uncertainty, uncertainty_tag)?;

    match &record.spend {
        SpendReference::Recorded {
            spend_id,
            spend_version,
            spend_sha256,
        } => {
            encoder.tag(TAG_SPEND_RECORDED);
            encoder.uuid(spend_id);
            encoder.u32(*spend_version);
            encoder.sha256(spend_sha256);
        }
        SpendReference::NotMeasured(reason) => {
            encoder.tag(TAG_SPEND_NOT_MEASURED);
            encoder.string(reason, MAX_SHORT_TEXT_SCALARS)?;
        }
    }

    match &record.supersedes {
        CompletionSupersedes::None => encoder.tag(TAG_NONE),
        CompletionSupersedes::Record(digest) => {
            encoder.tag(TAG_SOME);
            encoder.sha256(digest);
        }
    }

    encoder.string(&record.summary, MAX_SUMMARY_SCALARS)?;
    encoder.u64(record.completed_at_ms);

    if encoder.out.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedCompletion::RecordTooLarge);
    }
    Ok(encoder.out)
}

fn decode(input: &[u8]) -> Result<MissionCompletionV1, MalformedCompletion> {
    // Measured before the input is split: a decoder that measures afterwards has already done the
    // work it meant to refuse.
    if input.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedCompletion::RecordTooLarge);
    }
    let mut decoder = Decoder::new(input);
    decoder.magic()?;

    let workspace_id = decoder.uuid()?;
    let project_id = decoder.uuid()?;
    let mission_id = decoder.uuid()?;
    let completion_id = decoder.uuid()?;
    let actor = match decoder.tag()? {
        TAG_ACTOR_OPERATOR => CompletionActor::Operator,
        _ => return Err(MalformedCompletion::UnknownTag),
    };
    let accountable_resident_id = decoder.uuid()?;
    let accountable_resident_configuration_id = decoder.uuid()?;
    let boundary_event_seq = decoder.u64()?;
    let disposition = match decoder.tag()? {
        TAG_DISPOSITION_CONCLUDED => CompletionDisposition::Concluded,
        TAG_DISPOSITION_ABANDONED => CompletionDisposition::Abandoned,
        _ => return Err(MalformedCompletion::UnknownTag),
    };
    let declared_conditions_sha256 = decoder.sha256()?;

    let condition_count = decoder.count(MAX_CONDITIONS, MIN_CONDITION_BYTES)?;
    let mut condition_results = Vec::new();
    for _ in 0..condition_count {
        let ordinal = decoder.u32()?;
        let outcome = parse_outcome(decoder.tag()?)?;
        let support_count = decoder.count(MAX_SUPPORT_PER_CONDITION, MIN_SUPPORT_BYTES)?;
        let mut support = Vec::new();
        for _ in 0..support_count {
            support.push(match decoder.tag()? {
                TAG_SUPPORT_ARTIFACT => SupportRef::Artifact(decoder.sha256()?),
                TAG_SUPPORT_RECEIPT => SupportRef::Receipt {
                    run_id: decoder.uuid()?,
                    receipt_version: decoder.u32()?,
                    receipt_bytes_sha256: decoder.sha256()?,
                },
                TAG_SUPPORT_EVALUATION => SupportRef::Evaluation(decoder.sha256()?),
                TAG_SUPPORT_RULING => SupportRef::Ruling(decoder.sha256()?),
                _ => return Err(MalformedCompletion::UnknownTag),
            });
        }
        let note = match decoder.tag()? {
            TAG_NONE => None,
            TAG_SOME => Some(decoder.string(MAX_SHORT_TEXT_SCALARS)?),
            _ => return Err(MalformedCompletion::UnknownTag),
        };
        condition_results.push(ConditionResultRecord {
            ordinal,
            outcome,
            support,
            note,
        });
    }
    require_condition_results(&condition_results)?;

    let artifact_count = decoder.count(MAX_DELIVERED_ARTIFACTS, MIN_ARTIFACT_BYTES)?;
    let mut delivered_artifacts = Vec::new();
    for _ in 0..artifact_count {
        delivered_artifacts.push(decoder.sha256()?);
    }
    require_delivered_artifacts(&delivered_artifacts)?;

    let unfinished_work = decode_narrative(&mut decoder, parse_unfinished)?;
    let uncertainty = decode_narrative(&mut decoder, parse_uncertainty)?;

    let spend = match decoder.tag()? {
        TAG_SPEND_RECORDED => SpendReference::Recorded {
            spend_id: decoder.uuid()?,
            spend_version: decoder.u32()?,
            spend_sha256: decoder.sha256()?,
        },
        TAG_SPEND_NOT_MEASURED => {
            SpendReference::NotMeasured(decoder.string(MAX_SHORT_TEXT_SCALARS)?)
        }
        _ => return Err(MalformedCompletion::UnknownTag),
    };

    let supersedes = match decoder.tag()? {
        TAG_NONE => CompletionSupersedes::None,
        TAG_SOME => CompletionSupersedes::Record(decoder.sha256()?),
        _ => return Err(MalformedCompletion::UnknownTag),
    };

    let summary = decoder.string(MAX_SUMMARY_SCALARS)?;
    let completed_at_ms = decoder.u64()?;
    decoder.finish()?;

    Ok(MissionCompletionV1 {
        workspace_id,
        project_id,
        mission_id,
        completion_id,
        actor,
        accountable_resident_id,
        accountable_resident_configuration_id,
        boundary_event_seq,
        disposition,
        declared_conditions_sha256,
        condition_results,
        delivered_artifacts,
        unfinished_work,
        uncertainty,
        spend,
        supersedes,
        summary,
        completed_at_ms,
    })
}

impl MissionCompletionV1 {
    /// The one canonical byte encoding of this record.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MalformedCompletion> {
        encode(self)
    }

    /// The record's content identity: SHA-256 over its canonical bytes, carrying its own envelope.
    pub fn digest(&self) -> Result<CompletionDigest, MalformedCompletion> {
        Ok(CompletionDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: COMPLETION_CANONICALIZER.to_owned(),
            value: HEXLOWER.encode(&sha256(&self.canonical_bytes()?)),
        })
    }

    /// Decode canonical bytes back into a record. Strict: unknown tags, over-limit lengths, trailing
    /// bytes, invalid UTF-8, non-dense ordinals and non-canonical set order are all refused.
    pub fn from_canonical_bytes(input: &[u8]) -> Result<Self, MalformedCompletion> {
        decode(input)
    }
}
