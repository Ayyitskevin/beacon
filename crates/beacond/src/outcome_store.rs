//! Outcome Document slice 3-A: the immutable manifest store and `prepare` (ADR 0040).
//!
//! `main` had the whole Outcome pipeline except its ending. The renderer turns a manifest into
//! Markdown (slice 1), the collector reads sources through their owning verified readers (slice 2),
//! and the Artifact store holds content-addressed bytes with accountable authorship (ADR 0043) --
//! but nothing persisted a manifest and nothing turned rendered Markdown into a candidate Artifact,
//! so a completed Mission still had no durable Outcome. This module is that step, and only that
//! step: `publish`, the publication relation, `get(mission_id)` and correction versions are 3-B.
//!
//! Two properties are this module's whole subject.
//!
//! 1. **The stored bytes are the record.** Every column beside `canonical_bytes` is a projection
//!    for querying, and the verified reader re-parses the stored bytes and recomputes their digest
//!    on every read before it returns anything. A row that disagrees with its own bytes is a typed
//!    refusal -- the design's "aborts ... not rendered as ordinary broken" -- never an
//!    [`OutcomeState::Broken`](crate::OutcomeState::Broken) row in a document.
//! 2. **`prepare` is one transaction or none of it.** The manifest, the candidate Artifact holding
//!    the exact rendered Markdown, and that Artifact's `artifact.recorded` Event are written
//!    together. A candidate Artifact with no manifest is a document nothing can be verified
//!    against; a manifest with no candidate is a proof of nothing.
//!
//! **Preparing an Outcome creates no authority.** It does not complete a Mission, publish an
//! Outcome, qualify anything for publication, or make a success verdict. A candidate is a document
//! that exists.
//!
//! **The Mission is cited by identity (R18.4).** A Mission has no canonical bytes and no digest on
//! `main`, so the v2 grammar carries `mission_id` and nothing else about it; the completion the
//! manifest is frozen on is the verified anchor, and `prepare` proves the identity against the
//! completion's own bytes. The v1 field that demanded a Mission digest -- and the identity-only
//! citation this module briefly required in its place -- are gone with the grammar revision.
//!
//! **There is no `outcome.prepared` Event kind (R18.6).** A prepared manifest is positioned in the
//! log by its candidate Artifact's `artifact.recorded` Event -- the Artifact the row's foreign key
//! requires -- so the ledger accounts for every row without a new kind and without rebuilding the
//! closed `events` table. A manifest cannot exist without a candidate by construction (they are
//! written in one transaction, and the foreign key is `NOT NULL`);
//! `a_manifest_cannot_exist_without_its_candidate` says so, because R18.6 revisits this only if
//! one ever can. `verify_manifest_state` is what turns that position into an invariant (R18.7).

use std::fmt;

use std::collections::BTreeSet;
use std::sync::Arc;

use data_encoding::HEXLOWER;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::artifact::{
    ArtifactContext, ArtifactDigest, ArtifactError, ArtifactKind, VerifiedArtifactRecord,
    record_artifact_within,
};
use crate::evaluation::EvaluationDigest;
use crate::mission_completion::{COMPLETION_CANONICALIZER, MissionCompletionV1, SupportRef};
use crate::mission_completion_store::{CompletionReadRefusal, verified_completion_within};
use crate::mission_impact::{
    CANONICALIZER as MISSION_IMPACT_CANONICALIZER, ClassificationStoreError,
    verified_classification_for_citation,
};
use crate::operator_account::{OperatorAccountError, OperatorAccountService};
use crate::outcome::{
    AppendixRequest, MalformedOutcome, OutcomeManifestV1, OutcomeRefusal, OutcomeState,
    SourceDigest, SourceKind, VerifiedSources, render,
};
use crate::outcome_assembler::{AssemblyRefusal, SourcesRefusal, assemble, verified_sources_for};
use crate::outcome_collector::{CollectionRequest, Collector, CollectorRefusal};
use crate::outcome_publication::{
    ExportedOutcome, PublicationSummary, PublishRefusal, PublishedOutcome, get_version_within,
    get_within, publication_history_within, publish_within,
};
use crate::proposal::sha256;

const DIGEST_ALGORITHM: &str = "sha256";

/// The schema version a completion citation must carry.
///
/// `MissionCompletionV1` has no version field of its own -- its grammar version is the `v1` in its
/// canonicalizer -- so the citation's `schema_version` has exactly one admissible value, and until
/// astra's A5 it had none: a citation claiming version 999 was accepted.
const COMPLETION_SCHEMA_VERSION: u32 = 1;

/// The largest manifest this store will hold, applied to the canonical bytes before the insert.
///
/// It is the STORE's bound, not the codec's: `OutcomeManifestV1` has no envelope ceiling of its
/// own, so without one here a manifest's size would first be refused by a database CHECK, which
/// reaches a caller as a store error rather than as a fact about the bytes. The figure is
/// `outcome`'s own `MAX_VERIFIED_SOURCE_BYTES` -- the one existing bound of this scale -- and the
/// migration's CHECK repeats it so the two cannot drift apart in silence.
pub const MAX_STORED_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

/// The manifest's R15 reference identity, DERIVED from its content identity.
///
/// **A recorded exception to R15, ruled in R18.2**, not an oversight. R15 puts a record's UUID
/// inside its canonical bytes; a manifest is the one record that must not have one there. It is
/// immutable and `prepare` is idempotent on its bytes, so an immutable manifest WANTS content
/// identity and reference identity to be the same thing: two identical manifests are one manifest,
/// and corrections are new publication versions rather than new manifests. A minted UUID inside
/// the bytes would break that idempotency outright, and one stored beside the bytes would be a
/// second identity the bytes cannot contradict -- the decoy R15 exists to refuse.
///
/// So this is the first sixteen bytes of the digest re-spelled as a UUID: a citation's UUID and
/// digest cannot disagree, because one is a function of the other. R18.2 revisits this only if a
/// manifest ever needs an identity its bytes cannot give.
pub fn manifest_reference_id(manifest_digest: &str) -> Result<Uuid, ManifestStoreRefusal> {
    let decoded = HEXLOWER
        .decode(manifest_digest.as_bytes())
        .map_err(|_| ManifestStoreRefusal::BytesDisagree)?;
    let head: [u8; 16] = decoded
        .get(..16)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(ManifestStoreRefusal::BytesDisagree)?;
    Ok(Uuid::from_bytes(head))
}

// ---------------------------------------------------------------------------------------------
// Refusals -- each names the rule it broke
// ---------------------------------------------------------------------------------------------

/// Why a manifest could not be stored, or could not be read back as itself.
///
/// There is deliberately no arm that carries a partially verified manifest: a stored byte sequence
/// either proves itself on every read or it is refused, and a half-proved one is the shape that
/// would let a rewritten proof into a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestStoreRefusal {
    /// No manifest at this digest in this store at all.
    Absent,
    /// A manifest at this digest exists and belongs to another Workspace. A refusal, never an
    /// empty answer: scoping is structural here, not a filter.
    CrossWorkspace,
    /// The stored bytes do not hash to the digest they are filed under, or they do not survive an
    /// exact decode/re-encode round trip.
    BytesDisagree,
    /// A projection column says something the stored bytes do not. Each names itself, because "a
    /// column disagrees" and "which column" are different amounts of help.
    ColumnDisagrees { column: &'static str },
    /// The canonical bytes are larger than this store's own ceiling, refused before the insert.
    ManifestTooLarge { configured: u64, observed: u64 },
    /// The bytes cannot be a v1 manifest at all.
    Malformed(MalformedOutcome),
    /// A stored manifest says something about its completion that the completion does not say.
    ///
    /// The same disagreement `prepare` refuses, found on the read path instead of the write path
    /// (astra's A3). It spans two records, so it is neither owner's refusal and lives here.
    Irreconcilable { detail: String },
    /// The row is self-consistent and the ledger does not account for it (R18.7).
    ///
    /// A manifest's candidate Artifact is its position in the log -- there is no
    /// `outcome.prepared` Event (R18.6) -- so "this row was prepared" and "this row was inserted
    /// behind the ledger" are the same sentence to a reader that only re-proves bytes. The startup
    /// pass is what separates them, and `reason` names which link failed.
    CandidateNotAccounted { reason: &'static str },
    /// The store itself failed. Never silently an absence.
    Store(String),
}

impl std::fmt::Display for ManifestStoreRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absent => write!(formatter, "no outcome manifest at that digest"),
            Self::CrossWorkspace => write!(
                formatter,
                "that outcome manifest belongs to another Workspace"
            ),
            Self::BytesDisagree => write!(
                formatter,
                "the stored manifest bytes and their digest disagree"
            ),
            Self::ColumnDisagrees { column } => write!(
                formatter,
                "the stored column {column} disagrees with the manifest's own bytes"
            ),
            Self::ManifestTooLarge {
                configured,
                observed,
            } => write!(
                formatter,
                "bound stored_manifest_bytes exhausted: configured {configured}, observed \
                 {observed}"
            ),
            Self::Malformed(reason) => {
                write!(formatter, "not a v1 outcome manifest: {}", reason.as_str())
            }
            Self::Irreconcilable { detail } => {
                write!(
                    formatter,
                    "the manifest and its completion disagree: {detail}"
                )
            }
            Self::CandidateNotAccounted { reason } => write!(
                formatter,
                "the ledger does not account for this outcome manifest: {reason}"
            ),
            Self::Store(detail) => write!(formatter, "the store failed: {detail}"),
        }
    }
}

