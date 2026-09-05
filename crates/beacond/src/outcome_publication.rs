//! The publication relation: which candidate Outcome one Evaluation qualified, and when it was
//! version n of a Mission's publication history (ADR 0040, gpt2 design section 1, kimi's slice-3
//! read section 2).
//!
//! # Why the relation is external
//!
//! The candidate is content-addressed and the Evaluation is about its exact digest. Putting the
//! Evaluation inside the candidate would change the candidate's bytes, so it would change the
//! digest the Evaluation names -- an impossible self-reference. Putting the candidate inside the
//! Evaluation would make the document a field of its own review. The pairing has to live beside
//! both, and the row's append IS the publication event.
//!
//! # What publication is not
//!
//! A `PublishedOutcome` is a document with an immutable history. Its existence is not Mission
//! completion, not permission to proceed, and not a success verdict (ADR 0040). Nothing in this
//! module writes to a Mission, a completion, or an authority surface, and the type carries no
//! predicate a caller could branch on to derive one: there is no `is_successful`, no `is_complete`,
//! and no `may_proceed`. A caller that wants to know whether a Mission completed reads the
//! completion record through its own owner.

use std::fmt;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use uuid::Uuid;

use crate::artifact::{
    ArtifactContext, ArtifactDigest, ArtifactError, VerifiedArtifactRecord, supersession_tail,
    verified_artifact_within,
};
use crate::evaluation::{
    EvaluationDigest, EvaluationError, EvaluationSubject, Verdict, verified_evaluation_within,
};
use crate::outcome::AppendixMode;
use crate::outcome_store::{ManifestStoreRefusal, StoredOutcomeManifest, verified_manifest_within};
use crate::publication_seam::{PublicationSeamRefusal, publication_candidacy};

/// When a fact was proved -- as a value, so a surface cannot quietly promote one to the other.
///
/// R12.10's two links are two different questions, and the answer to each is only as good as the
/// moment it was asked. A boundary fact is not stale and it is not current either: it is exactly
/// what was true at `boundary_event_seq`, which is the whole reason the Outcome freezes there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Proved when the manifest was assembled, at the completion's boundary, and stored. This read
    /// re-proved the manifest's own bytes and digest; it did NOT re-read the cited domain objects,
    /// and saying otherwise would be the relabelling R12.10 forbids.
    VerifiedAtBoundary { boundary_event_seq: u64 },
    /// Derived on this read, from the store as it is right now. True when it was asked and not a
    /// moment longer.
    DerivedAtReadTime,
}

impl fmt::Display for Provenance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VerifiedAtBoundary { boundary_event_seq } => {
                write!(formatter, "verified at boundary event {boundary_event_seq}")
            }
            Self::DerivedAtReadTime => formatter.write_str("derived at read time"),
        }
    }
}

/// Where each chain stands NOW. Beside the cited digests, never in place of them (R12.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersessionAtReadTime {
    candidate: Vec<ArtifactDigest>,
    evaluation: Option<EvaluationDigest>,
}

impl SupersessionAtReadTime {
    /// The candidate's chain in chain order, the cited digest FIRST. A one-element chain means
    /// nothing has superseded the published candidate.
    pub fn candidate_chain(&self) -> &[ArtifactDigest] {
        &self.candidate
    }

    /// The candidate chain's tail: the newest Artifact in the chain, which is the cited one when
    /// nothing has superseded it.
    pub fn candidate_tail(&self) -> &ArtifactDigest {
        self.candidate
            .last()
            .expect("a chain always contains the digest it started from")
    }

    /// The Evaluation chain's tail, or `None` when the cited Evaluation is itself the tail.
    pub fn evaluation_tail(&self) -> Option<&EvaluationDigest> {
        self.evaluation.as_ref()
    }

    /// Always `DerivedAtReadTime`. A method rather than a constant so the label travels with the
    /// value a caller actually holds.
    pub fn provenance(&self) -> Provenance {
        Provenance::DerivedAtReadTime
    }
}

