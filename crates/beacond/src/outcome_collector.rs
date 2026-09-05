//! Outcome Document slice 2-B: the verified collector.
//!
//! The collector turns a set of citations into the manifest references `outcome.rs` renders from. It
//! is the place where five modules meet, and its whole discipline is one sentence from the design:
//! **a relation or a joined column may identify a candidate, and may never supply a renderable
//! claim.** Every source is re-read through its owning module's verified reader, which proves the
//! Workspace, the exact identity, the stored bytes and the digest before returning a typed value.
//!
//! All four readers the skeleton once typed as missing now exist and are wired here: measured spend
//! and the Mission impact classification (`0025`), `MissionCompletionV1` (`0026`), and the Artifact
//! store (`0027`, #63). A delivered Artifact is read through `artifact::verified_artifact_within` --
//! the reader `ArtifactService::get` wraps -- and the collector adds only the two facts that reader
//! cannot know: that the record is this Workspace's and scoped to this Mission. No seam remains,
//! and the enum that named them went with the last one: a variant no code can produce is a lie
//! about the store.
//!
//! Two properties are this module's own, and neither belongs to any single owning reader:
//!
//! 1. **The boundary freeze.** A completion reports at `boundary_event_seq`; everything cited beside
//!    it must have been recorded at or before that cut. The collector positions each source by the
//!    Event that recorded it and refuses one that arrives after — so an append-only record written
//!    later cannot join a document that was frozen before it existed (Kevin's ruling 4). The cut is
//!    the completion's **stored** number, never "the newest event now".
//! 2. **Reconciliation between sources.** Each owning reader proves its own record; nobody proves
//!    that a narrative item's `condition_ordinal` names a condition that completion actually has, or
//!    that the spend record the completion cites is the one the store returns for that citation.
//!    Those comparisons live here because they span two records, and a check that spans two records
//!    has no single owner.
//!
//! This slice reads; it does not persist and it does not assemble an `OutcomeManifestV1`. Manifest
//! storage and reconciliation against a stored manifest are slice C. **No migration:** every table
//! this module reads is already on `main`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use data_encoding::HEXLOWER;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::mission_completion::{
    ConditionOutcome, MissionCompletionV1, SpendReference, SupportRef,
};
use crate::outcome::{
    BoundRefusal, MAX_CITATIONS, OutcomeBound, OutcomeState, RECEIPT_CHECKSUM_LABEL, ReceiptRef,
    SourceKind,
};
use crate::proposal::sha256;
use crate::spend::{MeasuredSpend, SpendMeasurement};

/// Why a collection did not happen.
///
/// Every arm is a refusal. There is deliberately no arm that carries a partially collected result:
/// the design's §5 rule 6 says a corrupt store, a digest mismatch, a cross-Workspace reference or a
/// broken reader **aborts** rather than becoming an honest domain `broken` row, and a half-built
/// manifest is the shape that would let one through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectorRefusal {
    /// Outcome's own bound, refused before anything was read.
    Bound(BoundRefusal),
    /// The cited object is not in this Workspace's store at all.
    ///
    /// `kind` is a name rather than a `SourceKind`, because a Receipt is not one: it has no content
    /// digest and is cited by `(Workspace, run UUID, version)`. Borrowing a source tag to say
    /// "receipt" is the R13.8 mistake, and a refusal is a worse place to make it than a census.
    Absent { kind: &'static str, id: String },
    /// The owning reader refused it. The reason is the reader's, repeated rather than reinterpreted.
    Unverified { kind: &'static str, reason: String },
    /// The object is stored and verified, and it was recorded **after** the completion's boundary.
    ///
    /// Distinct from `Absent`: it exists, it is this Workspace's, and it is simply not part of this
    /// Outcome. Collapsing it into absence would tell a caller their store is missing something it
    /// has.
    AfterBoundary {
        kind: &'static str,
        id: String,
        recorded_at_seq: i64,
        boundary_event_seq: u64,
    },
    /// The object is stored and no Event places it in the log, so the boundary cannot be applied.
    ///
    /// A third state, not absence and not lateness: the store holds a record the event log does not
    /// account for, and a freeze that cannot be evaluated must not be assumed to pass.
    Unpositioned { kind: &'static str, id: String },
    /// A citation's reference identity and the record the store returned disagree (R15).
    CitedIdentityMismatch { kind: &'static str, id: String },
    /// The completion says something about another record that the other record does not say.
    Irreconcilable { detail: String },
    /// A measured spend record carries more than one unit, and `SpendRef` holds exactly one.
    ///
    /// The collector will not pick a line and will not add across units, so this is a refusal rather
    /// than an arithmetic decision made where nobody can see it. See the report's seam note.
    SpendUnitsNotSingular { units: usize },
    /// A cited record verifies, and its own bytes say it is about a DIFFERENT Mission than the
    /// completion it was collected beside. Never a join: the completion's `mission_id` and the
    /// record's `mission_id` are both read from canonical bytes through their owning readers, and
    /// the refusal names both so the reader knows which record disagreed.
    RecordForAnotherMission {
        kind: &'static str,
        id: String,
        declared_mission: Uuid,
        expected_mission: Uuid,
    },
    /// A cited spend record's declared coverage window starts AFTER the Mission it measures was
    /// admitted, so "actual spend" would be a figure for part of the Mission presented as the
    /// whole (sol1's cold read, #11). The spend owner says a window that does not span the
    /// Mission is the citing collector's refusal, and this is it -- keyed on the Mission's own
    /// admission Event, its record of beginning, never on the record's word for where the
    /// Mission began.
    SpendCoverageStartsAfterMission {
        id: String,
        coverage_from_event_seq: u64,
        mission_admitted_at_seq: u64,
    },
    /// A condition cites more support than the collector will read for one condition. Refused
    /// before the first reference is read, so the bound is on the work, not on the result.
    SupportOverBound {
        ordinal: u32,
        observed: usize,
        bound: usize,
    },
    /// A delivered Artifact's own bytes name another Workspace (R19.2: the collector's own arm,
    /// like the other sources', never `Unverified` with a reason).
    ///
    /// Defence in depth, and said so: the owning reader refuses such a row first -- by its digest,
    /// its column comparison AND its Event pairing, all three measured -- so a realistic store
    /// never lets one reach here. The check stays because it is one line and the owner's guards are
    /// somebody else's to keep; it is tested as a pure function over the payload, because a test
    /// that reached it through the owner would have to bypass three of the owner's own rules.
    ArtifactInAnotherWorkspace { id: String },
    /// A delivered Artifact's own bytes place it outside this Outcome: another Mission, or no
    /// Mission scope at all.
    ///
    /// This aborts the collection rather than dropping the Artifact from the list or marking it
    /// `broken`. A document delivered under a different Mission is not weaker evidence for this
    /// one -- it is evidence the completion's own bytes are wrong about what it delivered, and a
    /// manifest that rendered the rest would be a document nobody could tell was incomplete.
    ArtifactOutOfScope {
        id: String,
        /// What the Artifact's own bytes declare. `None` is `context = none`, which is a different
        /// sentence from "a different Mission" and is kept as one.
        declared_mission: Option<Uuid>,
        expected_mission: Uuid,
    },
}

impl fmt::Display for CollectorRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bound(bound) => write!(
                formatter,
                "bound {} exhausted: configured {}, observed at least {}",
                bound.bound.as_str(),
                bound.configured,
                bound.observed_at_least
            ),
            Self::Absent { kind, id } => {
                write!(formatter, "{kind} {id} is not in this Workspace")
            }
            Self::Unverified { kind, reason } => {
                write!(formatter, "{kind} refused by its owning reader: {reason}")
            }
            Self::AfterBoundary {
                kind,
                id,
                recorded_at_seq,
                boundary_event_seq,
            } => write!(
                formatter,
                "{kind} {id} was recorded at event {recorded_at_seq}, after the boundary \
                 {boundary_event_seq} this Outcome is frozen at"
            ),
            Self::Unpositioned { kind, id } => write!(
                formatter,
                "{kind} {id} is stored and no event places it relative to the boundary"
            ),
            Self::CitedIdentityMismatch { kind, id } => write!(
                formatter,
                "{kind} {id}: the citation's identity and the stored record's disagree"
            ),
            Self::Irreconcilable { detail } => {
                write!(formatter, "sources do not reconcile: {detail}")
            }
            Self::SpendUnitsNotSingular { units } => write!(
                formatter,
                "the measured spend record carries {units} units and a manifest spend reference \
                 holds one"
            ),
            Self::RecordForAnotherMission {
                kind,
                id,
                declared_mission,
                expected_mission,
            } => write!(
                formatter,
                "the {kind} {id} is about Mission {declared_mission}; this Outcome is about \
                 Mission {expected_mission}"
            ),
            Self::SpendCoverageStartsAfterMission {
                id,
                coverage_from_event_seq,
                mission_admitted_at_seq,
            } => write!(
                formatter,
                "the spend {id}'s coverage starts at event {coverage_from_event_seq}, after the \
                 Mission was admitted at event {mission_admitted_at_seq}"
            ),
            Self::SupportOverBound {
                ordinal,
                observed,
                bound,
            } => write!(
                formatter,
                "condition {ordinal} cites {observed} support references; the collector reads at \
                 most {bound}"
            ),
            Self::ArtifactInAnotherWorkspace { id } => write!(
                formatter,
                "the delivered Artifact {id}'s own bytes name another Workspace"
            ),
            Self::ArtifactOutOfScope {
                id,
                declared_mission,
                expected_mission,
            } => match declared_mission {
                Some(declared) => write!(
                    formatter,
                    "the delivered Artifact {id} declares Mission {declared}, and this Outcome is \
                     about Mission {expected_mission}"
                ),
                None => write!(
                    formatter,
                    "the delivered Artifact {id} declares no Mission scope, so nothing places it \
                     in Mission {expected_mission}"
                ),
            },
        }
    }
}