impl std::error::Error for ManifestStoreRefusal {}

impl From<MalformedOutcome> for ManifestStoreRefusal {
    fn from(reason: MalformedOutcome) -> Self {
        Self::Malformed(reason)
    }
}

fn store_error(error: rusqlite::Error) -> ManifestStoreRefusal {
    ManifestStoreRefusal::Store(error.to_string())
}

/// Why one `prepare` did not produce a candidate.
///
/// Each arm repeats the owning module's refusal rather than reinterpreting it: a completion the
/// completion store refused is refused here for the completion store's reason, in the completion
/// store's words.
#[derive(Debug)]
pub enum PrepareRefusal {
    /// The manifest names another Workspace than the service's.
    CrossWorkspace,
    /// The manifest's bytes cannot be produced or stored.
    Manifest(ManifestStoreRefusal),
    /// The completion this manifest is frozen on was refused by its owning verified reader.
    Completion(CompletionReadRefusal),
    /// The Policy classification whose high-impact bit decides the appendix was refused by its
    /// owning verified reader, or its bit disagrees with the manifest's copy (R13.7).
    Classification(ClassificationStoreError),
    /// The manifest cites a Policy classification this Workspace's store does not hold. Distinct
    /// from a reader refusal: nothing is wrong with a record, there is no record.
    ClassificationAbsent,
    /// The manifest says something about the completion that the completion does not say. These
    /// comparisons span two records, so they have no single owner and live here.
    Irreconcilable { detail: String },
    /// The renderer refused: an exhausted bound, an unknown renderer version, or sources that do
    /// not correspond to the manifest.
    Render(String),
    /// The Artifact store refused the candidate.
    Artifact(ArtifactError),
    /// The Operator is not authenticated.
    Account(OperatorAccountError),
    /// `prepare_for_mission`: this Workspace holds no completion for the Mission. There is nothing
    /// to freeze on, and a manifest is not assembled from defaults.
    MissionNotCompleted { mission_id: Uuid },
    /// `prepare_for_mission`: the Mission has no impact classification, so the appendix policy has
    /// no bit to key on.
    MissionNotClassified { mission_id: Uuid },
    /// `prepare_for_mission`: the Mission is classified, but every version's Event falls after the
    /// completion's boundary. None of them is the classification that was current when the Mission
    /// completed, and the appendix policy will not read from the future (R23.4).
    MissionNotClassifiedAtBoundary {
        mission_id: Uuid,
        boundary_event_seq: u64,
        classifications_after: usize,
    },
    /// The collector refused, in its own words.
    Collector(CollectorRefusal),
    /// The assembler refused to guess, in its own words.
    Assembly(AssemblyRefusal),
    /// A text the document prints could not be re-read through its owner.
    Sources(SourcesRefusal),
}

impl std::fmt::Display for PrepareRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CrossWorkspace => {
                write!(formatter, "the manifest names another Workspace")
            }
            Self::Manifest(refusal) => write!(formatter, "{refusal}"),
            Self::Completion(refusal) => write!(formatter, "the completion: {refusal}"),
            Self::Classification(error) => {
                write!(formatter, "the policy classification: {error}")
            }
            Self::ClassificationAbsent => write!(
                formatter,
                "the manifest cites a policy classification this Workspace does not hold"
            ),
            Self::Irreconcilable { detail } => {
                write!(formatter, "sources do not reconcile: {detail}")
            }
            Self::Render(detail) => write!(formatter, "the renderer refused: {detail}"),
            Self::Artifact(error) => write!(formatter, "the candidate Artifact: {error:?}"),
            Self::Account(error) => write!(formatter, "{error}"),
            Self::MissionNotCompleted { mission_id } => {
                write!(
                    formatter,
                    "Mission {mission_id} has no completion in this Workspace"
                )
            }
            Self::MissionNotClassified { mission_id } => {
                write!(
                    formatter,
                    "Mission {mission_id} has no impact classification"
                )
            }
            Self::MissionNotClassifiedAtBoundary {
                mission_id,
                boundary_event_seq,
                classifications_after,
            } => write!(
                formatter,
                "Mission {mission_id} has {classifications_after} impact classification(s), all \
                 recorded after the completion's boundary (event {boundary_event_seq}); none was \
                 current when the Mission completed"
            ),
            Self::Collector(refusal) => write!(formatter, "the collector: {refusal}"),
            Self::Assembly(refusal) => write!(formatter, "the assembler: {refusal}"),
            Self::Sources(refusal) => write!(formatter, "the document's sources: {refusal}"),
        }
    }
}

impl std::error::Error for PrepareRefusal {}

impl From<CollectorRefusal> for PrepareRefusal {
    fn from(refusal: CollectorRefusal) -> Self {
        Self::Collector(refusal)
    }
}

impl From<AssemblyRefusal> for PrepareRefusal {
    fn from(refusal: AssemblyRefusal) -> Self {
        Self::Assembly(refusal)
    }
}

impl From<SourcesRefusal> for PrepareRefusal {
    fn from(refusal: SourcesRefusal) -> Self {
        Self::Sources(refusal)
    }
}

impl From<ManifestStoreRefusal> for PrepareRefusal {
    fn from(refusal: ManifestStoreRefusal) -> Self {
        Self::Manifest(refusal)
    }
}

impl From<CompletionReadRefusal> for PrepareRefusal {
    fn from(refusal: CompletionReadRefusal) -> Self {
        Self::Completion(refusal)
    }
}

impl From<ClassificationStoreError> for PrepareRefusal {
    fn from(error: ClassificationStoreError) -> Self {
        Self::Classification(error)
    }
}

impl From<ArtifactError> for PrepareRefusal {
    fn from(error: ArtifactError) -> Self {
        Self::Artifact(error)
    }
}

impl From<OperatorAccountError> for PrepareRefusal {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<OutcomeRefusal> for PrepareRefusal {
    fn from(refusal: OutcomeRefusal) -> Self {
        Self::Render(refusal.to_string())
    }
}

impl From<rusqlite::Error> for PrepareRefusal {
    fn from(error: rusqlite::Error) -> Self {
        Self::Manifest(store_error(error))
    }
}

// ---------------------------------------------------------------------------------------------
// What a verified read returns
// ---------------------------------------------------------------------------------------------

/// One stored manifest, proved against its own bytes at read time.
///
/// The manifest inside is the DECODED stored bytes, not a projection assembled from columns. There
/// is no accessor that returns a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredOutcomeManifest {
    manifest_id: Uuid,
    digest: SourceDigest,
    canonical_bytes: Vec<u8>,
    manifest: OutcomeManifestV1,
    candidate_artifact_digest: String,
}

impl StoredOutcomeManifest {
    /// The derived reference identity (R15). A function of the digest, so the two cannot disagree.
    pub fn manifest_id(&self) -> Uuid {
        self.manifest_id
    }