/// One published Outcome, re-proved on every read.
///
/// `get` re-verifies the manifest's exact stored bytes and digest, the candidate Artifact's exact
/// bytes and digest, the Evaluation's identity/digest/subject, and this relation row. It does NOT
/// re-read every cited domain object: those stay labelled `VerifiedAtBoundary` (ADR 0040).
#[derive(Debug, Clone)]
pub struct PublishedOutcome {
    publication_id: Uuid,
    mission_id: Uuid,
    publication_version: u32,
    publication_version_count: u32,
    manifest: StoredOutcomeManifest,
    candidate: VerifiedArtifactRecord,
    cited_candidate_artifact_digest: ArtifactDigest,
    cited_evaluation_digest: EvaluationDigest,
    supersession: SupersessionAtReadTime,
}

impl PublishedOutcome {
    pub fn publication_id(&self) -> Uuid {
        self.publication_id
    }

    /// Identity only. The Mission's title is not here and never will be: this row is a relation
    /// between identities, and a renderable Mission fact is read through the Mission's owner.
    pub fn mission_id(&self) -> Uuid {
        self.mission_id
    }

    /// 1 for the first publication of a Mission, n+1 for each correction. Contiguous.
    pub fn publication_version(&self) -> u32 {
        self.publication_version
    }

    /// How many versions this Mission's publication history holds, as of this read (R24.2).
    ///
    /// Beside `publication_version`, never instead of it: "version 2" alone cannot tell a reader
    /// whether they are holding the current Outcome or a superseded one, and a corrected Outcome
    /// that reads as the only one is the same misreading R12.10 forbids one layer down. Read-time
    /// state -- a later correction changes it and changes nothing about this version.
    pub fn publication_version_count(&self) -> u32 {
        self.publication_version_count
    }

    /// True when this version is the newest the Mission has, as of this read.
    pub fn is_current_version(&self) -> bool {
        self.publication_version == self.publication_version_count
    }

    /// The manifest, re-proved against its own stored bytes on this read.
    pub fn manifest(&self) -> &StoredOutcomeManifest {
        &self.manifest
    }

    /// The candidate Artifact, re-proved against its own stored bytes on this read. Its `content()`
    /// is the exact published Markdown.
    pub fn candidate(&self) -> &VerifiedArtifactRecord {
        &self.candidate
    }

    /// The candidate digest the relation row CITES -- the bytes that were published and evaluated.
    /// Not the chain tail: a corrected Outcome does not retroactively become the document an
    /// earlier Evaluation approved.
    pub fn cited_candidate_artifact_digest(&self) -> &ArtifactDigest {
        &self.cited_candidate_artifact_digest
    }

    /// The Evaluation digest the relation row CITES, re-verified on this read.
    pub fn cited_evaluation_digest(&self) -> &EvaluationDigest {
        &self.cited_evaluation_digest
    }

    /// Where both chains stand now -- read-time state, beside the cited digests above (R12.10).
    pub fn supersession_at_read_time(&self) -> &SupersessionAtReadTime {
        &self.supersession
    }

    /// What the manifest's citations are: facts proved at the completion's boundary. A caller
    /// rendering the appendix labels them with THIS, not with "verified now".
    pub fn citation_provenance(&self) -> Provenance {
        Provenance::VerifiedAtBoundary {
            boundary_event_seq: self.manifest.manifest().boundary_event_seq,
        }
    }
}

/// One published Outcome's exact Markdown, as the store holds it.
///
/// The bytes are COPIED from the stored candidate Artifact, never re-rendered: an export that
/// re-ran the renderer would be a second path to the document, and the two would agree only for as
/// long as nobody changed the renderer. What a reader gets here is what was published and what the
/// qualifying Evaluation was about.
///
/// `digest` is the CITED candidate digest -- the bytes that were evaluated -- and not the chain
/// tail; `publication_version_count` rides along for R24.2's reason, because "version 2" alone
/// cannot tell a caller whether a version 3 exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedOutcome {
    bytes: Vec<u8>,
    digest: ArtifactDigest,
    publication_version: u32,
    publication_version_count: u32,
    media_type: String,
}

impl ExportedOutcome {
    /// The exact stored Markdown. Owned, because the wire copies it anyway.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The digest of the bytes above: the candidate the relation cites.
    pub fn digest(&self) -> &ArtifactDigest {
        &self.digest
    }

