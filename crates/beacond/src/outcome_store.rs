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
//! **The Mission's digest field is NOT verified, and cannot be (R18.4).** `main` has no
//! `MissionDeclarationV1`, no Mission canonical bytes and no Mission digest, so nothing exists to
//! prove a Mission declaration digest against -- and slice 1's `mission: SourceRef`, which requires
//! a digest, over-generalized a design that cites the Mission by identity. Until the R18.3-5
//! grammar revision removes the field, this module refuses to let it carry a fabrication: a Mission
//! citation must declare itself identity-only, through the canonicalizer token
//! [`MISSION_IDENTITY_ONLY`] and the completion digest that R18.4 names as its verified anchor.
//! The renderer already prints a citation's canonicalizer beside its value, so a reader of the
//! document sees the token and knows no Mission declaration was hashed. `prepare` and the verified
//! reader both enforce it, so neither a caller nor a row planted behind them can produce a
//! Mission digest that is presented as verified.
//!
//! **There is no `outcome.prepared` Event kind (R18.6).** A prepared manifest is positioned in the
//! log by its candidate Artifact's `artifact.recorded` Event -- the Artifact the row's foreign key
//! requires -- so the ledger accounts for every row without a new kind and without rebuilding the
//! closed `events` table. A manifest cannot exist without a candidate by construction (they are
//! written in one transaction, and the foreign key is `NOT NULL`);
//! `a_manifest_cannot_exist_without_its_candidate` says so, because R18.6 revisits this only if
//! one ever can. `verify_manifest_state` is what turns that position into an invariant (R18.7).

use std::sync::Arc;

use data_encoding::HEXLOWER;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::artifact::{
    ArtifactContext, ArtifactError, ArtifactKind, VerifiedArtifactRecord, record_artifact_within,
};
use crate::mission_completion::MissionCompletionV1;
use crate::mission_completion_store::{CompletionReadRefusal, verified_completion_within};
use crate::mission_impact::{ClassificationStoreError, verified_classification_for_citation};
use crate::operator_account::{OperatorAccountError, OperatorAccountService};
use crate::outcome::{
    MalformedOutcome, OutcomeManifestV1, OutcomeRefusal, SourceDigest, SourceKind, VerifiedSources,
    render,
};
use crate::proposal::sha256;

const DIGEST_ALGORITHM: &str = "sha256";

/// The canonicalizer a Mission citation must declare, because there is nothing to hash (R18.4).
///
/// A `SourceRef` requires a digest and a Mission has none, so the question is only what the field
/// is allowed to say. It says this: the value beside it is not a Mission declaration digest, it is
/// the completion digest that anchors the Mission citation, and the Mission itself is cited by
/// `object_id`. The token lives in the manifest's canonical bytes and is rendered beside the value,
/// so the claim travels with the document rather than living in a comment beside the code.
///
/// It is temporary by construction. The R18.3-5 grammar revision removes the digest from the
/// Mission citation entirely, and this constant goes with it.
pub const MISSION_IDENTITY_ONLY: &str = "beacon.outcome.mission-identity-only.v1";

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
    /// The Mission citation claims a digest instead of declaring itself identity-only (R18.4).
    ///
    /// Nothing on `main` can verify a Mission declaration digest, so a citation that carries one
    /// is asserting something no reader can check -- and the renderer prints it beneath "Every
    /// fact below was verified". Refused here rather than rendered.
    MissionCitationNotIdentityOnly,
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
            Self::MissionCitationNotIdentityOnly => write!(
                formatter,
                "the Mission citation must be identity-only: canonicalizer `{MISSION_IDENTITY_ONLY}` \
                 over the completion digest, because no Mission declaration digest exists to verify"
            ),
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
        }
    }
}

impl std::error::Error for PrepareRefusal {}

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