impl std::error::Error for CollectorRefusal {}

// ---------------------------------------------------------------------------------------------
// Positioning a source in the event log
// ---------------------------------------------------------------------------------------------

/// Where each source's identity lives in the Event that recorded it.
///
/// **One table, not six call sites.** Every row is `(label, event kind, JSON path to the identity)`,
/// and the boundary check reads only from here — so adding a citable source is one row rather than
/// a literal buried in a query, and `every_positionable_source_has_a_locator` proves the table
/// covers what the collector can cite.
/// Where each positioning Event carries the record's EXACT canonical bytes.
///
/// Positioning keys on the bytes the record's own reader verified, not on any Event that happens to
/// name its digest: a forged early Event naming a digest -- and nothing else -- positioned a
/// classification recorded after the boundary INSIDE the frozen inputs (sol1's cold read, #8). An
/// Event that carries the bytes is the Event that recorded them; one that only names them is a
/// claim about a record it does not hold. One row per `EVENT_LOCATORS` label, and the unit test
/// below refuses the two tables drifting apart.
const EVENT_BYTES_PATHS: &[(&str, &str)] = &[
    ("proposal", "$.proposal.canonicalBytesBase64"),
    ("ruling", "$.ruling.canonicalBytesBase64"),
    ("evaluation", "$.evaluation.canonical_bytes_base64"),
    ("spend", "$.canonical_bytes_base64"),
    ("classification", "$.canonical_bytes_base64"),
    // The completion is positioned by its locator row for the table's own test; it is never
    // filtered by the cut it defines, but its Event carries its bytes like every other.
    ("completion", "$.canonicalBytesBase64"),
];

const EVENT_LOCATORS: &[(&str, &str, &str)] = &[
    ("proposal", "proposal.submitted", "$.proposal.digest.value"),
    ("ruling", "ruling.recorded", "$.ruling.digest.value"),
    (
        "evaluation",
        "evaluation.recorded",
        "$.evaluation.digest.value",
    ),
    ("spend", "spend.measured", "$.digest.value"),
    (
        "classification",
        "mission.impact_classified",
        "$.digest.value",
    ),
    ("completion", "mission.completed", "$.digestValue"),
];

/// The run-event kinds a Receipt can be positioned by.
///
/// A Receipt has no Event of its own: it is a row a run writes and later run events replace in
/// place (`version` moves 1 → 2 → 3). So it is positioned at the **latest** event naming its run,
/// which is the conservative direction — a Receipt whose newest state arrived after the boundary is
/// refused rather than quietly cited at its older one.
const RECEIPT_RUN_EVENT_KINDS: &[&str] = &["run.preflighted", "run.executed"];

/// The most support references the collector will read for one condition -- the grammar's own
/// bound, applied HERE before anything is allocated or read (the #6 lesson). The completion's
/// decoder refuses a longer list, so today no verified record can exceed it; the check stays
/// because "the owner would have refused it" is a claim about somebody else's code, and a bound
/// that is applied after the clone is a bound on nothing.
const MAX_SUPPORT_REFS_PER_CONDITION: usize = crate::mission_completion::MAX_SUPPORT_PER_CONDITION;

// ---------------------------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------------------------

/// What a caller asks the collector to resolve.
///
/// Identities only — digests and run UUIDs. Nothing renderable crosses this boundary, because
/// everything renderable is what the collector goes and reads.
///
/// There is deliberately **no spend digest**: the spend record a document cites is the one the
/// completion names, so a caller cannot cite a measurement the completion did not claim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectionRequest {
    /// The `MissionCompletionV1` this document is frozen on. Without it there is no boundary, and
    /// the collector reads without one — which is what the pre-completion skeleton did.
    pub completion_digest: Option<String>,
    /// The Mission impact classification whose high-impact bit decides the appendix (R13.7).
    pub classification_digest: Option<String>,
    pub proposal_digests: Vec<String>,
    pub ruling_digests: Vec<String>,
    pub evaluation_digests: Vec<String>,
    pub receipt_run_uuids: Vec<Uuid>,
}

impl CollectionRequest {
    /// How many distinct objects this request would cite.
    ///
    /// Counted from the request, before a single read, because the bound exists to prevent the work
    /// and a bound checked afterwards has already paid for it.
    fn citation_count(&self) -> u64 {
        let mut seen: BTreeSet<(u8, String)> = BTreeSet::new();
        if let Some(digest) = &self.completion_digest {
            seen.insert((SourceKind::Completion.tag(), digest.clone()));
        }
        if let Some(digest) = &self.classification_digest {
            seen.insert((SourceKind::Policy.tag(), digest.clone()));
        }
        for digest in &self.proposal_digests {
            seen.insert((SourceKind::Proposal.tag(), digest.clone()));
        }
        for digest in &self.ruling_digests {
            seen.insert((SourceKind::Ruling.tag(), digest.clone()));
        }
        for digest in &self.evaluation_digests {
            seen.insert((SourceKind::Evaluation.tag(), digest.clone()));
        }
        for run in &self.receipt_run_uuids {
            seen.insert((0xff, run.hyphenated().to_string()));
        }
        seen.len() as u64
    }
}

// ---------------------------------------------------------------------------------------------
// What was collected
// ---------------------------------------------------------------------------------------------

/// What the collector resolved, all of it verified and all of it at or before the boundary.
///
/// Sealed: only the collector builds one. The assembler that turns a collected set into a manifest
/// is not a second verifier -- it copies what the collector proved and refuses to guess -- so the
/// only way to keep a forged set out of it is to make a forged set unwritable (sol1's cold read of
/// the assembler, P0-1). Outside this crate a hand-built set does not compile, and neither does a
/// call to the assembler: the seam from collected values to a manifest is
/// `OutcomeService::prepare_for_mission`, which collects and assembles inside one transaction. The
/// fields stay readable, because reading what the collector resolved is not a way to forge it.
///
/// A hand-built set is a type error:
///
/// ```compile_fail
/// let forged = beacond::CollectedSources {
///     proposals: Vec::new(),
///     rulings: Vec::new(),
///     evaluations: Vec::new(),
///     receipts: Vec::new(),
///     completion: None,
///     classification: None,
///     spend: None,
///     boundary_event_seq: None,
/// };
/// ```
///
/// and so is reaching the assembler from outside the crate:
///
/// ```compile_fail
/// use beacond::assemble;
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedSources {
    pub proposals: Vec<CollectedDigest>,
    pub rulings: Vec<CollectedDigest>,
    pub evaluations: Vec<CollectedDigest>,
    pub receipts: Vec<ReceiptRef>,
    pub completion: Option<CollectedCompletion>,
    pub classification: Option<CollectedClassification>,
    /// The spend the completion cited, read through its owning reader. `None` means the completion
    /// itself was not requested; a completion that measured nothing produces
    /// `Some(CollectedSpend::not_measured(..))`, which is not the same statement.
    pub spend: Option<CollectedSpend>,
    /// The cut everything above was frozen at, when a completion supplied one.
    pub boundary_event_seq: Option<u64>,
    /// The collector's mark. Crate-private on purpose: it is the one field an outside caller cannot
    /// write, which makes this a struct an outside caller cannot build.
    pub(crate) proof: CollectorProof,
}

/// The collector's mark on a [`CollectedSources`]; see the seal note there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollectorProof;

