//! Outcome Document v1, slice 1: the pure manifest model and the Markdown renderer.
//!
//! An Outcome Document is the durable, human-readable Artifact that records what happened for one
//! Mission at one fixed event boundary. This slice is deliberately pure: it owns the immutable
//! [`OutcomeManifestV1`], its canonical bytes, the four honesty states, the three function-owned
//! bounds, and one in-memory renderer. It reads no database, writes no Activity, produces no PDF,
//! and has no UI. The verified collector, candidate Artifact, publication qualification, and
//! exports are later slices (ADR 0040).
//!
//! Two structures, not one. The manifest carries identities, digests, and states; it never carries
//! a joined projection of another module's text. [`VerifiedSources`] carries the values the owning
//! modules' verified readers returned at the manifest's boundary event. Slice 1 constructs them by
//! hand; slice 2's collector produces them. The renderer takes both and refuses when they do not
//! correspond exactly, which is what stops it inventing a row that no verified read produced.
//!
//! Nothing here creates authority. A rendered Outcome is a document with a history: no module may
//! derive permission, Mission completion, or a success verdict from its existence.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

use data_encoding::HEXLOWER;
use uuid::Uuid;

use crate::proposal::sha256;

// ---------------------------------------------------------------------------------------------
// Identities, limits, and the closed tag table -- the executable form of ADR 0040
// ---------------------------------------------------------------------------------------------

/// The canonicalizer identity. Any future grammar is a new canonicalizer; existing bytes are never
/// reinterpreted under a later grammar's rules (ADR 0034's rule, kept).
///
/// **v2 is the R18.3-5 grammar revision**, made before any manifest was persisted outside tests:
/// the condition vocabulary widened to the completion's own, the Mission cited by identity, and
/// Proposal / Ruling / Evaluation cited as `kind + workspace + digest` with typed identity present
/// only where the owning record carries one inside its canonical bytes. v1 is RETIRED, not
/// reinterpreted: [`RETIRED_CANONICALIZER`] bytes are refused by name, because no v1 manifest exists
/// anywhere but a test fixture and pretending to read one would be a decoder lying about a grammar
/// it does not hold.
pub const CANONICALIZER: &str = "beacon.outcome.manifest.v2";
/// The literal ASCII magic that starts every manifest, including its terminating NUL: 27 bytes.
const MAGIC: &[u8] = b"beacon.outcome.manifest.v2\0";
/// The retired v1 grammar, recognised only to be refused by name.
pub const RETIRED_CANONICALIZER: &str = "beacon.outcome.manifest.v1";
const RETIRED_MAGIC: &[u8] = b"beacon.outcome.manifest.v1\0";
const DIGEST_ALGORITHM: &str = "sha256";

/// The one renderer this version ships. `renderer_version` owns the escaping function: a document
/// says which function escaped its text, and an unknown version is refused rather than defaulted.
pub const RENDERER_V1: &str = "beacon.outcome.md.v1";
/// The media type of the bytes this renderer emits. Named in the appendix, never guessed from a
/// filename.
const CANDIDATE_MEDIA_TYPE: &str = "text/markdown; charset=utf-8";

/// The label carried in the bytes beside every Receipt checksum.
///
/// Kevin's ruling 5: this is an Outcome verification checksum over the exact stored `receipt_json`,
/// not a retroactive Receipt domain digest. ADR 0027 and Receipt v2 define no Receipt content
/// digest, so the label lives in the canonical bytes where it cannot be dropped by a renderer.
pub(crate) const RECEIPT_CHECKSUM_LABEL: &str = "beacon.outcome.receipt-checksum.v1";

/// The three bounds this function owns. Kevin's ruling 9: the Outcome function owns them, a caller
/// cannot supply them, and an exceeded bound refuses the whole render.
pub(crate) const MAX_CITATIONS: u64 = 256;
const MAX_VERIFIED_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EMITTED_MARKDOWN_BYTES: u64 = 1024 * 1024;

const MAX_TEXT_SCALARS: usize = 2_000;
const MAX_TEXT_BYTES: usize = MAX_TEXT_SCALARS * 4;
/// Bounds a declared length before any of it is read, so a hostile length cannot reach a
/// comparison (ruling.rs's rule, kept).
const MAX_TOKEN_BYTES: usize = 64;
/// A decoder-side guard on any declared collection length. It is not a product bound -- the
/// citation bound is -- it only stops a declared count from being trusted.
const MAX_COLLECTION_ENTRIES: u32 = 4_096;

/// The smallest number of bytes one entry of each collection can occupy.
///
/// ADR 0034's accepted rule, and `proposal.rs`'s `Decoder::count`: a declared count is checked
/// against the remaining input using the smallest possible size of one entry, before the first
/// entry is read and before anything reserves capacity for it. A cap alone leaves a hostile count
/// buying 4,096 slots and one decode attempt; the arithmetic refuses it without reading a byte.
///
/// Each figure is the structural minimum: every fixed-width field, every length prefix, and the
/// cheapest arm of every tag, with zero-length strings.
// A source reference's cheapest shape is identity-absent: 16 (Workspace) + 1 (kind) + 1 (identity
// presence) + 32 (sha256) + 8 + 8 (two zero-length token prefixes) + 1 (revision state tag) + 8
// (zero-length reason prefix). The identity-present shape adds 16 + 4 and is never the minimum.
const MIN_SOURCE_REF_BYTES: usize = 75;
const MIN_RECEIPT_REF_BYTES: usize = 84;
const MIN_NARRATIVE_REF_BYTES: usize = 12;
const MIN_CONDITION_BYTES: usize = 19;
const DIGEST_HEX_CHARS: usize = 64;
const GIT_COMMIT_HEX_CHARS: usize = 40;

// One-byte tags. Assigned explicitly and never renumbered.
const TAG_STATE_VERIFIED: u8 = 0x01;
const TAG_STATE_ABSENT: u8 = 0x02;
const TAG_STATE_BROKEN: u8 = 0x03;
const TAG_STATE_NOT_EVALUATED: u8 = 0x04;

const TAG_SOURCE_MISSION: u8 = 0x01;
const TAG_SOURCE_COMPLETION: u8 = 0x02;
const TAG_SOURCE_ARTIFACT: u8 = 0x03;
const TAG_SOURCE_PROPOSAL: u8 = 0x04;
const TAG_SOURCE_RULING: u8 = 0x05;
const TAG_SOURCE_EVALUATION: u8 = 0x06;
const TAG_SOURCE_POLICY: u8 = 0x07;
const TAG_SOURCE_SPEND: u8 = 0x08;

const TAG_CONDITION_MET: u8 = 0x01;
const TAG_CONDITION_NOT_MET: u8 = 0x02;
const TAG_CONDITION_INCONCLUSIVE: u8 = 0x03;

const TAG_APPENDIX_POLICY_DEFAULT: u8 = 0x01;
const TAG_APPENDIX_INCLUDE: u8 = 0x02;

const TAG_REVISION_GIT_COMMIT: u8 = 0x01;

// Reference-identity presence. Explicit, never by omission, and neither arm is 0x00.
const TAG_IDENTITY_NONE: u8 = 0x01;
const TAG_IDENTITY_SOME: u8 = 0x02;

// The spend owner's measurement vocabulary, copied whole (R23.1): four arms, four tags, none
// collapsed. A `SpendRef` carries an amount, so in practice only the first two reach a manifest --
// `Unmeasured` and `Broken` arrive as `OutcomeState`s with no amount to attach a token to -- but a
// closed vocabulary is copied, not abridged, so the day an owner arm changes the codec notices.
const TAG_SPEND_MEASURED: u8 = 0x01;
const TAG_SPEND_PARTIALLY_MEASURED: u8 = 0x02;
const TAG_SPEND_UNMEASURED: u8 = 0x03;
const TAG_SPEND_BROKEN: u8 = 0x04;

const TAG_FALSE: u8 = 0x00;
const TAG_TRUE: u8 = 0x01;

// The literal spellings. They are renderer-owned tokens from closed enums, never human text, so
// they are emitted verbatim and never reach the escaping function: `not_evaluated` must survive
// into the document as `not_evaluated`.
const STATE_VERIFIED: &str = "verified";
const STATE_ABSENT: &str = "absent";
const STATE_BROKEN: &str = "broken";
const STATE_NOT_EVALUATED: &str = "not_evaluated";

const BOUND_CITATION_COUNT: &str = "citation_count";
const BOUND_VERIFIED_SOURCE_BYTES: &str = "verified_source_bytes";
const BOUND_EMITTED_MARKDOWN_BYTES: &str = "emitted_markdown_bytes";

// ---------------------------------------------------------------------------------------------
// The four honesty states
// ---------------------------------------------------------------------------------------------

/// Exactly one of four states for every condition, referenced fact, and optional source.
///
/// The four stay distinct literals in Markdown. They are never converted to blanks, em dashes,
/// `N/A`, success, failure, `$0`, or each other. An empty collection is not one of them either,
/// which is why every collection in the manifest is wrapped here: "the completion listed no
/// delivered Artifacts" and "no Artifact record exists at this boundary" are different claims, and
/// deriving the second from an empty vector is the renderer synthesizing a result from a missing
/// row.
///
/// `Verified(value)` is a claim about the manifest's boundary event, not about read time. Every
/// surface that shows one must say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeState<T> {
    /// The owning verified reader established exact identity, scope, version, stored bytes, and
    /// digest at the manifest's boundary event.
    Verified(T),
    /// The owning domain confirms there is no applicable stored object at the boundary. This is
    /// not a failed lookup.
    Absent(String),
    /// An expected external or material subject exists, but its measured result is definitively
    /// unusable under the owning domain's rules. This is not Beacon-store corruption: corruption
    /// aborts the operation instead of becoming an honest domain row.
    Broken(String),
    /// The object or condition has not received the required evaluation or measurement. It is
    /// neither absent nor broken.
    NotEvaluated(String),
}

impl<T> OutcomeState<T> {
    fn tag(&self) -> u8 {
        match self {
            Self::Verified(_) => TAG_STATE_VERIFIED,
            Self::Absent(_) => TAG_STATE_ABSENT,
            Self::Broken(_) => TAG_STATE_BROKEN,
            Self::NotEvaluated(_) => TAG_STATE_NOT_EVALUATED,
        }
    }

    /// The literal spelling this state renders as. Four distinct words, no synonyms.
    pub fn literal(&self) -> &'static str {
        match self {
            Self::Verified(_) => STATE_VERIFIED,
            Self::Absent(_) => STATE_ABSENT,
            Self::Broken(_) => STATE_BROKEN,
            Self::NotEvaluated(_) => STATE_NOT_EVALUATED,
        }
    }

    fn reason(&self) -> Option<&str> {
        match self {
            Self::Verified(_) => None,
            Self::Absent(reason) | Self::Broken(reason) | Self::NotEvaluated(reason) => {
                Some(reason)
            }
        }
    }

    /// The verified value, if this state carries one.
    pub fn verified(&self) -> Option<&T> {
        match self {
            Self::Verified(value) => Some(value),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The immutable v1 payload
// ---------------------------------------------------------------------------------------------

/// A digest that carries its algorithm and canonicalizer identity, never a bare hex string.
///
/// A recorded value alone would leave a reader to assume which function produced it: the
/// assumption these records exist to remove (ADR 0034/0037).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDigest {
    pub algorithm: String,
    pub canonicalizer: String,
    /// The lowercase 64-character hexadecimal digest.
    pub value: String,
}

/// The closed set of source kinds a v1 manifest may cite.
///
/// Receipts are not here: they are cited by `(Workspace, run UUID, Receipt version)` plus an
/// Outcome checksum, because Receipt v1/v2 defines no content digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceKind {
    Mission,
    Completion,
    Artifact,
    Proposal,
    Ruling,
    Evaluation,
    Policy,
    Spend,
}

impl SourceKind {
    pub(crate) fn tag(self) -> u8 {
        match self {
            Self::Mission => TAG_SOURCE_MISSION,
            Self::Completion => TAG_SOURCE_COMPLETION,
            Self::Artifact => TAG_SOURCE_ARTIFACT,
            Self::Proposal => TAG_SOURCE_PROPOSAL,
            Self::Ruling => TAG_SOURCE_RULING,
            Self::Evaluation => TAG_SOURCE_EVALUATION,
            Self::Policy => TAG_SOURCE_POLICY,
            Self::Spend => TAG_SOURCE_SPEND,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedOutcome> {
        match tag {
            TAG_SOURCE_MISSION => Ok(Self::Mission),
            TAG_SOURCE_COMPLETION => Ok(Self::Completion),
            TAG_SOURCE_ARTIFACT => Ok(Self::Artifact),
            TAG_SOURCE_PROPOSAL => Ok(Self::Proposal),
            TAG_SOURCE_RULING => Ok(Self::Ruling),
            TAG_SOURCE_EVALUATION => Ok(Self::Evaluation),
            TAG_SOURCE_POLICY => Ok(Self::Policy),
            TAG_SOURCE_SPEND => Ok(Self::Spend),
            _ => Err(MalformedOutcome::UnknownTag),
        }
    }

    /// The lowercase token this kind renders and sorts as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mission => "mission",
            Self::Completion => "completion",
            Self::Artifact => "artifact",
            Self::Proposal => "proposal",
            Self::Ruling => "ruling",
            Self::Evaluation => "evaluation",
            Self::Policy => "policy",
            Self::Spend => "spend",
        }
    }
}

/// The kind of content revision a cited object pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionKind {
    /// A lowercase 40-character hexadecimal Git commit id.
    GitCommit,
}

impl RevisionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::GitCommit => "git_commit",
        }
    }
}

/// A content or repository revision pinned by a cited object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedRevision {
    pub kind: RevisionKind,
    pub value: String,
}

/// A record's REFERENCE identity (R15): which record a citation names, beside its schema version.
/// The digest is CONTENT identity; this is only ever what the owning record carries inside its own
/// canonical bytes, and it is never minted by a manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceIdentity {
    pub object_id: Uuid,
    pub schema_version: u32,
}