    /// The identity envelope (algorithm, canonicalizer, value), recomputed at read time.
    pub fn digest(&self) -> &SourceDigest {
        &self.digest
    }

    /// The exact stored canonical bytes -- the one sequence a published Outcome's appendix lets a
    /// reader recompute the digest from.
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    /// The decoded manifest, proved against those bytes at read time.
    pub fn manifest(&self) -> &OutcomeManifestV1 {
        &self.manifest
    }

    /// The Artifact holding the exact Markdown these bytes rendered to.
    pub fn candidate_artifact_digest(&self) -> &str {
        &self.candidate_artifact_digest
    }
}

/// What one `prepare` produced.
///
/// Three identities and nothing else. It carries no rendered text, no verdict, and no permission:
/// a caller that wants the document reads the candidate Artifact, and a caller that wants to know
/// whether the Outcome is published asks the publication relation, which is 3-B's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeCandidate {
    pub manifest_id: Uuid,
    pub manifest_digest: String,
    pub candidate_artifact_digest: String,
}

struct StoredRow {
    workspace_id: String,
    canonical_bytes: Vec<u8>,
    digest_algorithm: String,
    digest_canonicalizer: String,
    manifest_id: String,
    mission_id: String,
    completion_digest: String,
    canonicalizer_version: String,
    renderer_version: String,
    boundary_event_seq: i64,
    candidate_artifact_digest: String,
}

/// Does this Artifact's content actually PROVE this manifest?
///
/// Every rendered Outcome Document embeds the exact canonical `OutcomeManifestV1` bytes as
/// lowercase hexadecimal, in both appendix modes, precisely so a reader can recompute the manifest
/// digest from the document in front of them (design §3, "Verification appendix"). That embedding
/// is the binding between a manifest and its candidate, and it is the only one available without
/// re-rendering: `VerifiedSources` does not survive `prepare`, so nothing after it can render the
/// document again to compare.
///
/// The comparison strips newlines rather than reproducing the renderer's 64-character chunking. A
/// second copy of that format here would be a second spelling of it, and the two would drift; the
/// hex lines carry no other whitespace, so removing `\n` is enough and stays correct if the
/// chunk width ever changes.
fn candidate_proves_manifest(candidate_content: &[u8], manifest_bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(candidate_content) else {
        return false;
    };
    let mut unwrapped = String::with_capacity(text.len());
    unwrapped.extend(text.chars().filter(|character| *character != '\n'));
    unwrapped.contains(&HEXLOWER.encode(manifest_bytes))
}

// ---------------------------------------------------------------------------------------------
// The verified reader
// ---------------------------------------------------------------------------------------------