/// One source, named by the identity its owning module actually has.
///
/// Proposal, Ruling and Evaluation are identified by digest and carry no UUID and no version, so
/// this carries what they have. The manifest's `SourceRef` wants a UUID and a schema version, which
/// none of them has -- that gap is `SourceRef`'s, and it is named in the report rather than papered
/// over here with a minted identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedDigest {
    pub kind: SourceKind,
    pub workspace_id: Uuid,
    /// The digest ENVELOPE as the owning reader returned it, all three parts. The assembler cites a
    /// source as `algorithm:canonicalizer:value`, and a value alone would leave it to guess the
    /// function that produced it -- which for a Ruling (two grammars on `main`) is not a guess it
    /// can make. Carried from the owner, never assembled here.
    pub digest_algorithm: String,
    pub digest_canonicalizer: String,
    pub digest_value: String,
}

/// The verified completion, reduced to what a manifest cites.
///
/// The per-condition vocabulary is `ConditionOutcome` — `MissionCompletionV1`'s own type, imported
/// rather than restated (R13.3). `outcome::ConditionResult` still carries two arms and cannot hold
/// `not_assessed` or `assessment_inconclusive`; mapping into it here would collapse them onto
/// `not_met`, which is the exact conflation both designs exist to prevent, so this type holds the
/// owner's vocabulary and the manifest's widening stays the manifest's business.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedCompletion {
    pub digest_value: String,
    pub completion_id: Uuid,
    pub mission_id: Uuid,
    pub project_id: Uuid,
    pub accountable_resident_id: Uuid,
    pub boundary_event_seq: u64,
    pub completed_at_ms: u64,
    /// One entry per declared success condition, ascending by ordinal. The reader proved this is
    /// exactly the Mission's declared set, exactly once each.
    pub condition_results: Vec<CollectedCondition>,
    /// The Artifact **record** digests (R16) this Mission delivered, as a state: `Absent` when the
    /// completion delivered none, `Verified` with the exact cited digests otherwise -- each read
    /// through `artifact::verified_artifact_within` and required to be this Workspace's and scoped
    /// to this Mission. A digest nothing stored, or one scoped elsewhere, aborts the collection
    /// rather than shortening this list: a manifest rendering some of a delivery is a document
    /// nobody could tell was incomplete.
    pub delivered_artifacts: OutcomeState<Vec<String>>,
    pub unfinished_work: Vec<CollectedNarrative>,
    pub uncertainty: Vec<CollectedNarrative>,
}

/// One declared success condition and the completion's result for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedCondition {
    pub ordinal: u32,
    pub outcome: ConditionOutcome,
    /// What the completion cites for this outcome, each entry read through its owning verified
    /// reader at the completion's stored boundary and scoped to this Workspace and Mission from
    /// its own bytes (R23.2). In the record's order. A support reference the owner cannot
    /// verify, one recorded after the boundary, or one about another Mission aborts the
    /// collection rather than shortening this list.
    pub support: Vec<CollectedSupport>,
}

impl CollectedCondition {
    /// The count is the list's length and nothing else: there is no second field to disagree
    /// with it, so "the count says three and the list holds two" is not a state this type can be in.
    pub fn support_count(&self) -> usize {
        self.support.len()
    }
}

/// One piece of support a condition cites, typed by the owner's own vocabulary (`SupportRef`).
///
/// Per kind rather than one list of digests, because a Receipt has no digest: it is identified by
/// `(run, version)` plus an Outcome checksum over its exact stored bytes, and a single-digest list
/// could hold it only by minting an identity its owner does not have (the R13.8 mistake, again).
/// The three digest-identified kinds carry the same `CollectedDigest` the top-level citations do,
/// so the assembler cites a condition's support with the vocabulary it already has. Closed: the
/// owner's decoder admits exactly these four tags, and there is no `Other` for a fifth to hide in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectedSupport {
    Artifact(CollectedDigest),
    Evaluation(CollectedDigest),
    Ruling(CollectedDigest),
    Receipt(ReceiptRef),
}

/// One narrative item, with the condition it relates to when it relates to one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedNarrative {
    pub ordinal: u32,
    pub kind: String,
    pub condition_ordinal: Option<u32>,
}

/// The verified Mission impact classification, reduced to the bit that decides the appendix.
///
/// `policy_high_impact` is not read off the record and copied: it is produced by
/// `recheck_copied_bit`, so the copy and the check are one call and a caller cannot obtain the bit
/// without the check having run (R13.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedClassification {
    pub digest_value: String,
    pub classification_id: Uuid,
    /// The record's own version, as its verified reader proved it (R15: `classification_id` +
    /// `version` are the reference identity). Carried so the assembler copies a verified value into
    /// the Policy citation instead of a constant (R23.5) -- a constant where a verified value
    /// belongs is a decoy waiting to happen.
    pub version: u32,
    /// The Mission the record's own bytes classify. Carried, not dropped: without it the
    /// completion/classification mismatch this collector refuses would be erased before the
    /// `prepare` seam could ever check it (sol1's cold read, #3).
    pub mission_id: Uuid,
    pub project_id: Uuid,
    pub policy_revision: u32,
    pub charter_revision: u32,
    pub policy_high_impact: bool,
}

/// The measured spend the completion cited.
///
/// **Coverage is a fact of the record and survives every measurement state.** The amount is a state
/// and may be absent; the window it was measured over is not a state and is always here. A reader
/// that needs "how much" and a reader that needs "over what" ask different questions, and only the
/// first one can be unanswered (R17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedSpend {
    /// `None` when the completion recorded `not_measured`: there is no spend record to cite.
    pub digest_value: Option<String>,
    pub spend_id: Option<Uuid>,
    pub version: Option<u32>,
    /// The record's own word for its measurement, or `not_measured` when the completion recorded no
    /// spend record at all. Never synthesised, never a synonym.
    pub measurement: String,
    /// The declared coverage window, preserved exactly as recorded. `None` only when there is no
    /// record, because coverage is a property of a measurement and not of its absence.
    pub coverage: Option<(u64, u64)>,
    /// The amount, as a state. `Verified` only when the record actually counted something in
    /// exactly one unit; never zero standing in for absence.
    pub amount: OutcomeState<SpendAmount>,
    /// Whether the count is whole, and if not, where it is not -- as a state, so that "complete",
    /// "incomplete and it says where" and "there is no count to be complete" never collapse into
    /// one another (sol1's cold read, #14). A `partially_measured` record's named gaps travel
    /// with its amount; a reader that drops them has turned "usable and incomplete" into a
    /// figure that looks whole.
    pub completeness: SpendCompleteness,
}

/// How much of the window a counted amount covers, in the record's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendCompleteness {
    /// `measured`: every priced unit inside the window was counted. Nothing is missing, and the
    /// record says so -- a positive claim, not the absence of gaps.
    Complete,
    /// `partially_measured`: counted, with at least one named gap, carried whole. The grammar
    /// refuses a partial record with no gap, so this list is never empty.
    Partial(Vec<CollectedUncounted>),
    /// There is no count to be complete or incomplete: `unmeasured`, `broken`, or no record at
    /// all. Distinct from `Complete`, which is a count with nothing missing.
    NoCount,
}

/// One thing a partial measurement could not count, exactly as the record says it: the kind as
/// the record's own literal (never re-spelled), and the human detail that says where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedUncounted {
    pub kind: String,
    pub detail: String,
}

/// A counted amount in one unit, in stored integer minor units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendAmount {
    pub unit: String,
    pub amount_minor: i64,
    pub minor_units_exponent: u8,
    pub counted_units: u64,
}

// ---------------------------------------------------------------------------------------------
// The collector
// ---------------------------------------------------------------------------------------------

/// Reads a Workspace's stored sources through their owning verified readers.
pub struct Collector<'a> {
    connection: &'a Connection,
    workspace_id: Uuid,
}

impl<'a> Collector<'a> {
    pub fn new(connection: &'a Connection, workspace_id: Uuid) -> Self {
        Self {
            connection,
            workspace_id,
        }
    }