/// A Mission citation may not carry a digest anything would have to take on trust (R18.4).
///
/// One function, called by both `prepare` and the verified reader, so a caller and a row planted
/// behind the caller are held to the same rule. It is a check on the manifest's own bytes and
/// needs no store.
fn require_mission_identity_citation(
    manifest: &OutcomeManifestV1,
) -> Result<(), ManifestStoreRefusal> {
    let digest = &manifest.mission.digest;
    if digest.algorithm != DIGEST_ALGORITHM
        || digest.canonicalizer != MISSION_IDENTITY_ONLY
        || digest.value != manifest.completion.digest.value
    {
        return Err(ManifestStoreRefusal::MissionCitationNotIdentityOnly);
    }
    Ok(())
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
    // Before any column is compared: a row whose Mission citation asserts an unverifiable digest is
    // not a differently-shaped record, it is one this module refuses to answer from.
    require_mission_identity_citation(&manifest)?;

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
            manifest.mission.object_id.to_string(),
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
        if manifest.workspace_id != self.workspace_id {
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
        require_mission_identity_citation(manifest)?;
        let manifest_digest = HEXLOWER.encode(&sha256(&canonical_bytes));
        let manifest_id = manifest_reference_id(&manifest_digest)?;

        self.accounts.with_authenticated_store(|connection, now| {
            let recorded_at_ms = u64::try_from(now).map_err(|_| {
                PrepareRefusal::Manifest(ManifestStoreRefusal::Store(
                    "the service clock is before the Unix epoch".to_owned(),
                ))
            })?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

            // Already prepared. Read it back through the verified reader rather than trusting the
            // row that answered the existence question, and write nothing.
            match verified_manifest_within(&transaction, self.workspace_id, &manifest_digest) {
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

            let completion = read_completion(&transaction, self.workspace_id, manifest)?;
            recheck_policy(&transaction, self.workspace_id, manifest)?;

            let markdown = render(manifest, sources)?;
            let candidate = record_artifact_within(
                &transaction,
                self.workspace_id,
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
                &transaction,
                self.workspace_id,
                manifest,
                &canonical_bytes,
                &manifest_digest,
                manifest_id,
                &candidate_artifact_digest,
            )?;

            // Read the record back through the verified reader, inside the transaction, before
            // committing -- `record_completion`'s rule. Without it a write can leave the store in a
            // state its own reader refuses, and the caller is told it succeeded.
            let stored =
                verified_manifest_within(&transaction, self.workspace_id, &manifest_digest)?;
            let candidate = OutcomeCandidate {
                manifest_id: stored.manifest_id(),
                manifest_digest: stored.digest().value.clone(),
                candidate_artifact_digest: stored.candidate_artifact_digest().to_owned(),
            };
            transaction.commit()?;
            Ok(candidate)
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

/// The completion this manifest is frozen on, read through its owning verified reader, and then
/// reconciled against what the manifest says about it.
///
/// **The Mission is proved by IDENTITY here, not by digest, and R18.4 says that is the design.**
/// There is no `MissionDeclarationV1` in v1 and none is coming: gpt2's design cites the Mission by
/// `mission_id` and anchors it on the completion, and slice 1's `mission: SourceRef` -- which
/// requires a digest -- over-generalized that. So `manifest.mission.digest` is a field this
/// function does not verify and cannot: nothing on `main` produces a Mission declaration digest to
/// verify it against. It is a temporary pass-through, and it goes away with the one grammar
/// revision of `OutcomeManifestV1` that R18.3-5 order (the condition vocabulary widened to the
/// completion's, the Mission cited by identity, and Proposal/Ruling/Evaluation cited as
/// `kind + workspace + digest`) -- a canonicalizer version and a regenerated frozen vector, built
/// by a non-author seat, landing as slice 3b's item 0 before the assembler.
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
            manifest.completion.object_id.to_string(),
            record.completion_id.to_string(),
        ),
        (
            "mission",
            manifest.mission.object_id.to_string(),
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
) -> Result<(), PrepareRefusal> {
    if manifest.policy_classification.kind != SourceKind::Policy {
        return Err(PrepareRefusal::Irreconcilable {
            detail: format!(
                "the manifest's policy reference is a {}",
                manifest.policy_classification.kind.as_str()
            ),
        });
    }
    let record = verified_classification_for_citation(
        connection,
        workspace_id,
        manifest.policy_classification.object_id,
        manifest.policy_classification.schema_version,
        &manifest.policy_classification.digest.value,
    )?
    .ok_or(PrepareRefusal::ClassificationAbsent)?;
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
             ) VALUES (?1, 'sha256', 'beacon.outcome.manifest.v1', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                       ?9, ?10)",
            params![
                workspace_id.to_string(),
                manifest_digest,
                canonical_bytes,
                manifest_id.to_string(),
                manifest.mission.object_id.to_string(),
                manifest.completion.digest.value.as_str(),
                manifest.canonicalizer_version.as_str(),
                manifest.renderer_version.as_str(),
                i64::try_from(manifest.boundary_event_seq).unwrap_or(i64::MAX),
                candidate_artifact_digest,
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
pub(crate) fn verify_manifest_state(connection: &Connection) -> Result<(), ManifestStoreRefusal> {
    let mut statement = connection
        .prepare(
            "SELECT workspace_id, manifest_digest FROM outcome_manifests
             ORDER BY workspace_id, manifest_digest",
        )
        .map_err(store_error)?;
    let rows: Vec<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(store_error)?;
    drop(statement);

    for (workspace_id, manifest_digest) in &rows {
        let workspace_id =
            Uuid::parse_str(workspace_id).map_err(|_| ManifestStoreRefusal::BytesDisagree)?;
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
            ArtifactContext::Mission { mission_id, .. }
                if mission_id == manifest.mission.object_id => {}
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
    }
    Ok(())
}