/// Read one stored manifest by its digest, proving every claim before returning one.
///
/// The order is the point. The row is fetched, its Workspace checked, its bytes hashed, its bytes
/// decoded, and its bytes re-encoded -- and only then are the projection columns compared against
/// what the bytes say. Nothing here reads a claim off a column: the columns are checked, never
/// consulted.
pub fn verified_manifest_within(
    connection: &Connection,
    workspace_id: Uuid,
    manifest_digest: &str,
) -> Result<StoredOutcomeManifest, ManifestStoreRefusal> {
    let row: Option<StoredRow> = connection
        .query_row(
            "SELECT workspace_id, canonical_bytes, digest_algorithm, digest_canonicalizer,
                    manifest_id, mission_id, completion_digest, canonicalizer_version,
                    renderer_version, boundary_event_seq, candidate_artifact_digest
             FROM outcome_manifests WHERE manifest_digest = ?1",
            params![manifest_digest],
            |row| {
                Ok(StoredRow {
                    workspace_id: row.get(0)?,
                    canonical_bytes: row.get(1)?,
                    digest_algorithm: row.get(2)?,
                    digest_canonicalizer: row.get(3)?,
                    manifest_id: row.get(4)?,
                    mission_id: row.get(5)?,
                    completion_digest: row.get(6)?,
                    canonicalizer_version: row.get(7)?,
                    renderer_version: row.get(8)?,
                    boundary_event_seq: row.get(9)?,
                    candidate_artifact_digest: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(store_error)?;
    let Some(stored) = row else {
        return Err(ManifestStoreRefusal::Absent);
    };
    if stored.workspace_id != workspace_id.to_string() {
        return Err(ManifestStoreRefusal::CrossWorkspace);
    }

    // Identity is a computation over the stored bytes, never the column it sits beside.
    let recomputed = HEXLOWER.encode(&sha256(&stored.canonical_bytes));
    if recomputed != manifest_digest {
        return Err(ManifestStoreRefusal::BytesDisagree);
    }
    let manifest = OutcomeManifestV1::from_canonical_bytes(&stored.canonical_bytes)?;
    if manifest.canonical_bytes()? != stored.canonical_bytes {
        return Err(ManifestStoreRefusal::BytesDisagree);
    }
    let manifest_id = manifest_reference_id(&recomputed)?;

    // Every projection repeats what the bytes say. A decoy row -- one whose columns lie while its
    // bytes stay correct -- is refused here rather than answered from.
    for (column, observed, from_bytes) in [
        // The digest ENVELOPE, not the value alone (R15): 0029 persists the algorithm and the
        // canonicalizer beside the bytes, so both are projections and both are checked. A reader
        // that returned a freshly built envelope instead could hand back `sha256` over bytes filed
        // under `sha512` and never notice -- measured, it did.
        (
            "digest_algorithm",
            stored.digest_algorithm.clone(),
            DIGEST_ALGORITHM.to_owned(),
        ),
        (
            "digest_canonicalizer",
            stored.digest_canonicalizer.clone(),
            manifest.canonicalizer_version.clone(),
        ),
        (
            "workspace_id",
            stored.workspace_id.clone(),
            manifest.workspace_id.to_string(),
        ),
        (
            "manifest_id",
            stored.manifest_id.clone(),
            manifest_id.to_string(),
        ),
        (
            "mission_id",
            stored.mission_id.clone(),
            manifest.mission_id.to_string(),
        ),
        (
            "completion_digest",
            stored.completion_digest.clone(),
            manifest.completion.digest.value.clone(),
        ),
        (
            "canonicalizer_version",
            stored.canonicalizer_version.clone(),
            manifest.canonicalizer_version.clone(),
        ),
        (
            "renderer_version",
            stored.renderer_version.clone(),
            manifest.renderer_version.clone(),
        ),
        (
            "boundary_event_seq",
            stored.boundary_event_seq.to_string(),
            manifest.boundary_event_seq.to_string(),
        ),
    ] {
        if observed != from_bytes {
            return Err(ManifestStoreRefusal::ColumnDisagrees { column });
        }
    }

    // The candidate projection is a projection like every other, and until astra's A2 it was the
    // one column nothing checked: changing it pointed `manifest()`, `candidate()` and the
    // idempotent `prepare` retry at an unrelated document, and all three accepted it. It cannot be
    // compared against the manifest's own bytes -- a content-addressed document cannot name itself
    // (R13.1) -- so it is proved the other way round: the Artifact it names must embed these exact
    // canonical bytes, which is the binding the document already carries for its readers.
    let candidate = crate::artifact::verified_artifact_within(
        connection,
        workspace_id,
        &stored.candidate_artifact_digest,
    )
    .map_err(|_| ManifestStoreRefusal::CandidateNotAccounted {
        reason: "the candidate Artifact does not verify against its own bytes",
    })?
    .ok_or(ManifestStoreRefusal::CandidateNotAccounted {
        reason: "no candidate Artifact in this Workspace carries that digest",
    })?;
    if !candidate_proves_manifest(candidate.content(), &stored.canonical_bytes) {
        return Err(ManifestStoreRefusal::ColumnDisagrees {
            column: "candidate_artifact_digest",
        });
    }

    Ok(StoredOutcomeManifest {
        manifest_id,
        // The envelope returned is the STORED one, now proved equal to what the bytes and this
        // reader's own hash function say. Building a fresh one here is what let a lie through.
        digest: SourceDigest {
            algorithm: stored.digest_algorithm,
            canonicalizer: stored.digest_canonicalizer,
            value: recomputed,
        },
        canonical_bytes: stored.canonical_bytes,
        manifest,
        candidate_artifact_digest: stored.candidate_artifact_digest,
    })
}

// ---------------------------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------------------------

/// Owns preparing an Outcome candidate and reading a stored manifest back.
///
/// **Preparing creates no authority.** It completes no Mission, opens no Gate, publishes nothing,
/// and qualifies nothing for publication. No module may branch on a candidate's existence to
/// One rendered PDF and the identity of the document it came from.
///
/// It carries no bytes of the Markdown: the PDF is the derivative, the export is the source, and a
/// caller that wants both asks for both. `publication_version_count` rides along for R24.2's
/// reason -- "version 2" alone cannot say whether it is the current one.
#[derive(Debug, Clone)]
pub struct RenderedPdf {
    bytes: Vec<u8>,
    candidate_digest: crate::artifact::ArtifactDigest,
    publication_version: u32,
    publication_version_count: u32,
    profile: &'static str,
}

impl RenderedPdf {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The candidate Artifact digest the publication CITES -- the document these bytes render.
    pub fn candidate_digest(&self) -> &crate::artifact::ArtifactDigest {
        &self.candidate_digest
    }

    pub fn publication_version(&self) -> u32 {
        self.publication_version
    }

    pub fn publication_version_count(&self) -> u32 {
        self.publication_version_count
    }

    /// The render profile's name, the same one the PDF's `Creator` carries (R30.2).
    pub fn profile(&self) -> &'static str {
        self.profile
    }
}

/// derive any of those.
pub struct OutcomeService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl OutcomeService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Store one manifest and record the Markdown it renders to as a candidate Artifact, in one
    /// transaction.
    ///
    /// **Idempotent on the manifest's canonical bytes.** `renderer_version` is one of those bytes,
    /// so "the same manifest under the same renderer version" is one condition, not two: a second
    /// `prepare` for identical bytes reads the stored manifest back through the verified reader,
    /// returns its digests, and inserts nothing. A different renderer version is different bytes,
    /// a different digest, and a different candidate -- which is what a renderer change should be.
    ///
    /// The accountable author is the completion's accountable Resident, read through the
    /// completion's own verified reader. Never a projection column: `mission_completions` has an
    /// `accountable_resident_id` column beside the bytes, and reading it would make the candidate's
    /// authorship a claim about a row rather than about the record.
    ///
    /// The appendix is keyed on the VERIFIED Policy classification, not on the manifest's copy of
    /// its bit. The manifest carries the copy so drift is detectable (ADR 0037's argument) and
    /// `recheck_copied_bit` proves the copy against the record at prepare time (R13.7) -- so a
    /// caller cannot dodge a mandatory appendix by writing `false` into the manifest it supplies.
    ///
    /// **This is an authenticated service method in v1, not a Command (R18.1)** -- the same posture
    /// as `record_completion`. There is no admission check, no transport and no Agent path to it,
    /// and nothing outside tests may call it: the Command, its admission check and its transport
    /// arrive with the presentation surface (gpt2 slice 4), under R12.12's rule that retrieval and
    /// export are Operator actions inside the Mission's Workspace scope. An input surface built
    /// before its consumers exist is how a grammar gets frozen around a guess.
    pub fn prepare(
        &self,
        manifest: &OutcomeManifestV1,
        sources: &VerifiedSources,
    ) -> Result<OutcomeCandidate, PrepareRefusal> {
        self.accounts.with_authenticated_store(|connection, now| {
            let recorded_at_ms = service_clock(now)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let candidate = prepare_within(
                &transaction,
                self.workspace_id,
                recorded_at_ms,
                manifest,
                sources,
            )?;
            transaction.commit()?;
            Ok(candidate)
        })
    }

    /// The whole path, for one Mission: collector -> assembler -> owner-read text -> `prepare`,
    /// inside ONE transaction at the completion's boundary (slice 3b item 0b, item 3).
    ///
    /// Still an authenticated service method and not a Command (R18.1): the admission check and
    /// the transport arrive with the presentation surface. What this adds over `prepare` is that
    /// nothing outside a test has to build a manifest by hand any more.
    ///
    /// The two lookups by Mission -- its completion and its newest classification -- identify
    /// CANDIDATE digests only (gpt2 §2: a relation may identify, never supply a claim). Everything
    /// they name is then read through its owner by the collector, and the collection request's
    /// citations come from the verified completion's own support references, not from a query.
    /// Idempotent through `prepare`'s identity: the same completion assembles to the same bytes.
    pub fn prepare_for_mission(
        &self,
        mission_id: Uuid,
        appendix_request: AppendixRequest,
    ) -> Result<OutcomeCandidate, PrepareRefusal> {
        self.accounts.with_authenticated_store(|connection, now| {
            let recorded_at_ms = service_clock(now)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let workspace_id = self.workspace_id;

            let completion_digest: Option<String> = transaction
                .query_row(
                    "SELECT digest_value FROM mission_completions
                     WHERE workspace_id = ?1 AND mission_id = ?2",
                    params![workspace_id.to_string(), mission_id.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(completion_digest) = completion_digest else {
                return Err(PrepareRefusal::MissionNotCompleted { mission_id });
            };
            // Every classification of the Mission, newest first. "Current" is decided below, not
            // here: it is the newest version whose Event is at or before the completion's boundary
            // (R23.4), and only the collector knows where the boundary falls.
            let classification_digests: Vec<String> = transaction
                .prepare(
                    "SELECT digest_value FROM mission_impact_classifications
                     WHERE workspace_id = ?1 AND mission_id = ?2
                     ORDER BY version DESC",
                )?
                .query_map(
                    params![workspace_id.to_string(), mission_id.to_string()],
                    |row| row.get(0),
                )?
                .collect::<Result<_, _>>()?;
            if classification_digests.is_empty() {
                return Err(PrepareRefusal::MissionNotClassified { mission_id });
            }

            // What the document cites is what the completion says supported its conditions -- read
            // from the completion's bytes through its owner, then handed to the collector to read
            // each one through ITS owner at the boundary.
            let completion =
                verified_completion_within(&transaction, workspace_id, &completion_digest)?;
            let collector = Collector::new(&transaction, workspace_id);

            // Newest first; a classification the collector places after the boundary is not the
            // current one, it is a later one -- step down a version. Any other refusal is the
            // collector's own and stands.
            let mut collected = None;
            let mut newest_boundary = None;
            for classification_digest in &classification_digests {
                let request = request_from(&completion, &completion_digest, classification_digest);
                match collector.collect(&request) {
                    Ok(sources) => {
                        collected = Some(sources);
                        break;
                    }
                    Err(CollectorRefusal::AfterBoundary {
                        kind: "classification",
                        boundary_event_seq,
                        ..
                    }) => {
                        newest_boundary.get_or_insert(boundary_event_seq);
                    }
                    Err(refusal) => return Err(refusal.into()),
                }
            }
            let Some(collected) = collected else {
                return Err(PrepareRefusal::MissionNotClassifiedAtBoundary {
                    mission_id,
                    boundary_event_seq: newest_boundary.unwrap_or_default(),
                    classifications_after: classification_digests.len(),
                });
            };
            let manifest = assemble(&collected, workspace_id, appendix_request)?;
            let sources = verified_sources_for(&transaction, workspace_id, &manifest)?;
            let candidate = prepare_within(
                &transaction,
                workspace_id,
                recorded_at_ms,
                &manifest,
                &sources,
            )?;
            transaction.commit()?;
            Ok(candidate)
        })
    }

    /// Publish one prepared candidate, qualified by one independent Evaluation.
    ///
    /// One transaction: the seam proves the pairing from both records' canonical bytes, the
    /// manifest and completion are re-read through their owning verified readers, appendix policy
    /// is re-checked against the candidate's own bytes, and the relation row is appended. The
    /// append IS the publication event.
    ///
    /// **This completes nothing and grants nothing.** No Mission, completion, or authority record
    /// is written or consulted for permission, and `PublishedOutcome` carries no predicate a
    /// caller could branch on to derive success, completion, or permission (ADR 0040).
    ///
    /// The digests are taken as full ENVELOPES rather than hex strings so the seam's
    /// `CitedEnvelopeDisagrees` check has something to catch: a caller that names the right 64
    /// characters under an algorithm or canonicalizer no owning module produced is making the
    /// assumption these records exist to remove.
    pub fn publish(
        &self,
        mission_id: Uuid,
        candidate_artifact_digest: &ArtifactDigest,
        evaluation_digest: &EvaluationDigest,
    ) -> Result<PublishedOutcome, PublishRefusal> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let published = publish_within(
                &transaction,
                self.workspace_id,
                mission_id,
                candidate_artifact_digest,
                evaluation_digest,
            )?;
            transaction.commit()?;
            Ok(published)
        })
    }

    /// The Mission's current published Outcome -- its highest version -- re-proved on every read.
    ///
    /// Retrieval inherits the Mission's Workspace access and is never public (R12.12): the read
    /// goes through the authenticated store like every other, and a Workspace scopes it.
    pub fn get(&self, mission_id: Uuid) -> Result<PublishedOutcome, PublishRefusal> {
        self.accounts.with_authenticated_store(|connection, _now| {
            get_within(connection, self.workspace_id, mission_id)
        })
    }

    /// The Mission's current published Outcome as its exact stored Markdown bytes.
    ///
    /// **A copy, never a re-render.** The bytes come from the stored candidate Artifact that the
    /// relation cites and that the qualifying Evaluation was about. Re-running the renderer would
    /// make the export a second path to the document, and the two would agree only until someone
    /// changed the renderer -- at which point the export would no longer be the thing that was
    /// published and evaluated.
    ///
    /// **Access is `get`'s, unchanged**: the same `with_authenticated_store` session gate, the same
    /// `self.workspace_id`, the same `get_within`. There is no separate export permission, and the
    /// reason is not that one was skipped: this codebase has one Operator, the schema admits no
    /// second role, and no action records an actor, so "an explicit authenticated Operator action"
    /// reduces here to exactly the check retrieval performs.
    ///
    /// **What is NOT recorded, named rather than left to be discovered (R29.4):** exporting emits
    /// no Event, so the store does not know that bytes left it or who took them. "Who exported
    /// this" belongs to the Operator and audit model that does not exist yet (Kevin's access item
    /// 12); when it does, this method is one of its subjects.
    pub fn export_markdown(&self, mission_id: Uuid) -> Result<ExportedOutcome, PublishRefusal> {
        self.accounts.with_authenticated_store(|connection, _now| {
            Ok(ExportedOutcome::from(get_within(
                connection,
                self.workspace_id,
                mission_id,
            )?))
        })
    }

    /// One version's PDF, rendered from the bytes `export_markdown_version` would return (R30.3).
    ///
    /// The seam between 4a and 4b, and deliberately nothing more. The bytes are the export's, the
    /// access check is the export's, and this method adds no rule of its own -- a second
    /// authorization here would be a second thing to keep true, and a PDF rendered from anything
    /// but the exported bytes would be a document that disagrees with the one an operator can save.
    ///
    /// The renderer version comes from the MANIFEST, not from a constant: the translator refuses a
    /// version it does not read, and telling it "v1" regardless of what the document says would
    /// defeat exactly that refusal. The manifest is the authority because `get_version_within`
    /// re-proves it on this read.
    ///
    /// **The PDF is not stored.** It is a deletable derivative (ADR 0040): rendering is a function
    /// of the exported bytes and the pinned profile, so there is nothing to cache and nothing to
    /// invalidate when the profile changes.
    pub fn render_pdf_version(
        &self,
        mission_id: Uuid,
        publication_version: u32,
    ) -> Result<RenderedPdf, PublishRefusal> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let published = get_version_within(
                connection,
                self.workspace_id,
                mission_id,
                publication_version,
            )?;
            let renderer_version = published.manifest().manifest().renderer_version.clone();
            let exported = ExportedOutcome::from(published);
            let bytes = crate::outcome_pdf::render_pdf(
                exported.bytes(),
                &renderer_version,
                &exported.digest().value,
                &crate::outcome_pdf::BEACON_OUTCOME_1,
            )
            .map_err(PublishRefusal::Pdf)?;
            Ok(RenderedPdf {
                bytes,
                candidate_digest: exported.digest().clone(),
                publication_version: exported.publication_version(),
                publication_version_count: exported.publication_version_count(),
                profile: crate::outcome_pdf::BEACON_OUTCOME_1.name,
            })
        })
    }

    /// One exact version's stored Markdown bytes. See `export_markdown` for the access note and
    /// for why this is a copy rather than a render.
    ///
    /// The versioned pair is not ceremony: a corrected Outcome keeps its superseded versions
    /// readable (R12.11), and an export that could only reach the newest one would make the version
    /// an auditor holds unexportable.
    pub fn export_markdown_version(
        &self,
        mission_id: Uuid,
        publication_version: u32,
    ) -> Result<ExportedOutcome, PublishRefusal> {
        self.accounts.with_authenticated_store(|connection, _now| {
            Ok(ExportedOutcome::from(get_version_within(
                connection,
                self.workspace_id,
                mission_id,
                publication_version,
            )?))
        })
    }

    /// Every version of this Mission's publication history, newest first.
    ///
    /// Reads the relation's own rows; it does not verify each document, because a list of versions
    /// must not cost one full verification per row (R29.5). A caller that wants a version's bytes
    /// asks for that version.
    pub fn publication_history(
        &self,
        mission_id: Uuid,
    ) -> Result<Vec<PublicationSummary>, PublishRefusal> {
        self.accounts.with_authenticated_store(|connection, _now| {
            publication_history_within(connection, self.workspace_id, mission_id)
        })
    }

    /// One exact version of a Mission's publication history. A corrected Outcome does not erase the
    /// version it superseded, so every version stays readable at its own number (R12.11).
    pub fn get_version(
        &self,
        mission_id: Uuid,
        publication_version: u32,
    ) -> Result<PublishedOutcome, PublishRefusal> {
        self.accounts.with_authenticated_store(|connection, _now| {
            get_version_within(
                connection,
                self.workspace_id,
                mission_id,
                publication_version,
            )
        })
    }

    /// Read one stored manifest back, proved against its own bytes.
    pub fn manifest(&self, manifest_digest: &str) -> Result<StoredOutcomeManifest, PrepareRefusal> {
        self.accounts.with_authenticated_store(|connection, _now| {
            Ok(verified_manifest_within(
                connection,
                self.workspace_id,
                manifest_digest,
            )?)
        })
    }

    /// The candidate Artifact one stored manifest rendered into, proved against its own bytes.
    pub fn candidate(
        &self,
        manifest_digest: &str,
    ) -> Result<VerifiedArtifactRecord, PrepareRefusal> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let stored = verified_manifest_within(connection, self.workspace_id, manifest_digest)?;
            Ok(crate::artifact::verified_artifact_within(
                connection,
                self.workspace_id,
                stored.candidate_artifact_digest(),
            )?
            .ok_or(ArtifactError::NotFound)?)
        })
    }
}