/// One cited object: Workspace scope, kind, canonical digest, and -- where the owning record has
/// one -- its reference identity (R18.5).
///
/// `identity` is `Some` exactly for the kinds whose records carry a UUID and version inside their
/// canonical bytes today (Completion, Policy classification, Spend) and `None` for the kinds whose
/// records do not (Artifact, Proposal, Ruling, Evaluation). The codec enforces that per kind in
/// both directions and encodes presence with an explicit tag, never by omission: when a record
/// gains its UUID under R15 the rule for its kind flips, and no byte in any existing manifest has
/// to move to say so. v1 required `object_id` + `schema_version` of every reference, which for four
/// of the six kinds meant a value the manifest had to invent.
///
/// `pinned_revision` is a state rather than an `Option` so that "this source type pins no
/// revision" is a sentence the appendix prints rather than a blank it leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub workspace_id: Uuid,
    pub kind: SourceKind,
    pub identity: Option<ReferenceIdentity>,
    pub digest: SourceDigest,
    pub pinned_revision: OutcomeState<PinnedRevision>,
}

/// One cited Receipt.
///
/// Identified by `(Workspace, run UUID, Receipt version)` -- the identity Receipt v1/v2 actually
/// has -- plus a SHA-256 over the exact stored immutable `receipt_json` bytes. The checksum is
/// labelled in the canonical bytes as an Outcome verification checksum so no later reader can
/// mistake it for a Receipt domain identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRef {
    pub workspace_id: Uuid,
    pub run_uuid: Uuid,
    pub receipt_version: u32,
    /// Lowercase 64-character hexadecimal SHA-256 over the exact stored `receipt_json` bytes.
    pub outcome_checksum: String,
}

/// The completion-owned result for one declared success condition, as the manifest carries it.
///
/// **This is `MissionCompletionV1`'s vocabulary (R13.3, R18.3), widened in the v2 grammar to hold
/// every arm the owner has.** The owner's four arms map onto the manifest's honesty states without
/// collapse:
///
/// | owner (`ConditionOutcome`) | manifest |
/// |---|---|
/// | `Met` | `Verified(Met)` |
/// | `NotMet` | `Verified(NotMet)` |
/// | `AssessmentInconclusive` | `Verified(Inconclusive)` -- evaluated, no verdict |
/// | `NotAssessed` | `NotEvaluated("not_assessed")` -- nobody looked; the same state, the owner's spelling |
///
/// The v1 grammar had two arms and a comment promising this widening. Mapping `inconclusive` onto
/// `NotEvaluated` with a reason string instead would have been the collapse the four-state
/// invariant forbids: a reason is prose beside a state, not a state. [`From<ConditionOutcome>`]
/// below is the one place the mapping lives, and it is exhaustive over the owner's enum so a fifth
/// owner arm fails to compile here rather than mapping to something by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionResult {
    Met,
    NotMet,
    /// Assessed, and the assessment reached no verdict. Not `NotMet` (that is a verdict), and not
    /// `NotEvaluated` (somebody looked).
    Inconclusive,
}

impl From<crate::mission_completion::ConditionOutcome> for OutcomeState<ConditionResult> {
    fn from(outcome: crate::mission_completion::ConditionOutcome) -> Self {
        use crate::mission_completion::ConditionOutcome;
        match outcome {
            ConditionOutcome::Met => OutcomeState::Verified(ConditionResult::Met),
            ConditionOutcome::NotMet => OutcomeState::Verified(ConditionResult::NotMet),
            ConditionOutcome::AssessmentInconclusive => {
                OutcomeState::Verified(ConditionResult::Inconclusive)
            }
            // The owner's own token as the reason, so the document says what the completion said
            // and not a paraphrase of it.
            ConditionOutcome::NotAssessed => OutcomeState::NotEvaluated("not_assessed".to_owned()),
        }
    }
}

impl ConditionResult {
    fn tag(self) -> u8 {
        match self {
            Self::Met => TAG_CONDITION_MET,
            Self::NotMet => TAG_CONDITION_NOT_MET,
            Self::Inconclusive => TAG_CONDITION_INCONCLUSIVE,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedOutcome> {
        match tag {
            TAG_CONDITION_MET => Ok(Self::Met),
            TAG_CONDITION_NOT_MET => Ok(Self::NotMet),
            TAG_CONDITION_INCONCLUSIVE => Ok(Self::Inconclusive),
            _ => Err(MalformedOutcome::UnknownTag),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Met => "met",
            Self::NotMet => "not_met",
            // The owner's spelling, verbatim: the document prints the completion's word.
            Self::Inconclusive => "assessment_inconclusive",
        }
    }
}

/// One declared success condition and what the completion recorded about it.
///
/// Ordinal ordering is the completion's; Outcome does not enforce ordinal uniqueness, because
/// "every declared success condition appears exactly once" is `MissionCompletionV1`'s own
/// validation. Outcome refuses an invalid completion; it does not reimplement the rule. The
/// canonical order is therefore `(ordinal, condition_id)` and must be strictly ascending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuccessCondition {
    pub ordinal: u32,
    /// The completion-owned stable ID, in canonical lowercase token form.
    pub condition_id: String,
    pub outcome: OutcomeState<ConditionResult>,
    /// The objects that support this condition's state.
    pub citations: OutcomeState<Vec<SourceRef>>,
}

/// One completion-owned narrative item: unfinished work or uncertainty.
///
/// The manifest carries the stable item ID and its completion-owned ordinal. The text comes from
/// the verified completion read, never from the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarrativeRef {
    pub ordinal: u32,
    pub item_id: String,
}

/// How much of the Mission's spend the cited record actually measured -- the owner's own word for
/// it (`spend::SpendMeasurement`), carried into the manifest so a partial measurement is a STATE of
/// the document and never presents as a complete one (R21.3, R23.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendMeasurementKind {
    /// The whole declared window was counted.
    Measured,
    /// Some entries in the window could not be counted; the amount is a floor over the window, not
    /// the Mission's spend.
    PartiallyMeasured,
    Unmeasured,
    Broken,
}

impl SpendMeasurementKind {
    fn tag(self) -> u8 {
        match self {
            Self::Measured => TAG_SPEND_MEASURED,
            Self::PartiallyMeasured => TAG_SPEND_PARTIALLY_MEASURED,
            Self::Unmeasured => TAG_SPEND_UNMEASURED,
            Self::Broken => TAG_SPEND_BROKEN,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedOutcome> {
        match tag {
            TAG_SPEND_MEASURED => Ok(Self::Measured),
            TAG_SPEND_PARTIALLY_MEASURED => Ok(Self::PartiallyMeasured),
            TAG_SPEND_UNMEASURED => Ok(Self::Unmeasured),
            TAG_SPEND_BROKEN => Ok(Self::Broken),
            _ => Err(MalformedOutcome::UnknownTag),
        }
    }

    /// The owner's literal, verbatim -- the document prints the spend record's own word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Measured => crate::spend::MEASURED,
            Self::PartiallyMeasured => crate::spend::PARTIALLY_MEASURED,
            Self::Unmeasured => crate::spend::UNMEASURED,
            Self::Broken => crate::spend::BROKEN,
        }
    }
}

/// A measured actual-spend record. Absence is never `$0`: it is a state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendRef {
    pub workspace_id: Uuid,
    pub spend_id: Uuid,
    pub schema_version: u32,
    pub digest: SourceDigest,
    /// Stored integer minor units. Never binary floating point.
    pub amount_minor: i64,
    /// The stored currency or unit token, exactly as stored. Never case-folded or normalized:
    /// those transformations can name a different unit.
    pub currency: String,
    /// How many decimal places the minor units carry.
    pub minor_units_exponent: u8,
    /// The event range this measurement covers. Coverage is declared, not inferred.
    pub coverage_from_event_seq: u64,
    pub coverage_to_event_seq: u64,
    /// The owner's measurement token. `PartiallyMeasured` means the amount is a floor over the
    /// window, and the document says so beside it (R23.1).
    pub measurement: SpendMeasurementKind,
}

/// What the caller asked for. There is deliberately no `omit` arm: a caller must not be able to
/// suppress a BLUEPRINT-mandated verification surface (Kevin's ruling 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendixRequest {
    PolicyDefault,
    Include,
}

/// The resolved appendix mode. Derived from the request and the verified Policy classification,
/// never stored: a stored resolution can disagree with its own inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendixMode {
    Included,
    /// The literal policy state section 12 prints when the appendix is not included.
    PolicyStateNotHighImpact,
}

impl AppendixRequest {
    fn tag(self) -> u8 {
        match self {
            Self::PolicyDefault => TAG_APPENDIX_POLICY_DEFAULT,
            Self::Include => TAG_APPENDIX_INCLUDE,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedOutcome> {
        match tag {
            TAG_APPENDIX_POLICY_DEFAULT => Ok(Self::PolicyDefault),
            TAG_APPENDIX_INCLUDE => Ok(Self::Include),
            _ => Err(MalformedOutcome::UnknownTag),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::PolicyDefault => "policy_default",
            Self::Include => "include",
        }
    }
}

impl AppendixMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Included => "included",
            Self::PolicyStateNotHighImpact => "policy_state_not_high_impact",
        }
    }
}

/// The immutable manifest one Outcome Document is rendered from.
///
/// It carries identities, digests, and states -- never another module's text. The values those
/// identities name arrive separately in [`VerifiedSources`], so a joined projection can never
/// become a rendered claim.
///
/// Every collection is an [`OutcomeState`]. `success_conditions` is the one exception: a Mission's
/// declared success conditions are the document's subject rather than evidence about it, and
/// BLUEPRINT requires the Outcome be linked to them, so their absence is a refusal rather than a
/// state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeManifestV1 {
    /// Every reference below must name this Workspace. A cross-Workspace reference aborts rather
    /// than rendering.
    pub workspace_id: Uuid,
    /// The Mission whose outcome this is, cited by IDENTITY (R18.4, gpt2 §2). A Mission has no
    /// canonical bytes and no declaration digest on `main`, and v1's `mission: SourceRef` -- which
    /// demanded one -- over-generalized the design; the field it demanded was, in practice, a
    /// pass-through nothing could verify. The completion cited beside it is the verified anchor:
    /// `prepare` proves this identity against the completion's own bytes.
    pub mission_id: Uuid,
    /// The `MissionCompletionV1` record this document is frozen on.
    pub completion: SourceRef,
    /// The Policy classification that decides whether the appendix is mandatory.
    pub policy_classification: SourceRef,
    /// The event cursor everything below is frozen at. Append-only records after it do not mutate
    /// this Outcome; corrections create a new version (Kevin's ruling 4).
    pub boundary_event_seq: u64,
    /// The stored UTC millisecond timestamp the completion was recorded at. A stored value, never
    /// a clock read at render time.
    pub completion_recorded_at_ms: u64,
    pub accountable_resident_id: Uuid,
    /// The verified Policy classification's high-impact bit. It, not the caller, forces the
    /// appendix.
    pub policy_high_impact: bool,
    pub appendix_request: AppendixRequest,
    pub canonicalizer_version: String,
    pub renderer_version: String,
    pub success_conditions: Vec<SuccessCondition>,
    pub delivered_artifacts: OutcomeState<Vec<SourceRef>>,
    pub proposal_refs: OutcomeState<Vec<SourceRef>>,
    pub ruling_refs: OutcomeState<Vec<SourceRef>>,
    pub evaluation_refs: OutcomeState<Vec<SourceRef>>,
    pub receipt_refs: OutcomeState<Vec<ReceiptRef>>,
    pub spend_ref: OutcomeState<SpendRef>,
    pub unfinished_work: OutcomeState<Vec<NarrativeRef>>,
    pub uncertainty: OutcomeState<Vec<NarrativeRef>>,
}

impl OutcomeManifestV1 {
    /// The resolved appendix mode. High-impact always resolves to `Included`; the caller has no
    /// way to say `omit`.
    pub fn appendix_mode(&self) -> AppendixMode {
        if self.policy_high_impact || self.appendix_request == AppendixRequest::Include {
            AppendixMode::Included
        } else {
            AppendixMode::PolicyStateNotHighImpact
        }
    }

    /// The one canonical byte encoding of this manifest. Arrays are sorted here, before the bytes
    /// exist, so two insertion orders of one manifest produce one identity.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MalformedOutcome> {
        encode(self)
    }

    /// The manifest's digest: SHA-256 over its canonical bytes, carrying its own envelope.
    pub fn digest(&self) -> Result<SourceDigest, MalformedOutcome> {
        Ok(SourceDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: CANONICALIZER.to_owned(),
            value: HEXLOWER.encode(&sha256(&self.canonical_bytes()?)),
        })
    }

    /// Decode canonical bytes back into a manifest. Strict: unknown tags, over-limit or invalid
    /// lengths, trailing bytes, non-canonical text, and non-canonical array order are refused.
    pub fn from_canonical_bytes(input: &[u8]) -> Result<Self, MalformedOutcome> {
        decode(input)
    }
}

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

/// Bytes or field values that cannot be a v1 manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedOutcome {
    Magic,
    Truncated,
    TrailingBytes,
    UnknownTag,
    OverLimitLength,
    InvalidUtf8,
    /// A digest value that is not lowercase 64-character hexadecimal.
    DigestValue,
    /// A digest envelope this grammar cannot have produced or cited.
    DigestEnvelope,
    /// A pinned revision that is not its declared kind's canonical form.
    PinnedRevision,
    /// Text with no scalars at all.
    EmptyText,
    /// Text with leading or trailing whitespace. Trimming it would alter a stored value.
    UntrimmedText,
    /// Text longer than the versioned scalar limit.
    TextTooLong,
    /// A control character other than LF or TAB. LF is narrative structure and TAB is content;
    /// everything else in that class is refused, and this names which refusal fired.
    ControlCharacter,
    /// A line break in a value whose rendered position is a table cell. Narrative text may carry
    /// line breaks; a label cannot, because one would end the row.
    LineBreakInLabel,
    /// A token outside `[a-z0-9._-]`, or one that is not already canonical lowercase.
    Token,
    /// A canonicalizer or renderer version this build does not implement.
    UnknownVersion,
    /// A reference of a kind whose records carry a UUID + version inside their canonical bytes
    /// arrived without one (R18.5).
    ReferenceIdentityRequired,
    /// A reference of a kind whose records carry NO UUID inside their canonical bytes arrived with
    /// one: an identity the manifest minted (R18.5, R15's "never minted").
    ReferenceIdentityNotCarried,
    /// Bytes under a canonicalizer this decoder once held and no longer does. Distinct from
    /// `Magic` (not a manifest at all) and from `UnknownVersion` (a grammar this build has never
    /// heard of): a retired grammar is a known thing, refused on purpose, and the refusal says so.
    RetiredVersion,
    /// An array whose entries are not strictly ascending in canonical order, or that repeats a key.
    NonCanonicalOrder,
    /// A manifest with no declared success conditions.
    NoSuccessConditions,
    /// A minor-units exponent no decimal rendering can use.
    MinorUnitsExponent,
    /// A spend coverage range that ends before it starts.
    SpendCoverage,
    /// A reference whose typed kind is not the one its position means.
    SourceKind,
    /// A reference that names a Workspace other than the manifest's.
    CrossWorkspace,
}

