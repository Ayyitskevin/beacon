//! The contract the publication relation consumes, and nothing else (K-2 design §8).
//!
//! Outcome slice 3 owns the relation itself. This module owns what the Evaluation arm **guarantees**
//! to it: that "this Evaluation is about these exact candidate bytes" and "the evaluator is not the
//! candidate's author" are checkable **from the two records alone**, before any relation row exists.
//!
//! # What does not cross this seam (R4.3)
//!
//! Nothing renderable. `PublicationCandidacy` carries two digests and a Workspace, and there is no
//! accessor for the verdict's text, the rationale, the findings, the rubric, or the evaluator's
//! name -- not because a consumer could not fetch them (it can, through the owning reader, and then
//! it is that consumer's citation) but because a value handed across a seam is a value a surface
//! will render without asking where it came from. The seam answers one question, `may this be
//! published`, and answers it as a type rather than as a sentence.
//!
//! # What it deliberately does not guarantee
//!
//! **Currency.** An Evaluation of a since-superseded candidate stays a true record about that
//! candidate. Surfacing that is the read-time both-links rule (R12.10,
//! `Evaluation::subject_superseded_by`), not a publish-time refusal -- refusing here would make the
//! publication history depend on when someone looked.

use std::fmt;

use rusqlite::Connection;
use uuid::Uuid;

use crate::artifact::ArtifactDigest;
use crate::artifact::verified_artifact_within;
use crate::evaluation::{EvaluationDigest, EvaluationSubject, Verdict, verified_evaluation_within};

/// One publishable pairing, proved. Two identities and a Workspace: exactly the columns the
/// relation's own foreign keys need, and nothing a surface could print as a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationCandidacy {
    workspace_id: Uuid,
    candidate_artifact_digest: ArtifactDigest,
    evaluation_digest: EvaluationDigest,
}

impl PublicationCandidacy {
    pub fn workspace_id(&self) -> Uuid {
        self.workspace_id
    }

    /// The candidate Artifact's RECORD digest -- the identity the relation keys on, in the
    /// envelope its owning module verified.
    pub fn candidate_artifact_digest(&self) -> &ArtifactDigest {
        &self.candidate_artifact_digest
    }

    pub fn evaluation_digest(&self) -> &EvaluationDigest {
        &self.evaluation_digest
    }
}

/// Why a pairing may not be published. Each arm names one fact that was not true; none of them
/// carries a value from either record, so a refusal cannot become a rendered claim either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationSeamRefusal {
    /// No Evaluation in this Workspace has that digest.
    EvaluationAbsent,
    /// No Artifact in this Workspace has that digest.
    CandidateAbsent,
    /// The Evaluation's subject is not an Artifact at all (arm `0x01`).
    SubjectIsNotAnArtifact,
    /// The Evaluation is about a different Artifact than the candidate. Read from the Evaluation's
    /// canonical bytes, never from a projection alone.
    SubjectIsNotTheCandidate,
    /// The verdict is not `meets`. `uncertain` and `revision_requested` are first-class outcomes,
    /// and neither is a soft yes.
    VerdictIsNotMeets,
    /// The evaluator is the candidate's accountable Resident. Keyed on the Resident, never on the
    /// (Resident, Configuration) pair.
    EvaluatorAuthoredTheCandidate,
    /// The Evaluation's derived Mission is not the Outcome's Mission.
    MissionDiffers,
    /// A supplied digest envelope is not the one the owning module verified: same value, different
    /// algorithm or canonicalizer. A digest read back under an envelope nobody produced is the
    /// assumption these records exist to remove.
    CitedEnvelopeDisagrees,
    /// A record did not verify. Deliberately opaque: the owning module's refusal is that module's
    /// to report, and repeating it here would be a second place for it to drift.
    Unverified,
}

