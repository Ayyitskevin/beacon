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
    CANONICALIZER, MalformedOutcome, OutcomeManifestV1, OutcomeRefusal, SourceDigest, SourceKind,
    VerifiedSources, render,
};
use crate::proposal::sha256;

const DIGEST_ALGORITHM: &str = "sha256";

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
/// `OutcomeManifestV1` carries no UUID inside its canonical bytes and its grammar is frozen
/// (R12/R13), so a freshly minted UUID stored beside those bytes would be a second identity the
/// bytes do not carry -- exactly the decoy R15 exists to refuse, since nothing in the record could
/// contradict it. This one is the first sixteen bytes of the digest re-spelled, so a citation's
/// UUID and digest cannot disagree: one is a function of the other. Whether the manifest should
/// instead carry a UUID inside its canonical bytes -- a canonicalizer version and a new frozen
/// vector -- is the Steward's call, not this slice's.
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
    manifest_id: String,
    mission_id: String,
    completion_digest: String,
    canonicalizer_version: String,
    renderer_version: String,
    boundary_event_seq: i64,
    candidate_artifact_digest: String,
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
            "SELECT workspace_id, canonical_bytes, manifest_id, mission_id, completion_digest,
                    canonicalizer_version, renderer_version, boundary_event_seq,
                    candidate_artifact_digest
             FROM outcome_manifests WHERE manifest_digest = ?1",
            params![manifest_digest],
            |row| {
                Ok(StoredRow {
                    workspace_id: row.get(0)?,
                    canonical_bytes: row.get(1)?,
                    manifest_id: row.get(2)?,
                    mission_id: row.get(3)?,
                    completion_digest: row.get(4)?,
                    canonicalizer_version: row.get(5)?,
                    renderer_version: row.get(6)?,
                    boundary_event_seq: row.get(7)?,
                    candidate_artifact_digest: row.get(8)?,
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

    Ok(StoredOutcomeManifest {
        manifest_id,
        digest: SourceDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: CANONICALIZER.to_owned(),
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
            return Err(PrepareRefusal::Irreconcilable {
                detail: format!(
                    "the manifest names {what} {claimed} and the completion names {actual}"
                ),
            });
        }
    }
    Ok(record)
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