fn service_clock(now: i64) -> Result<u64, PrepareRefusal> {
    u64::try_from(now).map_err(|_| {
        PrepareRefusal::Manifest(ManifestStoreRefusal::Store(
            "the service clock is before the Unix epoch".to_owned(),
        ))
    })
}

/// The collection request a completion implies: its own digest, the Mission's classification, and
/// every Evaluation, Ruling and Receipt its condition results cite as support. Artifact support is
/// a per-condition citation and is not a collector lane; the assembler refuses a condition whose
/// support was counted but not carried, so nothing is silently dropped here.
fn request_from(
    completion: &MissionCompletionV1,
    completion_digest: &str,
    classification_digest: &str,
) -> CollectionRequest {
    let mut request = CollectionRequest {
        completion_digest: Some(completion_digest.to_owned()),
        classification_digest: Some(classification_digest.to_owned()),
        ..CollectionRequest::default()
    };
    for result in &completion.condition_results {
        for support in &result.support {
            match support {
                SupportRef::Evaluation(digest) => {
                    request.evaluation_digests.push(HEXLOWER.encode(digest));
                }
                SupportRef::Ruling(digest) => request.ruling_digests.push(HEXLOWER.encode(digest)),
                SupportRef::Receipt { run_id, .. } => request.receipt_run_uuids.push(*run_id),
                SupportRef::Artifact(_) => {}
            }
        }
    }
    request
}