impl MalformedOutcome {
    /// A stable lowercase reason token. Refusals are named, never spelled differently by two
    /// callers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Magic => "magic",
            Self::Truncated => "truncated",
            Self::TrailingBytes => "trailing_bytes",
            Self::UnknownTag => "unknown_tag",
            Self::OverLimitLength => "over_limit_length",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::DigestValue => "digest_value",
            Self::DigestEnvelope => "digest_envelope",
            Self::PinnedRevision => "pinned_revision",
            Self::EmptyText => "empty_text",
            Self::UntrimmedText => "untrimmed_text",
            Self::TextTooLong => "text_too_long",
            Self::ControlCharacter => "control_character",
            Self::LineBreakInLabel => "line_break_in_label",
            Self::Token => "token",
            Self::UnknownVersion => "unknown_version",
            Self::ReferenceIdentityRequired => "reference_identity_required",
            Self::ReferenceIdentityNotCarried => "reference_identity_not_carried",
            Self::RetiredVersion => "retired_version",
            Self::NonCanonicalOrder => "non_canonical_order",
            Self::NoSuccessConditions => "no_success_conditions",
            Self::MinorUnitsExponent => "minor_units_exponent",
            Self::SpendCoverage => "spend_coverage",
            Self::SourceKind => "source_kind",
            Self::CrossWorkspace => "cross_workspace",
        }
    }
}

impl fmt::Display for MalformedOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The three bounds the Outcome function owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeBound {
    CitationCount,
    VerifiedSourceBytes,
    EmittedMarkdownBytes,
}

impl OutcomeBound {
    /// The bound's own name. A refusal names which bound bit -- an exhausted total budget must
    /// never report itself as some other bound.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CitationCount => BOUND_CITATION_COUNT,
            Self::VerifiedSourceBytes => BOUND_VERIFIED_SOURCE_BYTES,
            Self::EmittedMarkdownBytes => BOUND_EMITTED_MARKDOWN_BYTES,
        }
    }
}

/// An exhausted bound: which one, what it was configured to, and how much was observed before the
/// render stopped.
///
/// `observed_at_least` is a lower bound on purpose. The render refuses at the moment the bound is
/// exceeded rather than measuring the whole input first, so the honest report is "at least this
/// many", never a total nobody counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundRefusal {
    pub bound: OutcomeBound,
    pub configured: u64,
    pub observed_at_least: u64,
}

/// The manifest and the verified sources do not correspond.
///
/// Slice 1's renderer refuses rather than rendering a row no verified read produced, or dropping a
/// cited object no source value covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceMismatch {
    /// A manifest reference has no verified value.
    MissingValue { kind: &'static str, id: String },
    /// A verified value names something the manifest does not cite.
    UnreferencedValue { kind: &'static str, id: String },
    /// Two verified values claim the same identity.
    DuplicateValue { kind: &'static str, id: String },
}

/// Why a render did not happen. There is no partial output on any arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeRefusal {
    Malformed(MalformedOutcome),
    Bound(BoundRefusal),
    Sources(SourceMismatch),
}

impl From<MalformedOutcome> for OutcomeRefusal {
    fn from(value: MalformedOutcome) -> Self {
        Self::Malformed(value)
    }
}

impl From<SourceMismatch> for OutcomeRefusal {
    fn from(value: SourceMismatch) -> Self {
        Self::Sources(value)
    }
}

impl fmt::Display for OutcomeRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(reason) => write!(formatter, "malformed manifest: {reason}"),
            Self::Bound(refusal) => write!(
                formatter,
                "bound {} exhausted: configured {}, observed at least {}",
                refusal.bound.as_str(),
                refusal.configured,
                refusal.observed_at_least
            ),
            Self::Sources(mismatch) => match mismatch {
                SourceMismatch::MissingValue { kind, id } => {
                    write!(formatter, "no verified value for {kind} {id}")
                }
                SourceMismatch::UnreferencedValue { kind, id } => {
                    write!(formatter, "verified value {kind} {id} is not cited")
                }
                SourceMismatch::DuplicateValue { kind, id } => {
                    write!(formatter, "two verified values for {kind} {id}")
                }
            },
        }
    }
}

impl std::error::Error for OutcomeRefusal {}

// ---------------------------------------------------------------------------------------------
// Canonical forms and the one comparator
// ---------------------------------------------------------------------------------------------

/// The comparator every textual sort key goes through.
///
/// Canonical lowercase form, compared by Unicode code point -- which for UTF-8 is unsigned byte
/// order. Never a locale collation and never a database collation: `localeCompare` and SQLite
/// `NOCASE` disagree with this and with each other, and a renderer that sorts one way while a
/// store paginates another produces a different document from the same manifest.
///
/// Public because it is the executable form of a rule ADR 0040 states in prose, and a rule stated
/// in two places drifts.
pub fn compare_canonical_keys(left: &str, right: &str) -> Ordering {
    left.as_bytes().cmp(right.as_bytes())
}

/// One part of a canonical sort key. Numbers compare as numbers; text compares by code point.
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeyPart {
    Num(u64),
    Text(String),
}

impl Ord for KeyPart {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Num(left), Self::Num(right)) => left.cmp(right),
            (Self::Text(left), Self::Text(right)) => compare_canonical_keys(left, right),
            (Self::Num(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Num(_)) => Ordering::Greater,
        }
    }
}

impl PartialOrd for KeyPart {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The canonical lowercase hyphenated form of a UUID. `Uuid`'s `Display` is already lowercase; the
/// call is explicit so the canonical form is stated rather than inherited.
fn uuid_key(value: &Uuid) -> String {
    value.hyphenated().to_string()
}

/// The canonical order and identity of one cited source.
///
/// The digest is a three-part envelope and all three parts are in the key. A key built from the
/// value alone makes two references that differ only in canonicalizer -- `beacon.artifact.v1` and
/// `beacon.artifact.v2` over the same bytes -- collide: the encoder would reject them as a repeated
/// canonical key and the census would count them as one object. They are two objects, because a
/// digest value without the function that produced it names nothing (ADR 0034/0037).
fn source_ref_key(value: &SourceRef) -> Vec<KeyPart> {
    let mut key = vec![KeyPart::Num(u64::from(value.kind.tag()))];
    key.extend(source_identity_key(value));
    key
}

/// What a cited source *is*, with its kind left out.
///
/// The census names the kind once, in its lane; carrying it a second time inside the identity
/// would make a lane confusion structurally harmless and therefore untestable, which is a worse
/// place to be than a lane confusion a test catches.
fn source_identity_key(value: &SourceRef) -> Vec<KeyPart> {
    // Presence first, so the two shapes never interleave; then the identity if there is one; then
    // the whole digest envelope. For a kind with no reference identity the digest IS the identity.
    let mut key = match &value.identity {
        Some(identity) => vec![
            KeyPart::Num(1),
            KeyPart::Text(uuid_key(&identity.object_id)),
            KeyPart::Num(u64::from(identity.schema_version)),
        ],
        None => vec![KeyPart::Num(0)],
    };
    key.extend([
        KeyPart::Text(value.digest.algorithm.clone()),
        KeyPart::Text(value.digest.canonicalizer.clone()),
        KeyPart::Text(value.digest.value.clone()),
    ]);
    key
}

fn receipt_ref_key(value: &ReceiptRef) -> Vec<KeyPart> {
    vec![
        KeyPart::Text(uuid_key(&value.run_uuid)),
        KeyPart::Num(u64::from(value.receipt_version)),
    ]
}

fn condition_key(value: &SuccessCondition) -> Vec<KeyPart> {
    vec![
        KeyPart::Num(u64::from(value.ordinal)),
        KeyPart::Text(value.condition_id.clone()),
    ]
}

fn narrative_key(value: &NarrativeRef) -> Vec<KeyPart> {
    vec![
        KeyPart::Num(u64::from(value.ordinal)),
        KeyPart::Text(value.item_id.clone()),
    ]
}

/// Sorts a slice into canonical order. Called before the bytes exist, so insertion order cannot
/// reach an identity.
fn sorted<T: Clone>(items: &[T], key: fn(&T) -> Vec<KeyPart>) -> Vec<T> {
    let mut keyed: Vec<(Vec<KeyPart>, T)> =
        items.iter().map(|item| (key(item), item.clone())).collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed.into_iter().map(|(_, item)| item).collect()
}

/// Refuses a decoded array that is not already strictly ascending. Equal keys are refused too: a
/// repeated key is a second spelling of one entry, and two spellings of one identity must not both
/// decode.
fn require_canonical_order<T>(
    items: &[T],
    key: fn(&T) -> Vec<KeyPart>,
) -> Result<(), MalformedOutcome> {
    for window in items.windows(2) {
        if key(&window[0]).cmp(&key(&window[1])) != Ordering::Less {
            return Err(MalformedOutcome::NonCanonicalOrder);
        }
    }
    Ok(())
}

/// A stable lowercase identifier: `[a-z0-9._-]`, non-empty, bounded.
///
/// Uppercase is refused rather than folded, for the reason ADR 0034 gives for the pinned base
/// revision: folding would let `A-1` and `a-1` name one identity, and case-folding an identity is a
/// silent normalization this grammar refuses.
fn require_token(value: &str) -> Result<(), MalformedOutcome> {
    if value.is_empty() || value.len() > MAX_TOKEN_BYTES {
        return Err(MalformedOutcome::Token);
    }
    if value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
    }) {
        Ok(())
    } else {
        Err(MalformedOutcome::Token)
    }
}

/// Narrative text: prose a reader is meant to read, rendered as CommonMark blocks.
///
/// LF is structure and TAB is content; both are preserved into the emitted bytes. Every other
/// control character is refused, and each refusal names itself: "empty", "padded", "too long" and
/// "carries a control character" are four different faults, and one name for all four tells a
/// caller nothing about which it hit.
///
/// The whitespace rule is one property, applied per line: **anything the renderer would collapse is
/// refused, so a stored value and its document correspond one to one.** A leading space is stripped
/// by CommonMark; trailing whitespace of any kind is forbidden by the byte rules; a leading or
/// trailing blank line renders identically to its absence. A leading TAB is none of those -- it
/// survives into the bytes -- so it is accepted, which is what ADR 0040 already said and what this
/// function previously contradicted by testing `trim()` first.
fn require_narrative_text(value: &str) -> Result<(), MalformedOutcome> {
    if value.is_empty() {
        return Err(MalformedOutcome::EmptyText);
    }
    if value.chars().count() > MAX_TEXT_SCALARS {
        return Err(MalformedOutcome::TextTooLong);
    }
    if value
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(MalformedOutcome::ControlCharacter);
    }

    let mut lines = value.split('\n').peekable();
    let mut first = true;
    while let Some(line) = lines.next() {
        let last = lines.peek().is_none();
        // An empty first or last line is a leading or trailing newline, which renders identically
        // to its absence. Interior blank lines are paragraph breaks and are kept, one for one.
        if (first || last) && line.is_empty() {
            return Err(MalformedOutcome::UntrimmedText);
        }
        if line.starts_with(' ') || line.ends_with(char::is_whitespace) {
            return Err(MalformedOutcome::UntrimmedText);
        }
        first = false;
    }
    Ok(())
}

/// Label text: a short value whose rendered position is a table cell.
///
/// Everything narrative text must be, plus no line break. A line break in a cell ends the row, and
/// the two honest alternatives -- raw HTML, or replacing it with a space -- are respectively
/// forbidden and a silent alteration of a stored value. So a label refuses it, by a name that says
/// which rule it broke rather than by the narrative rule's name.
fn require_label_text(value: &str) -> Result<(), MalformedOutcome> {
    require_narrative_text(value)?;
    if value.contains('\n') {
        return Err(MalformedOutcome::LineBreakInLabel);
    }
    Ok(())
}

fn require_hex(
    value: &str,
    characters: usize,
    refusal: MalformedOutcome,
) -> Result<(), MalformedOutcome> {
    if value.len() != characters {
        return Err(refusal);
    }
    match HEXLOWER.decode(value.as_bytes()) {
        Ok(_) => Ok(()),
        Err(_) => Err(refusal),
    }
}

fn require_digest(digest: &SourceDigest) -> Result<(), MalformedOutcome> {
    if digest.algorithm != DIGEST_ALGORITHM {
        return Err(MalformedOutcome::DigestEnvelope);
    }
    require_token(&digest.canonicalizer).map_err(|_| MalformedOutcome::DigestEnvelope)?;
    require_hex(
        &digest.value,
        DIGEST_HEX_CHARS,
        MalformedOutcome::DigestValue,
    )
}

fn digest_raw(value: &str) -> Result<[u8; 32], MalformedOutcome> {
    require_hex(value, DIGEST_HEX_CHARS, MalformedOutcome::DigestValue)?;
    let decoded = HEXLOWER
        .decode(value.as_bytes())
        .map_err(|_| MalformedOutcome::DigestValue)?;
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&decoded);
    Ok(bytes)
}

// ---------------------------------------------------------------------------------------------
// The canonical bytes
// ---------------------------------------------------------------------------------------------

/// Appends canonical bytes. Every length is checked against its field's limit here too, so the
/// encoder and the decoder cannot disagree about what fits.
struct Encoder {
    out: Vec<u8>,
}

impl Encoder {
    fn new() -> Self {
        Self {
            out: MAGIC.to_vec(),
        }
    }

    fn tag(&mut self, tag: u8) {
        self.out.push(tag);
    }

    fn bool(&mut self, value: bool) {
        self.out.push(if value { TAG_TRUE } else { TAG_FALSE });
    }

    fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    fn uuid(&mut self, value: &Uuid) {
        self.out.extend_from_slice(value.as_bytes());
    }

    fn sha256(&mut self, value: &[u8; 32]) {
        self.out.extend_from_slice(value);
    }