    /// Resolve a request, or refuse.
    ///
    /// **The bound runs first, before any read.** "Refuses before a partial manifest" is a property
    /// of this ordering and nothing else: once a read has happened, a refusal has already produced
    /// the state the bound existed to prevent.
    ///
    /// **The completion runs second**, because it is what supplies the boundary every later read is
    /// filtered by. A collection with no completion has no boundary and applies none — that is the
    /// pre-completion behaviour, kept rather than faked with "now".
    pub fn collect(
        &self,
        request: &CollectionRequest,
    ) -> Result<CollectedSources, CollectorRefusal> {
        let observed = request.citation_count();
        if observed > MAX_CITATIONS {
            return Err(CollectorRefusal::Bound(BoundRefusal {
                bound: OutcomeBound::CitationCount,
                configured: MAX_CITATIONS,
                observed_at_least: observed,
            }));
        }

        let mut completion = None;
        let mut spend = None;
        let mut boundary = None;
        if let Some(digest) = &request.completion_digest {
            let record = self.completion_record(digest)?;
            // The completion DEFINES the cut and is not filtered by it. Its own Event is strictly
            // after its boundary -- that is an invariant its store enforces -- so applying the
            // boundary to it would refuse every valid completion there is.
            let cut = record.boundary_event_seq;
            spend = Some(self.spend_for(&record, cut)?);
            completion = Some(self.collected_completion(digest, record)?);
            boundary = Some(cut);
        }

        let mut classification = None;
        if let Some(digest) = &request.classification_digest {
            let expected_mission = completion
                .as_ref()
                .map(|collected: &CollectedCompletion| collected.mission_id);
            classification = Some(self.classification(digest, boundary, expected_mission)?);
        }

        let mut proposals = Vec::new();
        for digest in &request.proposal_digests {
            proposals.push(self.proposal(digest, boundary)?);
        }
        let mut rulings = Vec::new();
        for digest in &request.ruling_digests {
            rulings.push(self.ruling(digest, boundary)?);
        }
        let mut evaluations = Vec::new();
        for digest in &request.evaluation_digests {
            evaluations.push(self.evaluation(digest, boundary)?);
        }
        let mut receipts = Vec::new();
        for run in &request.receipt_run_uuids {
            receipts.push(self.receipt(*run, boundary)?);
        }

        Ok(CollectedSources {
            proposals,
            rulings,
            evaluations,
            receipts,
            completion,
            classification,
            spend,
            boundary_event_seq: boundary,
            proof: CollectorProof,
        })
    }

    // -----------------------------------------------------------------------------------------
    // The boundary
    // -----------------------------------------------------------------------------------------

    /// Refuse anything the log records after the cut.
    ///
    /// `None` for the boundary means no completion was cited, so there is nothing to freeze
    /// against. It never means "use the newest event": a missing boundary that silently becomes
    /// `now` is precisely the mutation this design's gate exists to kill.
    fn at_or_before(
        &self,
        kind: &'static str,
        id: &str,
        canonical_bytes: &[u8],
        boundary_event_seq: u64,
    ) -> Result<(), CollectorRefusal> {
        let Some(sequence) = self.event_sequence_of(kind, id, canonical_bytes)? else {
            return Err(CollectorRefusal::Unpositioned {
                kind,
                id: id.to_owned(),
            });
        };
        let cut = i64::try_from(boundary_event_seq).map_err(|_| CollectorRefusal::Unverified {
            kind,
            reason: "the completion's boundary is not a sequence number".to_owned(),
        })?;
        if sequence > cut {
            return Err(CollectorRefusal::AfterBoundary {
                kind,
                id: id.to_owned(),
                recorded_at_seq: sequence,
                boundary_event_seq,
            });
        }
        Ok(())
    }