/// `prepare`, inside a caller's transaction: the manifest's bytes and bound, the already-prepared
/// short-circuit, the completion and Policy re-checks, the render, the candidate Artifact, the
/// manifest row, and the read-back -- everything but the authentication and the commit.
fn prepare_within(
    transaction: &Transaction<'_>,
    workspace_id: Uuid,
    recorded_at_ms: u64,
    manifest: &OutcomeManifestV1,
    sources: &VerifiedSources,
) -> Result<OutcomeCandidate, PrepareRefusal> {
    if manifest.workspace_id != workspace_id {
        return Err(PrepareRefusal::CrossWorkspace);
    }
    // The bytes and the bound first, before a transaction is opened and before anything is
    // read: a bound that exists to prevent work must run before the work.
    let canonical_bytes = manifest
        .canonical_bytes()
        .map_err(ManifestStoreRefusal::from)?;
    let observed = canonical_bytes.len() as u64;
    if observed > MAX_STORED_MANIFEST_BYTES {
        return Err(ManifestStoreRefusal::ManifestTooLarge {
            configured: MAX_STORED_MANIFEST_BYTES,
            observed,
        }
        .into());
    }
    let manifest_digest = HEXLOWER.encode(&sha256(&canonical_bytes));
    let manifest_id = manifest_reference_id(&manifest_digest)?;

    // Already prepared. Read it back through the verified reader rather than trusting the
    // row that answered the existence question, and write nothing.
    match verified_manifest_within(transaction, workspace_id, &manifest_digest) {
        Ok(stored) => {
            return Ok(OutcomeCandidate {
                manifest_id: stored.manifest_id(),
                manifest_digest,
                candidate_artifact_digest: stored.candidate_artifact_digest().to_owned(),
            });
        }
        Err(ManifestStoreRefusal::Absent) => {}
        Err(refusal) => return Err(refusal.into()),
    }

    let completion = read_completion(transaction, workspace_id, manifest)?;
    recheck_policy(transaction, workspace_id, manifest, completion.mission_id)?;

    let markdown = render(manifest, sources)?;
    let candidate = record_artifact_within(
        transaction,
        workspace_id,
        recorded_at_ms,
        ArtifactKind::OutcomeDocument,
        markdown.as_bytes(),
        ArtifactContext::Mission {
            project_id: completion.project_id,
            mission_id: completion.mission_id,
        },
        completion.accountable_resident_id,
        completion.accountable_resident_configuration_id,
        None,
    )?;
    let candidate_artifact_digest = candidate.digest().value.clone();

    insert_manifest(
        transaction,
        workspace_id,
        manifest,
        &canonical_bytes,
        &manifest_digest,
        manifest_id,
        &candidate_artifact_digest,
    )?;

    // Read the record back through the verified reader, inside the transaction, before
    // committing -- `record_completion`'s rule. Without it a write can leave the store in a
    // state its own reader refuses, and the caller is told it succeeded.
    let stored = verified_manifest_within(transaction, workspace_id, &manifest_digest)?;
    let candidate = OutcomeCandidate {
        manifest_id: stored.manifest_id(),
        manifest_digest: stored.digest().value.clone(),
        candidate_artifact_digest: stored.candidate_artifact_digest().to_owned(),
    };
    Ok(candidate)
}

/// The completion this manifest is frozen on, read through its owning verified reader, and then
/// reconciled against what the manifest says about it.
///
/// The Mission is compared by identity -- `mission_id` against the completion's -- because
/// that is all a Mission has (R18.4).
///
/// The reconciliation spans two records, so it belongs to neither: the completion's reader proves
/// the completion, and the manifest's codec proves the manifest, and nobody else checks that the
/// manifest's copy of the boundary, the accountable Resident, the Mission and the recorded time is
/// the completion's.
fn read_completion(
    connection: &Connection,
    workspace_id: Uuid,
    manifest: &OutcomeManifestV1,
) -> Result<MissionCompletionV1, PrepareRefusal> {
    if manifest.completion.kind != SourceKind::Completion {
        return Err(PrepareRefusal::Irreconcilable {
            detail: format!(
                "the manifest's completion reference is a {}",
                manifest.completion.kind.as_str()
            ),
        });
    }
    // The whole envelope, not the value alone (R15, and astra's A5). A citation that names the
    // right 64 hex characters under a canonicalizer or a schema version its owner does not have is
    // different evidence, not the same evidence spelled differently -- and both were accepted:
    // `beacon.mission_completion.v999` and schema version 999 both prepared successfully. The
    // digest value is proved by the owning reader below; these two are the parts no reader saw.
    if manifest.completion.digest.algorithm != DIGEST_ALGORITHM
        || manifest.completion.digest.canonicalizer != COMPLETION_CANONICALIZER
    {
        return Err(PrepareRefusal::Irreconcilable {
            detail: format!(
                "the completion citation declares envelope {}:{} and a completion is \
                 {DIGEST_ALGORITHM}:{COMPLETION_CANONICALIZER}",
                manifest.completion.digest.algorithm, manifest.completion.digest.canonicalizer
            ),
        });
    }
    // A completion carries its reference identity inside its bytes, so the citation must carry
    // one too; the codec already refused a manifest without it, and this names the seam if a
    // decoded value ever reaches here otherwise.
    let Some(completion_identity) = manifest.completion.identity else {
        return Err(PrepareRefusal::Irreconcilable {
            detail: "the completion citation carries no reference identity".to_owned(),
        });
    };
    if completion_identity.schema_version != COMPLETION_SCHEMA_VERSION {
        return Err(PrepareRefusal::Irreconcilable {
            detail: format!(
                "the completion citation declares schema version {} and a completion is version \
                 {COMPLETION_SCHEMA_VERSION}",
                completion_identity.schema_version
            ),
        });
    }
    let record =
        verified_completion_within(connection, workspace_id, &manifest.completion.digest.value)?;
    reconcile_completion(manifest, &record)
        .map_err(|detail| PrepareRefusal::Irreconcilable { detail })?;
    Ok(record)
}

/// What the manifest says about its completion, against what the completion says.
///
/// Split out for astra's A3: the startup pass verified the completion record ALONE and skipped
/// this, so a manifest whose copies of the boundary, the accountable Resident, the Mission or the
/// recorded time disagreed with the completion -- refused by `prepare` -- survived a restart once
/// it was in the store. `prepare` and `verify_manifest_state` now call the same function, because
/// a rule enforced on the write path and not on the read path is a rule a planted row does not
/// have to follow.
///
/// It spans two records and so belongs to neither owner: the completion's reader proves the
/// completion and the manifest's codec proves the manifest, and nobody else checks that the second
/// is talking about the first.
fn reconcile_completion(
    manifest: &OutcomeManifestV1,
    record: &MissionCompletionV1,
) -> Result<(), String> {
    let mismatches: [(&str, String, String); 6] = [
        (
            "completion reference identity",
            manifest
                .completion
                .identity
                .map(|identity| identity.object_id.to_string())
                .unwrap_or_default(),
            record.completion_id.to_string(),
        ),
        (
            "mission",
            manifest.mission_id.to_string(),
            record.mission_id.to_string(),
        ),
        (
            "accountable Resident",
            manifest.accountable_resident_id.to_string(),
            record.accountable_resident_id.to_string(),
        ),
        (
            "boundary event",
            manifest.boundary_event_seq.to_string(),
            record.boundary_event_seq.to_string(),
        ),
        (
            "completion recorded time",
            manifest.completion_recorded_at_ms.to_string(),
            record.completed_at_ms.to_string(),
        ),
        (
            "workspace",
            manifest.workspace_id.to_string(),
            record.workspace_id.to_string(),
        ),
    ];
    for (what, claimed, actual) in mismatches {
        if claimed != actual {
            return Err(format!(
                "the manifest names {what} {claimed} and the completion names {actual}"
            ));
        }
    }

    // The delivered set, against the completion's own (the record calls it a set). A manifest
    // that says the completion delivered an Artifact its bytes never cited -- or none where it
    // cited some -- was accepted here and became a durable claim (sol1's cold read of the
    // assembler, P0-1). Only the completion's bytes decide what was delivered; the manifest's
    // list is a copy, and this is where the copy is proved, on the write path and the startup
    // pass alike. `Verified([])` and `Absent` both claim nothing, which is one claim, not two.
    let claimed: BTreeSet<String> = match &manifest.delivered_artifacts {
        OutcomeState::Verified(refs) => refs.iter().map(|r| r.digest.value.clone()).collect(),
        OutcomeState::Absent(_) | OutcomeState::Broken(_) | OutcomeState::NotEvaluated(_) => {
            BTreeSet::new()
        }
    };
    let actual: BTreeSet<String> = record
        .delivered_artifacts
        .iter()
        .map(|digest| HEXLOWER.encode(digest))
        .collect();
    if claimed != actual {
        let list = |set: &BTreeSet<String>| set.iter().cloned().collect::<Vec<_>>().join(", ");
        return Err(format!(
            "the manifest names delivered Artifacts [{}] and the completion names [{}]",
            list(&claimed),
            list(&actual)
        ));
    }
    Ok(())
}