    fn string(&mut self, value: &str, limit: usize) -> Result<(), MalformedOutcome> {
        if value.len() > limit {
            return Err(MalformedOutcome::OverLimitLength);
        }
        let length = u64::try_from(value.len()).map_err(|_| MalformedOutcome::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn count(&mut self, length: usize) -> Result<(), MalformedOutcome> {
        let count = u32::try_from(length).map_err(|_| MalformedOutcome::OverLimitLength)?;
        if count > MAX_COLLECTION_ENTRIES {
            return Err(MalformedOutcome::OverLimitLength);
        }
        self.u32(count);
        Ok(())
    }
}

/// Reads canonical bytes. Self-delimiting: every field proves it fits within the remaining input
/// and its own limit before any of it is consumed.
struct Decoder<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.input.len() - self.offset
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], MalformedOutcome> {
        if count > self.remaining() {
            return Err(MalformedOutcome::Truncated);
        }
        let slice = &self.input[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn magic(&mut self) -> Result<(), MalformedOutcome> {
        if self
            .take(MAGIC.len())
            .map_err(|_| MalformedOutcome::Magic)?
            == MAGIC
        {
            Ok(())
        } else {
            Err(MalformedOutcome::Magic)
        }
    }

    fn tag(&mut self) -> Result<u8, MalformedOutcome> {
        Ok(self.take(1)?[0])
    }

    fn bool(&mut self) -> Result<bool, MalformedOutcome> {
        match self.tag()? {
            TAG_FALSE => Ok(false),
            TAG_TRUE => Ok(true),
            _ => Err(MalformedOutcome::UnknownTag),
        }
    }

    fn u32(&mut self) -> Result<u32, MalformedOutcome> {
        let mut value = [0_u8; 4];
        value.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(value))
    }

    fn u64(&mut self) -> Result<u64, MalformedOutcome> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }

    fn i64(&mut self) -> Result<i64, MalformedOutcome> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(i64::from_be_bytes(value))
    }

    fn uuid(&mut self) -> Result<Uuid, MalformedOutcome> {
        let mut value = [0_u8; 16];
        value.copy_from_slice(self.take(16)?);
        Ok(Uuid::from_bytes(value))
    }

    fn sha256_hex(&mut self) -> Result<String, MalformedOutcome> {
        let mut value = [0_u8; 32];
        value.copy_from_slice(self.take(32)?);
        Ok(HEXLOWER.encode(&value))
    }

    fn string(&mut self, limit: usize) -> Result<String, MalformedOutcome> {
        let length = self.u64()?;
        let limit = u64::try_from(limit).map_err(|_| MalformedOutcome::OverLimitLength)?;
        if length > limit {
            return Err(MalformedOutcome::OverLimitLength);
        }
        let length = usize::try_from(length).map_err(|_| MalformedOutcome::Truncated)?;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| MalformedOutcome::InvalidUtf8)
    }

    /// Reads a declared entry count and proves the input could hold that many entries.
    ///
    /// Refuse before allocate: the count is checked against its cap and then against the remaining
    /// input at `entry_bytes` each, before any entry is read and before any `Vec` reserves for it.
    fn count(&mut self, entry_bytes: usize) -> Result<usize, MalformedOutcome> {
        let count = self.u32()?;
        if count > MAX_COLLECTION_ENTRIES {
            return Err(MalformedOutcome::OverLimitLength);
        }
        let count = count as usize;
        let smallest = count
            .checked_mul(entry_bytes)
            .ok_or(MalformedOutcome::Truncated)?;
        if smallest > self.remaining() {
            return Err(MalformedOutcome::Truncated);
        }
        Ok(count)
    }

    fn finish(self) -> Result<(), MalformedOutcome> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MalformedOutcome::TrailingBytes)
        }
    }
}

fn encode_state<T>(
    encoder: &mut Encoder,
    state: &OutcomeState<T>,
    value: impl FnOnce(&mut Encoder, &T) -> Result<(), MalformedOutcome>,
) -> Result<(), MalformedOutcome> {
    encoder.tag(state.tag());
    match state {
        OutcomeState::Verified(inner) => value(encoder, inner),
        OutcomeState::Absent(reason)
        | OutcomeState::Broken(reason)
        | OutcomeState::NotEvaluated(reason) => {
            require_label_text(reason)?;
            encoder.string(reason, MAX_TEXT_BYTES)
        }
    }
}

fn decode_state<T>(
    decoder: &mut Decoder<'_>,
    value: impl FnOnce(&mut Decoder<'_>) -> Result<T, MalformedOutcome>,
) -> Result<OutcomeState<T>, MalformedOutcome> {
    // The tag decides what follows, so it is judged FIRST and by itself. Reading a reason before
    // rejecting an unknown tag let the bytes after that tag -- which are not a reason, and were
    // never promised to be one -- be parsed as a length and become the refusal a caller saw
    // (`OverLimitLength`, not `UnknownTag`; sol1's cold read of the v2 grammar). A closed vocabulary
    // refuses the unknown member by name before consuming anything after it.
    let tag = decoder.tag()?;
    let build: fn(String) -> OutcomeState<T> = match tag {
        TAG_STATE_VERIFIED => return Ok(OutcomeState::Verified(value(decoder)?)),
        TAG_STATE_ABSENT => OutcomeState::Absent,
        TAG_STATE_BROKEN => OutcomeState::Broken,
        TAG_STATE_NOT_EVALUATED => OutcomeState::NotEvaluated,
        _ => return Err(MalformedOutcome::UnknownTag),
    };
    let reason = decoder.string(MAX_TEXT_BYTES)?;
    require_label_text(&reason)?;
    Ok(build(reason))
}

fn encode_digest(encoder: &mut Encoder, digest: &SourceDigest) -> Result<(), MalformedOutcome> {
    require_digest(digest)?;
    encoder.sha256(&digest_raw(&digest.value)?);
    encoder.string(&digest.algorithm, MAX_TOKEN_BYTES)?;
    encoder.string(&digest.canonicalizer, MAX_TOKEN_BYTES)
}

fn decode_digest(decoder: &mut Decoder<'_>) -> Result<SourceDigest, MalformedOutcome> {
    let value = decoder.sha256_hex()?;
    let algorithm = decoder.string(MAX_TOKEN_BYTES)?;
    let canonicalizer = decoder.string(MAX_TOKEN_BYTES)?;
    let digest = SourceDigest {
        algorithm,
        canonicalizer,
        value,
    };
    require_digest(&digest)?;
    Ok(digest)
}

fn encode_source_ref(encoder: &mut Encoder, value: &SourceRef) -> Result<(), MalformedOutcome> {
    // The tag stays assigned and is never reused (R13.8), but no reference may carry it: a Mission
    // has no bytes to cite, so a `SourceRef` of this kind would be a digest nobody can verify --
    // the field R18.4 removed, back in through a side door.
    if value.kind == SourceKind::Mission {
        return Err(MalformedOutcome::SourceKind);
    }
    require_identity_shape(value.kind, value.identity.as_ref())?;
    encoder.uuid(&value.workspace_id);
    encoder.tag(value.kind.tag());
    match &value.identity {
        Some(identity) => {
            encoder.tag(TAG_IDENTITY_SOME);
            encoder.uuid(&identity.object_id);
            encoder.u32(identity.schema_version);
        }
        None => encoder.tag(TAG_IDENTITY_NONE),
    }
    encode_digest(encoder, &value.digest)?;
    encode_state(encoder, &value.pinned_revision, |encoder, revision| {
        encoder.tag(match revision.kind {
            RevisionKind::GitCommit => TAG_REVISION_GIT_COMMIT,
        });
        match revision.kind {
            RevisionKind::GitCommit => require_hex(
                &revision.value,
                GIT_COMMIT_HEX_CHARS,
                MalformedOutcome::PinnedRevision,
            )?,
        }
        encoder.string(&revision.value, GIT_COMMIT_HEX_CHARS)
    })
}

fn decode_source_ref(decoder: &mut Decoder<'_>) -> Result<SourceRef, MalformedOutcome> {
    let workspace_id = decoder.uuid()?;
    let kind = SourceKind::parse(decoder.tag()?)?;
    if kind == SourceKind::Mission {
        return Err(MalformedOutcome::SourceKind);
    }
    let identity = match decoder.tag()? {
        TAG_IDENTITY_SOME => Some(ReferenceIdentity {
            object_id: decoder.uuid()?,
            schema_version: decoder.u32()?,
        }),
        TAG_IDENTITY_NONE => None,
        _ => return Err(MalformedOutcome::UnknownTag),
    };
    require_identity_shape(kind, identity.as_ref())?;
    let digest = decode_digest(decoder)?;
    let pinned_revision = decode_state(decoder, |decoder| {
        let kind = match decoder.tag()? {
            TAG_REVISION_GIT_COMMIT => RevisionKind::GitCommit,
            _ => return Err(MalformedOutcome::UnknownTag),
        };
        let value = decoder.string(GIT_COMMIT_HEX_CHARS)?;
        match kind {
            RevisionKind::GitCommit => require_hex(
                &value,
                GIT_COMMIT_HEX_CHARS,
                MalformedOutcome::PinnedRevision,
            )?,
        }
        Ok(PinnedRevision { kind, value })
    })?;
    Ok(SourceRef {
        workspace_id,
        kind,
        identity,
        digest,
        pinned_revision,
    })
}

/// R18.5's rule, executable: identity present exactly where the owning record carries one inside
/// its canonical bytes today. Kept as ONE table so that when a record gains its UUID under R15 the
/// flip is one line here and nothing in the byte layout.
fn require_identity_shape(
    kind: SourceKind,
    identity: Option<&ReferenceIdentity>,
) -> Result<(), MalformedOutcome> {
    let carries_identity = match kind {
        // `completion_id`, `classification_id` + `version`, `spend_id` + `version`: in the bytes.
        SourceKind::Completion | SourceKind::Policy | SourceKind::Spend => true,
        // Record-digest identity (R16) or no UUID yet (R15 ruled, not implemented for them).
        SourceKind::Artifact
        | SourceKind::Proposal
        | SourceKind::Ruling
        | SourceKind::Evaluation => false,
        // Refused before this is asked; listed so the match stays exhaustive.
        SourceKind::Mission => false,
    };
    match (carries_identity, identity) {
        (true, None) => Err(MalformedOutcome::ReferenceIdentityRequired),
        (false, Some(_)) => Err(MalformedOutcome::ReferenceIdentityNotCarried),
        _ => Ok(()),
    }
}

/// The two identity cells of a reference row: the UUID and the schema version, or the literal
/// that says this kind's records carry no reference identity -- a sentence, never a blank.
///
/// The completion and Policy rows use it too. Their identities are REQUIRED, and the codec has
/// refused any manifest lacking one before a document is rendered (`render` canonicalizes first);
/// if the literal ever printed for them it would still be a true sentence, which is more than a
/// fabricated UUID would be.
fn identity_cells(reference: &SourceRef) -> (String, String) {
    match &reference.identity {
        Some(identity) => (
            uuid_key(&identity.object_id),
            identity.schema_version.to_string(),
        ),
        None => (
            "absent (the owner carries no reference identity)".to_owned(),
            "absent".to_owned(),
        ),
    }
}

fn encode_receipt_ref(encoder: &mut Encoder, value: &ReceiptRef) -> Result<(), MalformedOutcome> {
    encoder.uuid(&value.workspace_id);
    encoder.uuid(&value.run_uuid);
    encoder.u32(value.receipt_version);
    encoder.sha256(&digest_raw(&value.outcome_checksum)?);
    encoder.string(DIGEST_ALGORITHM, MAX_TOKEN_BYTES)?;
    // The label travels in the bytes: a Receipt checksum can never be read back as a Receipt
    // domain digest, because the record says what it is.
    encoder.string(RECEIPT_CHECKSUM_LABEL, MAX_TOKEN_BYTES)
}

fn decode_receipt_ref(decoder: &mut Decoder<'_>) -> Result<ReceiptRef, MalformedOutcome> {
    let workspace_id = decoder.uuid()?;
    let run_uuid = decoder.uuid()?;
    let receipt_version = decoder.u32()?;
    let outcome_checksum = decoder.sha256_hex()?;
    let algorithm = decoder.string(MAX_TOKEN_BYTES)?;
    let label = decoder.string(MAX_TOKEN_BYTES)?;
    if algorithm != DIGEST_ALGORITHM || label != RECEIPT_CHECKSUM_LABEL {
        return Err(MalformedOutcome::DigestEnvelope);
    }
    Ok(ReceiptRef {
        workspace_id,
        run_uuid,
        receipt_version,
        outcome_checksum,
    })
}

/// Refuses a reference whose typed kind is not the one its position in the manifest means.
///
/// A `proposal_refs` entry that says `ruling` is not a naming slip: it would render a Ruling under
/// a Proposal heading, which is the renderer asserting something no verified read established.
fn require_kind(value: &SourceRef, expected: SourceKind) -> Result<(), MalformedOutcome> {
    if value.kind == expected {
        Ok(())
    } else {
        Err(MalformedOutcome::SourceKind)
    }
}

fn encode_refs(
    encoder: &mut Encoder,
    state: &OutcomeState<Vec<SourceRef>>,
    expected: Option<SourceKind>,
) -> Result<(), MalformedOutcome> {
    encode_state(encoder, state, |encoder, refs| {
        // The bound is applied to the STORED count before anything is cloned or sorted. `sorted`
        // clones every entry; refusing after it meant an already-refusable 50,000-entry array cost
        // 60 MB of allocation on its way to a refusal (sol1's cold read, #6). The count is written
        // here, in the position the table gives it, and the sort follows.
        encoder.count(refs.len())?;
        let refs = sorted(refs, source_ref_key);
        require_canonical_order(&refs, source_ref_key)?;
        for entry in &refs {
            if let Some(expected) = expected {
                require_kind(entry, expected)?;
            }
            encode_source_ref(encoder, entry)?;
        }
        Ok(())
    })
}

fn decode_refs(
    decoder: &mut Decoder<'_>,
    expected: Option<SourceKind>,
) -> Result<OutcomeState<Vec<SourceRef>>, MalformedOutcome> {
    decode_state(decoder, |decoder| {
        let count = decoder.count(MIN_SOURCE_REF_BYTES)?;
        let mut refs = Vec::new();
        for _ in 0..count {
            let entry = decode_source_ref(decoder)?;
            if let Some(expected) = expected {
                require_kind(&entry, expected)?;
            }
            refs.push(entry);
        }
        require_canonical_order(&refs, source_ref_key)?;
        Ok(refs)
    })
}

fn encode_narrative(
    encoder: &mut Encoder,
    state: &OutcomeState<Vec<NarrativeRef>>,
) -> Result<(), MalformedOutcome> {
    encode_state(encoder, state, |encoder, items| {
        let items = sorted(items, narrative_key);
        require_canonical_order(&items, narrative_key)?;
        encoder.count(items.len())?;
        for item in &items {
            require_token(&item.item_id)?;
            encoder.u32(item.ordinal);
            encoder.string(&item.item_id, MAX_TOKEN_BYTES)?;
        }
        Ok(())
    })
}

fn decode_narrative(
    decoder: &mut Decoder<'_>,
) -> Result<OutcomeState<Vec<NarrativeRef>>, MalformedOutcome> {
    decode_state(decoder, |decoder| {
        let count = decoder.count(MIN_NARRATIVE_REF_BYTES)?;
        let mut items = Vec::new();
        for _ in 0..count {
            let ordinal = decoder.u32()?;
            let item_id = decoder.string(MAX_TOKEN_BYTES)?;
            require_token(&item_id)?;
            items.push(NarrativeRef { ordinal, item_id });
        }
        require_canonical_order(&items, narrative_key)?;
        Ok(items)
    })
}

const MAX_UNIT_BYTES: usize = 16;
/// The largest number of decimal places minor units may carry. Nine is more than any real currency
/// or metered unit and keeps the decimal rendering in integer arithmetic.
const MAX_MINOR_UNITS_EXPONENT: u8 = 9;

/// A stored currency or metered unit token.
///
/// Deliberately *not* case-folded and not normalized: `USD` and `usd` may name the same currency to
/// a human, but folding a stored unit is a transformation the spend owner did not ask for. The
/// token is stored and rendered exactly as measured.
fn require_unit(value: &str) -> Result<(), MalformedOutcome> {
    if value.is_empty() || value.len() > MAX_UNIT_BYTES {
        return Err(MalformedOutcome::Token);
    }
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(MalformedOutcome::Token)
    }
}