    /// The Event at which the Mission was admitted: its own record of beginning in the log, keyed
    /// on the Mission's id inside the Event's payload. `None` is a Mission the log does not
    /// account for, which is `Unpositioned` and never assumed to have begun anywhere.
    fn mission_admitted_at(&self, mission_id: Uuid) -> Result<Option<u64>, CollectorRefusal> {
        self.connection
            .query_row(
                "SELECT min(sequence) FROM events
                 WHERE workspace_id = ?1 AND kind = 'mission.admitted'
                   AND json_extract(payload_json, '$.mission.id') = ?2",
                rusqlite::params![self.workspace_id.to_string(), mission_id.to_string()],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(|error| CollectorRefusal::Unverified {
                kind: "mission",
                reason: error.to_string(),
            })
            .map(|found| found.and_then(|sequence| u64::try_from(sequence).ok()))
    }

    /// The sequence of the Event that recorded this object, from the one locator table.
    fn event_sequence_of(
        &self,
        kind: &'static str,
        id: &str,
        canonical_bytes: &[u8],
    ) -> Result<Option<i64>, CollectorRefusal> {
        let Some((_, event_kind, path)) = EVENT_LOCATORS
            .iter()
            .find(|(label, _, _)| *label == kind)
            .copied()
        else {
            return Err(CollectorRefusal::Unpositioned {
                kind,
                id: id.to_owned(),
            });
        };
        let Some((_, bytes_path)) = EVENT_BYTES_PATHS
            .iter()
            .find(|(label, _)| *label == kind)
            .copied()
        else {
            return Err(CollectorRefusal::Unpositioned {
                kind,
                id: id.to_owned(),
            });
        };
        // Identity AND bytes. The digest says which record; the bytes say this Event is the one
        // that recorded it. A forged Event can name a digest it never held -- it cannot carry
        // bytes the verified reader did not just prove.
        self.connection
            .query_row(
                "SELECT MIN(sequence) FROM events
                 WHERE workspace_id = ?1 AND kind = ?2
                   AND json_extract(payload_json, ?3) = ?4
                   AND json_extract(payload_json, ?5) = ?6",
                params![
                    self.workspace_id.to_string(),
                    event_kind,
                    path,
                    id,
                    bytes_path,
                    data_encoding::BASE64.encode(canonical_bytes)
                ],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(|error| CollectorRefusal::Unverified {
                kind,
                reason: error.to_string(),
            })
    }

    // -----------------------------------------------------------------------------------------
    // The completion, and everything it cites
    // -----------------------------------------------------------------------------------------

    /// The completion, through `mission_completion_store::verified_completion_within`.
    ///
    /// Its refusals are repeated, never reinterpreted: `Absent` is the collector's `Absent`, and
    /// every integrity refusal — a moved condition set, a boundary out of range, a column that
    /// disagrees with the bytes — arrives as `Unverified` carrying the reader's own word for it.
    /// The collector does not decide which of the reader's refusals are serious.
    fn completion_record(
        &self,
        digest_value: &str,
    ) -> Result<MissionCompletionV1, CollectorRefusal> {
        use crate::mission_completion_store::CompletionReadRefusal;
        match crate::mission_completion_store::verified_completion_within(
            self.connection,
            self.workspace_id,
            digest_value,
        ) {
            Ok(record) => Ok(record),
            Err(CompletionReadRefusal::Absent) => Err(CollectorRefusal::Absent {
                kind: "completion",
                id: digest_value.to_owned(),
            }),
            Err(other) => Err(CollectorRefusal::Unverified {
                kind: "completion",
                reason: format!("{other:?}"),
            }),
        }
    }

    /// Reduce a verified completion, reconciling what only a second record can check.
    fn collected_completion(
        &self,
        digest_value: &str,
        record: MissionCompletionV1,
    ) -> Result<CollectedCompletion, CollectorRefusal> {
        let mut condition_results: Vec<CollectedCondition> =
            Vec::with_capacity(record.condition_results.len());
        for result in &record.condition_results {
            let support = collected_support(result.ordinal, &result.support, |reference| {
                self.support(reference, &record)
            })?;
            condition_results.push(CollectedCondition {
                ordinal: result.ordinal,
                outcome: result.outcome,
                support,
            });
        }

        // Reconciliation nobody else can make: a narrative item may name a condition, and the
        // condition it names has to be one this completion actually has. The record's own decoder
        // proves the ordinal is a `u32`; that it is *this* completion's ordinal spans two lists.
        let declared: BTreeSet<u32> = condition_results.iter().map(|c| c.ordinal).collect();
        let narrative = |items: Vec<CollectedNarrative>,
                         label: &str|
         -> Result<Vec<CollectedNarrative>, CollectorRefusal> {
            for item in &items {
                if let Some(ordinal) = item.condition_ordinal
                    && !declared.contains(&ordinal)
                {
                    return Err(CollectorRefusal::Irreconcilable {
                        detail: format!(
                            "{label} item {} names condition {ordinal}, which this completion does \
                             not report",
                            item.ordinal
                        ),
                    });
                }
            }
            Ok(items)
        };

        let unfinished_work = narrative(
            record
                .unfinished_work
                .iter()
                .map(|item| CollectedNarrative {
                    ordinal: item.ordinal,
                    kind: item.kind.as_str().to_owned(),
                    condition_ordinal: item.condition_ordinal,
                })
                .collect(),
            "unfinished work",
        )?;
        let uncertainty = narrative(
            record
                .uncertainty
                .iter()
                .map(|item| CollectedNarrative {
                    ordinal: item.ordinal,
                    kind: item.kind.as_str().to_owned(),
                    condition_ordinal: item.condition_ordinal,
                })
                .collect(),
            "uncertainty",
        )?;

        // Two different sentences, and the difference is the whole point of this arm.
        //
        // Empty is `Absent`: the domain says the Mission delivered nothing. Never an empty
        // `Verified` list, which says the same thing in a word that means something else.
        //
        // Non-empty is now READ, one digest at a time, through the Artifact module's own verified
        // reader -- and a digest that does not resolve, or resolves to a record scoped elsewhere,
        // ABORTS the collection. It never becomes a shorter list or a `broken` entry: the
        // completion's bytes said the Mission delivered these documents, and a manifest rendering
        // some of them is a document nobody could tell was incomplete.
        //
        // Worth keeping visible: `NoArtifactEntity` is enforced on the WRITE path only
        // (`record_completion`), never in `verified_completion_within`. A stored delivery reads
        // back as a valid completion, so this arm is the only thing between one and a rendered
        // document.
        let delivered_artifacts = if record.delivered_artifacts.is_empty() {
            OutcomeState::Absent("the completion delivered no Artifacts".to_owned())
        } else {
            let mut delivered = Vec::with_capacity(record.delivered_artifacts.len());
            for cited in &record.delivered_artifacts {
                let collected = self.delivered_artifact(
                    &HEXLOWER.encode(cited),
                    record.mission_id,
                    record.boundary_event_seq,
                )?;
                delivered.push(collected.digest_value);
            }
            OutcomeState::Verified(delivered)
        };

        Ok(CollectedCompletion {
            digest_value: digest_value.to_owned(),
            completion_id: record.completion_id,
            mission_id: record.mission_id,
            project_id: record.project_id,
            accountable_resident_id: record.accountable_resident_id,
            boundary_event_seq: record.boundary_event_seq,
            completed_at_ms: record.completed_at_ms,
            condition_results,
            delivered_artifacts,
            unfinished_work,
            uncertainty,
        })
    }

    /// The spend record the completion cited, read through `spend::verified_spend_for_citation`.
    ///
    /// The citation carries `(spend_id, version, sha256)` and the reader refuses a pair that
    /// disagrees (R15), so the collector never has to compare identities itself — except for the
    /// one thing the reader cannot see: that the digest the completion recorded is the digest the
    /// store answered with.
    fn spend_for(
        &self,
        completion: &MissionCompletionV1,
        boundary_event_seq: u64,
    ) -> Result<CollectedSpend, CollectorRefusal> {
        let (spend_id, spend_version, spend_sha256) = match &completion.spend {
            SpendReference::NotMeasured(reason) => {
                return Ok(CollectedSpend {
                    digest_value: None,
                    spend_id: None,
                    version: None,
                    measurement: "not_measured".to_owned(),
                    coverage: None,
                    amount: OutcomeState::NotEvaluated(reason.clone()),
                    completeness: SpendCompleteness::NoCount,
                });
            }
            SpendReference::Recorded {
                spend_id,
                spend_version,
                spend_sha256,
            } => (*spend_id, *spend_version, *spend_sha256),
        };
        let digest_value = HEXLOWER.encode(&spend_sha256);

        let record: MeasuredSpend = match crate::spend::verified_spend_for_citation(
            self.connection,
            self.workspace_id,
            spend_id,
            spend_version,
            &digest_value,
        ) {
            Ok(Some(record)) => record,
            Ok(None) => {
                return Err(CollectorRefusal::Absent {
                    kind: "spend",
                    id: digest_value,
                });
            }
            Err(error) => {
                return Err(CollectorRefusal::Unverified {
                    kind: "spend",
                    reason: error.to_string(),
                });
            }
        };

        // The reader proved the record's own identity against the citation. This proves the store
        // answered with the bytes the completion named, which is the half a reader keyed on a
        // digest cannot check for itself.
        if record.digest().value != digest_value {
            return Err(CollectorRefusal::CitedIdentityMismatch {
                kind: "spend",
                id: digest_value,
            });
        }
        self.at_or_before(
            "spend",
            &digest_value,
            record.canonical_bytes(),
            boundary_event_seq,
        )?;

        let payload = record.payload();
        // The cited record's own bytes must measure THIS completion's Mission. R15 proved the
        // citation names these bytes; this proves the bytes are about the Mission the completion
        // says they are -- a completion for Mission A citing Mission B's ledger as its own is a
        // verified record in the wrong document (sol1's cold read, #4).
        if payload.mission_id != completion.mission_id {
            return Err(CollectorRefusal::RecordForAnotherMission {
                kind: "spend",
                id: digest_value,
                declared_mission: payload.mission_id,
                expected_mission: completion.mission_id,
            });
        }
        // The declared window must span the Mission: the spend owner says a window that does not
        // is the citing collector's refusal (sol1's cold read, #11), and it is keyed on the
        // Mission's own admission Event -- its record of beginning -- never on the record's word
        // for it. Every measurement state declares a window, so every state is checked. The
        // window's END is not checked here: the record's Event precedes the completion's, so
        // `coverage_to` can never reach the boundary, and what the window failed to reach is the
        // spend owner's word (`partially_measured`), not this collector's.
        // A Mission with no admission Event is `Unpositioned` here for the record; through
        // `collect()` the completion's owning reader fails first on the same missing Event, with a
        // store error rather than a name, so no test reaches this arm today (declared).
        let Some(admitted) = self.mission_admitted_at(completion.mission_id)? else {
            return Err(CollectorRefusal::Unpositioned {
                kind: "mission",
                id: completion.mission_id.to_string(),
            });
        };
        if payload.coverage_from_event_seq > admitted {
            return Err(CollectorRefusal::SpendCoverageStartsAfterMission {
                id: digest_value,
                coverage_from_event_seq: payload.coverage_from_event_seq,
                mission_admitted_at_seq: admitted,
            });
        }
        let coverage = Some((
            payload.coverage_from_event_seq,
            payload.coverage_to_event_seq,
        ));
        // The gaps come first and whole: the amount below folds the two counted arms together for
        // its number, and this is what keeps them apart (sol1's cold read, #14).
        let completeness = match &payload.measurement {
            SpendMeasurement::Measured(_) => SpendCompleteness::Complete,
            SpendMeasurement::PartiallyMeasured(_, uncounted) => SpendCompleteness::Partial(
                uncounted
                    .iter()
                    .map(|entry| CollectedUncounted {
                        kind: entry.kind.as_str().to_owned(),
                        detail: entry.detail.clone(),
                    })
                    .collect(),
            ),
            SpendMeasurement::Unmeasured(_) | SpendMeasurement::Broken(_) => {
                SpendCompleteness::NoCount
            }
        };
        let amount = match &payload.measurement {
            SpendMeasurement::Unmeasured(reason) => OutcomeState::NotEvaluated(reason.clone()),
            SpendMeasurement::Broken(reason) => OutcomeState::Broken(reason.clone()),
            SpendMeasurement::Measured(totals) | SpendMeasurement::PartiallyMeasured(totals, _) => {
                // Exactly one line is an amount. Everything else refuses, and the empty case is
                // deliberately NOT its own arm: it used to be `Absent("counted no priced lines")`,
                // which answered "there was none" for a state the grammar forbids --
                // `MeasuredSpendV1`'s encoder and decoder both refuse a counted measurement with no
                // lines (`MalformedSpend::NoLines`), so no verified record can carry one. An
                // unreachable arm that reports absence is worse than no arm: it reads as coverage
                // of a domain state, and reaching it would mean the store holds something its own
                // codec says cannot exist. The catch-all refuses it, because corruption aborts
                // rather than becoming an honest domain row (design §5 rule 6).
                match totals.lines.as_slice() {
                    [line] => OutcomeState::Verified(SpendAmount {
                        unit: line.unit.clone(),
                        amount_minor: line.amount_minor,
                        minor_units_exponent: line.minor_units_exponent,
                        counted_units: totals.counted_units,
                    }),
                    lines => {
                        return Err(CollectorRefusal::SpendUnitsNotSingular { units: lines.len() });
                    }
                }
            }
        };

        Ok(CollectedSpend {
            digest_value: Some(digest_value),
            spend_id: Some(spend_id),
            version: Some(spend_version),
            measurement: payload.measurement.literal().to_owned(),
            coverage,
            amount,
            completeness,
        })
    }

    // -----------------------------------------------------------------------------------------
    // The other owning readers
    // -----------------------------------------------------------------------------------------

    /// The Mission impact classification, through `mission_impact::verified_classification_within`.
    fn classification(
        &self,
        digest_value: &str,
        boundary: Option<u64>,
        expected_mission: Option<Uuid>,
    ) -> Result<CollectedClassification, CollectorRefusal> {
        let record = match crate::mission_impact::verified_classification_within(
            self.connection,
            self.workspace_id,
            digest_value,
        ) {
            Ok(Some(record)) => record,
            Ok(None) => {
                return Err(CollectorRefusal::Absent {
                    kind: "classification",
                    id: digest_value.to_owned(),
                });
            }
            Err(error) => {
                return Err(CollectorRefusal::Unverified {
                    kind: "classification",
                    reason: error.to_string(),
                });
            }
        };
        if let Some(cut) = boundary {
            self.at_or_before(
                "classification",
                digest_value,
                record.canonical_bytes(),
                cut,
            )?;
        }

        // R13.7, and what this slice can and cannot honour of it.
        //
        // The bit is produced by `appendix_is_mandatory()` ON the verified record, which is already
        // the shape R13.7 asks for: there is no `is_high_impact(mission_id)` and no way to obtain
        // the bit without holding the record whose identity the manifest must cite beside it.
        //
        // What is NOT here is the re-check at `prepare`. `recheck_copied_bit` compares a copied bit
        // against the record, and the only bit this function has is one it just derived from that
        // same record -- so calling it here would be a tautology that no mutation can falsify, and
        // an unfalsifiable check is worse than none because it reads as coverage. The re-check
        // belongs where a bit is actually copied from somewhere else, which is slice 3's `prepare`.
        let policy_high_impact = record.payload().appendix_is_mandatory();

        let payload = record.payload();
        // Its own bytes must classify the Mission this Outcome is about. Both sides of the
        // comparison come from canonical bytes through their owning readers; nothing here joins a
        // column to a column.
        if let Some(expected) = expected_mission
            && payload.mission_id != expected
        {
            return Err(CollectorRefusal::RecordForAnotherMission {
                kind: "classification",
                id: digest_value.to_owned(),
                declared_mission: payload.mission_id,
                expected_mission: expected,
            });
        }
        Ok(CollectedClassification {
            digest_value: record.digest().value.clone(),
            classification_id: payload.classification_id,
            version: payload.version,
            mission_id: payload.mission_id,
            project_id: payload.policy.project_id,
            policy_revision: payload.policy.policy_revision,
            charter_revision: payload.policy.charter_revision,
            policy_high_impact,
        })
    }

    /// One support reference, through its owner, at the completion's boundary, scoped to the
    /// completion's Mission from the record's own bytes (R23.2).
    ///
    /// Nothing here is read off the completion's word for the record: the reference says which
    /// record, and the owner says what it is. An Artifact declares its Mission in its context; an
    /// Evaluation in its subject -- directly for an Artifact subject, through the Proposal's own
    /// verified scope for a Proposal subject; a Ruling through the Proposal it rules on. A Receipt
    /// carries no Mission in its bytes (ADR 0039): it is scoped to the Workspace by its reader and
    /// held to the exact `(run, version, checksum)` the completion cited.
    fn support(
        &self,
        reference: &SupportRef,
        completion: &MissionCompletionV1,
    ) -> Result<CollectedSupport, CollectorRefusal> {
        let cut = completion.boundary_event_seq;
        let expected_mission = completion.mission_id;
        match reference {
            SupportRef::Artifact(digest) => {
                let digest_value = HEXLOWER.encode(digest);
                let (record, collected) =
                    self.verified_artifact(&digest_value, expected_mission)?;
                // Positioned by the record's own creation cursor, which the owner re-derives
                // against the Event that carries its bytes on every read.
                let recorded_at_seq = record.creation_event_seq();
                if recorded_at_seq > cut {
                    return Err(CollectorRefusal::AfterBoundary {
                        kind: "artifact",
                        id: digest_value,
                        recorded_at_seq: i64::try_from(recorded_at_seq).unwrap_or(i64::MAX),
                        boundary_event_seq: cut,
                    });
                }
                Ok(CollectedSupport::Artifact(collected))
            }
            SupportRef::Evaluation(digest) => {
                let digest_value = HEXLOWER.encode(digest);
                let (evaluation, collected) = self.verified_evaluation(&digest_value, Some(cut))?;
                let declared_mission = match &evaluation.payload().subject {
                    crate::evaluation::EvaluationSubject::Artifact(subject) => subject.mission_id,
                    crate::evaluation::EvaluationSubject::Proposal(proposal) => {
                        self.proposal_mission("evaluation", &digest_value, &proposal.value)?
                    }
                };
                if declared_mission != expected_mission {
                    return Err(CollectorRefusal::RecordForAnotherMission {
                        kind: "evaluation",
                        id: digest_value,
                        declared_mission,
                        expected_mission,
                    });
                }
                Ok(CollectedSupport::Evaluation(collected))
            }
            SupportRef::Ruling(digest) => {
                let digest_value = HEXLOWER.encode(digest);
                let (ruling, collected) = self.verified_ruling(&digest_value, Some(cut))?;
                let declared_mission = self.proposal_mission(
                    "ruling",
                    &digest_value,
                    &ruling.payload().proposal.value,
                )?;
                if declared_mission != expected_mission {
                    return Err(CollectorRefusal::RecordForAnotherMission {
                        kind: "ruling",
                        id: digest_value,
                        declared_mission,
                        expected_mission,
                    });
                }
                Ok(CollectedSupport::Ruling(collected))
            }
            SupportRef::Receipt {
                run_id,
                receipt_version,
                receipt_bytes_sha256,
            } => {
                let collected = self.receipt(*run_id, Some(cut))?;
                // The completion's word for the Receipt against the owner's bytes: the version it
                // cited and the checksum it recorded must both be what the store holds (R15).
                if collected.receipt_version != *receipt_version
                    || collected.outcome_checksum != HEXLOWER.encode(receipt_bytes_sha256)
                {
                    return Err(CollectorRefusal::CitedIdentityMismatch {
                        kind: "receipt",
                        id: run_id.hyphenated().to_string(),
                    });
                }
                Ok(CollectedSupport::Receipt(collected))
            }
        }
    }

    /// The Mission a Proposal declares in its own scope, read through the Proposal's verified
    /// reader -- for the records (an Evaluation of a Proposal, a Ruling) whose Mission is the
    /// Proposal's. A subject Proposal the store does not hold is the citing record's fault to
    /// name, not a Proposal absence.
    fn proposal_mission(
        &self,
        kind: &'static str,
        id: &str,
        proposal_digest: &str,
    ) -> Result<Uuid, CollectorRefusal> {
        match crate::proposal::verified_proposal_within(
            self.connection,
            self.workspace_id,
            proposal_digest,
        ) {
            Ok(Some(proposal)) => Ok(proposal.payload().scope.mission_id),
            Ok(None) => Err(CollectorRefusal::Unverified {
                kind,
                reason: format!("{id}: its Proposal {proposal_digest} is not in this Workspace"),
            }),
            Err(error) => Err(CollectorRefusal::Unverified {
                kind,
                reason: format!("{id}: its Proposal refused by its owning reader: {error}"),
            }),
        }
    }

    /// The Proposal, through `proposal::verified_proposal_within`.
    fn proposal(
        &self,
        digest_value: &str,
        boundary: Option<u64>,
    ) -> Result<CollectedDigest, CollectorRefusal> {
        match crate::proposal::verified_proposal_within(
            self.connection,
            self.workspace_id,
            digest_value,
        ) {
            Ok(Some(proposal)) => {
                if let Some(cut) = boundary {
                    self.at_or_before("proposal", digest_value, proposal.canonical_bytes(), cut)?;
                }
                Ok(CollectedDigest {
                    kind: SourceKind::Proposal,
                    workspace_id: self.workspace_id,
                    // From the verified value, never from the caller's string: the reader is what
                    // proved this digest names these bytes.
                    digest_algorithm: proposal.digest().algorithm.clone(),
                    digest_canonicalizer: proposal.digest().canonicalizer.clone(),
                    digest_value: proposal.digest().value.clone(),
                })
            }
            Ok(None) => Err(CollectorRefusal::Absent {
                kind: "proposal",
                id: digest_value.to_owned(),
            }),
            Err(error) => Err(CollectorRefusal::Unverified {
                kind: "proposal",
                reason: error.to_string(),
            }),
        }
    }

    /// The Ruling, through `ruling::verified_ruling_within`.
    fn ruling(
        &self,
        digest_value: &str,
        boundary: Option<u64>,
    ) -> Result<CollectedDigest, CollectorRefusal> {
        Ok(self.verified_ruling(digest_value, boundary)?.1)
    }

    /// The Ruling with its verified record beside the citation, for a caller that has to read the
    /// record's own scope (support under a condition) as well as cite it.
    fn verified_ruling(
        &self,
        digest_value: &str,
        boundary: Option<u64>,
    ) -> Result<(crate::ruling::Ruling, CollectedDigest), CollectorRefusal> {
        match crate::ruling::verified_ruling_within(
            self.connection,
            self.workspace_id,
            digest_value,
        ) {
            Ok(Some(ruling)) => {
                if let Some(cut) = boundary {
                    self.at_or_before("ruling", digest_value, ruling.canonical_bytes(), cut)?;
                }
                let collected = CollectedDigest {
                    kind: SourceKind::Ruling,
                    workspace_id: self.workspace_id,
                    digest_algorithm: ruling.digest().algorithm.clone(),
                    digest_canonicalizer: ruling.digest().canonicalizer.clone(),
                    digest_value: ruling.digest().value.clone(),
                };
                Ok((ruling, collected))
            }
            Ok(None) => Err(CollectorRefusal::Absent {
                kind: "ruling",
                id: digest_value.to_owned(),
            }),
            Err(error) => Err(CollectorRefusal::Unverified {
                kind: "ruling",
                reason: error.to_string(),
            }),
        }
    }

    /// The Evaluation, through `evaluation::verified_evaluation_within`.
    fn evaluation(
        &self,
        digest_value: &str,
        boundary: Option<u64>,
    ) -> Result<CollectedDigest, CollectorRefusal> {
        Ok(self.verified_evaluation(digest_value, boundary)?.1)
    }

    /// The Evaluation with its verified record beside the citation (see `verified_ruling`).
    fn verified_evaluation(
        &self,
        digest_value: &str,
        boundary: Option<u64>,
    ) -> Result<(crate::evaluation::Evaluation, CollectedDigest), CollectorRefusal> {
        match crate::evaluation::verified_evaluation_within(
            self.connection,
            self.workspace_id,
            digest_value,
        ) {
            Ok(Some(evaluation)) => {
                if let Some(cut) = boundary {
                    self.at_or_before(
                        "evaluation",
                        digest_value,
                        evaluation.canonical_bytes(),
                        cut,
                    )?;
                }
                let collected = CollectedDigest {
                    kind: SourceKind::Evaluation,
                    workspace_id: self.workspace_id,
                    digest_algorithm: evaluation.digest().algorithm.clone(),
                    digest_canonicalizer: evaluation.digest().canonicalizer.clone(),
                    digest_value: evaluation.digest().value.clone(),
                };
                Ok((evaluation, collected))
            }
            Ok(None) => Err(CollectorRefusal::Absent {
                kind: "evaluation",
                id: digest_value.to_owned(),
            }),
            Err(error) => Err(CollectorRefusal::Unverified {
                kind: "evaluation",
                reason: error.to_string(),
            }),
        }
    }

    /// One Receipt, cited by the identity a Receipt actually has, with an Outcome checksum over its
    /// exact stored bytes.
    ///
    /// **The `outcome` column is not read.** It is a projection of a value inside `receipt_json`,
    /// and a joined column may identify a candidate but may never supply a renderable claim. The
    /// checksum covers the stored bytes and nothing else, so a store whose column disagrees with its
    /// bytes produces the same citation as one whose column agrees -- the projection cannot reach
    /// the document.
    ///
    /// The bytes are read as bytes -- `CAST(receipt_json AS BLOB)` -- rather than as a `String`,
    /// because "exact stored bytes" is the claim the checksum makes, and a round trip through Rust's
    /// UTF-8 validation is a transformation nobody asked it to prove.
    fn receipt(
        &self,
        run_uuid: Uuid,
        boundary: Option<u64>,
    ) -> Result<ReceiptRef, CollectorRefusal> {
        let row: Option<(i64, Vec<u8>)> = self
            .connection
            .query_row(
                "SELECT version, CAST(receipt_json AS BLOB) FROM receipts
                 WHERE workspace_id = ?1 AND run_id = ?2",
                params![self.workspace_id.to_string(), run_uuid.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| CollectorRefusal::Unverified {
                kind: "receipt",
                reason: error.to_string(),
            })?;

        let Some((version, receipt_json)) = row else {
            return Err(CollectorRefusal::Absent {
                kind: "receipt",
                id: run_uuid.hyphenated().to_string(),
            });
        };
        let version = u32::try_from(version).map_err(|_| CollectorRefusal::Unverified {
            kind: "receipt",
            reason: "receipt version is not a u32".to_owned(),
        })?;
        if let Some(cut) = boundary {
            self.receipt_at_or_before(run_uuid, cut)?;
        }

        Ok(ReceiptRef {
            workspace_id: self.workspace_id,
            run_uuid,
            receipt_version: version,
            outcome_checksum: HEXLOWER.encode(&sha256(&receipt_json)),
        })
    }

    /// A Receipt's position: the latest run Event naming its run. See `RECEIPT_RUN_EVENT_KINDS`.
    fn receipt_at_or_before(
        &self,
        run_uuid: Uuid,
        boundary_event_seq: u64,
    ) -> Result<(), CollectorRefusal> {
        let id = run_uuid.to_string();
        let mut latest: Option<i64> = None;
        for kind in RECEIPT_RUN_EVENT_KINDS {
            let found: Option<i64> = self
                .connection
                .query_row(
                    "SELECT MAX(sequence) FROM events
                     WHERE workspace_id = ?1 AND kind = ?2
                       AND json_extract(payload_json, '$.runId') = ?3",
                    params![self.workspace_id.to_string(), kind, id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .map_err(|error| CollectorRefusal::Unverified {
                    kind: "receipt",
                    reason: error.to_string(),
                })?;
            latest = match (latest, found) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (only, None) | (None, only) => only,
            };
        }
        let Some(sequence) = latest else {
            return Err(CollectorRefusal::Unpositioned {
                kind: "receipt",
                id: run_uuid.hyphenated().to_string(),
            });
        };
        let cut = i64::try_from(boundary_event_seq).map_err(|_| CollectorRefusal::Unverified {
            kind: "receipt",
            reason: "the completion's boundary is not a sequence number".to_owned(),
        })?;
        if sequence > cut {
            return Err(CollectorRefusal::AfterBoundary {
                kind: "receipt",
                id: run_uuid.hyphenated().to_string(),
                recorded_at_seq: sequence,
                boundary_event_seq,
            });
        }
        Ok(())
    }

    /// One delivered Artifact, through `artifact::verified_artifact_within` -- the reader
    /// `ArtifactService::get` wraps.
    ///
    /// The last of the skeleton's four seams, and the store it waited for is on `main` (#63). The
    /// owning module re-parses the canonical bytes, recomputes the digest and compares every
    /// projection column, so this does **not** verify a second way: it reads through the owner and
    /// then checks the two things the owner cannot know -- that the record is in THIS Workspace and
    /// scoped to THIS Mission.
    ///
    /// The value collected is the **exact cited digest**. R12.10's tail belongs to a read-time
    /// surface (`ArtifactService::list_supersession_tail`, and the Evaluation arm's
    /// `subject_superseded_by`); an Outcome cites the bytes the Mission delivered, and resolving
    /// that to a later version would make the document say something the completion never did.
    pub fn artifact(
        &self,
        digest_value: &str,
        mission_id: Uuid,
    ) -> Result<CollectedDigest, CollectorRefusal> {
        Ok(self.verified_artifact(digest_value, mission_id)?.1)
    }

    /// One delivered Artifact at the completion's cut: `artifact`, plus the boundary freeze.
    ///
    /// The module doc has claimed the freeze for every source since slice 2-B, and the delivered
    /// Artifact was the one source it was never applied to: an Artifact recorded AFTER the
    /// completion's boundary came back `Verified` in a completion frozen before it existed (astra's
    /// S5). Its position is the creation cursor its owning reader re-derives against the Event
    /// that carries its exact bytes on every read -- the #8 rule already paid for, so no second
    /// query is made here and no second spelling of the rule exists to drift.
    fn delivered_artifact(
        &self,
        digest_value: &str,
        mission_id: Uuid,
        boundary_event_seq: u64,
    ) -> Result<CollectedDigest, CollectorRefusal> {
        let (record, collected) = self.verified_artifact(digest_value, mission_id)?;
        let recorded_at_seq = record.creation_event_seq();
        if recorded_at_seq > boundary_event_seq {
            return Err(CollectorRefusal::AfterBoundary {
                kind: "artifact",
                id: digest_value.to_owned(),
                recorded_at_seq: i64::try_from(recorded_at_seq).unwrap_or(i64::MAX),
                boundary_event_seq,
            });
        }
        Ok(collected)
    }

    /// The owner's verified read plus the two scope facts the owner cannot know, with the record
    /// kept for the caller that needs its cursor: `delivered_artifact` for the completion's own
    /// delivery, and the Artifact arm of `support` for a reference cited under a condition. One
    /// read, one place the scope facts are checked, and the boundary applied by each caller against
    /// the cut it owns.
    fn verified_artifact(
        &self,
        digest_value: &str,
        mission_id: Uuid,
    ) -> Result<(crate::artifact::VerifiedArtifactRecord, CollectedDigest), CollectorRefusal> {
        let record = match crate::artifact::verified_artifact_within(
            self.connection,
            self.workspace_id,
            digest_value,
        ) {
            Ok(Some(record)) => record,
            Ok(None) => {
                return Err(CollectorRefusal::Absent {
                    kind: "artifact",
                    id: digest_value.to_owned(),
                });
            }
            Err(error) => {
                return Err(CollectorRefusal::Unverified {
                    kind: "artifact",
                    reason: error.to_string(),
                });
            }
        };

        artifact_scope(
            record.payload(),
            digest_value,
            self.workspace_id,
            mission_id,
        )?;

        let collected = CollectedDigest {
            kind: SourceKind::Artifact,
            workspace_id: self.workspace_id,
            // From the verified value, never from the caller's string: the reader is what proved
            // this digest names these bytes.
            digest_algorithm: record.digest().algorithm.clone(),
            digest_canonicalizer: record.digest().canonicalizer.clone(),
            digest_value: record.digest().value.clone(),
        };
        Ok((record, collected))
    }
}

/// A condition's support, read one reference at a time through `read`, with the bound applied
/// BEFORE the first read and before the list is allocated. Pure over the slice and a closure, so
/// a unit test can hand it a longer list than any verified record can carry and count the reads.
fn collected_support(
    ordinal: u32,
    references: &[SupportRef],
    mut read: impl FnMut(&SupportRef) -> Result<CollectedSupport, CollectorRefusal>,
) -> Result<Vec<CollectedSupport>, CollectorRefusal> {
    if references.len() > MAX_SUPPORT_REFS_PER_CONDITION {
        return Err(CollectorRefusal::SupportOverBound {
            ordinal,
            observed: references.len(),
            bound: MAX_SUPPORT_REFS_PER_CONDITION,
        });
    }
    let mut support = Vec::with_capacity(references.len());
    for reference in references {
        support.push(read(reference)?);
    }
    Ok(support)
}

/// The two things the owning reader cannot know about a delivered Artifact: that it is THIS
/// Workspace's and scoped to THIS Mission. Pure over the decoded payload, so both refusals are
/// reachable by a unit test without bypassing the owner's own guards.
///
/// The Workspace comes from the DECODED BYTES, not from the query predicate that found the row: a
/// filter says which rows were looked at, and this says whose the record is.
fn artifact_scope(
    payload: &crate::artifact::ArtifactV1,
    digest_value: &str,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<(), CollectorRefusal> {
    if payload.workspace_id != workspace_id {
        return Err(CollectorRefusal::ArtifactInAnotherWorkspace {
            id: digest_value.to_owned(),
        });
    }
    match payload.context {
        crate::artifact::ArtifactContext::Mission {
            mission_id: declared,
            ..
        } if declared == mission_id => Ok(()),
        crate::artifact::ArtifactContext::Mission {
            mission_id: declared,
            ..
        } => Err(CollectorRefusal::ArtifactOutOfScope {
            id: digest_value.to_owned(),
            declared_mission: Some(declared),
            expected_mission: mission_id,
        }),
        crate::artifact::ArtifactContext::None => Err(CollectorRefusal::ArtifactOutOfScope {
            id: digest_value.to_owned(),
            declared_mission: None,
            expected_mission: mission_id,
        }),
    }
}

/// Every source the collector can position, and the locator row that positions it.
///
/// Exposed so a test can assert the table covers what `collect` cites: a source added to the
/// collector without a locator row would silently skip the boundary check, which is the one failure
/// this table exists to prevent.
pub fn positionable_sources() -> BTreeMap<&'static str, (&'static str, &'static str)> {
    EVENT_LOCATORS
        .iter()
        .map(|(label, kind, path)| (*label, (*kind, *path)))
        .collect()
}

/// The label every Outcome Receipt checksum carries.
///
/// Re-exported here so a consumer of the collector never has to reach into the manifest module to
/// learn what the checksum it just received is. It is an **Outcome verification checksum** over the
/// exact stored `receipt_json` bytes, not a Receipt content digest: ADR 0027 and Receipt v2 define
/// none, and minting one would contradict them (Kevin's ruling 5).
pub const OUTCOME_RECEIPT_CHECKSUM_LABEL: &str = RECEIPT_CHECKSUM_LABEL;

#[cfg(test)]
mod scope_tests {
    use super::*;
    use crate::artifact::{ArtifactContext, ArtifactDigest, ArtifactKind, ArtifactV1};

    fn payload(workspace_id: Uuid, context: ArtifactContext) -> ArtifactV1 {
        ArtifactV1 {
            workspace_id,
            kind: ArtifactKind::OutcomeDocument,
            media_type: "text/markdown".to_owned(),
            content_digest: ArtifactDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.raw.v1".to_owned(),
                value: "0".repeat(64),
            },
            content_bytes: 1,
            accountable_resident_id: Uuid::nil(),
            accountable_configuration_id: Uuid::nil(),
            context,
            supersedes: None,
            recorded_at_ms: 0,
        }
    }

    /// Every positionable label has a bytes path, and every bytes path has a label: two tables
    /// that describe one Event each cannot be allowed to drift.
    #[test]
    fn every_locator_has_a_bytes_path_and_no_bytes_path_is_orphaned() {
        for (label, _, _) in EVENT_LOCATORS {
            assert!(
                EVENT_BYTES_PATHS
                    .iter()
                    .any(|(bytes_label, _)| bytes_label == label),
                "{label} can be positioned but its Event has no declared bytes path"
            );
        }
        for (label, _) in EVENT_BYTES_PATHS {
            assert!(
                EVENT_LOCATORS
                    .iter()
                    .any(|(locator_label, _, _)| locator_label == label),
                "{label} has a bytes path but is not a positionable source"
            );
        }
    }

    /// Acceptance row 0. The realistic row never reaches this line -- the owning reader refuses it
    /// three ways first -- so the arm is proved here, on the payload, where a mutant that disables
    /// the comparison has nowhere to hide behind the owner.
    /// The bound is on the WORK: a list one past it is refused before a single reference is read,
    /// and one at it is read exactly once per entry.
    #[test]
    fn support_past_the_bound_is_refused_before_any_reference_is_read() {
        let reference = |byte: u8| SupportRef::Evaluation([byte; 32]);
        let reads = std::cell::Cell::new(0_usize);
        let read = |_: &SupportRef| {
            reads.set(reads.get() + 1);
            Ok(CollectedSupport::Evaluation(CollectedDigest {
                kind: SourceKind::Evaluation,
                workspace_id: Uuid::nil(),
                digest_value: String::new(),
            }))
        };
        let over: Vec<SupportRef> = (0..=MAX_SUPPORT_REFS_PER_CONDITION)
            .map(|i| reference(u8::try_from(i).unwrap()))
            .collect();
        let refused = collected_support(3, &over, read).expect_err("one past the bound");
        assert_eq!(
            refused,
            CollectorRefusal::SupportOverBound {
                ordinal: 3,
                observed: MAX_SUPPORT_REFS_PER_CONDITION + 1,
                bound: MAX_SUPPORT_REFS_PER_CONDITION,
            }
        );
        assert_eq!(
            reads.get(),
            0,
            "a reference was read before the bound was applied"
        );

        let at: Vec<SupportRef> = (0..MAX_SUPPORT_REFS_PER_CONDITION)
            .map(|i| reference(u8::try_from(i).unwrap()))
            .collect();
        let collected = collected_support(3, &at, read).expect("at the bound");
        assert_eq!(collected.len(), MAX_SUPPORT_REFS_PER_CONDITION);
        assert_eq!(reads.get(), MAX_SUPPORT_REFS_PER_CONDITION);
    }

    #[test]
    fn a_payload_naming_another_workspace_is_the_collectors_own_refusal() {
        let mine = Uuid::new_v4();
        let mission = Uuid::new_v4();
        let theirs = payload(
            Uuid::new_v4(),
            ArtifactContext::Mission {
                project_id: Uuid::new_v4(),
                mission_id: mission,
            },
        );
        assert_eq!(
            artifact_scope(&theirs, "d", mine, mission),
            Err(CollectorRefusal::ArtifactInAnotherWorkspace { id: "d".to_owned() })
        );
        // The control: same payload, its own Workspace, its own Mission.
        let ours = payload(
            mine,
            ArtifactContext::Mission {
                project_id: Uuid::new_v4(),
                mission_id: mission,
            },
        );
        assert_eq!(artifact_scope(&ours, "d", mine, mission), Ok(()));
        // ...and the Workspace is checked BEFORE the Mission: a foreign record scoped to "our"
        // Mission id is still foreign.
        let mut foreign_same_mission = theirs.clone();
        foreign_same_mission.context = ArtifactContext::Mission {
            project_id: Uuid::new_v4(),
            mission_id: mission,
        };
        assert!(matches!(
            artifact_scope(&foreign_same_mission, "d", mine, mission),
            Err(CollectorRefusal::ArtifactInAnotherWorkspace { .. })
        ));
    }
}