/// Prove the manifest's copy of the high-impact bit against the Policy classification it cites.
///
/// `recheck_copied_bit` is the classification module's own instrument: the copy and the check are
/// one call, so a caller cannot obtain the bit without the check having run (R13.7).
fn recheck_policy(
    connection: &Connection,
    workspace_id: Uuid,
    manifest: &OutcomeManifestV1,
    completed_mission_id: Uuid,
) -> Result<(), PrepareRefusal> {
    if manifest.policy_classification.kind != SourceKind::Policy {
        return Err(PrepareRefusal::Irreconcilable {
            detail: format!(
                "the manifest's policy reference is a {}",
                manifest.policy_classification.kind.as_str()
            ),
        });
    }
    // Not one of astra's findings, and the same hole in the same shape one function away: the
    // Policy citation's envelope was unchecked too. `verified_classification_for_citation` proves
    // the id and the version, so only the envelope was open. Fixed here rather than left as a known
    // instance of a class being closed elsewhere in the same commit.
    if manifest.policy_classification.digest.algorithm != DIGEST_ALGORITHM
        || manifest.policy_classification.digest.canonicalizer != MISSION_IMPACT_CANONICALIZER
    {
        return Err(PrepareRefusal::Irreconcilable {
            detail: format!(
                "the Policy citation declares envelope {}:{} and a classification is \
                 {DIGEST_ALGORITHM}:{MISSION_IMPACT_CANONICALIZER}",
                manifest.policy_classification.digest.algorithm,
                manifest.policy_classification.digest.canonicalizer
            ),
        });
    }
    let Some(policy_identity) = manifest.policy_classification.identity else {
        return Err(PrepareRefusal::Irreconcilable {
            detail: "the Policy citation carries no reference identity".to_owned(),
        });
    };
    let record = verified_classification_for_citation(
        connection,
        workspace_id,
        policy_identity.object_id,
        policy_identity.schema_version,
        &manifest.policy_classification.digest.value,
    )?
    .ok_or(PrepareRefusal::ClassificationAbsent)?;
    // The classification must be THIS Mission's. `verified_classification_for_citation` proves the
    // record against its own bytes and its cited identity, and nothing proved the record was about
    // the Mission being completed -- so another Mission's perfectly valid ORDINARY classification
    // let a High-impact Mission prepare a document with the appendix omitted (astra's A4, the same
    // class as sol1's #3). The Mission comes from the classification's canonical bytes, never from
    // a column beside them.
    if record.payload().mission_id != completed_mission_id {
        return Err(PrepareRefusal::Irreconcilable {
            detail: format!(
                "the Policy classification is about Mission {} and the completion is about {}",
                record.payload().mission_id,
                completed_mission_id
            ),
        });
    }
    record.recheck_copied_bit(manifest.policy_high_impact)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_manifest(
    transaction: &Transaction<'_>,
    workspace_id: Uuid,
    manifest: &OutcomeManifestV1,
    canonical_bytes: &[u8],
    manifest_digest: &str,
    manifest_id: Uuid,
    candidate_artifact_digest: &str,
) -> Result<(), ManifestStoreRefusal> {
    transaction
        .execute(
            "INSERT INTO outcome_manifests (
                 workspace_id, digest_algorithm, digest_canonicalizer, manifest_digest,
                 canonical_bytes, manifest_id, mission_id, completion_digest,
                 canonicalizer_version, renderer_version, boundary_event_seq,
                 candidate_artifact_digest
             ) VALUES (?1, 'sha256', ?11, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                       ?9, ?10)",
            params![
                workspace_id.to_string(),
                manifest_digest,
                canonical_bytes,
                manifest_id.to_string(),
                manifest.mission_id.to_string(),
                manifest.completion.digest.value.as_str(),
                manifest.canonicalizer_version.as_str(),
                manifest.renderer_version.as_str(),
                i64::try_from(manifest.boundary_event_seq).unwrap_or(i64::MAX),
                candidate_artifact_digest,
                // The digest envelope's canonicalizer is the manifest's own, from its bytes -- the
                // codec pins it to `CANONICALIZER`, so a literal here was a second copy of that
                // token, and the grammar revision is exactly when a second copy drifts.
                manifest.canonicalizer_version.as_str(),
            ],
        )
        .map_err(store_error)?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Startup: every stored manifest is accounted for by the ledger (R18.7)
// ---------------------------------------------------------------------------------------------

/// Re-proves every stored manifest against its own bytes AND against the log, at startup.
///
/// Re-proving on every read is not the invariant the other stores hold. A row inserted behind the
/// ledger -- self-consistent bytes, a digest they hash to, projections that repeat them, every
/// foreign key satisfied -- reads clean through [`verified_manifest_within`], and measurably did:
/// `foreign_key_violations: 0`, `status: Ready`. This pass is what refuses it, by name.
///
/// **It enumerates `outcome_manifests` by the manifest's own key.** Enumerating the candidate
/// Artifacts instead and asking each for its manifest would visit only rows the ledger already
/// reaches, which is the one set that never contained the orphan -- a startup pass keyed on the
/// wrong column is a pass that cannot fail (sol1's E-1 P0, not repeated here).
///
/// What "accounted for" means, given there is no `outcome.prepared` Event kind (R18.6): the
/// manifest's candidate Artifact must exist in the manifest's Workspace, verify against its own
/// bytes, sit at exactly one `artifact.recorded` Event, and actually BE this manifest's document --
/// an Outcome Document scoped to the manifest's Mission and authored by the manifest's accountable
/// Resident. Without those last three, a manifest could name any Artifact in the Workspace and
/// borrow its Event as a position.
/// Which fault the manifest startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
///
/// `ManifestStoreRefusal` says what is wrong with a manifest and never which one: the reader is
/// handed the digest, so it has no reason to repeat it. The row identity is added HERE, at the
/// pass, where the loop knows it.
pub(crate) enum ManifestStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One row did not re-prove. `cause` is the owning reader's OWN refusal, carried whole.
    Row {
        workspace_id: Uuid,
        manifest_digest: String,
        cause: ManifestStoreRefusal,
    },
    /// A row whose Workspace column is not a UUID. Unreachable while the passes before this one
    /// hold -- `workspace_id` is a foreign key into `workspaces`, and the foreign-key check and
    /// the Workspace pass both run earlier -- but reported as what it is rather than folded into
    /// a row refusal that would have to invent the identity it is missing.
    Identity {
        workspace_id: String,
        manifest_digest: String,
    },
}

impl ManifestStoreRefusal {
    /// The variant's NAME, for R28.4's one-line startup message. `Irreconcilable { detail }`,
    /// `Malformed(..)` and `CandidateNotAccounted { reason }` all carry sentences; none of them
    /// belongs in a refusal line. Exhaustive, so a new refusal cannot be added without deciding
    /// what startup calls it.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Absent => "Absent",
            Self::CrossWorkspace => "CrossWorkspace",
            Self::BytesDisagree => "BytesDisagree",
            Self::ColumnDisagrees { .. } => "ColumnDisagrees",
            Self::ManifestTooLarge { .. } => "ManifestTooLarge",
            Self::Malformed(_) => "Malformed",
            Self::Irreconcilable { .. } => "Irreconcilable",
            Self::CandidateNotAccounted { .. } => "CandidateNotAccounted",
            Self::Store(_) => "Store",
        }
    }
}