fn encode_spend(encoder: &mut Encoder, value: &SpendRef) -> Result<(), MalformedOutcome> {
    if value.minor_units_exponent > MAX_MINOR_UNITS_EXPONENT {
        return Err(MalformedOutcome::MinorUnitsExponent);
    }
    if value.coverage_to_event_seq < value.coverage_from_event_seq {
        return Err(MalformedOutcome::SpendCoverage);
    }
    require_unit(&value.currency)?;
    encoder.uuid(&value.workspace_id);
    encoder.uuid(&value.spend_id);
    encoder.u32(value.schema_version);
    encode_digest(encoder, &value.digest)?;
    encoder.i64(value.amount_minor);
    encoder.string(&value.currency, MAX_UNIT_BYTES)?;
    encoder.tag(value.minor_units_exponent);
    encoder.u64(value.coverage_from_event_seq);
    encoder.u64(value.coverage_to_event_seq);
    encoder.tag(value.measurement.tag());
    Ok(())
}

fn decode_spend(decoder: &mut Decoder<'_>) -> Result<SpendRef, MalformedOutcome> {
    let workspace_id = decoder.uuid()?;
    let spend_id = decoder.uuid()?;
    let schema_version = decoder.u32()?;
    let digest = decode_digest(decoder)?;
    let amount_minor = decoder.i64()?;
    let currency = decoder.string(MAX_UNIT_BYTES)?;
    require_unit(&currency)?;
    let minor_units_exponent = decoder.tag()?;
    if minor_units_exponent > MAX_MINOR_UNITS_EXPONENT {
        return Err(MalformedOutcome::MinorUnitsExponent);
    }
    let coverage_from_event_seq = decoder.u64()?;
    let coverage_to_event_seq = decoder.u64()?;
    if coverage_to_event_seq < coverage_from_event_seq {
        return Err(MalformedOutcome::SpendCoverage);
    }
    let measurement = SpendMeasurementKind::parse(decoder.tag()?)?;
    Ok(SpendRef {
        workspace_id,
        spend_id,
        schema_version,
        digest,
        amount_minor,
        currency,
        minor_units_exponent,
        coverage_from_event_seq,
        coverage_to_event_seq,
        measurement,
    })
}

fn encode_condition(
    encoder: &mut Encoder,
    condition: &SuccessCondition,
) -> Result<(), MalformedOutcome> {
    require_token(&condition.condition_id)?;
    encoder.u32(condition.ordinal);
    encoder.string(&condition.condition_id, MAX_TOKEN_BYTES)?;
    encode_state(encoder, &condition.outcome, |encoder, result| {
        encoder.tag(result.tag());
        Ok(())
    })?;
    encode_refs(encoder, &condition.citations, None)
}

fn decode_condition(decoder: &mut Decoder<'_>) -> Result<SuccessCondition, MalformedOutcome> {
    let ordinal = decoder.u32()?;
    let condition_id = decoder.string(MAX_TOKEN_BYTES)?;
    require_token(&condition_id)?;
    let outcome = decode_state(decoder, |decoder| ConditionResult::parse(decoder.tag()?))?;
    let citations = decode_refs(decoder, None)?;
    Ok(SuccessCondition {
        ordinal,
        condition_id,
        outcome,
        citations,
    })
}

/// The one canonical encoding of a manifest.
///
/// Every accepted byte difference changes the manifest identity, so alternate encodings of the same
/// parsed value do not exist: this function is the only producer, and every read proves the stored
/// bytes equal what it returns. The field order, widths, and tag values are ADR 0040's table, and
/// this is the executable form of it.
fn encode(manifest: &OutcomeManifestV1) -> Result<Vec<u8>, MalformedOutcome> {
    // Retired and unknown are two refusals on the encode side too: a value that still says v1 is a
    // known thing this grammar chose to stop producing, not a grammar it has never heard of.
    if manifest.canonicalizer_version == RETIRED_CANONICALIZER {
        return Err(MalformedOutcome::RetiredVersion);
    }
    if manifest.canonicalizer_version != CANONICALIZER {
        return Err(MalformedOutcome::UnknownVersion);
    }
    if escaping_for(&manifest.renderer_version).is_none() {
        return Err(MalformedOutcome::UnknownVersion);
    }
    if manifest.success_conditions.is_empty() {
        return Err(MalformedOutcome::NoSuccessConditions);
    }

    require_kind(&manifest.completion, SourceKind::Completion)?;
    require_kind(&manifest.policy_classification, SourceKind::Policy)?;

    let mut encoder = Encoder::new();
    encoder.string(&manifest.canonicalizer_version, MAX_TOKEN_BYTES)?;
    encoder.string(&manifest.renderer_version, MAX_TOKEN_BYTES)?;
    encoder.uuid(&manifest.workspace_id);
    encoder.uuid(&manifest.mission_id);
    encode_source_ref(&mut encoder, &manifest.completion)?;
    encode_source_ref(&mut encoder, &manifest.policy_classification)?;
    encoder.u64(manifest.boundary_event_seq);
    encoder.u64(manifest.completion_recorded_at_ms);
    encoder.uuid(&manifest.accountable_resident_id);
    encoder.bool(manifest.policy_high_impact);
    encoder.tag(manifest.appendix_request.tag());

    let conditions = sorted(&manifest.success_conditions, condition_key);
    require_canonical_order(&conditions, condition_key)?;
    encoder.count(conditions.len())?;
    for condition in &conditions {
        encode_condition(&mut encoder, condition)?;
    }

    encode_refs(
        &mut encoder,
        &manifest.delivered_artifacts,
        Some(SourceKind::Artifact),
    )?;
    encode_refs(
        &mut encoder,
        &manifest.proposal_refs,
        Some(SourceKind::Proposal),
    )?;
    encode_refs(
        &mut encoder,
        &manifest.ruling_refs,
        Some(SourceKind::Ruling),
    )?;
    encode_refs(
        &mut encoder,
        &manifest.evaluation_refs,
        Some(SourceKind::Evaluation),
    )?;
    encode_state(&mut encoder, &manifest.receipt_refs, |encoder, refs| {
        let refs = sorted(refs, receipt_ref_key);
        require_canonical_order(&refs, receipt_ref_key)?;
        encoder.count(refs.len())?;
        for entry in &refs {
            encode_receipt_ref(encoder, entry)?;
        }
        Ok(())
    })?;
    encode_state(&mut encoder, &manifest.spend_ref, encode_spend)?;
    encode_narrative(&mut encoder, &manifest.unfinished_work)?;
    encode_narrative(&mut encoder, &manifest.uncertainty)?;
    Ok(encoder.out)
}

fn decode(input: &[u8]) -> Result<OutcomeManifestV1, MalformedOutcome> {
    // The retired grammar is recognised BEFORE the magic check, so v1 bytes are refused as what
    // they are rather than as garbage. A decoder that answered `Magic` here would be telling a
    // caller their manifest is not a manifest, when it is one this build chose not to read.
    if input.starts_with(RETIRED_MAGIC) {
        return Err(MalformedOutcome::RetiredVersion);
    }
    let mut decoder = Decoder::new(input);
    decoder.magic()?;

    let canonicalizer_version = decoder.string(MAX_TOKEN_BYTES)?;
    if canonicalizer_version != CANONICALIZER {
        return Err(MalformedOutcome::UnknownVersion);
    }
    let renderer_version = decoder.string(MAX_TOKEN_BYTES)?;
    if escaping_for(&renderer_version).is_none() {
        return Err(MalformedOutcome::UnknownVersion);
    }
    let workspace_id = decoder.uuid()?;
    let mission_id = decoder.uuid()?;
    let completion = decode_source_ref(&mut decoder)?;
    let policy_classification = decode_source_ref(&mut decoder)?;
    require_kind(&completion, SourceKind::Completion)?;
    require_kind(&policy_classification, SourceKind::Policy)?;
    let boundary_event_seq = decoder.u64()?;
    let completion_recorded_at_ms = decoder.u64()?;
    let accountable_resident_id = decoder.uuid()?;
    let policy_high_impact = decoder.bool()?;
    let appendix_request = AppendixRequest::parse(decoder.tag()?)?;

    let count = decoder.count(MIN_CONDITION_BYTES)?;
    if count == 0 {
        return Err(MalformedOutcome::NoSuccessConditions);
    }
    let mut success_conditions = Vec::new();
    for _ in 0..count {
        success_conditions.push(decode_condition(&mut decoder)?);
    }
    require_canonical_order(&success_conditions, condition_key)?;

    let delivered_artifacts = decode_refs(&mut decoder, Some(SourceKind::Artifact))?;
    let proposal_refs = decode_refs(&mut decoder, Some(SourceKind::Proposal))?;
    let ruling_refs = decode_refs(&mut decoder, Some(SourceKind::Ruling))?;
    let evaluation_refs = decode_refs(&mut decoder, Some(SourceKind::Evaluation))?;
    let receipt_refs = decode_state(&mut decoder, |decoder| {
        let count = decoder.count(MIN_RECEIPT_REF_BYTES)?;
        let mut refs = Vec::new();
        for _ in 0..count {
            refs.push(decode_receipt_ref(decoder)?);
        }
        require_canonical_order(&refs, receipt_ref_key)?;
        Ok(refs)
    })?;
    let spend_ref = decode_state(&mut decoder, decode_spend)?;
    let unfinished_work = decode_narrative(&mut decoder)?;
    let uncertainty = decode_narrative(&mut decoder)?;
    decoder.finish()?;

    Ok(OutcomeManifestV1 {
        workspace_id,
        mission_id,
        completion,
        policy_classification,
        boundary_event_seq,
        completion_recorded_at_ms,
        accountable_resident_id,
        policy_high_impact,
        appendix_request,
        canonicalizer_version,
        renderer_version,
        success_conditions,
        delivered_artifacts,
        proposal_refs,
        ruling_refs,
        evaluation_refs,
        receipt_refs,
        spend_ref,
        unfinished_work,
        uncertainty,
    })
}

// ---------------------------------------------------------------------------------------------
// Workspace scope and the citation census
// ---------------------------------------------------------------------------------------------

/// Every `SourceRef` a manifest cites, in a fixed walk order.
///
/// One walk, used by the Workspace check, the citation census, and the appendix, so the three can
/// never disagree about what "cited" means.
///
/// **Lazy on purpose.** Its two bounded callers stop early -- the census at the 257th distinct
/// object, the Workspace check at the first foreign reference -- and a walk that collected first
/// would pay for the whole input before refusing part of it. That was measured: collecting cost one
/// pointer for every citation past the bound. The appendix collects, but it runs after the census
/// has already proved there are at most 256.
fn cited_refs(manifest: &OutcomeManifestV1) -> impl Iterator<Item = &SourceRef> {
    // The Mission is an identity, not a citation: it has no bytes to cite (R18.4).
    let identities = [&manifest.completion, &manifest.policy_classification];
    let conditions = manifest.success_conditions.iter().flat_map(|condition| {
        condition
            .citations
            .verified()
            .map(|citations| citations.iter())
            .into_iter()
            .flatten()
    });
    let collections = [
        &manifest.delivered_artifacts,
        &manifest.proposal_refs,
        &manifest.ruling_refs,
        &manifest.evaluation_refs,
    ]
    .into_iter()
    .flat_map(|collection| {
        collection
            .verified()
            .map(|entries| entries.iter())
            .into_iter()
            .flatten()
    });

    identities.into_iter().chain(conditions).chain(collections)
}

/// Refuses a manifest that reaches outside its own Workspace.
///
/// A cross-Workspace reference is an integrity failure, not an honest domain state: it aborts
/// rather than rendering as a `broken` row.
fn require_single_workspace(manifest: &OutcomeManifestV1) -> Result<(), MalformedOutcome> {
    for reference in cited_refs(manifest) {
        if reference.workspace_id != manifest.workspace_id {
            return Err(MalformedOutcome::CrossWorkspace);
        }
    }
    if let Some(receipts) = manifest.receipt_refs.verified() {
        for receipt in receipts {
            if receipt.workspace_id != manifest.workspace_id {
                return Err(MalformedOutcome::CrossWorkspace);
            }
        }
    }
    if let Some(spend) = manifest.spend_ref.verified()
        && spend.workspace_id != manifest.workspace_id
    {
        return Err(MalformedOutcome::CrossWorkspace);
    }
    Ok(())
}