impl fmt::Display for PublicationSeamRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EvaluationAbsent => "no Evaluation in this Workspace has that digest",
            Self::CandidateAbsent => "no Artifact in this Workspace has that digest",
            Self::SubjectIsNotAnArtifact => "the Evaluation's subject is not an Artifact",
            Self::SubjectIsNotTheCandidate => {
                "the Evaluation is about different bytes than the candidate"
            }
            Self::VerdictIsNotMeets => "the Evaluation's verdict is not `meets`",
            Self::EvaluatorAuthoredTheCandidate => {
                "the evaluator is the candidate's accountable Resident"
            }
            Self::MissionDiffers => "the Evaluation's Mission is not this Outcome's Mission",
            Self::CitedEnvelopeDisagrees => {
                "the supplied digest envelope is not the one the owning module verified"
            }
            Self::Unverified => "a cited record did not verify",
        })
    }
}

impl std::error::Error for PublicationSeamRefusal {}

/// Prove one publishable pairing from the two records alone.
///
/// Every check reads the Evaluation's **canonical bytes** through its owning verified reader and
/// the candidate through its own: this function compares, it does not re-derive, and it never reads
/// a projection column of either table. The five checks are §8's, in the order that makes each
/// refusal mean one thing.
pub fn publication_candidacy(
    connection: &Connection,
    workspace_id: Uuid,
    evaluation_digest: &EvaluationDigest,
    candidate_artifact_digest: &ArtifactDigest,
    outcome_mission_id: Uuid,
) -> Result<PublicationCandidacy, PublicationSeamRefusal> {
    let evaluation =
        verified_evaluation_within(connection, workspace_id, evaluation_digest.value.as_str())
            .map_err(|_| PublicationSeamRefusal::Unverified)?
            .ok_or(PublicationSeamRefusal::EvaluationAbsent)?;
    let candidate = verified_artifact_within(
        connection,
        workspace_id,
        candidate_artifact_digest.value.as_str(),
    )
    .map_err(|_| PublicationSeamRefusal::Unverified)?
    .ok_or(PublicationSeamRefusal::CandidateAbsent)?;

    // 0. The envelopes the caller supplied are the ones the owning modules verified.
    //
    //    Types stop a SWAP; they do not stop a FORGE. `ArtifactDigest` and `EvaluationDigest` have
    //    public fields, so a caller can hand-build one with the right `value` and a wrong
    //    algorithm or canonicalizer -- and a value read back under an envelope nobody produced is
    //    exactly the "which function made this digest?" assumption these records exist to remove.
    //    So the whole envelope is compared, not just the value the lookup used.
    if evaluation.digest() != evaluation_digest {
        return Err(PublicationSeamRefusal::CitedEnvelopeDisagrees);
    }
    if candidate.digest() != candidate_artifact_digest {
        return Err(PublicationSeamRefusal::CitedEnvelopeDisagrees);
    }

    // 1. The subject is an Artifact, and 2. it is THIS one -- from the Evaluation's own bytes.
    let EvaluationSubject::Artifact(subject) = &evaluation.payload().subject else {
        return Err(PublicationSeamRefusal::SubjectIsNotAnArtifact);
    };
    if subject.digest.value != candidate.digest().value {
        return Err(PublicationSeamRefusal::SubjectIsNotTheCandidate);
    }
    // 3. `meets`, and only `meets`.
    if evaluation.payload().verdict != Verdict::Meets {
        return Err(PublicationSeamRefusal::VerdictIsNotMeets);
    }
    // 4. Non-authorship, re-proved here from both records rather than trusted from the store's
    //    triggers: `publish` verifies rather than trusts, which is what §8 says this arm buys it.
    if evaluation.payload().evaluator.resident_id == candidate.payload().accountable_resident_id {
        return Err(PublicationSeamRefusal::EvaluatorAuthoredTheCandidate);
    }
    // 5. One Mission: the Evaluation's derived Mission is the Artifact's declared scope (the
    //    reader proved that), and the Outcome must be about the same one.
    if subject.mission_id != outcome_mission_id {
        return Err(PublicationSeamRefusal::MissionDiffers);
    }

    Ok(PublicationCandidacy {
        workspace_id,
        // From the VERIFIED records, never from the caller's arguments: what the relation stores
        // is the envelope the owning modules proved, not the one it was handed.
        candidate_artifact_digest: candidate.digest().clone(),
        evaluation_digest: evaluation.digest().clone(),
    })
}