impl fmt::Display for ManifestStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried refusal's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the Outcome manifests could not be read"),
            Self::Row {
                workspace_id,
                manifest_digest,
                cause,
            } => write!(
                formatter,
                "manifest {manifest_digest} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Identity {
                workspace_id,
                manifest_digest,
            } => write!(
                formatter,
                "a manifest row's Workspace is not readable: {workspace_id}, manifest \
                 {manifest_digest}"
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for ManifestStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Row {
                workspace_id,
                manifest_digest,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("manifest_digest", manifest_digest)
                .field("cause", cause)
                .finish(),
            Self::Identity {
                workspace_id,
                manifest_digest,
            } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .field("manifest_digest", manifest_digest)
                .finish(),
        }
    }
}

impl std::error::Error for ManifestStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            Self::Identity { .. } => None,
        }
    }
}

/// The rows `verify_manifest_state` enumerates, in the order it enumerates them.
///
/// EXTRACTED SO A BAR CAN CALL IT (R44.6): a placement assertion that COPIES this query agrees
/// with itself, so flipping the pass's own `ORDER BY` leaves it decorative. One function, called
/// by both, moves the pass and the bar's view of it together.
pub fn enumerate_manifests(connection: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "SELECT workspace_id, manifest_digest FROM outcome_manifests
         ORDER BY workspace_id, manifest_digest",
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn verify_manifest_state(connection: &Connection) -> Result<(), ManifestStateFault> {
    let rows: Vec<(String, String)> =
        enumerate_manifests(connection).map_err(ManifestStateFault::Store)?;

    for (workspace_id, manifest_digest) in &rows {
        let parsed = Uuid::parse_str(workspace_id).map_err(|_| ManifestStateFault::Identity {
            workspace_id: workspace_id.clone(),
            manifest_digest: manifest_digest.clone(),
        })?;
        verify_one_manifest(connection, parsed, manifest_digest).map_err(|cause| {
            ManifestStateFault::Row {
                workspace_id: parsed,
                manifest_digest: manifest_digest.clone(),
                cause,
            }
        })?;
    }
    Ok(())
}

/// One stored manifest, re-proved. Unchanged from what the loop used to hold inline: keeping the
/// reader's own error type here is what lets the pass add the row identity around it without
/// rewriting a single check.
fn verify_one_manifest(
    connection: &Connection,
    workspace_id: Uuid,
    manifest_digest: &str,
) -> Result<(), ManifestStoreRefusal> {
    // The bytes first, through the same reader every caller uses: a row that cannot prove
    // itself is refused before anything is asked of the log about it.
    let stored = verified_manifest_within(connection, workspace_id, manifest_digest)?;
    let manifest = stored.manifest();

    // The completion the manifest is frozen on, through its owning reader. The row's foreign
    // key to `mission_completions` is single-column -- that key has no Workspace to be
    // composite over -- so this is where a manifest naming another Workspace's completion is
    // caught.
    let completion =
        verified_completion_within(connection, workspace_id, &manifest.completion.digest.value)
            .map_err(|_| ManifestStoreRefusal::CandidateNotAccounted {
                reason: "the completion this manifest names does not verify in its Workspace",
            })?;
    // The SAME reconciliation `prepare` requires. Verifying the completion alone proves the
    // completion; it proves nothing about whether this manifest is describing it.
    reconcile_completion(manifest, &completion)
        .map_err(|detail| ManifestStoreRefusal::Irreconcilable { detail })?;

    let candidate_digest = stored.candidate_artifact_digest();
    let candidate =
        crate::artifact::verified_artifact_within(connection, workspace_id, candidate_digest)
            .map_err(|_| ManifestStoreRefusal::CandidateNotAccounted {
                reason: "the candidate Artifact does not verify against its own bytes",
            })?
            .ok_or(ManifestStoreRefusal::CandidateNotAccounted {
                reason: "no candidate Artifact in this Workspace carries that digest",
            })?;

    // Exactly one `artifact.recorded` Event names it, in this Workspace, and it is the cursor
    // (masked today: the artifact pass at lib.rs ~1130 refuses a second recording Event before
    // this arm runs, so no test can reach it while that ordering holds -- kept for when it changes;
    // kimi's read, D-kimi-read-o3a.md P2)
    // the Artifact itself carries. One is the count: none is an unaccounted row, and two would
    // make "where this entered the ledger" a question with two answers.
    let mut statement = connection
        .prepare(
            "SELECT count(*) FROM events
                 WHERE workspace_id = ?1 AND kind = 'artifact.recorded'
                   AND json_extract(payload_json, '$.artifact.digest.value') = ?2",
        )
        .map_err(store_error)?;
    let events: i64 = statement
        .query_row(params![workspace_id.to_string(), candidate_digest], |row| {
            row.get(0)
        })
        .map_err(store_error)?;
    drop(statement);
    if events != 1 {
        return Err(ManifestStoreRefusal::CandidateNotAccounted {
            reason: "the candidate Artifact is not named by exactly one artifact.recorded event",
        });
    }

    // ...and the Artifact is THIS manifest's document, not merely some Artifact the log
    // happens to reach. Every field here lives inside the Artifact's canonical bytes, so none
    // of it is a projection and none of it needs a re-render.
    let payload = candidate.payload();
    if payload.kind != ArtifactKind::OutcomeDocument {
        return Err(ManifestStoreRefusal::CandidateNotAccounted {
            reason: "the candidate Artifact is not an Outcome Document",
        });
    }
    match payload.context {
        ArtifactContext::Mission { mission_id, .. } if mission_id == manifest.mission_id => {}
        _ => {
            return Err(ManifestStoreRefusal::CandidateNotAccounted {
                reason: "the candidate Artifact is not scoped to this manifest's Mission",
            });
        }
    }
    if payload.accountable_resident_id != manifest.accountable_resident_id {
        return Err(ManifestStoreRefusal::CandidateNotAccounted {
            reason: "the candidate Artifact's accountable Resident is not the manifest's",
        });
    }

    // The CONTENT binding is not repeated here: `verified_manifest_within` above already made
    // it, on this exact row, and one rule with two implementations is two rules.
    Ok(())
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()`; see the note in `outcome_publication`'s twin.
    fn expected_name(refusal: &ManifestStoreRefusal) -> &'static str {
        match refusal {
            ManifestStoreRefusal::Absent => "Absent",
            ManifestStoreRefusal::CrossWorkspace => "CrossWorkspace",
            ManifestStoreRefusal::BytesDisagree => "BytesDisagree",
            ManifestStoreRefusal::ColumnDisagrees { .. } => "ColumnDisagrees",
            ManifestStoreRefusal::ManifestTooLarge { .. } => "ManifestTooLarge",
            ManifestStoreRefusal::Malformed(_) => "Malformed",
            ManifestStoreRefusal::Irreconcilable { .. } => "Irreconcilable",
            ManifestStoreRefusal::CandidateNotAccounted { .. } => "CandidateNotAccounted",
            ManifestStoreRefusal::Store(_) => "Store",
        }
    }

    #[test]
    fn every_manifest_refusal_arm_has_its_own_pinned_name() {
        let every_arm = [
            ManifestStoreRefusal::Absent,
            ManifestStoreRefusal::CrossWorkspace,
            ManifestStoreRefusal::BytesDisagree,
            ManifestStoreRefusal::ColumnDisagrees { column: "anything" },
            ManifestStoreRefusal::ManifestTooLarge {
                configured: 1,
                observed: 2,
            },
            ManifestStoreRefusal::Malformed(crate::outcome::MalformedOutcome::Magic),
            ManifestStoreRefusal::Irreconcilable {
                detail: "anything at all".to_owned(),
            },
            ManifestStoreRefusal::CandidateNotAccounted { reason: "anything" },
            ManifestStoreRefusal::Store("anything at all".to_owned()),
        ];
        assert_eq!(
            every_arm.len(),
            9,
            "an arm was added to `ManifestStoreRefusal` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(ManifestStoreRefusal::name).collect();
        assert_eq!(
            names.len(),
            9,
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