/// Counts distinct cited objects, stopping at the first count that exceeds the bound.
///
/// Distinct by typed identity, schema version, and digest -- one object cited twice is one cited
/// object. The walk stops the moment the set exceeds the limit, so the reported figure is an
/// observed-at-least count rather than a total nobody finished counting.
/// The closed set of lanes the citation census counts.
///
/// Receipts are a lane of their own because they are not a `SourceKind`: they have no content
/// digest, so they are cited by `(Workspace, run UUID, Receipt version)` plus an Outcome checksum.
/// A census over a closed vocabulary does not borrow an unassigned tag byte to make room for them
/// (R13.8) -- an unassigned tag is unassigned because the next arm will want it, and a census that
/// silently shares a lane with a future source kind merges two cited objects into one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CitationLane {
    Source(SourceKind),
    Receipt,
}

impl CitationLane {
    /// The lane's own name. Text rather than a tag byte, so a lane cannot collide with a source
    /// kind by arithmetic and a new lane costs a new word rather than a free number.
    fn as_str(self) -> &'static str {
        match self {
            Self::Source(kind) => kind.as_str(),
            Self::Receipt => "receipt",
        }
    }
}

/// One cited object's census identity: its lane, then the identity that lane spells.
fn citation_key(lane: CitationLane, identity: Vec<KeyPart>) -> Vec<KeyPart> {
    let mut key = vec![KeyPart::Text(lane.as_str().to_owned())];
    key.extend(identity);
    key
}

/// Every collection's stored length against `MAX_COLLECTION_ENTRIES`, with no allocation: the
/// lengths are facts the manifest already holds. This is the same bound `Encoder::count` applies
/// while encoding, lifted to run before anything clones, sorts or walks -- the bound is owned by
/// the grammar and applied before the work it exists to prevent.
fn require_collection_lengths(manifest: &OutcomeManifestV1) -> Result<(), MalformedOutcome> {
    let over = |length: usize| length > MAX_COLLECTION_ENTRIES as usize;
    let refs_over = |state: &OutcomeState<Vec<SourceRef>>| matches!(state, OutcomeState::Verified(refs) if over(refs.len()));
    if over(manifest.success_conditions.len())
        || refs_over(&manifest.delivered_artifacts)
        || refs_over(&manifest.proposal_refs)
        || refs_over(&manifest.ruling_refs)
        || refs_over(&manifest.evaluation_refs)
        || matches!(&manifest.receipt_refs, OutcomeState::Verified(refs) if over(refs.len()))
        || matches!(&manifest.unfinished_work, OutcomeState::Verified(items) if over(items.len()))
        || matches!(&manifest.uncertainty, OutcomeState::Verified(items) if over(items.len()))
    {
        return Err(MalformedOutcome::OverLimitLength);
    }
    Ok(())
}

fn citation_census(manifest: &OutcomeManifestV1) -> Result<u64, BoundRefusal> {
    let mut seen: BTreeSet<Vec<KeyPart>> = BTreeSet::new();
    let refuse_if_over = |seen: &BTreeSet<Vec<KeyPart>>| -> Result<(), BoundRefusal> {
        let observed = seen.len() as u64;
        if observed > MAX_CITATIONS {
            return Err(BoundRefusal {
                bound: OutcomeBound::CitationCount,
                configured: MAX_CITATIONS,
                observed_at_least: observed,
            });
        }
        Ok(())
    };

    for reference in cited_refs(manifest) {
        seen.insert(citation_key(
            CitationLane::Source(reference.kind),
            source_identity_key(reference),
        ));
        refuse_if_over(&seen)?;
    }
    if let Some(receipts) = manifest.receipt_refs.verified() {
        for receipt in receipts {
            seen.insert(citation_key(
                CitationLane::Receipt,
                vec![
                    KeyPart::Text(uuid_key(&receipt.run_uuid)),
                    KeyPart::Num(u64::from(receipt.receipt_version)),
                    KeyPart::Text(DIGEST_ALGORITHM.to_owned()),
                    KeyPart::Text(RECEIPT_CHECKSUM_LABEL.to_owned()),
                    KeyPart::Text(receipt.outcome_checksum.clone()),
                ],
            ));
            refuse_if_over(&seen)?;
        }
    }
    if let Some(spend) = manifest.spend_ref.verified() {
        seen.insert(citation_key(
            CitationLane::Source(SourceKind::Spend),
            vec![
                KeyPart::Text(uuid_key(&spend.spend_id)),
                KeyPart::Num(u64::from(spend.schema_version)),
                KeyPart::Text(spend.digest.algorithm.clone()),
                KeyPart::Text(spend.digest.canonicalizer.clone()),
                KeyPart::Text(spend.digest.value.clone()),
            ],
        ));
        refuse_if_over(&seen)?;
    }
    Ok(seen.len() as u64)
}

// ---------------------------------------------------------------------------------------------
// The versioned escaping function
// ---------------------------------------------------------------------------------------------

/// A versioned CommonMark escaping function. `renderer_version` selects it; there is no default.
type Escaper = fn(&str) -> String;

/// Looks up the escaping function a renderer version owns.
///
/// `None` means the version is unknown, and an unknown version is refused rather than silently
/// escaped by whatever function happens to be newest. A document that cannot say which function
/// escaped its text is not reproducible.
fn escaping_for(version: &str) -> Option<Escaper> {
    match version {
        RENDERER_V1 => Some(escape_v1),
        _ => None,
    }
}

/// Characters that can begin a CommonMark construct anywhere in a line, plus the table delimiter.
const ESCAPE_ANYWHERE: &[char] = &['\\', '`', '*', '_', '[', ']', '<', '>', '&', '|', '~', '!'];
/// Characters that can begin a leaf block only at the start of a line. Escaped only in position
/// zero, which keeps the function context-free while leaving ordinary prose readable.
const ESCAPE_AT_START: &[char] = &['#', '+', '-', '=', '.', ')'];

/// `beacon.outcome.md.v1`'s escaping function.
///
/// Three rules, each a function of the string alone:
///
/// 1. every character in [`ESCAPE_ANYWHERE`] is backslash-escaped wherever it appears;
/// 2. a leading character in [`ESCAPE_AT_START`] is backslash-escaped;
/// 3. a leading run of digits followed by `.` or `)` has that punctuation escaped, because that is
///    the one ordered-list opener a leading-character rule cannot see.
///
/// Raw HTML never survives: `<` and `>` are escaped in rule 1, so a stored `<script>` renders as
/// the literal text `<script>` and starts no HTML block. Table cells survive too, because `|` is
/// escaped in the same rule. Line breaks never reach this function: v1 refuses multi-line human
/// text at the manifest and source boundary.
///
/// It is deliberately not context-dependent. A function whose output depends on where its result
/// lands would change bytes when a section moves, and `renderer_version` could not name it.
fn escape_v1(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    let digits = value.chars().take_while(char::is_ascii_digit).count();
    for (index, character) in value.chars().enumerate() {
        let at_start = index == 0 && ESCAPE_AT_START.contains(&character);
        let after_digits = digits > 0 && index == digits && matches!(character, '.' | ')');
        if ESCAPE_ANYWHERE.contains(&character) || at_start || after_digits {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Deterministic value formatting
// ---------------------------------------------------------------------------------------------

/// A stored UTC millisecond timestamp as RFC 3339 with `Z`, always to millisecond precision.
///
/// Integer arithmetic over the proleptic Gregorian calendar, from Howard Hinnant's
/// `civil_from_days`. No current clock, no timezone database, no locale, and no conditional
/// precision: a fixed-width field cannot render two ways for one stored value.
fn rfc3339_utc(milliseconds: u64) -> String {
    let total_seconds = milliseconds / 1_000;
    let millis = milliseconds % 1_000;
    let days = i64::try_from(total_seconds / 86_400).unwrap_or(i64::MAX);
    let second_of_day = total_seconds % 86_400;
    let (hour, minute, second) = (
        second_of_day / 3_600,
        (second_of_day % 3_600) / 60,
        second_of_day % 60,
    );

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = if month <= 2 { year + 1 } else { year };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Stored integer minor units as a decimal string.
///
/// Integer arithmetic only. Binary floating point is never involved, so no stored amount can
/// render as a value that is not exactly what was measured.
fn decimal_from_minor_units(amount_minor: i64, exponent: u8) -> String {
    let negative = amount_minor < 0;
    let magnitude = i128::from(amount_minor).unsigned_abs();
    let scale = 10_u128.pow(u32::from(exponent));
    let whole = magnitude / scale;
    let fraction = magnitude % scale;
    let sign = if negative { "-" } else { "" };
    if exponent == 0 {
        format!("{sign}{whole}")
    } else {
        let width = usize::from(exponent);
        format!("{sign}{whole}.{fraction:0width$}")
    }
}

// ---------------------------------------------------------------------------------------------
// The values the owning modules' verified readers returned
// ---------------------------------------------------------------------------------------------

/// One verified text value, keyed by the stable ID the manifest cites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedText {
    pub id: String,
    pub text: String,
}

/// The metadata an Artifact's owning module returns beside its exact bytes.
///
/// Keyed by the record digest, because that IS an Artifact's identity (R16): an Artifact carries no
/// UUID inside its canonical bytes, so a manifest cites it by digest and this value is matched to
/// the citation the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedArtifact {
    pub digest_value: String,
    pub media_type: String,
    pub title: String,
}

/// The values each owning module's verified reader returned at the manifest's boundary event.
///
/// Slice 1 constructs these by hand; slice 2's collector produces them from verified reads. They
/// are deliberately separate from the manifest: the manifest carries identities and digests, so a
/// joined projection can never become a rendered claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSources {
    /// The Mission declaration's title, read through the Mission module and proved against the completion the manifest
    /// is frozen on (the Mission itself has no digest to prove against, R18.4).
    pub mission_title: String,
    /// One statement per manifest success condition, keyed by `condition_id`.
    pub condition_statements: Vec<VerifiedText>,
    /// One entry per cited delivered Artifact, keyed by the record digest value.
    pub artifacts: Vec<VerifiedArtifact>,
    /// One entry per cited unfinished-work item, keyed by `item_id`.
    pub unfinished_work: Vec<VerifiedText>,
    /// One entry per cited uncertainty item, keyed by `item_id`.
    pub uncertainty: Vec<VerifiedText>,
    /// Total bytes read through owning verified readers to produce these values.
    ///
    /// Slice 1 takes the measurement as an input so the bound is testable; the *limit* is this
    /// function's, not the caller's. Slice 2's collector measures it while reading and must refuse
    /// before a partial manifest exists.
    pub verified_source_bytes: u64,
}

fn lookup_text<'a>(
    values: &'a [VerifiedText],
    kind: &'static str,
    id: &str,
) -> Result<&'a str, SourceMismatch> {
    let mut found: Option<&VerifiedText> = None;
    for value in values {
        if value.id == id {
            if found.is_some() {
                return Err(SourceMismatch::DuplicateValue {
                    kind,
                    id: id.to_owned(),
                });
            }
            found = Some(value);
        }
    }
    found
        .map(|value| value.text.as_str())
        .ok_or_else(|| SourceMismatch::MissingValue {
            kind,
            id: id.to_owned(),
        })
}

fn require_no_extra_text(
    values: &[VerifiedText],
    kind: &'static str,
    cited: &BTreeSet<String>,
) -> Result<(), SourceMismatch> {
    for value in values {
        if !cited.contains(&value.id) {
            return Err(SourceMismatch::UnreferencedValue {
                kind,
                id: value.id.clone(),
            });
        }
    }
    Ok(())
}

/// Renders one narrative value as CommonMark blocks, preserving its LF structure exactly.
///
/// A run of blank lines is a paragraph break; every other newline is a hard line break, written as
/// a trailing backslash rather than two trailing spaces because the byte rules forbid trailing
/// whitespace. Every line goes through the versioned escaping function first, so no stored value
/// becomes structure, and nothing is added to or removed from the text itself.
///
/// A leading TAB is preserved and may render as indentation. That is a rendering effect of a value
/// Beacon was told to keep, not an alteration of it.
fn narrative_lines(value: &str, escape: Escaper) -> Vec<String> {
    fn flush(paragraph: &mut Vec<String>, out: &mut Vec<String>) {
        if paragraph.is_empty() {
            return;
        }
        let last = paragraph.len() - 1;
        for (index, line) in paragraph.iter().enumerate() {
            out.push(if index == last {
                line.clone()
            } else {
                format!("{line}\\")
            });
        }
        paragraph.clear();
    }

    let mut out = Vec::new();
    let mut paragraph = Vec::new();
    for line in value.split('\n') {
        if line.is_empty() {
            // One emitted blank line per stored blank line. Collapsing a run of them would make two
            // stored values one document, which is the alteration this renderer refuses to make.
            flush(&mut paragraph, &mut out);
            out.push(String::new());
        } else {
            paragraph.push(escape(line));
        }
    }
    flush(&mut paragraph, &mut out);
    out
}

/// A media type as its owning Artifact module stored it.
///
/// Validated rather than escaped, because it renders inside a code span where a backslash would be
/// literal. The charset excludes backtick and `|` so a stored value can neither close the span nor
/// add a table cell.
fn require_media_type(value: &str) -> Result<(), MalformedOutcome> {
    if value.is_empty() || value.len() > MAX_TOKEN_BYTES {
        return Err(MalformedOutcome::Token);
    }
    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'/' | b';' | b'=' | b'+' | b'.' | b'-' | b'_' | b' ')
    }) {
        Ok(())
    } else {
        Err(MalformedOutcome::Token)
    }
}

// ---------------------------------------------------------------------------------------------
// The renderer
// ---------------------------------------------------------------------------------------------

/// Accumulates Markdown and refuses the moment the emitted-bytes bound is exceeded.
///
/// The buffer is owned here and dropped on refusal, so an exhausted bound produces no partial
/// document: there is no path that returns bytes and a refusal together.
struct Markdown {
    out: String,
}

impl Markdown {
    fn new() -> Self {
        Self { out: String::new() }
    }