    pub fn publication_version(&self) -> u32 {
        self.publication_version
    }

    /// How many versions the Mission's publication history holds, as of this read (R24.2).
    pub fn publication_version_count(&self) -> u32 {
        self.publication_version_count
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }
}

impl From<PublishedOutcome> for ExportedOutcome {
    fn from(published: PublishedOutcome) -> Self {
        Self {
            bytes: published.candidate().content().to_vec(),
            digest: published.cited_candidate_artifact_digest().clone(),
            publication_version: published.publication_version(),
            publication_version_count: published.publication_version_count(),
            media_type: published.candidate().payload().media_type.clone(),
        }
    }
}

/// One row of a Mission's publication history, carrying what the RELATION itself holds.
///
/// Nothing here needs a manifest decoded or a document verified: drawing a list of versions must
/// not cost one full verification per row (R29.5). The boundary event belongs to the document view,
/// which reads one version through `get_version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationSummary {
    pub publication_id: Uuid,
    pub publication_version: u32,
    pub candidate_artifact_digest: String,
    pub evaluation_digest: String,
    /// True for the highest version this Mission holds, as of this read.
    pub is_current: bool,
}

/// Why a pairing was not published, or why a published one cannot be read back.
#[derive(Debug)]
pub enum PublishRefusal {
    /// The seam refused the pairing itself, in its own words: evaluator equals the candidate's
    /// accountable Resident, the subject is not the candidate, the verdict is not `meets`, the
    /// Mission differs, an envelope disagrees, or a record did not verify.
    Seam(PublicationSeamRefusal),
    /// No manifest in this Workspace was prepared for that candidate Artifact. A candidate that no
    /// manifest points at is Markdown, not an Outcome.
    NoManifestForCandidate,
    /// The manifest store refused, in its own words.
    Manifest(ManifestStoreRefusal),
    /// The manifest exists and is about a different Mission than the one being published.
    ManifestMissionDiffers {
        manifest: Uuid,
        requested: Uuid,
    },
    /// The Mission's completion could not be read back, or says something the manifest does not.
    Completion(String),
    /// Policy classified the Mission high-impact and the candidate does not carry the verification
    /// appendix. There is no caller flag that reaches this state -- `AppendixRequest` has no `omit`
    /// arm -- so it is the store refusing a row that could only come from a tampered candidate.
    AppendixRequired,
    /// This Mission's next publication version is not the one about to be written. A gap would make
    /// "version 3" a claim about a version 2 nobody could produce.
    VersionGap {
        expected: u32,
        found: u32,
    },
    /// The relation already holds this candidate or this Evaluation.
    AlreadyPublished,
    /// The Mission's publication history is not contiguous from 1.
    HistoryNotContiguous {
        expected: u32,
        found: u32,
    },
    /// The qualifying Evaluation's verdict is not `meets`, read back from its own bytes.
    QualifyingVerdictNotMeets,
    /// A record the relation cites is about a different Mission than the relation row names.
    ///
    /// The relation's `mission_id` is an identity column with a foreign key to `missions` -- which
    /// proves the Mission EXISTS, and proves nothing about whether the cited records are its. Every
    /// cited record carries its own Mission inside its own bytes, and this is where the row's claim
    /// is held to them (sol1's cold read, P0).
    MissionBindingBroken {
        what: &'static str,
        named: Uuid,
        relation: Uuid,
    },
    /// No publication exists for that Mission, or not at that version.
    Absent,
    /// A cited record did not verify at read time.
    Unverified(String),
    Store(rusqlite::Error),
}