    /// Appends one line, or refuses before appending it.
    ///
    /// The cap is checked against the line's projected total, not against the buffer after it has
    /// already grown: a bound that exists to prevent work must run before the work. `String` also
    /// grows geometrically, so an append that overshoots the cap can double a megabyte of buffer
    /// on the way to reporting that a megabyte was too much.
    fn line(&mut self, text: &str) -> Result<(), BoundRefusal> {
        let projected = self.out.len() as u64 + text.len() as u64 + 1;
        if projected > MAX_EMITTED_MARKDOWN_BYTES {
            return Err(BoundRefusal {
                bound: OutcomeBound::EmittedMarkdownBytes,
                configured: MAX_EMITTED_MARKDOWN_BYTES,
                observed_at_least: projected,
            });
        }
        self.out.push_str(text);
        self.out.push('\n');
        Ok(())
    }

    fn blank(&mut self) -> Result<(), BoundRefusal> {
        self.line("")
    }
}

/// The one spelling of a digest anywhere in the document.
///
/// One value rendered three ways in one document is three things a reader has to learn are the
/// same thing, and three places a change has to be made in step. The appendix's cited-objects
/// table is the deliberate exception: it is a column layout, so the envelope's parts are columns
/// rather than one span.
fn digest_span(digest: &SourceDigest) -> String {
    format!("`{}:{}`", digest.algorithm, digest.value)
}

/// A Receipt's Outcome checksum, spelled by the same function as every other digest.
///
/// The envelope is fixed rather than stored, because a Receipt has no content digest of its own:
/// this is an Outcome verification checksum over stored `receipt_json` bytes (Kevin's ruling 5).
fn checksum_span(value: &str) -> String {
    digest_span(&SourceDigest {
        algorithm: DIGEST_ALGORITHM.to_owned(),
        canonicalizer: RECEIPT_CHECKSUM_LABEL.to_owned(),
        value: value.to_owned(),
    })
}

fn revision_cell(state: &OutcomeState<PinnedRevision>, escape: Escaper) -> String {
    match state {
        OutcomeState::Verified(revision) => {
            format!("`{}` `{}`", revision.kind.as_str(), revision.value)
        }
        other => format!(
            "`{}` (reason: {})",
            other.literal(),
            escape(other.reason().unwrap_or_default())
        ),
    }
}

fn state_cell(literal: &str, reason: Option<&str>, escape: Escaper) -> String {
    match reason {
        Some(reason) => format!("`{literal}` (reason: {})", escape(reason)),
        None => format!("`{literal}`"),
    }
}

/// Renders one Outcome Document from an immutable manifest and the verified values its references
/// name.
///
/// Pure: no clock, no environment, no filesystem, no database, no network, and no ambient
/// collection order. The same manifest and the same sources produce the same bytes on any host, in
/// any timezone, under any locale.
///
/// It refuses rather than approximating. An exhausted bound, a manifest that cannot encode, a
/// cross-Workspace reference, an unknown renderer version, or sources that do not correspond to the
/// manifest all return `Err` and no bytes.
pub fn render(
    manifest: &OutcomeManifestV1,
    sources: &VerifiedSources,
) -> Result<String, OutcomeRefusal> {
    // Bounds first, on the manifest exactly as supplied, before anything is cloned, sorted, or
    // serialized. Canonicalization is the allocation these two bounds exist to prevent -- it clones
    // and sorts every collection into a fresh byte vector -- so a bound checked after it has
    // already paid for what it refuses. Sorting cannot change the census either: it reorders a set
    // it does not add to.
    // The per-collection length bound, on the STORED counts, before the census walks anything.
    // The census clones a key per entry into a set; for an already-refusable 50,000-entry array
    // that was 20 MB of allocation on the way to a refusal the encoder would have given anyway
    // (sol1's cold read, #6). Same refusal, earlier: `OverLimitLength`, exactly what `encode`
    // raises for the same input, so nothing observable changes except when it is said.
    require_collection_lengths(manifest)?;
    let citations = citation_census(manifest).map_err(OutcomeRefusal::Bound)?;
    debug_assert!(citations <= MAX_CITATIONS);
    if sources.verified_source_bytes > MAX_VERIFIED_SOURCE_BYTES {
        return Err(OutcomeRefusal::Bound(BoundRefusal {
            bound: OutcomeBound::VerifiedSourceBytes,
            configured: MAX_VERIFIED_SOURCE_BYTES,
            observed_at_least: sources.verified_source_bytes,
        }));
    }

    // The canonical bytes are the identity, and rendering works from exactly them: encoding sorts
    // every array, so decoding them back is how insertion order is proved not to reach the
    // document.
    let manifest_bytes = manifest.canonical_bytes()?;
    let manifest = OutcomeManifestV1::from_canonical_bytes(&manifest_bytes)?;
    require_single_workspace(&manifest)?;
    let manifest_digest = HEXLOWER.encode(&sha256(&manifest_bytes));

    let escape =
        escaping_for(&manifest.renderer_version).ok_or(MalformedOutcome::UnknownVersion)?;

    check_sources(&manifest, sources)?;

    let mut document = Markdown::new();
    render_front_matter(
        &mut document,
        &manifest,
        &manifest_digest,
        manifest_bytes.len(),
    )
    .map_err(OutcomeRefusal::Bound)?;
    render_body(&mut document, &manifest, sources, escape).map_err(OutcomeRefusal::Bound)?;
    render_appendix(
        &mut document,
        &manifest,
        &manifest_bytes,
        &manifest_digest,
        escape,
    )
    .map_err(OutcomeRefusal::Bound)?;
    Ok(document.out)
}

/// Refuses when the manifest and the verified sources do not correspond exactly.
///
/// A cited object with no verified value would make the renderer invent a row; a verified value
/// nothing cites would make it render something the manifest never froze.
fn check_sources(
    manifest: &OutcomeManifestV1,
    sources: &VerifiedSources,
) -> Result<(), OutcomeRefusal> {
    require_label_text(&sources.mission_title)?;

    let mut condition_ids = BTreeSet::new();
    for condition in &manifest.success_conditions {
        let statement = lookup_text(
            &sources.condition_statements,
            "success condition",
            &condition.condition_id,
        )?;
        require_narrative_text(statement)?;
        condition_ids.insert(condition.condition_id.clone());
    }
    require_no_extra_text(
        &sources.condition_statements,
        "success condition",
        &condition_ids,
    )?;

    for (state, values, kind) in [
        (
            &manifest.unfinished_work,
            &sources.unfinished_work,
            "unfinished work",
        ),
        (&manifest.uncertainty, &sources.uncertainty, "uncertainty"),
    ] {
        let mut ids = BTreeSet::new();
        if let Some(items) = state.verified() {
            for item in items {
                let text = lookup_text(values, kind, &item.item_id)?;
                require_narrative_text(text)?;
                ids.insert(item.item_id.clone());
            }
        }
        require_no_extra_text(values, kind, &ids)?;
    }

    let mut artifact_ids = BTreeSet::new();
    if let Some(refs) = manifest.delivered_artifacts.verified() {
        for reference in refs {
            let id = reference.digest.value.clone();
            let mut found = 0_usize;
            for artifact in &sources.artifacts {
                if artifact.digest_value == reference.digest.value {
                    found += 1;
                    require_media_type(&artifact.media_type)?;
                    require_label_text(&artifact.title)?;
                }
            }
            match found {
                0 => {
                    return Err(SourceMismatch::MissingValue {
                        kind: "artifact",
                        id,
                    }
                    .into());
                }
                1 => {
                    artifact_ids.insert(id);
                }
                _ => {
                    return Err(SourceMismatch::DuplicateValue {
                        kind: "artifact",
                        id,
                    }
                    .into());
                }
            }
        }
    }
    for artifact in &sources.artifacts {
        if !artifact_ids.contains(&artifact.digest_value.clone()) {
            return Err(SourceMismatch::UnreferencedValue {
                kind: "artifact",
                id: artifact.digest_value.clone(),
            }
            .into());
        }
    }
    Ok(())
}

/// Versioned front matter: fixed key order, no omission, no derived value that could disagree with
/// its inputs.
///
/// It names the manifest, never the document's own digest. A document that hashed itself could
/// never be verified: the digest would have to exist before the bytes it covers. The owning
/// Artifact record stores the content digest externally.
fn render_front_matter(
    document: &mut Markdown,
    manifest: &OutcomeManifestV1,
    manifest_digest: &str,
    manifest_bytes: usize,
) -> Result<(), BoundRefusal> {
    document.line("---")?;
    document.line(&format!("renderer_version: {}", manifest.renderer_version))?;
    document.line(&format!(
        "canonicalizer_version: {}",
        manifest.canonicalizer_version
    ))?;
    document.line(&format!("manifest_digest_algorithm: {DIGEST_ALGORITHM}"))?;
    document.line(&format!("manifest_digest: {manifest_digest}"))?;
    document.line(&format!("manifest_bytes: {manifest_bytes}"))?;
    document.line(&format!(
        "workspace_id: {}",
        uuid_key(&manifest.workspace_id)
    ))?;
    document.line(&format!("mission_id: {}", uuid_key(&manifest.mission_id)))?;
    document.line(&format!(
        "completion_id: {}",
        identity_cells(&manifest.completion).0
    ))?;
    document.line(&format!(
        "boundary_event_seq: {}",
        manifest.boundary_event_seq
    ))?;
    document.line(&format!(
        "accountable_resident_id: {}",
        uuid_key(&manifest.accountable_resident_id)
    ))?;
    document.line(&format!(
        "policy_classification_id: {}",
        identity_cells(&manifest.policy_classification).0
    ))?;
    document.line(&format!(
        "policy_high_impact: {}",
        manifest.policy_high_impact
    ))?;
    document.line(&format!(
        "appendix_request: {}",
        manifest.appendix_request.as_str()
    ))?;
    document.line(&format!(
        "appendix_mode: {}",
        manifest.appendix_mode().as_str()
    ))?;
    document.line("---")?;
    document.blank()
}

fn render_body(
    document: &mut Markdown,
    manifest: &OutcomeManifestV1,
    sources: &VerifiedSources,
    escape: Escaper,
) -> Result<(), BoundRefusal> {
    let boundary = manifest.boundary_event_seq;
    document.line("# Mission outcome")?;
    document.blank()?;
    document.line(&format!(
        "Every fact below was verified at boundary event `{boundary}`; it is not re-verified when this document is read."
    ))?;
    document.line(
        "This document is a record. Nothing derives permission to proceed, Mission completion, or a success verdict from its existence.",
    )?;
    document.blank()?;

    document.line("## 1. Mission and completion identity")?;
    document.blank()?;
    document.line("| field | value |")?;
    document.line("| --- | --- |")?;
    document.line(&format!(
        "| workspace | `{}` |",
        uuid_key(&manifest.workspace_id)
    ))?;
    document.line(&format!(
        "| Mission | `{}` |",
        uuid_key(&manifest.mission_id)
    ))?;
    document.line(&format!(
        "| Mission declaration | {} |",
        escape(&sources.mission_title)
    ))?;
    document.line(&format!(
        "| completion | `{}` |",
        identity_cells(&manifest.completion).0
    ))?;
    document.line(&format!(
        "| completion schema version | `{}` |",
        identity_cells(&manifest.completion).1
    ))?;
    document.line(&format!(
        "| completion digest | {} (`{}`) |",
        digest_span(&manifest.completion.digest),
        manifest.completion.digest.canonicalizer
    ))?;
    document.line(&format!(
        "| completion recorded at | `{}` |",
        rfc3339_utc(manifest.completion_recorded_at_ms)
    ))?;
    document.line(&format!("| boundary event sequence | `{boundary}` |"))?;
    document.line(&format!(
        "| accountable Resident | `{}` |",
        uuid_key(&manifest.accountable_resident_id)
    ))?;
    document.line(&format!(
        "| Policy classification | `{}` |",
        identity_cells(&manifest.policy_classification).0
    ))?;
    document.line(&format!(
        "| Policy high impact | `{}` |",
        manifest.policy_high_impact
    ))?;
    document.blank()?;

    document.line("## 2. Success conditions")?;
    document.blank()?;
    for (position, condition) in manifest.success_conditions.iter().enumerate() {
        document.line(&format!(
            "### 2.{} `{}`",
            position + 1,
            condition.condition_id
        ))?;
        document.blank()?;
        let statement = sources
            .condition_statements
            .iter()
            .find(|value| value.id == condition.condition_id)
            .map(|value| value.text.as_str())
            .unwrap_or_default();
        for line in narrative_lines(statement, escape) {
            document.line(&line)?;
        }
        document.blank()?;
        document.line(&format!("- ordinal: `{}`", condition.ordinal))?;
        match &condition.outcome {
            OutcomeState::Verified(result) => document.line(&format!(
                "- state: `{}` (`{}`)",
                STATE_VERIFIED,
                result.as_str()
            ))?,
            other => document.line(&format!(
                "- state: {}",
                state_cell(other.literal(), other.reason(), escape)
            ))?,
        }
        match &condition.citations {
            OutcomeState::Verified(refs) if refs.is_empty() => {
                document.line(&format!("- citations: {VERIFIED_EMPTY}"))?;
            }
            OutcomeState::Verified(refs) => {
                document.line("- citations:")?;
                for reference in refs {
                    document.line(&format!("    - {}", reference_line(reference)))?;
                }
            }
            other => document.line(&format!(
                "- citations: {}",
                state_cell(other.literal(), other.reason(), escape)
            ))?,
        }
        document.blank()?;
    }

    document.line("## 3. Delivered Artifacts")?;
    document.blank()?;
    match &manifest.delivered_artifacts {
        OutcomeState::Verified(refs) if refs.is_empty() => document.line(VERIFIED_EMPTY)?,
        OutcomeState::Verified(refs) => {
            document.line("| Artifact | schema | digest | canonicalizer | media type | title |")?;
            document.line("| --- | --- | --- | --- | --- | --- |")?;
            for reference in refs {
                let artifact = sources
                    .artifacts
                    .iter()
                    .find(|value| value.digest_value == reference.digest.value);
                let media_type = artifact
                    .map(|value| value.media_type.as_str())
                    .unwrap_or("");
                let title = artifact.map(|value| value.title.as_str()).unwrap_or("");
                let (identity, schema) = identity_cells(reference);
                document.line(&format!(
                    "| `{}` | `{}` | {} | `{}` | `{}` | {} |",
                    identity,
                    schema,
                    digest_span(&reference.digest),
                    reference.digest.canonicalizer,
                    media_type,
                    escape(title)
                ))?;
            }
        }
        other => document.line(&state_cell(other.literal(), other.reason(), escape))?,
    }
    document.blank()?;

    document.line("## 4. Evaluations and cited support")?;
    document.blank()?;
    render_ref_table(document, &manifest.evaluation_refs, "Evaluation", escape)?;
    document.blank()?;

    document.line("## 5. Actual spend and coverage")?;
    document.blank()?;
    match &manifest.spend_ref {
        OutcomeState::Verified(spend) => {
            document.line("| field | value |")?;
            document.line("| --- | --- |")?;
            document.line(&format!(
                "| spend record | `{}` |",
                uuid_key(&spend.spend_id)
            ))?;
            document.line(&format!("| schema version | `{}` |", spend.schema_version))?;
            document.line(&format!(
                "| digest | {} (`{}`) |",
                digest_span(&spend.digest),
                spend.digest.canonicalizer
            ))?;
            document.line(&format!(
                "| amount | `{}` `{}` |",
                decimal_from_minor_units(spend.amount_minor, spend.minor_units_exponent),
                spend.currency
            ))?;
            document.line(&format!(
                "| coverage | events `{}` through `{}` |",
                spend.coverage_from_event_seq, spend.coverage_to_event_seq
            ))?;
            document.line(&format!(
                "| measurement | `{}` |",
                spend.measurement.as_str()
            ))?;
            if spend.measurement == SpendMeasurementKind::PartiallyMeasured {
                document.blank()?;
                document.line(
                    "This measurement is partial: entries inside the coverage window could not be counted, so the amount is a floor over that window and not the Mission's spend.",
                )?;
            }
        }
        other => {
            document.line(&state_cell(other.literal(), other.reason(), escape))?;
            document.blank()?;
            document.line(
                "No amount is stated. An unmeasured or absent spend record is not zero spend.",
            )?;
        }
    }
    document.blank()?;

    document.line("## 6. Unfinished work")?;
    document.blank()?;
    render_narrative(
        document,
        6,
        &manifest.unfinished_work,
        &sources.unfinished_work,
        escape,
    )?;
    document.blank()?;

    document.line("## 7. Uncertainty")?;
    document.blank()?;
    render_narrative(
        document,
        7,
        &manifest.uncertainty,
        &sources.uncertainty,
        escape,
    )?;
    document.blank()?;

    document.line("## 8. Rulings and Proposal lineage")?;
    document.blank()?;
    document.line("### 8.1 Rulings")?;
    document.blank()?;
    render_ref_table(document, &manifest.ruling_refs, "Ruling", escape)?;
    document.blank()?;
    document.line("### 8.2 Proposal lineage")?;
    document.blank()?;
    render_ref_table(document, &manifest.proposal_refs, "Proposal", escape)?;
    document.blank()?;

    document.line("## 9. Linked Receipts")?;
    document.blank()?;
    match &manifest.receipt_refs {
        OutcomeState::Verified(refs) if refs.is_empty() => document.line(VERIFIED_EMPTY)?,
        OutcomeState::Verified(refs) => {
            document.line("| run | Receipt version | Outcome checksum |")?;
            document.line("| --- | --- | --- |")?;
            for receipt in refs {
                document.line(&format!(
                    "| `{}` | `{}` | {} |",
                    uuid_key(&receipt.run_uuid),
                    receipt.receipt_version,
                    checksum_span(&receipt.outcome_checksum)
                ))?;
            }
            document.blank()?;
            document.line(&format!(
                "Each checksum above is an Outcome verification checksum (`{RECEIPT_CHECKSUM_LABEL}`) over the exact stored `receipt_json` bytes. It is not a Receipt content digest: Receipt v1 and v2 define none."
            ))?;
        }
        other => document.line(&state_cell(other.literal(), other.reason(), escape))?,
    }
    document.blank()
}

fn reference_line(reference: &SourceRef) -> String {
    let (identity, schema) = identity_cells(reference);
    format!(
        "`{}` `{}` schema `{}` {} (`{}`)",
        reference.kind.as_str(),
        identity,
        schema,
        digest_span(&reference.digest),
        reference.digest.canonicalizer
    )
}

fn render_ref_table(
    document: &mut Markdown,
    state: &OutcomeState<Vec<SourceRef>>,
    heading: &str,
    escape: Escaper,
) -> Result<(), BoundRefusal> {
    match state {
        OutcomeState::Verified(refs) if refs.is_empty() => document.line(VERIFIED_EMPTY),
        OutcomeState::Verified(refs) => {
            document.line(&format!("| {heading} | schema | digest | canonicalizer |"))?;
            document.line("| --- | --- | --- | --- |")?;
            for reference in refs {
                let (identity, schema) = identity_cells(reference);
                document.line(&format!(
                    "| `{}` | `{}` | {} | `{}` |",
                    identity,
                    schema,
                    digest_span(&reference.digest),
                    reference.digest.canonicalizer
                ))?;
            }
            Ok(())
        }
        other => document.line(&state_cell(other.literal(), other.reason(), escape)),
    }
}

fn render_narrative(
    document: &mut Markdown,
    section: usize,
    state: &OutcomeState<Vec<NarrativeRef>>,
    values: &[VerifiedText],
    escape: Escaper,
) -> Result<(), BoundRefusal> {
    match state {
        OutcomeState::Verified(items) if items.is_empty() => document.line(VERIFIED_EMPTY),
        OutcomeState::Verified(items) => {
            for (position, item) in items.iter().enumerate() {
                let text = values
                    .iter()
                    .find(|value| value.id == item.item_id)
                    .map(|value| value.text.as_str())
                    .unwrap_or_default();
                if position > 0 {
                    document.blank()?;
                }
                document.line(&format!(
                    "### {section}.{} `{}` (ordinal `{}`)",
                    position + 1,
                    item.item_id,
                    item.ordinal
                ))?;
                document.blank()?;
                for line in narrative_lines(text, escape) {
                    document.line(&line)?;
                }
            }
            Ok(())
        }
        other => document.line(&state_cell(other.literal(), other.reason(), escape)),
    }
}

/// The line a `verified` collection with no entries renders as.
///
/// "Verified as zero entries" and "absent" are different claims, and a blank section would say
/// neither. The renderer never derives one from the other.
const VERIFIED_EMPTY: &str =
    "`verified`: zero entries were recorded at the boundary event. This is not `absent`.";

/// Section 12 of the fixed shape: always present, as content or as a literal policy state.
///
/// Both modes embed the manifest proof, so any exported copy of this document lets a reader
/// recompute the manifest digest from the bytes in front of them rather than trusting the value in
/// the front matter.
fn render_appendix(
    document: &mut Markdown,
    manifest: &OutcomeManifestV1,
    manifest_bytes: &[u8],
    manifest_digest: &str,
    escape: Escaper,
) -> Result<(), BoundRefusal> {
    let boundary = manifest.boundary_event_seq;
    document.line("## 10. Verification appendix")?;
    document.blank()?;

    let mut subsection = 1_usize;
    match manifest.appendix_mode() {
        AppendixMode::PolicyStateNotHighImpact => {
            document.line(&format!(
                "`{}`",
                AppendixMode::PolicyStateNotHighImpact.as_str()
            ))?;
            document.blank()?;
            document.line(
                "The verified Policy classification is not high-impact and the caller requested `policy_default`, so the per-object verification rows are not included. The manifest proof below is present in every appendix mode.",
            )?;
            document.blank()?;
        }
        AppendixMode::Included => {
            document.line(&format!(
                "Every result below was verified at boundary event `{boundary}`; it is not re-verified at read time."
            ))?;
            document.blank()?;

            document.line(&format!("### 10.{subsection} Cited objects"))?;
            document.blank()?;
            document.line(
                "| kind | identity | schema | digest algorithm | digest | canonicalizer | pinned revision | verification result |",
            )?;
            document.line("| --- | --- | --- | --- | --- | --- | --- | --- |")?;
            let mut rows = cited_refs(manifest).cloned().collect::<Vec<_>>();
            rows = sorted(&rows, source_ref_key);
            // One object cited twice is one row: the appendix reports objects, and the
            // census it must agree with counts objects too.
            rows.dedup_by(|left, right| source_ref_key(left) == source_ref_key(right));
            for reference in &rows {
                let (identity, schema) = identity_cells(reference);
                document.line(&format!(
                    "| `{}` | `{}` | `{}` | `{}` | `{}` | `{}` | {} | `verified at boundary event {boundary}` |",
                    reference.kind.as_str(),
                    identity,
                    schema,
                    reference.digest.algorithm,
                    reference.digest.value,
                    reference.digest.canonicalizer,
                    revision_cell(&reference.pinned_revision, escape)
                ))?;
            }
            if let Some(spend) = manifest.spend_ref.verified() {
                document.line(&format!(
                    "| `{}` | `{}` | `{}` | `{}` | `{}` | `{}` | `{}` (reason: a spend measurement pins no content revision) | `verified at boundary event {boundary}` |",
                    SourceKind::Spend.as_str(),
                    uuid_key(&spend.spend_id),
                    spend.schema_version,
                    spend.digest.algorithm,
                    spend.digest.value,
                    spend.digest.canonicalizer,
                    STATE_ABSENT
                ))?;
            }
            document.blank()?;
            subsection += 1;

            document.line(&format!("### 10.{subsection} Linked Receipt checksums"))?;
            document.blank()?;
            match &manifest.receipt_refs {
                OutcomeState::Verified(refs) if refs.is_empty() => {
                    document.line(VERIFIED_EMPTY)?;
                }
                OutcomeState::Verified(refs) => {
                    document.line(
                        "| run | Receipt version | checksum algorithm | checksum | label |",
                    )?;
                    document.line("| --- | --- | --- | --- | --- |")?;
                    for receipt in refs {
                        document.line(&format!(
                            "| `{}` | `{}` | `{}` | `{}` | `{RECEIPT_CHECKSUM_LABEL}` |",
                            uuid_key(&receipt.run_uuid),
                            receipt.receipt_version,
                            DIGEST_ALGORITHM,
                            receipt.outcome_checksum
                        ))?;
                    }
                }
                other => document.line(&state_cell(other.literal(), other.reason(), escape))?,
            }
            document.blank()?;
            subsection += 1;

            document.line(&format!("### 10.{subsection} Candidate and render profile"))?;
            document.blank()?;
            document.line("| field | value |")?;
            document.line("| --- | --- |")?;
            document.line(&format!(
                "| candidate media type | `{CANDIDATE_MEDIA_TYPE}` |"
            ))?;
            document.line(&format!(
                "| renderer version | `{}` |",
                manifest.renderer_version
            ))?;
            document.line(&format!(
                "| PDF render profile | `{STATE_ABSENT}` (reason: this version generates no PDF derivative) |"
            ))?;
            document.blank()?;
            subsection += 1;

            document.line(&format!("### 10.{subsection} Policy and appendix mode"))?;
            document.blank()?;
            document.line("| field | value |")?;
            document.line("| --- | --- |")?;
            document.line(&format!(
                "| Policy classification | `{}` |",
                identity_cells(&manifest.policy_classification).0
            ))?;
            document.line(&format!(
                "| Policy classification digest | {} (`{}`) |",
                digest_span(&manifest.policy_classification.digest),
                manifest.policy_classification.digest.canonicalizer
            ))?;
            document.line(&format!(
                "| Policy high impact | `{}` |",
                manifest.policy_high_impact
            ))?;
            document.line(&format!(
                "| appendix request | `{}` |",
                manifest.appendix_request.as_str()
            ))?;
            document.line(&format!(
                "| appendix mode | `{}` |",
                manifest.appendix_mode().as_str()
            ))?;
            document.line(&format!("| selecting rule | {} |", appendix_rule(manifest)))?;
            document.blank()?;
            subsection += 1;
        }
    }

    document.line(&format!("### 10.{subsection} Manifest proof"))?;
    document.blank()?;
    document.line("| field | value |")?;
    document.line("| --- | --- |")?;
    document.line(&format!(
        "| canonicalizer | `{}` |",
        manifest.canonicalizer_version
    ))?;
    document.line(&format!("| digest algorithm | `{DIGEST_ALGORITHM}` |"))?;
    document.line(&format!("| digest | `{manifest_digest}` |"))?;
    document.line(&format!("| byte length | `{}` |", manifest_bytes.len()))?;
    document.line(&format!("| boundary event sequence | `{boundary}` |"))?;
    document.blank()?;
    document.line(
        "Concatenate the lines below in order, decode as lowercase hexadecimal, and hash the result with SHA-256 to obtain the manifest digest above.",
    )?;
    document.blank()?;
    document.line("```")?;
    let hex = HEXLOWER.encode(manifest_bytes);
    for chunk in hex.as_bytes().chunks(64) {
        document.line(std::str::from_utf8(chunk).unwrap_or_default())?;
    }
    document.line("```")
}

/// The rule that selected the appendix mode, stated so a reader never has to infer it.
fn appendix_rule(manifest: &OutcomeManifestV1) -> &'static str {
    if manifest.policy_high_impact {
        "the verified Policy classification is high-impact, which forces the appendix"
    } else if manifest.appendix_request == AppendixRequest::Include {
        "the caller requested `include`; there is no way to request omission"
    } else {
        "the caller requested `policy_default` and the verified Policy classification is not high-impact"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The emitted-Markdown cap is checked before the append, and this is the only place that can
    /// be measured: `render` drops the buffer on refusal, so no caller can observe how large it
    /// grew. Moving the check after `push_str` leaves the buffer over the cap and turns this red.
    #[test]
    fn the_markdown_cap_refuses_before_the_append_that_would_exceed_it() {
        let mut document = Markdown::new();
        let filler = "x".repeat(1_000);
        while (document.out.len() as u64) < MAX_EMITTED_MARKDOWN_BYTES - 1_002 {
            document.line(&filler).expect("under the cap");
        }
        let before = document.out.len() as u64;
        assert!(before < MAX_EMITTED_MARKDOWN_BYTES);

        let overshoot = "y".repeat(usize::try_from(MAX_EMITTED_MARKDOWN_BYTES).unwrap());
        let refusal = document.line(&overshoot).expect_err("over the cap");

        assert_eq!(refusal.bound, OutcomeBound::EmittedMarkdownBytes);
        assert_eq!(refusal.configured, MAX_EMITTED_MARKDOWN_BYTES);
        assert_eq!(
            refusal.observed_at_least,
            before + overshoot.len() as u64 + 1,
            "the refusal did not report the projected total"
        );
        assert_eq!(
            document.out.len() as u64,
            before,
            "the buffer grew past the cap before the cap was checked"
        );
    }
}