impl fmt::Display for PublishRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Seam(refusal) => write!(formatter, "{refusal}"),
            Self::NoManifestForCandidate => {
                formatter.write_str("no prepared manifest names that candidate Artifact")
            }
            Self::Manifest(refusal) => write!(formatter, "{refusal}"),
            Self::ManifestMissionDiffers {
                manifest,
                requested,
            } => write!(
                formatter,
                "the candidate's manifest is about Mission {manifest} and publication was \
                 requested for Mission {requested}"
            ),
            Self::Completion(detail) => write!(formatter, "{detail}"),
            Self::AppendixRequired => formatter.write_str(
                "Policy classified this Mission high-impact and the candidate carries no \
                 verification appendix",
            ),
            Self::VersionGap { expected, found } => write!(
                formatter,
                "the next publication version for this Mission is {expected} and {found} was about \
                 to be written"
            ),
            Self::AlreadyPublished => {
                formatter.write_str("that candidate or Evaluation is already published")
            }
            Self::HistoryNotContiguous { expected, found } => write!(
                formatter,
                "this Mission's publication history is not contiguous from 1: version {expected} \
                 is missing and {found} versions are stored"
            ),
            Self::QualifyingVerdictNotMeets => formatter.write_str(
                "the Evaluation this publication cites does not carry a `meets` verdict",
            ),
            Self::MissionBindingBroken {
                what,
                named,
                relation,
            } => write!(
                formatter,
                "the {what} this publication cites is about Mission {named} and the publication is \
                 filed under Mission {relation}"
            ),
            Self::Absent => formatter.write_str("this Mission has no published Outcome"),
            Self::Unverified(detail) => write!(formatter, "{detail}"),
            Self::Store(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PublishRefusal {}

impl From<crate::operator_account::OperatorAccountError> for PublishRefusal {
    fn from(error: crate::operator_account::OperatorAccountError) -> Self {
        Self::Unverified(format!("the store could not be opened: {error}"))
    }
}

impl From<rusqlite::Error> for PublishRefusal {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<PublicationSeamRefusal> for PublishRefusal {
    fn from(refusal: PublicationSeamRefusal) -> Self {
        Self::Seam(refusal)
    }
}

impl From<ManifestStoreRefusal> for PublishRefusal {
    fn from(refusal: ManifestStoreRefusal) -> Self {
        Self::Manifest(refusal)
    }
}

impl From<ArtifactError> for PublishRefusal {
    fn from(error: ArtifactError) -> Self {
        Self::Unverified(format!("the candidate Artifact did not verify: {error}"))
    }
}

impl From<EvaluationError> for PublishRefusal {
    fn from(error: EvaluationError) -> Self {
        Self::Unverified(format!("the Evaluation did not verify: {error}"))
    }
}

/// One relation row, exactly as stored.
struct PublicationRow {
    publication_id: Uuid,
    mission_id: Uuid,
    publication_version: u32,
    candidate_artifact_digest: String,
    evaluation_digest: String,
}

/// Write one publication INSIDE the caller's transaction, or nothing at all.
///
/// Every check reads through an owning verified reader and compares; none of them consults a
/// projection column. The order is chosen so each refusal means one thing.
pub(crate) fn publish_within(
    transaction: &Transaction<'_>,
    workspace_id: Uuid,
    mission_id: Uuid,
    candidate_artifact_digest: &ArtifactDigest,
    evaluation_digest: &EvaluationDigest,
) -> Result<PublishedOutcome, PublishRefusal> {
    // 1. The pairing itself, from the two records' canonical bytes: subject is this exact
    //    candidate, verdict is `meets`, evaluator is not the candidate's accountable Resident, and
    //    the Evaluation's derived Mission is this one -- a field comparison inside the subject arm's
    //    copied envelope, never a join (kimi's section 2). The seam re-proves all of it rather than
    //    trusting the stores' triggers, and returns the envelopes the OWNING modules verified.
    let candidacy = publication_candidacy(
        transaction,
        workspace_id,
        evaluation_digest,
        candidate_artifact_digest,
        mission_id,
    )?;

    // 2. A manifest was prepared for this candidate. `outcome_manifests` holds one candidate per
    //    manifest (its own UNIQUE), so this lookup cannot be ambiguous -- and the manifest is then
    //    read through its verified reader, which re-proves its bytes rather than trusting the row
    //    the lookup came from.
    let manifest_digest: Option<String> = transaction
        .query_row(
            "SELECT manifest_digest FROM outcome_manifests
             WHERE workspace_id = ?1 AND candidate_artifact_digest = ?2",
            params![
                workspace_id.to_string(),
                candidacy.candidate_artifact_digest().value.as_str()
            ],
            |row| row.get(0),
        )
        .optional()?;
    let manifest_digest = manifest_digest.ok_or(PublishRefusal::NoManifestForCandidate)?;
    let stored = verified_manifest_within(transaction, workspace_id, &manifest_digest)?;

    // 3. The manifest is about the Mission being published. The seam proved the EVALUATION's
    //    Mission; this proves the DOCUMENT's, and they are different records.
    if stored.manifest().mission_id != mission_id {
        return Err(PublishRefusal::ManifestMissionDiffers {
            manifest: stored.manifest().mission_id,
            requested: mission_id,
        });
    }

    // 4. The completion the manifest cites is readable and is this Mission's. `prepare` reconciled
    //    the manifest against it on the write path; `publish` re-reads it because a publication is
    //    a durable claim about a completed Mission, and "it verified once" is not a property of the
    //    store now (R5).
    let completion = crate::mission_completion_store::verified_completion_within(
        transaction,
        workspace_id,
        &stored.manifest().completion.digest.value,
    )
    .map_err(|refusal| {
        PublishRefusal::Completion(format!(
            "the completion this manifest is frozen on did not verify: {refusal}"
        ))
    })?;
    if completion.mission_id != mission_id {
        return Err(PublishRefusal::Completion(format!(
            "the cited completion is about Mission {} and publication was requested for Mission \
             {mission_id}",
            completion.mission_id
        )));
    }

    // 5. Appendix policy, re-checked against the CANDIDATE'S OWN BYTES rather than against the
    //    request that produced them. `prepare` resolves the mode and there is no `omit` arm for a
    //    caller to pass, so the only way to reach a high-impact candidate with no appendix is a
    //    document that did not come from this renderer.
    if stored.manifest().policy_high_impact {
        let candidate = verified_artifact_within(
            transaction,
            workspace_id,
            candidacy.candidate_artifact_digest().value.as_str(),
        )?
        .ok_or(PublishRefusal::Seam(
            PublicationSeamRefusal::CandidateAbsent,
        ))?;
        if stored.manifest().appendix_mode() != AppendixMode::Included
            || !appendix_carries_manifest(candidate.content(), stored.canonical_bytes())
        {
            return Err(PublishRefusal::AppendixRequired);
        }
    }

    // 6. The version, contiguous from 1. Read inside this transaction, which is `BEGIN IMMEDIATE`,
    //    so two concurrent publishes cannot both read the same maximum.
    let highest: Option<i64> = transaction
        .query_row(
            "SELECT max(publication_version) FROM outcome_publications
             WHERE workspace_id = ?1 AND mission_id = ?2",
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let next = u32::try_from(highest.unwrap_or(0))
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    if let Some(highest) = highest {
        let contiguous = count_versions(transaction, workspace_id, mission_id)?;
        if i64::from(contiguous) != highest {
            return Err(PublishRefusal::VersionGap {
                expected: contiguous.saturating_add(1),
                found: next,
            });
        }
    }

    let publication_id = Uuid::new_v4();
    transaction
        .execute(
            "INSERT INTO outcome_publications (
                 workspace_id, mission_id, publication_id, candidate_artifact_digest,
                 evaluation_digest, publication_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                workspace_id.to_string(),
                mission_id.to_string(),
                publication_id.to_string(),
                candidacy.candidate_artifact_digest().value.as_str(),
                candidacy.evaluation_digest().value.as_str(),
                i64::from(next),
            ],
        )
        .map_err(|error| match error {
            rusqlite::Error::SqliteFailure(failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                PublishRefusal::AlreadyPublished
            }
            other => PublishRefusal::Store(other),
        })?;

    read_publication_within(
        transaction,
        workspace_id,
        PublicationRow {
            publication_id,
            mission_id,
            publication_version: next,
            candidate_artifact_digest: candidacy.candidate_artifact_digest().value.clone(),
            evaluation_digest: candidacy.evaluation_digest().value.clone(),
        },
    )
}

/// How many versions this Mission actually has. Compared against the maximum: equal means
/// contiguous from 1, and anything else means a gap.
fn count_versions(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<u32, PublishRefusal> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM outcome_publications WHERE workspace_id = ?1 AND mission_id = ?2",
        params![workspace_id.to_string(), mission_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// The appendix carries the manifest's exact bytes as recomputable hex.
///
/// The same property `prepare` proves when it records the candidate, re-proved here from the
/// stored candidate rather than from the render that produced it.
fn appendix_carries_manifest(candidate_content: &[u8], manifest_bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(candidate_content) else {
        return false;
    };
    let unwrapped: String = text
        .chars()
        .filter(|character| *character != '\n')
        .collect();
    unwrapped.contains(&data_encoding::HEXLOWER.encode(manifest_bytes))
}

/// Read one publication back, re-proving every record it cites.
fn read_publication_within(
    connection: &Connection,
    workspace_id: Uuid,
    row: PublicationRow,
) -> Result<PublishedOutcome, PublishRefusal> {
    // The candidate, through its owner: exact bytes, exact digest.
    let candidate =
        verified_artifact_within(connection, workspace_id, &row.candidate_artifact_digest)?.ok_or(
            PublishRefusal::Unverified(
                "the published candidate Artifact is no longer readable".to_owned(),
            ),
        )?;

    // The Evaluation, through its owner -- and its SUBJECT re-proved against the cited candidate
    // rather than taken from the relation row. The row saying they match is the claim; the
    // Evaluation's own bytes are the evidence.
    let evaluation = verified_evaluation_within(connection, workspace_id, &row.evaluation_digest)?
        .ok_or(PublishRefusal::Unverified(
            "the qualifying Evaluation is no longer readable".to_owned(),
        ))?;
    let EvaluationSubject::Artifact(subject) = &evaluation.payload().subject else {
        return Err(PublishRefusal::Seam(
            PublicationSeamRefusal::SubjectIsNotAnArtifact,
        ));
    };
    if subject.digest.value != candidate.digest().value {
        return Err(PublishRefusal::Seam(
            PublicationSeamRefusal::SubjectIsNotTheCandidate,
        ));
    }
    // The verdict, re-proved from the Evaluation's own bytes on every read (sol1's P0). `publish`
    // proved it through the seam at write time; a relation row whose Evaluation digest was swapped
    // for a `does_not_meet` Evaluation OF THE SAME CANDIDATE passes every subject check, because
    // the subject genuinely IS this candidate. Only the verdict separates the two records.
    if evaluation.payload().verdict != Verdict::Meets {
        return Err(PublishRefusal::QualifyingVerdictNotMeets);
    }
    // Every cited record's own Mission, against the Mission this row is filed under. The row's
    // foreign key proves the Mission exists; nothing until here proved the records are ITS.
    if subject.mission_id != row.mission_id {
        return Err(PublishRefusal::MissionBindingBroken {
            what: "Evaluation's subject",
            named: subject.mission_id,
            relation: row.mission_id,
        });
    }
    if let ArtifactContext::Mission { mission_id, .. } = candidate.payload().context {
        if mission_id != row.mission_id {
            return Err(PublishRefusal::MissionBindingBroken {
                what: "candidate Artifact",
                named: mission_id,
                relation: row.mission_id,
            });
        }
    } else {
        return Err(PublishRefusal::MissionBindingBroken {
            what: "candidate Artifact",
            named: Uuid::nil(),
            relation: row.mission_id,
        });
    }

    // The manifest, through its verified reader: bytes re-hashed, decoded, re-encoded.
    let manifest_digest: Option<String> = connection
        .query_row(
            "SELECT manifest_digest FROM outcome_manifests
             WHERE workspace_id = ?1 AND candidate_artifact_digest = ?2",
            params![workspace_id.to_string(), row.candidate_artifact_digest],
            |row| row.get(0),
        )
        .optional()?;
    let manifest_digest = manifest_digest.ok_or(PublishRefusal::NoManifestForCandidate)?;
    let manifest = verified_manifest_within(connection, workspace_id, &manifest_digest)?;
    if manifest.manifest().mission_id != row.mission_id {
        return Err(PublishRefusal::MissionBindingBroken {
            what: "manifest",
            named: manifest.manifest().mission_id,
            relation: row.mission_id,
        });
    }
    // And the completion the manifest is frozen on, through its owning reader: the deepest record
    // in the chain, and the one that says what Mission actually completed.
    let completion = crate::mission_completion_store::verified_completion_within(
        connection,
        workspace_id,
        &manifest.manifest().completion.digest.value,
    )
    .map_err(|refusal| {
        PublishRefusal::Unverified(format!(
            "the completion this publication's manifest is frozen on did not verify: {refusal}"
        ))
    })?;
    if completion.mission_id != row.mission_id {
        return Err(PublishRefusal::MissionBindingBroken {
            what: "completion",
            named: completion.mission_id,
            relation: row.mission_id,
        });
    }

    // Read-time state, derived now and labelled as such. The Artifact chain comes back cited-first,
    // so the cited digest is in the chain rather than replaced by it.
    let publication_version_count = count_versions(connection, workspace_id, row.mission_id)?;
    // The history the relation PROMISES: contiguous from 1. `publish` cannot create a gap, so a gap
    // means something else wrote the row -- and a history that begins at version 2, read back as
    // "version 2", is a claim about a version 1 nobody can produce (sol1's P1). Count against
    // maximum: equal means contiguous, and anything else is a hole.
    let highest: i64 = connection
        .query_row(
            "SELECT coalesce(max(publication_version), 0) FROM outcome_publications
             WHERE workspace_id = ?1 AND mission_id = ?2",
            params![workspace_id.to_string(), row.mission_id.to_string()],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if i64::from(publication_version_count) != highest {
        return Err(PublishRefusal::HistoryNotContiguous {
            expected: publication_version_count.saturating_add(1),
            found: publication_version_count,
        });
    }

    let supersession = SupersessionAtReadTime {
        candidate: supersession_tail(connection, workspace_id, &row.candidate_artifact_digest)?,
        evaluation: evaluation.force().superseded_by().cloned(),
    };

    Ok(PublishedOutcome {
        publication_id: row.publication_id,
        mission_id: row.mission_id,
        publication_version: row.publication_version,
        publication_version_count,
        manifest,
        cited_candidate_artifact_digest: candidate.digest().clone(),
        cited_evaluation_digest: evaluation.digest().clone(),
        candidate,
        supersession,
    })
}

/// The Mission's current published Outcome: the highest version.
pub(crate) fn get_within(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<PublishedOutcome, PublishRefusal> {
    let row = load_row(
        connection,
        workspace_id,
        mission_id,
        "SELECT publication_id, publication_version, candidate_artifact_digest, evaluation_digest
         FROM outcome_publications
         WHERE workspace_id = ?1 AND mission_id = ?2
         ORDER BY publication_version DESC LIMIT 1",
        None,
    )?;
    read_publication_within(connection, workspace_id, row)
}

/// One exact version of a Mission's publication history. A superseded version stays readable --
/// that is what "preserves" means (R12.11).
pub(crate) fn get_version_within(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
    publication_version: u32,
) -> Result<PublishedOutcome, PublishRefusal> {
    let row = load_row(
        connection,
        workspace_id,
        mission_id,
        "SELECT publication_id, publication_version, candidate_artifact_digest, evaluation_digest
         FROM outcome_publications
         WHERE workspace_id = ?1 AND mission_id = ?2 AND publication_version = ?3",
        Some(publication_version),
    )?;
    read_publication_within(connection, workspace_id, row)
}

/// Every version of a Mission's publication history, newest first.
///
/// **A reader over the relation's own key, never a loop over `get_version`.** Each `get_version`
/// re-proves a whole document -- manifest bytes, candidate bytes, Evaluation, subject, verdict,
/// four Mission bindings and the history's contiguity -- so drawing an N-version list that way
/// would cost N full verifications and would re-read the same manifest N times. This returns what
/// the relation holds and nothing it would have to decode a manifest to learn (R29.5).
///
/// It enumerates by `(workspace_id, mission_id)`, the relation's own key, the same reason
/// `verify_publication_state` enumerates by the relation's key rather than by any owner's.
///
/// Cross-Workspace is an EMPTY list, not a refusal, for the same reason export is `Absent`
/// (R29.6): a caller who may not read this Workspace learns nothing about whether the Mission
/// exists.
pub(crate) fn publication_history_within(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Vec<PublicationSummary>, PublishRefusal> {
    let mut statement = connection.prepare(
        "SELECT publication_id, publication_version, candidate_artifact_digest, evaluation_digest
         FROM outcome_publications
         WHERE workspace_id = ?1 AND mission_id = ?2
         ORDER BY publication_version DESC",
    )?;
    let rows: Vec<(String, i64, String, String)> = statement
        .query_map(
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?
        .collect::<Result<_, _>>()?;
    drop(statement);

    // The highest version is the first row, because the query orders by version descending. Taken
    // from the list rather than asked of the database again, so the two cannot disagree.
    let highest = rows.first().map(|(_, version, _, _)| *version);
    rows.into_iter()
        .map(|(publication_id, version, candidate, evaluation)| {
            Ok(PublicationSummary {
                publication_id: Uuid::parse_str(&publication_id).map_err(|_| {
                    PublishRefusal::Unverified("a publication id is not a UUID".to_owned())
                })?,
                publication_version: u32::try_from(version).unwrap_or(u32::MAX),
                candidate_artifact_digest: candidate,
                evaluation_digest: evaluation,
                is_current: Some(version) == highest,
            })
        })
        .collect()
}

fn load_row(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
    sql: &str,
    version: Option<u32>,
) -> Result<PublicationRow, PublishRefusal> {
    let mapper = |row: &rusqlite::Row<'_>| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    };
    let found = match version {
        None => connection
            .query_row(
                sql,
                params![workspace_id.to_string(), mission_id.to_string()],
                mapper,
            )
            .optional()?,
        Some(version) => connection
            .query_row(
                sql,
                params![
                    workspace_id.to_string(),
                    mission_id.to_string(),
                    i64::from(version)
                ],
                mapper,
            )
            .optional()?,
    };
    let (publication_id, publication_version, candidate_artifact_digest, evaluation_digest) =
        found.ok_or(PublishRefusal::Absent)?;
    Ok(PublicationRow {
        publication_id: Uuid::parse_str(&publication_id).map_err(|_| {
            PublishRefusal::Unverified("the publication id is not a UUID".to_owned())
        })?,
        mission_id,
        publication_version: u32::try_from(publication_version).unwrap_or(u32::MAX),
        candidate_artifact_digest,
        evaluation_digest,
    })
}

/// Reconcile every stored publication at startup (R18.7 shape, sol1's P0).
///
/// **It enumerates `outcome_publications` by the relation's own key.** Enumerating Missions and
/// asking each for its publications would visit only the Missions something already reaches, which
/// is the one set that never contained the forged row -- a startup pass keyed on the wrong column
/// is a pass that cannot fail. The same lesson `verify_manifest_state` records, applied to the
/// relation.
///
/// What it re-proves is exactly what `get` re-proves, through the same function: the manifest, the
/// candidate, the Evaluation and its subject, the qualifying verdict, the Mission binding of every
/// cited record, and the contiguity of the history the row belongs to. There is no second spelling
/// of those rules here -- a startup pass that restated them would be a second thing to keep true,
/// and the two would drift.
///
/// Why it is needed even though `publish` refuses the same rows: `publish` is the writer, and a
/// row nothing wrote through `publish` has never met it. Before this pass, a forged relation
/// survived reopen as `Ready` while the public writer refused to create it -- the store agreeing
/// with itself only for as long as nobody wrote to it directly.
pub(crate) fn verify_publication_state(connection: &Connection) -> Result<(), PublishRefusal> {
    let mut statement = connection.prepare(
        "SELECT workspace_id, mission_id, publication_version FROM outcome_publications
         ORDER BY workspace_id, mission_id, publication_version",
    )?;
    let rows: Vec<(String, String, i64)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    for (workspace_id, mission_id, publication_version) in &rows {
        let workspace_id = Uuid::parse_str(workspace_id).map_err(|_| {
            PublishRefusal::Unverified("a publication's Workspace id is not a UUID".to_owned())
        })?;
        let mission_id = Uuid::parse_str(mission_id).map_err(|_| {
            PublishRefusal::Unverified("a publication's Mission id is not a UUID".to_owned())
        })?;
        let version = u32::try_from(*publication_version).map_err(|_| {
            PublishRefusal::Unverified("a publication version is out of range".to_owned())
        })?;
        get_version_within(connection, workspace_id, mission_id, version)?;
    }
    Ok(())
}
