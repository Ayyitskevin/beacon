//! Evaluation v1: one non-author Resident's recorded verdict about one exact Proposal digest
//! (ADR 0038).
//!
//! **An Evaluation confers no authority.** It is a record that somebody judged, and a judgment is
//! not a permission (`docs/BLUEPRINT.md:445-447`). Nothing here opens a Gate, admits a dispatch,
//! prepares a worktree, starts a process, invokes a model, or makes a network request, and no
//! accessor answers a question with a boolean. `meets` is not approval -- approval is a Ruling
//! word, and this vocabulary deliberately does not contain it.
//!
//! **A Ruling does not cite an Evaluation in v1.** The citation relation is deferred until
//! `beacon.ruling.v2` can cover the cited set with the Ruling's own digest, because a relation
//! attached after the fact is indistinguishable from one made at ruling time.
//!
//! **Non-authorship is structural, in two layers.** The store refuses an insert whose projected
//! evaluator Resident is the bound Proposal's author; the Command, every verified read, and
//! startup re-derive the same relation from the canonical bytes. SQL proves the projection is
//! clean, the bytes layer proves the projection is the one the bytes describe, and the comparison
//! is on the Resident rather than the (Resident, configuration) pair -- two configurations of one
//! Resident are two evaluators for configuration purposes and one author for this one.
//!
//! The rubric fields are a **self-report**. Nothing in Beacon resolves a rubric: there is no rubric
//! table, no owning module and no foreign key. They record what the evaluator declared they
//! applied, exactly as ADR 0034 treats a declared readable context, and **no surface may present a
//! rubric as applied or verified on their strength alone.**
//!
//! The canonical grammar reuses `beacon.proposal.v1`'s primitives, and this module carries its own
//! copy of them rather than sharing one implementation -- the same decision Ruling v1 made and for
//! the same reason: an identity grammar's whole value is that it cannot move, and a shared codec
//! would couple two frozen grammars' futures.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use data_encoding::{BASE64, HEXLOWER};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::artifact::ArtifactDigest;
use crate::proposal::{
    ProposalDigest, UnmetPrecondition, is_human_text, proposal_binding_facts, sha256,
};
use crate::{OperatorAccountError, OperatorAccountService, ProposalError};

/// The canonicalizer identity. Existing v1 bytes are never reinterpreted under v2 rules, and
/// the name survives a RESERVED tag assignment: a change that alters the byte layout of records
/// that already exist takes a new canonicalizer; a change that assigns a value in a published
/// tag position (the `0x02`/`0x03` reservations, ADR 0038's 2026-09-04 amendment) does not.
const CANONICALIZER: &str = "beacon.evaluation.v1";
const DIGEST_ALGORITHM: &str = "sha256";

/// The exact envelope a v1 Evaluation may name as the Proposal it evaluated. Both are copied from
/// the Proposal and both are checked here, so bytes naming a canonicalizer this grammar cannot have
/// evaluated under are refused by the codec rather than only by a column constraint.
const PROPOSAL_DIGEST_ALGORITHM: &str = "sha256";
const PROPOSAL_CANONICALIZER: &str = "beacon.proposal.v1";
/// The Artifact arm's canonicalizer token. It is the Artifact RECORD's canonicalizer, imported
/// from the module that owns it (R13.3): a second spelling here could drift from the grammar the
/// digest actually covers.
const ARTIFACT_CANONICALIZER: &str = crate::artifact::CANONICALIZER;

/// The literal ASCII magic that starts every record, including its terminating NUL: 21 bytes.
const MAGIC: &[u8] = b"beacon.evaluation.v1\0";

const MAX_RATIONALE_SCALARS: usize = 2_000;
const MAX_RATIONALE_BYTES: usize = MAX_RATIONALE_SCALARS * 4;
const MAX_CONFLICTS_SCALARS: usize = 2_000;
const MAX_CONFLICTS_BYTES: usize = MAX_CONFLICTS_SCALARS * 4;
const MAX_SUMMARY_SCALARS: usize = 500;
const MAX_SUMMARY_BYTES: usize = MAX_SUMMARY_SCALARS * 4;
const MAX_DESCRIPTOR_TOKEN_BYTES: usize = 64;
/// The whole record's own bound, and the one the store column repeats.
///
/// The per-field limits alone admit 145,941 bytes -- 64 findings of 500 four-byte scalars is
/// 128,576 of them -- so a grammar with only per-field bounds is not total: a record every field
/// table accepts could not be stored. This is the bound that makes it total, it is checked by the
/// encoder before anything reaches SQL and by the decoder before it parses, and
/// `0021_evaluations.sql` repeats it as a CHECK so the two cannot drift apart silently.
const MAX_CANONICAL_BYTES: usize = 65_536;
const MAX_CITED_RUNS: usize = 64;
const MAX_FINDINGS: usize = 64;
const DIGEST_HEX_CHARS: usize = 64;

// One-byte tags. Assigned explicitly and never renumbered. `conflicts` and `supersedes` are the two
// positions where 0x00 is assigned, because absence is a value in both and must encode as one.
const TAG_SUBJECT_PROPOSAL: u8 = 0x01;
/// `0x02` stays RESERVED AND REFUSED for the Candidate arm; `0x03` is the Artifact arm, and
/// `0x04` and above are unassigned and refused (ADR 0038's 2026-09-04 amendment, the tag registry).
/// Assigning a reserved position does not take a new canonicalizer -- no existing record's byte
/// layout moves -- which is the rule that lets `beacon.evaluation.v1` survive this arm.
const TAG_SUBJECT_ARTIFACT: u8 = 0x03;
const TAG_VERDICT_MEETS: u8 = 0x01;
const TAG_VERDICT_REVISION_REQUESTED: u8 = 0x02;
const TAG_VERDICT_DOES_NOT_MEET: u8 = 0x03;
const TAG_VERDICT_UNCERTAIN: u8 = 0x04;
const TAG_SEVERITY_BLOCKING: u8 = 0x01;
const TAG_SEVERITY_CONCERN: u8 = 0x02;
const TAG_SEVERITY_NOTE: u8 = 0x03;
const TAG_CONFLICTS_NONE: u8 = 0x00;
const TAG_CONFLICTS_DECLARED: u8 = 0x01;
const TAG_SUPERSEDES_NONE: u8 = 0x00;
const TAG_SUPERSEDES_EVALUATION: u8 = 0x01;

const VERDICT_MEETS: &str = "meets";
const VERDICT_REVISION_REQUESTED: &str = "revision_requested";
const VERDICT_DOES_NOT_MEET: &str = "does_not_meet";
const VERDICT_UNCERTAIN: &str = "uncertain";

const SEVERITY_BLOCKING: &str = "blocking";
const SEVERITY_CONCERN: &str = "concern";
const SEVERITY_NOTE: &str = "note";

// ---------------------------------------------------------------------------------------------
// The immutable v1 payload
// ---------------------------------------------------------------------------------------------

/// The closed v1 verdict vocabulary, taken from `BLUEPRINT:630-631` rather than invented.
///
/// None of the four is `approved`. `uncertain` is a first-class outcome and not a soft rejection,
/// for the same reason the disk-encryption reader has `Unknown`: an unproved answer that presents
/// as a negative is a different lie from an unproved answer that says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Meets,
    RevisionRequested,
    DoesNotMeet,
    Uncertain,
}

impl Verdict {
    fn tag(self) -> u8 {
        match self {
            Self::Meets => TAG_VERDICT_MEETS,
            Self::RevisionRequested => TAG_VERDICT_REVISION_REQUESTED,
            Self::DoesNotMeet => TAG_VERDICT_DOES_NOT_MEET,
            Self::Uncertain => TAG_VERDICT_UNCERTAIN,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            TAG_VERDICT_MEETS => Self::Meets,
            TAG_VERDICT_REVISION_REQUESTED => Self::RevisionRequested,
            TAG_VERDICT_DOES_NOT_MEET => Self::DoesNotMeet,
            TAG_VERDICT_UNCERTAIN => Self::Uncertain,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Meets => VERDICT_MEETS,
            Self::RevisionRequested => VERDICT_REVISION_REQUESTED,
            Self::DoesNotMeet => VERDICT_DOES_NOT_MEET,
            Self::Uncertain => VERDICT_UNCERTAIN,
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            VERDICT_MEETS => Self::Meets,
            VERDICT_REVISION_REQUESTED => Self::RevisionRequested,
            VERDICT_DOES_NOT_MEET => Self::DoesNotMeet,
            VERDICT_UNCERTAIN => Self::Uncertain,
            _ => return None,
        })
    }
}

/// Three arms. A finding list with no severity forces every consumer to re-read prose to learn
/// whether anything mattered; two arms would make "worth saying, not worth blocking" unsayable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingSeverity {
    Blocking,
    Concern,
    Note,
}

impl FindingSeverity {
    fn tag(self) -> u8 {
        match self {
            Self::Blocking => TAG_SEVERITY_BLOCKING,
            Self::Concern => TAG_SEVERITY_CONCERN,
            Self::Note => TAG_SEVERITY_NOTE,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            TAG_SEVERITY_BLOCKING => Self::Blocking,
            TAG_SEVERITY_CONCERN => Self::Concern,
            TAG_SEVERITY_NOTE => Self::Note,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Blocking => SEVERITY_BLOCKING,
            Self::Concern => SEVERITY_CONCERN,
            Self::Note => SEVERITY_NOTE,
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            SEVERITY_BLOCKING => Self::Blocking,
            SEVERITY_CONCERN => Self::Concern,
            SEVERITY_NOTE => Self::Note,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: FindingSeverity,
    pub summary: String,
}

/// Absence is a value: `BLUEPRINT:630` requires conflicts to be *recorded*, so "no conflict
/// declared" encodes as a statement rather than as a missing field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflicts {
    None,
    Declared(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluationSupersedes {
    None,
    Evaluation(String),
}

/// What the evaluator says they applied. Beacon resolves none of it -- see the module note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubricDeclaration {
    pub id: String,
    pub version: u32,
    pub canonicalizer: String,
    pub sha256: String,
}

/// The evaluator, as a Resident under one exact configuration.
///
/// Two configurations of one Resident are two evaluators for configuration purposes and **one
/// author** for non-authorship purposes; slice 2 makes that structural.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evaluator {
    pub resident_id: Uuid,
    pub configuration_id: Uuid,
}

/// What one Evaluation is about: field 2's tagged union.
///
/// The tag and the canonicalizer token are two spellings of one fact, so the codec refuses a pair
/// that disagrees in either direction -- an Artifact arm naming `beacon.proposal.v1`, and a
/// Proposal arm naming `beacon.artifact.v1`. A column cannot enforce that; the grammar can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluationSubject {
    /// `0x01`: a Proposal, by the content digest of its canonical bytes. The v1 arm, unmoved.
    Proposal(ProposalDigest),
    /// `0x03`: an **Artifact record** digest, with the Mission scope that record declares.
    Artifact(ArtifactSubject),
}

impl EvaluationV1 {
    /// The Proposal this Evaluation binds, if it binds one.
    ///
    /// Slice E2 gives the Artifact arm its own columns. Until it lands, every store path calls this
    /// and refuses an Artifact subject BY NAME rather than writing a row whose subject columns do
    /// not exist: a seam, not a stub that silently records the wrong thing.
    pub fn proposal_subject(&self) -> Option<&ProposalDigest> {
        match &self.subject {
            EvaluationSubject::Proposal(digest) => Some(digest),
            EvaluationSubject::Artifact(_) => None,
        }
    }
}

impl EvaluationSubject {
    /// The subject's own digest value, whichever arm it is. For the one thing both arms share.
    pub fn digest_value(&self) -> &str {
        match self {
            Self::Proposal(digest) => &digest.value,
            Self::Artifact(subject) => &subject.digest.value,
        }
    }

    /// The tag byte this arm encodes as -- the registry value, not a position in a match.
    pub fn tag(&self) -> u8 {
        match self {
            Self::Proposal(_) => TAG_SUBJECT_PROPOSAL,
            Self::Artifact(_) => TAG_SUBJECT_ARTIFACT,
        }
    }
}

/// The Artifact arm: the record digest, and the scope copied from that record's own bytes.
///
/// `project_id` and `mission_id` are **copied from the subject Artifact's canonical bytes**, never
/// supplied beside them: an Evaluation that asserted its own Mission would let the weaker party
/// make the claim the roster check depends on (design §4). Every write re-derives them against the
/// Artifact and refuses a mismatch; this type only carries what the bytes say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactSubject {
    /// The **record** digest (SHA-256 over `beacon.artifact.v1` canonical bytes), never the bare
    /// content digest: criterion 26 adjudicates author versus evaluator, and the author lives in
    /// the record.
    pub digest: ArtifactDigest,
    pub project_id: Uuid,
    pub mission_id: Uuid,
}

/// The immutable v1 payload, in canonical field order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationV1 {
    pub subject: EvaluationSubject,
    pub evaluator: Evaluator,
    pub rubric: RubricDeclaration,
    pub cited_runs: Vec<Uuid>,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
    pub conflicts: Conflicts,
    pub rationale: String,
    pub supersedes: EvaluationSupersedes,
    pub evaluated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationDigest {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

/// What is true of a stored Evaluation NOW, as facts rather than as a verdict.
///
/// Derived at read time and never stored: a stored answer would be a second thing to keep true.
/// There is deliberately no `is_in_force` and no boolean anywhere on this type. An Evaluation that
/// has been superseded, or whose bound Proposal's revisions have moved, is still a true account of
/// what an evaluator judged about those exact bytes; whether that matters is the reader's question,
/// and collapsing "superseded" and "drifted" into one bit would answer it for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationForce {
    superseded_by: Option<EvaluationDigest>,
    drift: Vec<UnmetPrecondition>,
}

impl EvaluationForce {
    /// The Evaluation that supersedes this one, if any. `None` is the absence of a relation row,
    /// not a null column.
    pub fn superseded_by(&self) -> Option<&EvaluationDigest> {
        self.superseded_by.as_ref()
    }

    /// Which of the bound Proposal's revisions have moved since. Empty means none have moved --
    /// it does not mean the Evaluation is current, because supersession is the other fact.
    pub fn drift(&self) -> &[UnmetPrecondition] {
        &self.drift
    }
}

/// One stored Evaluation, re-verified on every read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    digest: EvaluationDigest,
    canonical_bytes: Vec<u8>,
    payload: EvaluationV1,
    force: EvaluationForce,
}

impl Evaluation {
    pub fn digest(&self) -> &EvaluationDigest {
        &self.digest
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    pub fn payload(&self) -> &EvaluationV1 {
        &self.payload
    }

    /// Supersession and drift, as two separate facts. Never a boolean.
    pub fn force(&self) -> &EvaluationForce {
        &self.force
    }
}

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

/// Why bytes are not a v1 Evaluation. Every arm is a fact about the bytes, never about the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedEvaluation {
    Magic,
    Truncated,
    TrailingBytes,
    InvalidUtf8,
    OverLimitLength,
    RecordTooLarge,
    Subject,
    ProposalDigestValue,
    ProposalDigestEnvelope,
    ArtifactDigestValue,
    ArtifactDigestEnvelope,
    RubricId,
    RubricVersion,
    RubricCanonicalizer,
    RubricDigest,
    CitedRunCount,
    CitedRunOrder,
    Verdict,
    FindingCount,
    FindingSeverity,
    FindingSummary,
    Conflicts,
    ConflictsText,
    Rationale,
    Supersedes,
    SupersedesDigest,
}

impl fmt::Display for MalformedEvaluation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Magic => "the bytes do not start with the beacon.evaluation.v1 magic",
            Self::Truncated => "the bytes end inside a field",
            Self::TrailingBytes => "bytes follow the record",
            Self::InvalidUtf8 => "a string is not valid UTF-8",
            Self::OverLimitLength => "a declared length exceeds its field's limit",
            Self::RecordTooLarge => "the whole record exceeds its own byte bound",
            Self::Subject => "the subject tag is not an assigned value",
            Self::ProposalDigestValue => "the Proposal digest is not 64 lowercase hex characters",
            Self::ProposalDigestEnvelope => {
                "the Proposal digest algorithm or canonicalizer is not the one v1 evaluates"
            }
            Self::ArtifactDigestValue => {
                "the Artifact record digest is not 64 lowercase hex characters"
            }
            Self::ArtifactDigestEnvelope => {
                "the Artifact digest algorithm or canonicalizer is not beacon.artifact.v1"
            }
            Self::RubricId => "the rubric id is empty or over its limit",
            Self::RubricVersion => "the rubric version is below one",
            Self::RubricCanonicalizer => "the rubric canonicalizer is empty or over its limit",
            Self::RubricDigest => "the rubric digest is not 64 lowercase hex characters",
            Self::CitedRunCount => "more cited Runs than the bound admits",
            Self::CitedRunOrder => "cited Runs are not in ascending order, or repeat",
            Self::Verdict => "the verdict tag is not an assigned value",
            Self::FindingCount => "more findings than the bound admits",
            Self::FindingSeverity => "a finding severity tag is not an assigned value",
            Self::FindingSummary => "a finding summary is empty or over its limit",
            Self::Conflicts => "the conflicts tag is not an assigned value",
            Self::ConflictsText => "the declared conflict text is empty or over its limit",
            Self::Rationale => "the rationale is empty or over its limit",
            Self::Supersedes => "the supersedes tag is not an assigned value",
            Self::SupersedesDigest => {
                "the superseded Evaluation digest is not 64 lowercase hex characters"
            }
        };
        formatter.write_str(message)
    }
}

/// Why a request cannot be recorded, or a stored row cannot be believed. Distinct from
/// `MalformedEvaluation`: these are facts about the store, not about bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InconsistentEvaluation {
    ProposalNotFound,
    DigestMismatch,
    BytesNotCanonical,
    ColumnDisagreesWithBytes,
    ProposalEnvelopeDisagrees,
    MissionDisagreesWithProposal,
    EvaluatorAuthoredTheProposal,
    /// The evaluator is the accountable Resident of the subject Artifact. Keyed on the Resident,
    /// never the (Resident, Configuration) pair.
    EvaluatorAuthoredTheArtifact,
    ArtifactNotFound,
    /// The subject Artifact declares `context = none`, so there is no roster to check the
    /// evaluator against. Not an Evaluation subject in v1.
    ArtifactHasNoMissionScope,
    SupersedesNotRecorded,
    SupersedesOtherSubject,
    NotTheCurrentTail,
    EvaluatedBeforeTail,
    SupersessionDisagreesWithBytes,
    FindingsDisagreeWithBytes,
    CitedRunsDisagreeWithBytes,
}

impl fmt::Display for InconsistentEvaluation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ProposalNotFound => "no stored Proposal has that digest in this Workspace",
            Self::DigestMismatch => "the stored digest is not the hash of the stored bytes",
            Self::BytesNotCanonical => "the stored bytes are not what re-encoding produces",
            Self::ColumnDisagreesWithBytes => "an indexed column disagrees with the stored bytes",
            Self::ProposalEnvelopeDisagrees => {
                "the copied Proposal envelope is not what that Proposal recorded"
            }
            Self::MissionDisagreesWithProposal => {
                "the derived Mission is not the bound Proposal's Mission"
            }
            Self::EvaluatorAuthoredTheProposal => {
                "the evaluator authored the Proposal being evaluated"
            }
            Self::EvaluatorAuthoredTheArtifact => {
                "the evaluator is the accountable Resident of the Artifact being evaluated"
            }
            Self::ArtifactNotFound => "no Artifact in this Workspace has that digest",
            Self::ArtifactHasNoMissionScope => {
                "the subject Artifact declares no Mission scope, so it has no roster to check the \
                 evaluator against"
            }
            Self::SupersedesNotRecorded => "the superseded digest names no stored Evaluation",
            Self::SupersedesOtherSubject => "the superseded Evaluation belongs to another Proposal",
            Self::NotTheCurrentTail => {
                "the superseded Evaluation is not the current end of the chain"
            }
            Self::EvaluatedBeforeTail => "the Evaluation predates the one it supersedes",
            Self::SupersessionDisagreesWithBytes => {
                "the supersession relation disagrees with the canonical bytes"
            }
            Self::FindingsDisagreeWithBytes => "the stored findings are not the ones in the bytes",
            Self::CitedRunsDisagreeWithBytes => {
                "the stored cited Runs are not the ones in the bytes"
            }
        };
        formatter.write_str(message)
    }
}

#[derive(Debug)]
pub enum EvaluationError {
    Account(OperatorAccountError),
    Malformed(MalformedEvaluation),
    Inconsistent(InconsistentEvaluation),
    NotFound,
    InvalidStore,
    /// The bytes are a valid Evaluation of a subject this store cannot hold yet. The Artifact arm's
    /// columns arrive with its migration (slice E2); until then the codec accepts the record and
    /// the store refuses it by name, which is a seam rather than a silent wrong row.
    SubjectNotStorable,
    Store(rusqlite::Error),
}

impl fmt::Display for EvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "{error}"),
            Self::Malformed(reason) => write!(formatter, "the Evaluation is malformed: {reason}"),
            Self::Inconsistent(reason) => {
                write!(formatter, "the Evaluation cannot be recorded: {reason}")
            }
            Self::NotFound => formatter.write_str("no Evaluation has that digest"),
            Self::InvalidStore => formatter.write_str("the store holds a value Beacon cannot read"),
            Self::SubjectNotStorable => {
                formatter.write_str("this store cannot hold an Evaluation of that subject kind yet")
            }
            Self::Store(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for EvaluationError {}

impl From<rusqlite::Error> for EvaluationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<OperatorAccountError> for EvaluationError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

fn map_proposal_error(error: ProposalError) -> EvaluationError {
    match error {
        ProposalError::Account(account) => EvaluationError::Account(account),
        ProposalError::Store(store) => EvaluationError::Store(store),
        _ => EvaluationError::InvalidStore,
    }
}

// ---------------------------------------------------------------------------------------------
// Canonical bytes
// ---------------------------------------------------------------------------------------------

struct Encoder {
    out: Vec<u8>,
}

impl Encoder {
    fn new() -> Self {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        Self { out }
    }

    fn tag(&mut self, tag: u8) {
        self.out.push(tag);
    }

    fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    fn sha256(&mut self, value: &[u8; 32]) {
        self.out.extend_from_slice(value);
    }

    fn uuid(&mut self, value: &Uuid) {
        self.out.extend_from_slice(value.as_bytes());
    }

    fn string(&mut self, value: &str, limit: usize) -> Result<(), MalformedEvaluation> {
        if value.len() > limit {
            return Err(MalformedEvaluation::OverLimitLength);
        }
        let length =
            u64::try_from(value.len()).map_err(|_| MalformedEvaluation::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value.as_bytes());
        Ok(())
    }
}

/// Reads canonical bytes. Self-delimiting: every field proves it fits within the remaining input
/// and its own limit before any of it is consumed, and **no declared count ever sizes an
/// allocation** -- a decoder that trusts a length prefix to reserve capacity is the oldest defect
/// in this class.
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

    fn take(&mut self, count: usize) -> Result<&'a [u8], MalformedEvaluation> {
        if count > self.remaining() {
            return Err(MalformedEvaluation::Truncated);
        }
        let slice = &self.input[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn magic(&mut self) -> Result<(), MalformedEvaluation> {
        if self
            .take(MAGIC.len())
            .map_err(|_| MalformedEvaluation::Magic)?
            == MAGIC
        {
            Ok(())
        } else {
            Err(MalformedEvaluation::Magic)
        }
    }

    fn tag(&mut self) -> Result<u8, MalformedEvaluation> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, MalformedEvaluation> {
        let mut value = [0_u8; 4];
        value.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(value))
    }

    fn u64(&mut self) -> Result<u64, MalformedEvaluation> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }

    fn sha256(&mut self) -> Result<[u8; 32], MalformedEvaluation> {
        let mut value = [0_u8; 32];
        value.copy_from_slice(self.take(32)?);
        Ok(value)
    }

    fn uuid(&mut self) -> Result<Uuid, MalformedEvaluation> {
        let mut value = [0_u8; 16];
        value.copy_from_slice(self.take(16)?);
        Ok(Uuid::from_bytes(value))
    }

    fn string(&mut self, limit: usize) -> Result<String, MalformedEvaluation> {
        let length = self.u64()?;
        let limit = u64::try_from(limit).map_err(|_| MalformedEvaluation::OverLimitLength)?;
        if length > limit {
            return Err(MalformedEvaluation::OverLimitLength);
        }
        let length = usize::try_from(length).map_err(|_| MalformedEvaluation::Truncated)?;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| MalformedEvaluation::InvalidUtf8)
    }

    /// A list count, proved against the remaining input before a single entry is read.
    ///
    /// `minimum_entry_bytes` is the smallest an entry can be, so a count that could not fit is
    /// refused before the loop rather than by running out of bytes partway through it.
    fn count(
        &self,
        declared: u32,
        limit: usize,
        minimum_entry_bytes: usize,
        refusal: MalformedEvaluation,
    ) -> Result<usize, MalformedEvaluation> {
        let declared = usize::try_from(declared).map_err(|_| refusal)?;
        if declared > limit {
            return Err(refusal);
        }
        let needed = declared
            .checked_mul(minimum_entry_bytes)
            .ok_or(MalformedEvaluation::Truncated)?;
        if needed > self.remaining() {
            return Err(MalformedEvaluation::Truncated);
        }
        Ok(declared)
    }

    fn finish(self) -> Result<(), MalformedEvaluation> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MalformedEvaluation::TrailingBytes)
        }
    }
}

/// A lowercase 64-character hexadecimal digest, as its thirty-two raw bytes.
///
/// Uppercase input is refused rather than folded: `HEXLOWER` accepts only the canonical form, so
/// two spellings of one digest can never both encode.
fn digest_bytes(
    value: &str,
    refusal: MalformedEvaluation,
) -> Result<[u8; 32], MalformedEvaluation> {
    if value.len() != DIGEST_HEX_CHARS {
        return Err(refusal);
    }
    let decoded = HEXLOWER.decode(value.as_bytes()).map_err(|_| refusal)?;
    let mut bytes = [0_u8; 32];
    if decoded.len() != bytes.len() {
        return Err(refusal);
    }
    bytes.copy_from_slice(&decoded);
    Ok(bytes)
}

/// The one canonical encoding of a payload. Every accepted byte difference changes the Evaluation
/// identity, so alternate encodings of one parsed value do not exist.
///
/// The field order, widths and tag values are ADR 0038's table, and this function is the
/// executable form of it. The published identity vector is what proves the two agree.
fn encode(payload: &EvaluationV1) -> Result<Vec<u8>, MalformedEvaluation> {
    let mut encoder = Encoder::new();

    // 2-5: the subject. One tag, then that arm's payload -- and the envelope is closed PER ARM,
    // here in the grammar rather than in a column, because the tag and the canonicalizer token are
    // two spellings of one fact and a store cannot see when they disagree.
    encoder.tag(payload.subject.tag());
    match &payload.subject {
        EvaluationSubject::Proposal(digest) => {
            if digest.algorithm != PROPOSAL_DIGEST_ALGORITHM
                || digest.canonicalizer != PROPOSAL_CANONICALIZER
            {
                return Err(MalformedEvaluation::ProposalDigestEnvelope);
            }
            encoder.sha256(&digest_bytes(
                &digest.value,
                MalformedEvaluation::ProposalDigestValue,
            )?);
            encoder.string(&digest.algorithm, MAX_DESCRIPTOR_TOKEN_BYTES)?;
            encoder.string(&digest.canonicalizer, MAX_DESCRIPTOR_TOKEN_BYTES)?;
        }
        EvaluationSubject::Artifact(subject) => {
            if subject.digest.algorithm != PROPOSAL_DIGEST_ALGORITHM
                || subject.digest.canonicalizer != ARTIFACT_CANONICALIZER
            {
                return Err(MalformedEvaluation::ArtifactDigestEnvelope);
            }
            encoder.sha256(&digest_bytes(
                &subject.digest.value,
                MalformedEvaluation::ArtifactDigestValue,
            )?);
            encoder.string(&subject.digest.algorithm, MAX_DESCRIPTOR_TOKEN_BYTES)?;
            encoder.string(&subject.digest.canonicalizer, MAX_DESCRIPTOR_TOKEN_BYTES)?;
            // The scope, copied from the subject Artifact's own bytes. Fields 6-20 begin after it,
            // identical in position, width and bound for both arms -- the arms of a tagged union
            // may differ in width, and everything past the union does not.
            encoder.uuid(&subject.project_id);
            encoder.uuid(&subject.mission_id);
        }
    }

    // 6-7: the evaluator, as a Resident under one exact configuration.
    encoder.uuid(&payload.evaluator.resident_id);
    encoder.uuid(&payload.evaluator.configuration_id);

    // 8-11: the rubric, as declared. Beacon resolves none of it.
    if payload.rubric.id.is_empty() {
        return Err(MalformedEvaluation::RubricId);
    }
    encoder.string(&payload.rubric.id, MAX_DESCRIPTOR_TOKEN_BYTES)?;
    if payload.rubric.version < 1 {
        return Err(MalformedEvaluation::RubricVersion);
    }
    encoder.u32(payload.rubric.version);
    if payload.rubric.canonicalizer.is_empty() {
        return Err(MalformedEvaluation::RubricCanonicalizer);
    }
    encoder.string(&payload.rubric.canonicalizer, MAX_DESCRIPTOR_TOKEN_BYTES)?;
    encoder.sha256(&digest_bytes(
        &payload.rubric.sha256,
        MalformedEvaluation::RubricDigest,
    )?);

    // 12: a SET -- ascending raw-UUID order, duplicates refused -- so a permutation of the same
    // inputs cannot produce a different identity for the same evaluation.
    if payload.cited_runs.len() > MAX_CITED_RUNS {
        return Err(MalformedEvaluation::CitedRunCount);
    }
    if payload
        .cited_runs
        .windows(2)
        .any(|pair| pair[0].as_bytes() >= pair[1].as_bytes())
    {
        return Err(MalformedEvaluation::CitedRunOrder);
    }
    encoder.u32(
        u32::try_from(payload.cited_runs.len()).map_err(|_| MalformedEvaluation::CitedRunCount)?,
    );
    for run in &payload.cited_runs {
        encoder.uuid(run);
    }

    // 13.
    encoder.tag(payload.verdict.tag());

    // 14-16: a SEQUENCE -- the evaluator wrote them in an order and re-sorting them would be the
    // interface deciding what mattered.
    if payload.findings.len() > MAX_FINDINGS {
        return Err(MalformedEvaluation::FindingCount);
    }
    encoder
        .u32(u32::try_from(payload.findings.len()).map_err(|_| MalformedEvaluation::FindingCount)?);
    for finding in &payload.findings {
        encoder.tag(finding.severity.tag());
        if !is_human_text(&finding.summary, MAX_SUMMARY_SCALARS) {
            return Err(MalformedEvaluation::FindingSummary);
        }
        encoder.string(&finding.summary, MAX_SUMMARY_BYTES)?;
    }

    // 17: absence is a value here, so it encodes as one.
    match &payload.conflicts {
        Conflicts::None => encoder.tag(TAG_CONFLICTS_NONE),
        Conflicts::Declared(text) => {
            encoder.tag(TAG_CONFLICTS_DECLARED);
            if !is_human_text(text, MAX_CONFLICTS_SCALARS) {
                return Err(MalformedEvaluation::ConflictsText);
            }
            encoder.string(text, MAX_CONFLICTS_BYTES)?;
        }
    }

    // 18.
    if !is_human_text(&payload.rationale, MAX_RATIONALE_SCALARS) {
        return Err(MalformedEvaluation::Rationale);
    }
    encoder.string(&payload.rationale, MAX_RATIONALE_BYTES)?;

    // 19: absence is a value here too.
    match &payload.supersedes {
        EvaluationSupersedes::None => encoder.tag(TAG_SUPERSEDES_NONE),
        EvaluationSupersedes::Evaluation(digest) => {
            encoder.tag(TAG_SUPERSEDES_EVALUATION);
            encoder.sha256(&digest_bytes(
                digest,
                MalformedEvaluation::SupersedesDigest,
            )?);
        }
    }

    // 20.
    encoder.u64(payload.evaluated_at_ms);

    // The record's own bound. Per-field limits admit a record the store cannot hold, so the
    // grammar is only total with this check -- and it is the encoder's, not SQL's, so a refusal
    // is a fact about the bytes rather than a database error surfacing through a Command.
    if encoder.out.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedEvaluation::RecordTooLarge);
    }
    Ok(encoder.out)
}

/// Strict: every field is checked against its own vocabulary and bound, and bytes that decode to a
/// value this grammar would not have produced are refused rather than normalised.
fn decode(input: &[u8]) -> Result<EvaluationV1, MalformedEvaluation> {
    // Checked BEFORE parsing: bytes can arrive from a store rather than from this encoder, and a
    // decoder that measures after splitting has already done the work it meant to refuse.
    if input.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedEvaluation::RecordTooLarge);
    }
    let mut decoder = Decoder::new(input);
    decoder.magic()?;

    // An unassigned tag is refused rather than defaulted: `0x02` is the Candidate arm's reserved
    // position and `0x04`+ are unassigned, and a decoder that fell through to `proposal` would read
    // some future arm's bytes as a Proposal Evaluation.
    let subject = match decoder.tag()? {
        TAG_SUBJECT_PROPOSAL => {
            let value = HEXLOWER.encode(&decoder.sha256()?);
            let algorithm = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
            let canonicalizer = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
            if algorithm != PROPOSAL_DIGEST_ALGORITHM || canonicalizer != PROPOSAL_CANONICALIZER {
                return Err(MalformedEvaluation::ProposalDigestEnvelope);
            }
            EvaluationSubject::Proposal(ProposalDigest {
                algorithm,
                canonicalizer,
                value,
            })
        }
        TAG_SUBJECT_ARTIFACT => {
            let value = HEXLOWER.encode(&decoder.sha256()?);
            let algorithm = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
            let canonicalizer = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
            if algorithm != PROPOSAL_DIGEST_ALGORITHM || canonicalizer != ARTIFACT_CANONICALIZER {
                return Err(MalformedEvaluation::ArtifactDigestEnvelope);
            }
            EvaluationSubject::Artifact(ArtifactSubject {
                digest: ArtifactDigest {
                    algorithm,
                    canonicalizer,
                    value,
                },
                project_id: decoder.uuid()?,
                mission_id: decoder.uuid()?,
            })
        }
        _ => return Err(MalformedEvaluation::Subject),
    };

    let evaluator = Evaluator {
        resident_id: decoder.uuid()?,
        configuration_id: decoder.uuid()?,
    };

    let rubric_id = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
    if rubric_id.is_empty() {
        return Err(MalformedEvaluation::RubricId);
    }
    let rubric_version = decoder.u32()?;
    if rubric_version < 1 {
        return Err(MalformedEvaluation::RubricVersion);
    }
    let rubric_canonicalizer = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
    if rubric_canonicalizer.is_empty() {
        return Err(MalformedEvaluation::RubricCanonicalizer);
    }
    let rubric_sha256 = HEXLOWER.encode(&decoder.sha256()?);

    let declared = decoder.u32()?;
    let cited_count = decoder.count(
        declared,
        MAX_CITED_RUNS,
        16,
        MalformedEvaluation::CitedRunCount,
    )?;
    let mut cited_runs = Vec::new();
    for _ in 0..cited_count {
        let run = decoder.uuid()?;
        if let Some(previous) = cited_runs.last() {
            let previous: &Uuid = previous;
            if previous.as_bytes() >= run.as_bytes() {
                return Err(MalformedEvaluation::CitedRunOrder);
            }
        }
        cited_runs.push(run);
    }

    let verdict = Verdict::from_tag(decoder.tag()?).ok_or(MalformedEvaluation::Verdict)?;

    let declared = decoder.u32()?;
    let finding_count =
        decoder.count(declared, MAX_FINDINGS, 9, MalformedEvaluation::FindingCount)?;
    let mut findings = Vec::new();
    for _ in 0..finding_count {
        let severity = FindingSeverity::from_tag(decoder.tag()?)
            .ok_or(MalformedEvaluation::FindingSeverity)?;
        let summary = decoder.string(MAX_SUMMARY_BYTES)?;
        if !is_human_text(&summary, MAX_SUMMARY_SCALARS) {
            return Err(MalformedEvaluation::FindingSummary);
        }
        findings.push(Finding { severity, summary });
    }

    let conflicts = match decoder.tag()? {
        TAG_CONFLICTS_NONE => Conflicts::None,
        TAG_CONFLICTS_DECLARED => {
            let text = decoder.string(MAX_CONFLICTS_BYTES)?;
            if !is_human_text(&text, MAX_CONFLICTS_SCALARS) {
                return Err(MalformedEvaluation::ConflictsText);
            }
            Conflicts::Declared(text)
        }
        _ => return Err(MalformedEvaluation::Conflicts),
    };

    let rationale = decoder.string(MAX_RATIONALE_BYTES)?;
    if !is_human_text(&rationale, MAX_RATIONALE_SCALARS) {
        return Err(MalformedEvaluation::Rationale);
    }

    let supersedes = match decoder.tag()? {
        TAG_SUPERSEDES_NONE => EvaluationSupersedes::None,
        TAG_SUPERSEDES_EVALUATION => {
            EvaluationSupersedes::Evaluation(HEXLOWER.encode(&decoder.sha256()?))
        }
        _ => return Err(MalformedEvaluation::Supersedes),
    };

    let evaluated_at_ms = decoder.u64()?;
    decoder.finish()?;

    Ok(EvaluationV1 {
        subject,
        evaluator,
        rubric: RubricDeclaration {
            id: rubric_id,
            version: rubric_version,
            canonicalizer: rubric_canonicalizer,
            sha256: rubric_sha256,
        },
        cited_runs,
        verdict,
        findings,
        conflicts,
        rationale,
        supersedes,
        evaluated_at_ms,
    })
}

// ---------------------------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------------------------

/// The audit Event's payload, as a CLOSED type rather than an open `serde_json::Value`.
///
/// `deny_unknown_fields` is the point: an Event carrying an extra field is refused rather than
/// ignored, so a payload nobody wrote cannot ride alongside one Beacon did. It carries the exact
/// canonical bytes, because an Event that names a digest can only be checked against the row it
/// names -- and then the Event proves nothing the row does not already say.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RecordedEvaluation {
    digest: EvaluationDigestPayload,
    /// The exact canonical bytes, as canonical RFC 4648 padded base64.
    canonical_bytes_base64: String,
    proposal_digest: String,
    verdict: String,
    evaluated_at_ms: u64,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct EvaluationDigestPayload {
    algorithm: String,
    canonicalizer: String,
    value: String,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct EvaluationRecordedPayload {
    evaluation: RecordedEvaluation,
}

struct StoredEvaluation {
    workspace_id: Uuid,
    digest_value: String,
    canonical_bytes: Vec<u8>,
    mission_id: Uuid,
    payload: EvaluationV1,
}

fn insert_evaluation(
    connection: &Connection,
    stored: &StoredEvaluation,
) -> Result<(), EvaluationError> {
    let (conflicts_declared, conflicts_text) = match &stored.payload.conflicts {
        Conflicts::None => (0_i64, None),
        Conflicts::Declared(text) => (1_i64, Some(text.clone())),
    };
    // Every subject column is derived HERE, from the payload, in one place: a caller cannot supply
    // a projection beside the bytes, and a new arm adds one match arm rather than a new call site.
    let (subject_kind, proposal, artifact) = match &stored.payload.subject {
        EvaluationSubject::Proposal(digest) => ("proposal", Some(digest), None),
        EvaluationSubject::Artifact(subject) => ("artifact", None, Some(subject)),
    };
    connection.execute(
        "INSERT INTO evaluations (
             workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
             subject_kind, subject_digest,
             proposal_digest, proposal_digest_algorithm, proposal_digest_canonicalizer,
             artifact_digest, artifact_digest_algorithm, artifact_digest_canonicalizer,
             artifact_project_id,
             mission_id, evaluator_resident_id, evaluator_configuration_id,
             rubric_id, rubric_version, rubric_canonicalizer, rubric_sha256,
             verdict, conflicts_declared, conflicts_text, rationale, evaluated_at_ms
         ) VALUES (
             ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?21, ?22, ?23, ?24, ?11, ?12, ?13, ?14,
             ?15, ?16, ?17, ?18, ?19, ?20, ?25, ?26
         )",
        params![
            stored.workspace_id.to_string(),
            DIGEST_ALGORITHM,
            CANONICALIZER,
            stored.digest_value,
            stored.canonical_bytes,
            subject_kind,
            stored.payload.subject.digest_value(),
            proposal.map(|digest| digest.value.clone()),
            proposal.map(|digest| digest.algorithm.clone()),
            proposal.map(|digest| digest.canonicalizer.clone()),
            stored.mission_id.to_string(),
            stored.payload.evaluator.resident_id.to_string(),
            stored.payload.evaluator.configuration_id.to_string(),
            stored.payload.rubric.id,
            stored.payload.rubric.version,
            stored.payload.rubric.canonicalizer,
            stored.payload.rubric.sha256,
            stored.payload.verdict.as_str(),
            conflicts_declared,
            conflicts_text,
            artifact.map(|subject| subject.digest.value.clone()),
            artifact.map(|subject| subject.digest.algorithm.clone()),
            artifact.map(|subject| subject.digest.canonicalizer.clone()),
            artifact.map(|subject| subject.project_id.to_string()),
            stored.payload.rationale,
            i64::try_from(stored.payload.evaluated_at_ms)
                .map_err(|_| EvaluationError::InvalidStore)?,
        ],
    )?;
    for (ordinal, finding) in stored.payload.findings.iter().enumerate() {
        connection.execute(
            "INSERT INTO evaluation_findings (
                 workspace_id, evaluation_digest, ordinal, severity, summary
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                stored.workspace_id.to_string(),
                stored.digest_value,
                i64::try_from(ordinal).map_err(|_| EvaluationError::InvalidStore)?,
                finding.severity.as_str(),
                finding.summary,
            ],
        )?;
    }
    for run in &stored.payload.cited_runs {
        connection.execute(
            "INSERT INTO evaluation_cited_runs (workspace_id, evaluation_digest, run_id)
             VALUES (?1, ?2, ?3)",
            params![
                stored.workspace_id.to_string(),
                stored.digest_value,
                run.to_string(),
            ],
        )?;
    }
    let event = serde_json::to_string(&EvaluationRecordedPayload {
        evaluation: RecordedEvaluation {
            digest: EvaluationDigestPayload {
                algorithm: DIGEST_ALGORITHM.to_owned(),
                canonicalizer: CANONICALIZER.to_owned(),
                value: stored.digest_value.clone(),
            },
            canonical_bytes_base64: BASE64.encode(&stored.canonical_bytes),
            proposal_digest: stored
                .payload
                .proposal_subject()
                .ok_or(EvaluationError::SubjectNotStorable)?
                .value
                .clone(),
            verdict: stored.payload.verdict.as_str().to_owned(),
            evaluated_at_ms: stored.payload.evaluated_at_ms,
        },
    })
    .map_err(|_| EvaluationError::InvalidStore)?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'evaluation.recorded', ?2, ?3)",
        params![
            stored.workspace_id.to_string(),
            event,
            i64::try_from(stored.payload.evaluated_at_ms)
                .map_err(|_| EvaluationError::InvalidStore)?,
        ],
    )?;
    Ok(())
}

/// One stored row, as columns. Nothing here is believed until `verified_evaluation` proves it
/// against the bytes.
struct StoredRow {
    digest_value: String,
    canonical_bytes: Vec<u8>,
    subject_kind: String,
    subject_digest: String,
    proposal_digest_algorithm: Option<String>,
    proposal_digest_canonicalizer: Option<String>,
    artifact_digest_algorithm: Option<String>,
    artifact_digest_canonicalizer: Option<String>,
    artifact_project_id: Option<String>,
    mission_id: String,
    evaluator_resident_id: String,
    evaluator_configuration_id: String,
    rubric_id: String,
    rubric_version: u32,
    rubric_canonicalizer: String,
    rubric_sha256: String,
    verdict: String,
    conflicts_declared: i64,
    conflicts_text: Option<String>,
    rationale: String,
    evaluated_at_ms: i64,
}

const SELECT_COLUMNS: &str = "digest_value, canonical_bytes, subject_kind, subject_digest,
     proposal_digest_algorithm, proposal_digest_canonicalizer,
     artifact_digest_algorithm, artifact_digest_canonicalizer, artifact_project_id,
     mission_id, evaluator_resident_id,
     evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer, rubric_sha256,
     verdict, conflicts_declared, conflicts_text, rationale, evaluated_at_ms";

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRow> {
    Ok(StoredRow {
        digest_value: row.get(0)?,
        canonical_bytes: row.get(1)?,
        subject_kind: row.get(2)?,
        subject_digest: row.get(3)?,
        proposal_digest_algorithm: row.get(4)?,
        proposal_digest_canonicalizer: row.get(5)?,
        artifact_digest_algorithm: row.get(6)?,
        artifact_digest_canonicalizer: row.get(7)?,
        artifact_project_id: row.get(8)?,
        mission_id: row.get(9)?,
        evaluator_resident_id: row.get(10)?,
        evaluator_configuration_id: row.get(11)?,
        rubric_id: row.get(12)?,
        rubric_version: row.get(13)?,
        rubric_canonicalizer: row.get(14)?,
        rubric_sha256: row.get(15)?,
        verdict: row.get(16)?,
        conflicts_declared: row.get(17)?,
        conflicts_text: row.get(18)?,
        rationale: row.get(19)?,
        evaluated_at_ms: row.get(20)?,
    })
}

fn load_row(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<StoredRow>, EvaluationError> {
    Ok(connection
        .query_row(
            &format!(
                "SELECT {SELECT_COLUMNS} FROM evaluations
                 WHERE workspace_id = ?1 AND digest_value = ?2"
            ),
            params![workspace_id.to_string(), digest_value],
            read_row,
        )
        .optional()?)
}

/// Every read re-derives the identity from the stored bytes, re-encodes the parsed value and
/// requires byte equality, and compares every indexed column to what the bytes say.
///
/// The Mission is the one column with no counterpart in the bytes, so it is compared against the
/// bound **Proposal** instead -- at every read and at startup. Without that a wrong Mission would
/// be invisible to both.
fn verified_evaluation(
    connection: &Connection,
    workspace_id: Uuid,
    row: StoredRow,
) -> Result<Evaluation, EvaluationError> {
    if HEXLOWER.encode(&sha256(&row.canonical_bytes)) != row.digest_value {
        return Err(EvaluationError::Inconsistent(
            InconsistentEvaluation::DigestMismatch,
        ));
    }
    let payload = decode(&row.canonical_bytes).map_err(EvaluationError::Malformed)?;
    if encode(&payload).map_err(EvaluationError::Malformed)? != row.canonical_bytes {
        return Err(EvaluationError::Inconsistent(
            InconsistentEvaluation::BytesNotCanonical,
        ));
    }
    if !columns_match(&row, &payload) {
        return Err(EvaluationError::Inconsistent(
            InconsistentEvaluation::ColumnDisagreesWithBytes,
        ));
    }
    // The subject record, read through ITS owning verified reader, and every derived column
    // re-derived against what that record's own bytes say. For the Artifact arm the derivation is
    // one hop through a different record -- the same discipline, not a weaker one.
    let facts = subject_facts_for_read(connection, workspace_id, &payload.subject)?;
    if facts.subject != payload.subject {
        return Err(EvaluationError::Inconsistent(
            InconsistentEvaluation::ProposalEnvelopeDisagrees,
        ));
    }
    if row.mission_id != facts.mission_id.to_string() {
        return Err(EvaluationError::Inconsistent(
            InconsistentEvaluation::MissionDisagreesWithProposal,
        ));
    }
    // Re-derived on EVERY read, even though the triggers forbid it. Reconciliation must be complete
    // on its own terms rather than on the order the store's checks happen to have run -- ADR 0037
    // re-derives two store-forbidden properties for exactly this reason. The evaluator here comes
    // from the canonical BYTES, so this is the layer that proves the projection the trigger saw was
    // the projection the bytes describe.
    if payload.evaluator.resident_id == facts.author_resident_id {
        return Err(EvaluationError::Inconsistent(match payload.subject {
            EvaluationSubject::Proposal(_) => InconsistentEvaluation::EvaluatorAuthoredTheProposal,
            EvaluationSubject::Artifact(_) => InconsistentEvaluation::EvaluatorAuthoredTheArtifact,
        }));
    }
    verify_children(connection, workspace_id, &row.digest_value, &payload)?;

    // The relation and the byte tag must agree. Checked on every read rather than only at startup,
    // so a row and its relation cannot disagree even between two restarts.
    let recorded_predecessor: Option<String> = connection
        .query_row(
            "SELECT supersedes_evaluation_digest FROM evaluation_supersessions
             WHERE workspace_id = ?1 AND subject_digest = ?2 AND evaluation_digest = ?3",
            params![
                workspace_id.to_string(),
                row.subject_digest.as_str(),
                row.digest_value.as_str()
            ],
            |row| row.get(0),
        )
        .optional()?;
    let declared = match &payload.supersedes {
        EvaluationSupersedes::None => None,
        EvaluationSupersedes::Evaluation(digest) => Some(digest.clone()),
    };
    if recorded_predecessor != declared {
        return Err(EvaluationError::Inconsistent(
            InconsistentEvaluation::SupersessionDisagreesWithBytes,
        ));
    }

    let force = match &facts.proposal_bindings {
        Some(bindings) => derive_force(connection, workspace_id, &row, bindings)?,
        None => EvaluationForce {
            superseded_by: superseded_by(connection, workspace_id, &row)?,
            drift: Vec::new(),
        },
    };
    Ok(Evaluation {
        digest: EvaluationDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: CANONICALIZER.to_owned(),
            value: row.digest_value,
        },
        canonical_bytes: row.canonical_bytes,
        payload,
        force,
    })
}

/// Supersession and drift, derived at read time.
///
/// Drift is about the bound PROPOSAL's revisions, because an Evaluation's own grammar carries no
/// bindings -- it judged the exact bytes a Proposal digest names, and those bytes name the
/// revisions. A Repository rebind that keeps the same pinned commit reports only the revision as
/// moved, one drift and not two; that distinction is the same one Ruling v1 draws, repeated here
/// rather than shared because the two records answer different questions about the same columns.
/// The successor link, for either arm: chains are per subject digest and carry no kind.
fn superseded_by(
    connection: &Connection,
    workspace_id: Uuid,
    row: &StoredRow,
) -> Result<Option<EvaluationDigest>, EvaluationError> {
    Ok(connection
        .query_row(
            "SELECT evaluation_digest FROM evaluation_supersessions
             WHERE workspace_id = ?1 AND subject_digest = ?2
               AND supersedes_evaluation_digest = ?3",
            params![
                workspace_id.to_string(),
                row.subject_digest.as_str(),
                row.digest_value.as_str()
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| EvaluationDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: CANONICALIZER.to_owned(),
            value,
        }))
}

fn derive_force(
    connection: &Connection,
    workspace_id: Uuid,
    row: &StoredRow,
    facts: &crate::proposal::ProposalBindingFacts,
) -> Result<EvaluationForce, EvaluationError> {
    let superseded_by = connection
        .query_row(
            "SELECT evaluation_digest FROM evaluation_supersessions
             WHERE workspace_id = ?1 AND subject_digest = ?2
               AND supersedes_evaluation_digest = ?3",
            params![
                workspace_id.to_string(),
                row.subject_digest.as_str(),
                row.digest_value.as_str()
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| EvaluationDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: CANONICALIZER.to_owned(),
            value,
        });

    let mut drift = Vec::new();
    let (current_charter, latest_repository) =
        crate::project_repository::latest_repository_for_preflight(
            connection,
            workspace_id,
            facts.project_id,
        )
        .map_err(|_| EvaluationError::InvalidStore)?;
    if current_charter != facts.bindings.charter_revision {
        drift.push(UnmetPrecondition::CharterRevisionMoved);
    }
    let policy = crate::project_policy::policy_revision_facts(
        connection,
        workspace_id,
        facts.project_id,
        facts.bindings.project_policy_revision,
    )
    .map_err(|_| EvaluationError::InvalidStore)?;
    if !policy.is_some_and(|facts| facts.is_current) {
        drift.push(UnmetPrecondition::ProjectPolicyRevisionMoved);
    }
    match latest_repository {
        Some(binding) if binding.revision == facts.bindings.repository_revision => {
            if binding.base_revision != facts.bindings.pinned_base_revision {
                drift.push(UnmetPrecondition::BaseRevisionMoved);
            }
        }
        Some(binding) => {
            drift.push(UnmetPrecondition::RepositoryRevisionMoved);
            if binding.base_revision != facts.bindings.pinned_base_revision {
                drift.push(UnmetPrecondition::BaseRevisionMoved);
            }
        }
        None => {
            drift.push(UnmetPrecondition::RepositoryRevisionMoved);
            drift.push(UnmetPrecondition::BaseRevisionMoved);
        }
    }
    Ok(EvaluationForce {
        superseded_by,
        drift,
    })
}

/// Every Evaluation recorded against one Proposal, in **chain order, oldest first**.
///
/// Chain order, never timestamp order: a timestamp is evidence about when, and the chain is what
/// the records say about each other. Two Evaluations can land in the same millisecond; only one of
/// them supersedes the other.
/// The supersession chain for one SUBJECT digest.
///
/// Chains are per subject, never per kind: a new Artifact version is a different subject, and the
/// old digest's Evaluations stay true of the old bytes rather than carrying over (R12.10).
fn evaluation_chain(
    connection: &Connection,
    workspace_id: Uuid,
    subject_digest: &str,
) -> Result<Vec<String>, EvaluationError> {
    let mut statement = connection.prepare(
        "SELECT digest_value FROM evaluations
         WHERE workspace_id = ?1 AND subject_digest = ?2",
    )?;
    let all = statement
        .query_map(params![workspace_id.to_string(), subject_digest], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<HashSet<_>, _>>()?;
    drop(statement);
    if all.is_empty() {
        return Ok(Vec::new());
    }

    let mut statement = connection.prepare(
        "SELECT supersedes_evaluation_digest, evaluation_digest FROM evaluation_supersessions
         WHERE workspace_id = ?1 AND subject_digest = ?2",
    )?;
    let links = statement
        .query_map(params![workspace_id.to_string(), subject_digest], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    // Three shapes the store already forbids, refused again here rather than relied upon, because
    // this walk has to be correct on its own terms: a predecessor cited twice (the relation's
    // unique index), an Evaluation citing two predecessors (its primary key), and a predecessor
    // outside this Proposal's set (the scoped composite foreign keys). No store state reaches
    // them, so no test here can either.
    let mut successor: HashMap<String, String> = HashMap::new();
    let mut cited: HashSet<String> = HashSet::new();
    for (superseded, successor_digest) in links {
        if successor
            .insert(superseded.clone(), successor_digest.clone())
            .is_some()
            || !cited.insert(successor_digest)
            || !all.contains(&superseded)
        {
            return Err(EvaluationError::InvalidStore);
        }
    }

    let Some(head) = all.iter().find(|digest| !cited.contains(*digest)).cloned() else {
        // Every Evaluation cites a predecessor, so the links form a cycle and there is no first.
        return Err(EvaluationError::InvalidStore);
    };

    let mut chain = Vec::with_capacity(all.len());
    let mut cursor = Some(head);
    while let Some(digest) = cursor {
        cursor = successor.get(&digest).cloned();
        chain.push(digest);
        // A bound on the walk, not a check on the store: in-edges are unique and the head has
        // none, so this is a simple path and cannot revisit. If a store ever violated that, this
        // returns rather than hanging.
        if chain.len() > all.len() {
            return Err(EvaluationError::InvalidStore);
        }
    }
    // The one comparison that refuses a FORK. A second head means at least one Evaluation is
    // unreachable from the first, so the walk always comes up short.
    if chain.len() != all.len() {
        return Err(EvaluationError::InvalidStore);
    }
    Ok(chain)
}

fn columns_match(row: &StoredRow, payload: &EvaluationV1) -> bool {
    let (declared, text) = match &payload.conflicts {
        Conflicts::None => (0_i64, None),
        Conflicts::Declared(value) => (1_i64, Some(value.as_str())),
    };
    // Per arm, and both halves: the populated arm agrees with the bytes AND the other arm is
    // absent. A row that carried both would satisfy a one-sided comparison.
    let subject_matches = match &payload.subject {
        EvaluationSubject::Proposal(digest) => {
            row.subject_kind == "proposal"
                && row.subject_digest == digest.value
                && row.proposal_digest_algorithm.as_deref() == Some(digest.algorithm.as_str())
                && row.proposal_digest_canonicalizer.as_deref()
                    == Some(digest.canonicalizer.as_str())
                && row.artifact_digest_algorithm.is_none()
                && row.artifact_digest_canonicalizer.is_none()
                && row.artifact_project_id.is_none()
        }
        EvaluationSubject::Artifact(subject) => {
            row.subject_kind == "artifact"
                && row.subject_digest == subject.digest.value
                && row.artifact_digest_algorithm.as_deref()
                    == Some(subject.digest.algorithm.as_str())
                && row.artifact_digest_canonicalizer.as_deref()
                    == Some(subject.digest.canonicalizer.as_str())
                && row.artifact_project_id.as_deref()
                    == Some(subject.project_id.to_string().as_str())
                && row.proposal_digest_algorithm.is_none()
                && row.proposal_digest_canonicalizer.is_none()
        }
    };
    subject_matches
        && row.evaluator_resident_id == payload.evaluator.resident_id.to_string()
        && row.evaluator_configuration_id == payload.evaluator.configuration_id.to_string()
        && row.rubric_id == payload.rubric.id
        && row.rubric_version == payload.rubric.version
        && row.rubric_canonicalizer == payload.rubric.canonicalizer
        && row.rubric_sha256 == payload.rubric.sha256
        && Verdict::from_str(&row.verdict) == Some(payload.verdict)
        && row.conflicts_declared == declared
        && row.conflicts_text.as_deref() == text
        && row.rationale == payload.rationale
        && u64::try_from(row.evaluated_at_ms).ok() == Some(payload.evaluated_at_ms)
}

/// The findings and cited Runs stored relationally must be exactly the ones in the bytes -- same
/// entries, same order for findings, same set for Runs, and no extra rows.
fn verify_children(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
    payload: &EvaluationV1,
) -> Result<(), EvaluationError> {
    let mut statement = connection.prepare(
        "SELECT ordinal, severity, summary FROM evaluation_findings
         WHERE workspace_id = ?1 AND evaluation_digest = ?2
         ORDER BY ordinal",
    )?;
    let stored: Vec<(i64, String, String)> = statement
        .query_map(params![workspace_id.to_string(), digest_value], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    if stored.len() != payload.findings.len() {
        return Err(EvaluationError::Inconsistent(
            InconsistentEvaluation::FindingsDisagreeWithBytes,
        ));
    }
    for (index, (ordinal, severity, summary)) in stored.iter().enumerate() {
        let expected = &payload.findings[index];
        if usize::try_from(*ordinal).ok() != Some(index)
            || FindingSeverity::from_str(severity) != Some(expected.severity)
            || summary != &expected.summary
        {
            return Err(EvaluationError::Inconsistent(
                InconsistentEvaluation::FindingsDisagreeWithBytes,
            ));
        }
    }

    let mut statement = connection.prepare(
        "SELECT run_id FROM evaluation_cited_runs
         WHERE workspace_id = ?1 AND evaluation_digest = ?2",
    )?;
    let stored: HashSet<String> = statement
        .query_map(params![workspace_id.to_string(), digest_value], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<_, _>>()?;
    let expected: HashSet<String> = payload
        .cited_runs
        .iter()
        .map(|run| run.to_string())
        .collect();
    if stored != expected {
        return Err(EvaluationError::Inconsistent(
            InconsistentEvaluation::CitedRunsDisagreeWithBytes,
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------------------------

/// Recording an Evaluation records that somebody judged. It grants nothing, and a read of what it
/// recorded is not a permission check.
pub struct EvaluationService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

/// What a caller supplies. `evaluated_at_ms` is deliberately absent: the service clock owns it,
/// because a caller-supplied time would let an Evaluation recorded after a Ruling be backdated
/// through the ordinary Command.
///
/// `supersedes` names the Evaluation this one replaces, and must be the CURRENT END of that
/// Proposal's chain: superseding a link that has already been superseded would fork it, and a fork
/// has no last link, so "the effective Evaluation" would stop having an answer.
/// What a caller names as the subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluationSubjectDraft {
    Proposal(String),
    Artifact(String),
}

impl EvaluationSubjectDraft {
    fn digest(&self) -> &str {
        match self {
            Self::Proposal(digest) | Self::Artifact(digest) => digest,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EvaluationDraft {
    /// Which record this Evaluation is about, by digest. The kind is named because the two are
    /// read from different stores; everything else about the subject -- its envelope, its Mission
    /// scope, its author -- is DERIVED from the stored record rather than supplied beside it.
    pub subject: EvaluationSubjectDraft,
    pub evaluator: Evaluator,
    pub rubric: RubricDeclaration,
    pub cited_runs: Vec<Uuid>,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
    pub conflicts: Conflicts,
    pub rationale: String,
    pub supersedes: EvaluationSupersedes,
}

/// What a subject record says about itself, read from its own verified bytes.
///
/// One shape for both arms so the Command reads the same four facts either way: the subject as it
/// will be recorded, the Mission the roster check needs, the author non-authorship excludes, and
/// nothing a caller supplied.
struct SubjectFacts {
    subject: EvaluationSubject,
    mission_id: Uuid,
    author_resident_id: Uuid,
    /// Present only for the Proposal arm. DRIFT IS EMPTY BY CONSTRUCTION for an Artifact subject:
    /// the record carries no Charter, Policy, Repository or base revision, so there is nothing that
    /// could have moved under it. `None` here is that fact, not a missing lookup.
    proposal_bindings: Option<crate::proposal::ProposalBindingFacts>,
}

/// The same four facts, for a subject already recorded: the read path knows the arm from the bytes
/// rather than from a caller. One function would need two shapes of input; two functions that share
/// `SubjectFacts` keep the comparison identical on both sides.
fn subject_facts_for_read(
    connection: &Connection,
    workspace_id: Uuid,
    subject: &EvaluationSubject,
) -> Result<SubjectFacts, EvaluationError> {
    let draft = match subject {
        EvaluationSubject::Proposal(digest) => {
            EvaluationSubjectDraft::Proposal(digest.value.clone())
        }
        EvaluationSubject::Artifact(subject) => {
            EvaluationSubjectDraft::Artifact(subject.digest.value.clone())
        }
    };
    subject_facts(connection, workspace_id, &draft)
}

fn subject_facts(
    connection: &Connection,
    workspace_id: Uuid,
    draft: &EvaluationSubjectDraft,
) -> Result<SubjectFacts, EvaluationError> {
    match draft {
        EvaluationSubjectDraft::Proposal(digest) => {
            let Some(facts) = proposal_binding_facts(connection, workspace_id, digest)
                .map_err(map_proposal_error)?
            else {
                return Err(EvaluationError::Inconsistent(
                    InconsistentEvaluation::ProposalNotFound,
                ));
            };
            Ok(SubjectFacts {
                subject: EvaluationSubject::Proposal(facts.digest.clone()),
                mission_id: facts.mission_id,
                author_resident_id: facts.author_resident_id,
                proposal_bindings: Some(facts),
            })
        }
        EvaluationSubjectDraft::Artifact(digest) => {
            let Some(record) =
                crate::artifact::verified_artifact_within(connection, workspace_id, digest)
                    .map_err(|_| EvaluationError::InvalidStore)?
            else {
                return Err(EvaluationError::Inconsistent(
                    InconsistentEvaluation::ArtifactNotFound,
                ));
            };
            // An Artifact with `context = none` has no roster to check an evaluator against, so it
            // is not an Evaluation subject in v1. The check cannot be performed, and performing a
            // weaker one instead is how "the evaluator was a member" becomes a sentence nobody
            // proved (design section 4).
            let crate::artifact::ArtifactContext::Mission {
                project_id,
                mission_id,
            } = record.payload().context
            else {
                return Err(EvaluationError::Inconsistent(
                    InconsistentEvaluation::ArtifactHasNoMissionScope,
                ));
            };
            Ok(SubjectFacts {
                subject: EvaluationSubject::Artifact(ArtifactSubject {
                    digest: record.digest().clone(),
                    project_id,
                    mission_id,
                }),
                mission_id,
                author_resident_id: record.payload().accountable_resident_id,
                proposal_bindings: None,
            })
        }
    }
}

impl EvaluationService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Records one immutable Evaluation of the exact bytes `proposal_digest` names.
    ///
    /// The Owner is authenticated before anything else happens, so an unauthenticated caller
    /// receives only the account refusal whatever else is wrong with the request. The caller's own
    /// values are then checked with the predicates the encoder uses, so a malformed argument never
    /// reaches the store, and because both call the same predicate they cannot disagree.
    pub fn record(&self, draft: EvaluationDraft) -> Result<Evaluation, EvaluationError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let subject_digest = draft.subject.digest().to_owned();
            digest_bytes(
                &subject_digest,
                match draft.subject {
                    EvaluationSubjectDraft::Proposal(_) => MalformedEvaluation::ProposalDigestValue,
                    EvaluationSubjectDraft::Artifact(_) => MalformedEvaluation::ArtifactDigestValue,
                },
            )
            .map_err(EvaluationError::Malformed)?;
            // Validated BEFORE any store lookup. Querying an unvalidated string and reporting
            // "not recorded" collapses a malformed identity into an absent one -- two different
            // mistakes, and the codec already has a distinct arm for the first.
            if let EvaluationSupersedes::Evaluation(digest) = &draft.supersedes {
                digest_bytes(digest, MalformedEvaluation::SupersedesDigest)
                    .map_err(EvaluationError::Malformed)?;
            }
            let evaluated_at_ms = u64::try_from(now).map_err(|_| EvaluationError::InvalidStore)?;

            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

            // The subject must exist in this Workspace and still agree with its own bytes. Its
            // envelope, its Mission and its author are read out of THOSE bytes, never off a row --
            // and for the Artifact arm that is one hop through a different record's verified
            // reader, which is the same discipline and not a weaker one.
            let subject = subject_facts(&transaction, self.workspace_id, &draft.subject)?;

            // Non-authorship, at the Command. The triggers would refuse this write anyway, but a
            // caller deserves the reason rather than a database abort, and the Command is where the
            // two-layer proof starts: this compares against the AUTHOR the subject's verified bytes
            // name, not against a column an earlier write chose. Keyed on the Resident and never on
            // the (Resident, Configuration) pair: an author who reconfigures is still the author.
            if draft.evaluator.resident_id == subject.author_resident_id {
                return Err(EvaluationError::Inconsistent(match draft.subject {
                    EvaluationSubjectDraft::Proposal(_) => {
                        InconsistentEvaluation::EvaluatorAuthoredTheProposal
                    }
                    EvaluationSubjectDraft::Artifact(_) => {
                        InconsistentEvaluation::EvaluatorAuthoredTheArtifact
                    }
                }));
            }

            // Cited Runs are a set in the bytes: sorted here so a caller's order cannot change the
            // identity, and duplicates refused rather than silently folded.
            let mut cited_runs = draft.cited_runs.clone();
            cited_runs.sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            let unique: HashSet<&Uuid> = cited_runs.iter().collect();
            if unique.len() != cited_runs.len() {
                return Err(EvaluationError::Malformed(
                    MalformedEvaluation::CitedRunOrder,
                ));
            }

            // The chain is read and extended inside this one immediate transaction, so two
            // concurrent Evaluations cannot both believe they extend it.
            let chain = evaluation_chain(&transaction, self.workspace_id, &subject_digest)?;
            let tail = chain.last();
            match (tail, &draft.supersedes) {
                (None, EvaluationSupersedes::None) => {}
                (Some(tail), EvaluationSupersedes::Evaluation(cited)) if tail == cited => {}
                (_, EvaluationSupersedes::Evaluation(cited)) => {
                    // Three different mistakes, three different corrections: a digest naming
                    // nothing, one naming another Proposal's Evaluation, and one naming an
                    // already-superseded link of this chain.
                    return Err(EvaluationError::Inconsistent(
                        match recorded_subject_of(&transaction, cited)? {
                            None => InconsistentEvaluation::SupersedesNotRecorded,
                            Some(other) if other != subject_digest => {
                                InconsistentEvaluation::SupersedesOtherSubject
                            }
                            Some(_) => InconsistentEvaluation::NotTheCurrentTail,
                        },
                    ));
                }
                (Some(_), EvaluationSupersedes::None) => {
                    return Err(EvaluationError::Inconsistent(
                        InconsistentEvaluation::NotTheCurrentTail,
                    ));
                }
            }
            if let Some(tail) = tail {
                // Equal times are allowed: two records can land in the same millisecond, and
                // refusing that would invent a precision the clock does not have. Chain order, not
                // timestamp order, is what says which superseded which.
                if evaluated_at_ms < evaluated_at_ms_of(&transaction, tail)? {
                    return Err(EvaluationError::Inconsistent(
                        InconsistentEvaluation::EvaluatedBeforeTail,
                    ));
                }
            }

            let payload = EvaluationV1 {
                // COPIED from the verified subject record, never rebuilt from this module's
                // constants: equality is not provenance. For the Artifact arm that copy includes
                // the Mission scope, which is why an Evaluation never asserts its own Mission.
                subject: subject.subject.clone(),
                evaluator: draft.evaluator,
                rubric: draft.rubric,
                cited_runs,
                verdict: draft.verdict,
                findings: draft.findings,
                conflicts: draft.conflicts,
                rationale: draft.rationale,
                supersedes: draft.supersedes.clone(),
                evaluated_at_ms,
            };
            let canonical_bytes = encode(&payload).map_err(EvaluationError::Malformed)?;
            let digest_value = HEXLOWER.encode(&sha256(&canonical_bytes));

            let stored = StoredEvaluation {
                workspace_id: self.workspace_id,
                digest_value: digest_value.clone(),
                canonical_bytes,
                mission_id: subject.mission_id,
                payload,
            };
            insert_evaluation(&transaction, &stored)?;
            if let EvaluationSupersedes::Evaluation(superseded) = &stored.payload.supersedes {
                transaction.execute(
                    "INSERT INTO evaluation_supersessions (
                         workspace_id, subject_digest, evaluation_digest,
                         supersedes_evaluation_digest
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        self.workspace_id.to_string(),
                        subject_digest,
                        digest_value,
                        superseded,
                    ],
                )?;
            }
            let row = load_row(&transaction, self.workspace_id, &digest_value)?
                .ok_or(EvaluationError::InvalidStore)?;
            let evaluation = verified_evaluation(&transaction, self.workspace_id, row)?;
            transaction.commit()?;
            Ok(evaluation)
        })
    }

    pub fn get(&self, digest: &str) -> Result<Evaluation, EvaluationError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let row = load_row(connection, self.workspace_id, digest)?
                .ok_or(EvaluationError::NotFound)?;
            verified_evaluation(connection, self.workspace_id, row)
        })
    }
}

/// One Evaluation, verified, by digest within a Workspace.
///
/// The shape `verified_proposal_within` and `verified_ruling_within` already have, so a consumer
/// that reads all three does not have to learn a third calling convention. `Ok(None)` is "no such
/// Evaluation in this Workspace" and is a different answer from a refusal.
pub(crate) fn verified_evaluation_within(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<Evaluation>, EvaluationError> {
    let Some(row) = load_row(connection, workspace_id, digest_value)? else {
        return Ok(None);
    };
    verified_evaluation(connection, workspace_id, row).map(Some)
}

fn recorded_subject_of(
    connection: &Connection,
    digest_value: &str,
) -> Result<Option<String>, EvaluationError> {
    Ok(connection
        .query_row(
            "SELECT subject_digest FROM evaluations WHERE digest_value = ?1",
            params![digest_value],
            |row| row.get(0),
        )
        .optional()?)
}

fn evaluated_at_ms_of(connection: &Connection, digest_value: &str) -> Result<u64, EvaluationError> {
    let value: i64 = connection.query_row(
        "SELECT evaluated_at_ms FROM evaluations WHERE digest_value = ?1",
        params![digest_value],
        |row| row.get(0),
    )?;
    u64::try_from(value).map_err(|_| EvaluationError::InvalidStore)
}

/// Startup reconciliation: every stored Evaluation is re-verified against its own bytes, its bound
/// Proposal, and its children, and every row must have exactly one `evaluation.recorded` Event.
pub(crate) fn verify_evaluation_state(connection: &Connection) -> Result<(), EvaluationError> {
    let mut statement = connection.prepare(&format!(
        "SELECT workspace_id, {SELECT_COLUMNS} FROM evaluations ORDER BY digest_value"
    ))?;
    let rows: Vec<(String, StoredRow)> = statement
        .query_map([], |row| {
            let workspace: String = row.get(0)?;
            Ok((
                workspace,
                StoredRow {
                    digest_value: row.get(1)?,
                    canonical_bytes: row.get(2)?,
                    subject_kind: row.get(3)?,
                    subject_digest: row.get(4)?,
                    proposal_digest_algorithm: row.get(5)?,
                    proposal_digest_canonicalizer: row.get(6)?,
                    artifact_digest_algorithm: row.get(7)?,
                    artifact_digest_canonicalizer: row.get(8)?,
                    artifact_project_id: row.get(9)?,
                    mission_id: row.get(10)?,
                    evaluator_resident_id: row.get(11)?,
                    evaluator_configuration_id: row.get(12)?,
                    rubric_id: row.get(13)?,
                    rubric_version: row.get(14)?,
                    rubric_canonicalizer: row.get(15)?,
                    rubric_sha256: row.get(16)?,
                    verdict: row.get(17)?,
                    conflicts_declared: row.get(18)?,
                    conflicts_text: row.get(19)?,
                    rationale: row.get(20)?,
                    evaluated_at_ms: row.get(21)?,
                },
            ))
        })?
        .collect::<Result<_, _>>()?;

    for (workspace, row) in rows {
        let workspace_id =
            Uuid::parse_str(&workspace).map_err(|_| EvaluationError::InvalidStore)?;
        let digest_value = row.digest_value.clone();
        verified_evaluation(connection, workspace_id, row)?;
        let events: Vec<(String, i64)> = connection
            .prepare(
                "SELECT payload_json, occurred_at_ms FROM events
                 WHERE workspace_id = ?1 AND kind = 'evaluation.recorded'
                   AND json_extract(payload_json, '$.evaluation.digest.value') = ?2",
            )?
            .query_map(params![workspace, digest_value.as_str()], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        // Exactly one, and it must bind the exact bytes -- not merely quote the digest.
        let [(payload_json, occurred_at_ms)] = events.as_slice() else {
            return Err(EvaluationError::InvalidStore);
        };
        let occurred_at_ms =
            u64::try_from(*occurred_at_ms).map_err(|_| EvaluationError::InvalidStore)?;
        verify_evaluation_event(connection, workspace_id, occurred_at_ms, payload_json)?;
    }

    // Every supersession chain: one first link, one path, every Evaluation on it, and
    // `evaluated_at_ms` non-decreasing ALONG THE CHAIN -- chain order, never timestamp order.
    let mut statement = connection.prepare(
        "SELECT DISTINCT workspace_id, proposal_digest FROM evaluations ORDER BY proposal_digest",
    )?;
    let chains: Vec<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    drop(statement);
    for (workspace, proposal_digest) in chains {
        let workspace_id =
            Uuid::parse_str(&workspace).map_err(|_| EvaluationError::InvalidStore)?;
        let chain = evaluation_chain(connection, workspace_id, &proposal_digest)?;
        let mut previous: Option<u64> = None;
        for digest in &chain {
            let at = evaluated_at_ms_of(connection, digest)?;
            if previous.is_some_and(|earlier| at < earlier) {
                return Err(EvaluationError::InvalidStore);
            }
            previous = Some(at);
        }
    }

    // No Event may name an Evaluation that is not stored.
    let orphan: i64 = connection.query_row(
        "SELECT count(*) FROM events
         WHERE kind = 'evaluation.recorded'
           AND json_extract(payload_json, '$.evaluation.digest.value') NOT IN (
               SELECT digest_value FROM evaluations
           )",
        [],
        |row| row.get(0),
    )?;
    if orphan != 0 {
        return Err(EvaluationError::InvalidStore);
    }
    Ok(())
}

/// One `evaluation.recorded` Event, checked against the Evaluation it names.
///
/// The Event is not a second source of truth: it must name a stored Evaluation in this Workspace,
/// and the time it claims must be the time those bytes carry. An Event that says something the
/// record does not is a mismatch, never a correction.
pub(crate) fn verify_evaluation_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), EvaluationError> {
    // Strictly typed and closed: an extra or renamed field is refused here, not ignored.
    let payload: EvaluationRecordedPayload =
        serde_json::from_str(payload_json).map_err(|_| EvaluationError::InvalidStore)?;
    let recorded = payload.evaluation;
    if recorded.digest.algorithm != DIGEST_ALGORITHM
        || recorded.digest.canonicalizer != CANONICALIZER
        || recorded.evaluated_at_ms != occurred_at_ms
    {
        return Err(EvaluationError::InvalidStore);
    }
    let canonical_bytes = BASE64
        .decode(recorded.canonical_bytes_base64.as_bytes())
        .map_err(|_| EvaluationError::InvalidStore)?;
    // Canonical base64: a second spelling of the same bytes is refused rather than folded.
    if BASE64.encode(&canonical_bytes) != recorded.canonical_bytes_base64 {
        return Err(EvaluationError::InvalidStore);
    }
    if HEXLOWER.encode(&sha256(&canonical_bytes)) != recorded.digest.value {
        return Err(EvaluationError::InvalidStore);
    }
    let row = load_row(connection, workspace_id, &recorded.digest.value)?
        .ok_or(EvaluationError::InvalidStore)?;
    // Byte-for-byte against the row. Stated honestly: with both sides independently proved to
    // hash to the same digest above and in `verified_evaluation`, this cannot currently fail
    // short of a SHA-256 collision -- removing it leaves every test green, and I checked. It is
    // kept because it is the invariant, not the detector: what makes the Event evidence rather
    // than a pointer is that it CARRIES the bytes, and the day a digest field becomes optional or
    // a second canonicalizer appears, this is the line that still holds.
    if row.canonical_bytes != canonical_bytes {
        return Err(EvaluationError::InvalidStore);
    }
    let evaluation = verified_evaluation(connection, workspace_id, row)?;
    if evaluation.payload.evaluated_at_ms != occurred_at_ms
        || Some(recorded.proposal_digest.as_str())
            != evaluation
                .payload
                .proposal_subject()
                .map(|s| s.value.as_str())
        || recorded.verdict != evaluation.payload.verdict.as_str()
    {
        return Err(EvaluationError::InvalidStore);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_encoding::BASE64;

    /// gpt2 built these values, and the bytes below, from ADR 0038's field table alone -- no
    /// Beacon encoder, decoder, fixture or constant took part -- and cross-checked the result
    /// through coreutils. That is the only order in which a vector proves anything: the grammar
    /// was written down, an independent reader encoded it, and this encoder is measured against
    /// what they produced.
    /// The Proposal arm of a payload the test built as one. Panics otherwise, which is what a test
    /// helper should do when its fixture is not the shape it was written for.
    fn proposal_subject_of(payload: &EvaluationV1) -> &ProposalDigest {
        match &payload.subject {
            EvaluationSubject::Proposal(digest) => digest,
            EvaluationSubject::Artifact(_) => panic!("this fixture is a Proposal subject"),
        }
    }

    fn published_vector_payload() -> EvaluationV1 {
        EvaluationV1 {
            subject: EvaluationSubject::Proposal(ProposalDigest {
                algorithm: DIGEST_ALGORITHM.to_owned(),
                canonicalizer: PROPOSAL_CANONICALIZER.to_owned(),
                value: HEXLOWER.encode(&core::array::from_fn::<u8, 32, _>(|index| index as u8)),
            }),
            evaluator: Evaluator {
                resident_id: Uuid::parse_str("10213243-5465-7687-98a9-bacbdcedfe0f").unwrap(),
                configuration_id: Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").unwrap(),
            },
            rubric: RubricDeclaration {
                id: "security-review-v1".to_owned(),
                version: 7,
                canonicalizer: "beacon.rubric.v1".to_owned(),
                sha256: HEXLOWER.encode(&core::array::from_fn::<u8, 32, _>(|index| {
                    0x20 + index as u8
                })),
            },
            cited_runs: vec![
                Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap(),
                Uuid::parse_str("fedcba98-7654-3210-0123-456789abcdef").unwrap(),
            ],
            verdict: Verdict::Uncertain,
            findings: vec![
                Finding {
                    severity: FindingSeverity::Blocking,
                    summary: "Authorship conflict".to_owned(),
                },
                Finding {
                    severity: FindingSeverity::Note,
                    summary: "Checks passed".to_owned(),
                },
            ],
            conflicts: Conflicts::Declared("Same lab lineage disclosed".to_owned()),
            rationale: "Independent evaluator reviewed exact proposal bytes.".to_owned(),
            supersedes: EvaluationSupersedes::Evaluation(HEXLOWER.encode(&core::array::from_fn::<
                u8,
                32,
                _,
            >(|index| {
                0x40 + index as u8
            }))),
            evaluated_at_ms: 0x0102_0304_0506_0708,
        }
    }

    const PUBLISHED_VECTOR_BYTES: usize = 439;
    const PUBLISHED_VECTOR_DIGEST: &str =
        "dab418e0866b19f6536615a9fe8374b88034107ccbbdb769cf07515fbb516683";
    const PUBLISHED_VECTOR_BASE64: &str = "YmVhY29uLmV2YWx1YXRpb24udjEAAQABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4fAAAAAAAAAAZzaGEyNTYAAAAAAAAAEmJlYWNvbi5wcm9wb3NhbC52MRAhMkNUZXaHmKm6y9zt/g//7t3Mu6qZiHdmVUQzIhEAAAAAAAAAABJzZWN1cml0eS1yZXZpZXctdjEAAAAHAAAAAAAAABBiZWFjb24ucnVicmljLnYxICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8AAAACABEiM0RVZneImaq7zN3u//7cuph2VDIQASNFZ4mrze8EAAAAAgEAAAAAAAAAE0F1dGhvcnNoaXAgY29uZmxpY3QDAAAAAAAAAA1DaGVja3MgcGFzc2VkAQAAAAAAAAAaU2FtZSBsYWIgbGluZWFnZSBkaXNjbG9zZWQAAAAAAAAANEluZGVwZW5kZW50IGV2YWx1YXRvciByZXZpZXdlZCBleGFjdCBwcm9wb3NhbCBieXRlcy4BQEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl8BAgMEBQYHCA==";

    /// The Artifact-subject payload the published arm vector below covers.
    ///
    /// Every field from 6 on is the SAME VALUE as the Proposal-subject vector wherever the two can
    /// be the same, so `the_artifact_arm_changes_no_field_past_field_5` compares two encodings that
    /// differ only in the union.
    fn artifact_vector_payload() -> EvaluationV1 {
        EvaluationV1 {
            subject: EvaluationSubject::Artifact(ArtifactSubject {
                digest: ArtifactDigest {
                    algorithm: DIGEST_ALGORITHM.to_owned(),
                    canonicalizer: ARTIFACT_CANONICALIZER.to_owned(),
                    value: HEXLOWER.encode(&core::array::from_fn::<u8, 32, _>(|index| {
                        0x80 + index as u8
                    })),
                },
                project_id: Uuid::from_bytes([0x11; 16]),
                mission_id: Uuid::from_bytes([0x22; 16]),
            }),
            evaluator: Evaluator {
                resident_id: Uuid::from_bytes([
                    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
                    0xdd, 0xee, 0xff,
                ]),
                configuration_id: Uuid::from_bytes([
                    0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89,
                    0xab, 0xcd, 0xef,
                ]),
            },
            rubric: RubricDeclaration {
                id: "security-review-v1".to_owned(),
                version: 7,
                canonicalizer: "beacon.rubric.v1".to_owned(),
                sha256: HEXLOWER.encode(&core::array::from_fn::<u8, 32, _>(|index| {
                    0x20 + index as u8
                })),
            },
            cited_runs: vec![],
            verdict: Verdict::RevisionRequested,
            findings: vec![Finding {
                severity: FindingSeverity::Blocking,
                summary: "Authorship conflict".to_owned(),
            }],
            conflicts: Conflicts::None,
            rationale: "Independent evaluator reviewed the artifact record.".to_owned(),
            supersedes: EvaluationSupersedes::None,
            evaluated_at_ms: 0x0102_0304_0506_0708,
        }
    }

    const ARTIFACT_VECTOR_BYTES: usize = 350;
    const ARTIFACT_VECTOR_DIGEST: &str =
        "95429c03f224b726929e40d0fbe92a5c7fa18a7558b27b9f2d05958036fa298c";
    const ARTIFACT_VECTOR_BASE64: &str = "YmVhY29uLmV2YWx1YXRpb24udjEAA4CBgoOEhYaHiImKi4yNjo+QkZKTlJWWl5iZmpucnZ6fAAAAAAAAAAZzaGEyNTYAAAAAAAAAEmJlYWNvbi5hcnRpZmFjdC52MREREREREREREREREREREREiIiIiIiIiIiIiIiIiIiIiABEiM0RVZneImaq7zN3u/wEjRWeJq83vASNFZ4mrze8AAAAAAAAAEnNlY3VyaXR5LXJldmlldy12MQAAAAcAAAAAAAAAEGJlYWNvbi5ydWJyaWMudjEgISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+PwAAAAACAAAAAQEAAAAAAAAAE0F1dGhvcnNoaXAgY29uZmxpY3QAAAAAAAAAADNJbmRlcGVuZGVudCBldmFsdWF0b3IgcmV2aWV3ZWQgdGhlIGFydGlmYWN0IHJlY29yZC4AAQIDBAUGBwg=";

    /// Slice E1, bullet 1. The arm's frozen identity vector.
    ///
    /// **Provenance, stated honestly.** These 350 bytes were assembled from ADR 0038's field table
    /// plus the amendment's arm rows, in a separate script, WITHOUT calling this encoder -- so a
    /// layout change fails here rather than silently redefining the vector. But that assembler and
    /// this encoder have the SAME AUTHOR, which the amendment's build rule (b) says the vector must
    /// not: it asks for a non-author seat to build it from the amended table before the encoder
    /// exists. That is the one thing this slice cannot supply about itself, and it is the first
    /// thing to ask of the review.
    #[test]
    fn an_artifact_subject_fixture_reproduces_its_published_digest() {
        let bytes = encode(&artifact_vector_payload()).expect("the arm vector encodes");
        assert_eq!(bytes.len(), ARTIFACT_VECTOR_BYTES, "byte count moved");
        assert_eq!(
            BASE64.encode(&bytes),
            ARTIFACT_VECTOR_BASE64,
            "the encoder no longer reproduces the published Artifact-subject vector"
        );
        assert_eq!(HEXLOWER.encode(&sha256(&bytes)), ARTIFACT_VECTOR_DIGEST);
        assert_eq!(decode(&bytes), Ok(artifact_vector_payload()));

        // The subject byte is where the table says it is, and it is the registry's value.
        assert_eq!(bytes[MAGIC.len()], 0x03);
    }

    /// Slice E1, bullet 2. The test that makes "v1 keeps its identity" a fact rather than a claim.
    ///
    /// It duplicates the assertions of `a_known_evaluation_fixture_reproduces_its_published_digest`
    /// on purpose: that test pins the vector, and THIS one says what the arm must not do to it. A
    /// reader deleting the arm's tests should have to read this name to delete this guarantee.
    #[test]
    fn the_proposal_subject_vector_is_byte_identical_after_the_arm_lands() {
        let bytes = encode(&published_vector_payload()).expect("the v1 vector still encodes");
        assert_eq!(bytes.len(), PUBLISHED_VECTOR_BYTES);
        assert_eq!(BASE64.encode(&bytes), PUBLISHED_VECTOR_BASE64);
        assert_eq!(HEXLOWER.encode(&sha256(&bytes)), PUBLISHED_VECTOR_DIGEST);
        // The arm added a tag VALUE, not a field: field 2 is still one byte, and it still reads
        // 0x01 for a Proposal subject.
        assert_eq!(bytes[MAGIC.len()], TAG_SUBJECT_PROPOSAL);
    }

    /// Slice E1, bullet 3. `0x02` is reserved for the Candidate arm and `0x04`+ are unassigned;
    /// both are refused, and neither falls through to `proposal`.
    ///
    /// The `0x04+` half is what the ADR amendment's build rule (c) owes: with `0x03` assigned, the
    /// `0x02` flip is no longer the boundary case for "the vocabulary is closed".
    #[test]
    fn an_unassigned_subject_tag_is_still_refused() {
        let bytes = encode(&published_vector_payload()).unwrap();
        for tag in [0x00_u8, 0x02, 0x04, 0x05, 0x7f, 0xff] {
            let mut tampered = bytes.clone();
            tampered[MAGIC.len()] = tag;
            assert_eq!(
                decode(&tampered),
                Err(MalformedEvaluation::Subject),
                "subject tag {tag:#04x} was not refused"
            );
        }
        // ...and the two assigned values are not refused, so the loop above is discriminating.
        for (tag, payload) in [
            (TAG_SUBJECT_PROPOSAL, published_vector_payload()),
            (TAG_SUBJECT_ARTIFACT, artifact_vector_payload()),
        ] {
            let encoded = encode(&payload).unwrap();
            assert_eq!(encoded[MAGIC.len()], tag);
            assert!(decode(&encoded).is_ok());
        }
    }

    /// Slice E1, bullet 4. The envelope is closed PER ARM, in both directions.
    ///
    /// The tag and the canonicalizer token are two spellings of one fact. A store column cannot see
    /// them disagree; the grammar can, and it is the grammar that the digest covers.
    #[test]
    fn an_artifact_subject_may_not_name_a_proposal_or_candidate_canonicalizer() {
        for wrong in [
            "beacon.proposal.v1",
            "beacon.candidate.v1",
            "beacon.artifact.v2",
            "",
        ] {
            let mut payload = artifact_vector_payload();
            match &mut payload.subject {
                EvaluationSubject::Artifact(subject) => {
                    subject.digest.canonicalizer = wrong.to_owned()
                }
                EvaluationSubject::Proposal(_) => {
                    unreachable!("the fixture is an Artifact subject")
                }
            }
            assert_eq!(
                encode(&payload),
                Err(MalformedEvaluation::ArtifactDigestEnvelope),
                "an Artifact arm naming {wrong:?} was encoded"
            );
        }

        // The converse: arm 0x01 may not name the Artifact canonicalizer.
        let mut payload = published_vector_payload();
        payload.subject = EvaluationSubject::Proposal(ProposalDigest {
            canonicalizer: ARTIFACT_CANONICALIZER.to_owned(),
            ..proposal_subject_of(&payload).clone()
        });
        assert_eq!(
            encode(&payload),
            Err(MalformedEvaluation::ProposalDigestEnvelope)
        );

        // And the refusal survives the round trip: bytes that carry the wrong pairing are refused
        // on the way IN as well, because a store is not the only thing that can hand over bytes.
        let mut bytes = encode(&artifact_vector_payload()).unwrap();
        bytes[MAGIC.len()] = TAG_SUBJECT_PROPOSAL;
        assert_eq!(
            decode(&bytes),
            Err(MalformedEvaluation::ProposalDigestEnvelope)
        );
    }

    /// Slice E1, bullet 5. The arms of the union may differ in width; nothing after the union may.
    ///
    /// Both encodings are built from payloads whose fields 6-20 are equal, and the tails are
    /// compared byte for byte from the end of each arm's own subject block. The Artifact arm is
    /// exactly 32 bytes wider -- two UUIDs -- and that is the whole difference.
    #[test]
    fn the_artifact_arm_changes_no_field_past_field_5() {
        let mut proposal_payload = published_vector_payload();
        let artifact_payload = artifact_vector_payload();
        // Make fields 6-20 identical; only the union differs.
        proposal_payload.evaluator = artifact_payload.evaluator;
        proposal_payload.rubric = artifact_payload.rubric.clone();
        proposal_payload.cited_runs = artifact_payload.cited_runs.clone();
        proposal_payload.verdict = artifact_payload.verdict;
        proposal_payload.findings = artifact_payload.findings.clone();
        proposal_payload.conflicts = artifact_payload.conflicts.clone();
        proposal_payload.rationale = artifact_payload.rationale.clone();
        proposal_payload.supersedes = artifact_payload.supersedes.clone();
        proposal_payload.evaluated_at_ms = artifact_payload.evaluated_at_ms;

        let proposal_bytes = encode(&proposal_payload).unwrap();
        let artifact_bytes = encode(&artifact_payload).unwrap();

        // Each arm's subject block: tag + 32 raw digest bytes + two length-prefixed strings, plus
        // the Artifact arm's two UUIDs.
        let envelope = |canonicalizer: &str| {
            MAGIC.len() + 1 + 32 + (8 + "sha256".len()) + (8 + canonicalizer.len())
        };
        let proposal_tail = envelope(PROPOSAL_CANONICALIZER);
        let artifact_tail = envelope(ARTIFACT_CANONICALIZER) + 16 + 16;
        assert_eq!(
            artifact_bytes.len() - artifact_tail,
            proposal_bytes.len() - proposal_tail,
            "the two tails are not the same length"
        );
        assert_eq!(
            &artifact_bytes[artifact_tail..],
            &proposal_bytes[proposal_tail..],
            "the arm changed a field past field 5"
        );
        // Both canonicalizer tokens are 18 bytes, so the arm's ENTIRE width difference is the
        // scope it carries: two UUIDs, 32 bytes. (I wrote 28 here first; the two witnesses
        // disagreed and the encoder was right.)
        assert_eq!(PROPOSAL_CANONICALIZER.len(), ARTIFACT_CANONICALIZER.len());
        assert_eq!(artifact_tail - proposal_tail, 32, "the arm's width moved");
    }

    #[test]
    fn a_known_evaluation_fixture_reproduces_its_published_digest() {
        let bytes = encode(&published_vector_payload()).expect("the vector encodes");
        assert_eq!(bytes.len(), PUBLISHED_VECTOR_BYTES, "byte count moved");
        assert_eq!(
            BASE64.encode(&bytes),
            PUBLISHED_VECTOR_BASE64,
            "the encoder no longer reproduces the published vector"
        );
        assert_eq!(HEXLOWER.encode(&sha256(&bytes)), PUBLISHED_VECTOR_DIGEST);
        // And it round-trips: the decoder reads back exactly what was encoded.
        assert_eq!(
            decode(&bytes).expect("the vector decodes"),
            published_vector_payload()
        );
    }

    /// Every field is positional and every byte is covered by the digest, so changing any one of
    /// them changes the identity. This is what the grammar offers INSTEAD of detecting a
    /// same-width transposition, which is not detectable by construction -- see ADR 0038.
    #[test]
    fn every_identity_bearing_field_changes_the_digest() {
        let base = published_vector_payload();
        let baseline = HEXLOWER.encode(&sha256(&encode(&base).unwrap()));

        let mut variants: Vec<(&str, EvaluationV1)> = Vec::new();
        let mut v = base.clone();
        v.subject = EvaluationSubject::Proposal(ProposalDigest {
            value: HEXLOWER.encode(&[0x11_u8; 32]),
            ..proposal_subject_of(&base).clone()
        });
        variants.push(("proposal digest", v));
        let mut v = base.clone();
        v.evaluator.resident_id = Uuid::nil();
        variants.push(("evaluator resident", v));
        let mut v = base.clone();
        v.evaluator.configuration_id = Uuid::nil();
        variants.push(("evaluator configuration", v));
        let mut v = base.clone();
        v.rubric.id = "other-rubric".to_owned();
        variants.push(("rubric id", v));
        let mut v = base.clone();
        v.rubric.version = 8;
        variants.push(("rubric version", v));
        let mut v = base.clone();
        v.rubric.canonicalizer = "beacon.rubric.v2".to_owned();
        variants.push(("rubric canonicalizer", v));
        let mut v = base.clone();
        v.rubric.sha256 = HEXLOWER.encode(&[0x99_u8; 32]);
        variants.push(("rubric digest", v));
        let mut v = base.clone();
        v.cited_runs.pop();
        variants.push(("cited runs", v));
        let mut v = base.clone();
        v.verdict = Verdict::Meets;
        variants.push(("verdict", v));
        let mut v = base.clone();
        v.findings[0].severity = FindingSeverity::Concern;
        variants.push(("finding severity", v));
        let mut v = base.clone();
        v.findings[0].summary = "Something else".to_owned();
        variants.push(("finding summary", v));
        let mut v = base.clone();
        v.findings.swap(0, 1);
        variants.push(("finding ORDER -- a sequence, not a set", v));
        let mut v = base.clone();
        v.conflicts = Conflicts::None;
        variants.push(("conflicts", v));
        let mut v = base.clone();
        v.rationale = "A different rationale entirely.".to_owned();
        variants.push(("rationale", v));
        let mut v = base.clone();
        v.supersedes = EvaluationSupersedes::None;
        variants.push(("supersedes", v));
        let mut v = base.clone();
        v.evaluated_at_ms += 1;
        variants.push(("evaluated_at_ms", v));

        for (name, variant) in variants {
            let digest = HEXLOWER.encode(&sha256(&encode(&variant).unwrap()));
            assert_ne!(digest, baseline, "{name} did not change the identity");
        }
    }

    #[test]
    fn a_trailing_byte_an_unknown_tag_or_invalid_utf8_is_refused() {
        let bytes = encode(&published_vector_payload()).unwrap();

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(decode(&trailing), Err(MalformedEvaluation::TrailingBytes));

        let mut truncated = bytes.clone();
        truncated.pop();
        assert_eq!(decode(&truncated), Err(MalformedEvaluation::Truncated));

        let mut wrong_magic = bytes.clone();
        wrong_magic[0] = b'B';
        assert_eq!(decode(&wrong_magic), Err(MalformedEvaluation::Magic));

        // The subject tag sits immediately after the magic.
        let mut subject = bytes.clone();
        subject[MAGIC.len()] = 0x02;
        assert_eq!(decode(&subject), Err(MalformedEvaluation::Subject));

        // An unassigned verdict, severity, conflicts or supersedes tag is refused rather than
        // treated as a default: a closed vocabulary has no fallthrough.
        //
        // The offsets are computed from the field table rather than found by searching for a byte
        // value -- a search finds the first byte that happens to match, which in this vector is
        // inside the proposal digest, and the test then measures nothing.
        for (label, offset, expected) in [
            ("verdict", VERDICT_TAG_OFFSET, MalformedEvaluation::Verdict),
            (
                "severity",
                FIRST_SEVERITY_TAG_OFFSET,
                MalformedEvaluation::FindingSeverity,
            ),
            (
                "conflicts",
                CONFLICTS_TAG_OFFSET,
                MalformedEvaluation::Conflicts,
            ),
            (
                "supersedes",
                SUPERSEDES_TAG_OFFSET,
                MalformedEvaluation::Supersedes,
            ),
        ] {
            let mut unassigned = bytes.clone();
            assert_ne!(unassigned[offset], 0x7f, "{label} already holds the probe");
            unassigned[offset] = 0x7f;
            assert_eq!(decode(&unassigned), Err(expected), "{label}");
        }

        // Invalid UTF-8 inside the rationale string.
        let mut invalid = bytes.clone();
        let needle = b"Independent evaluator";
        let at = invalid
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("the rationale is present");
        invalid[at] = 0xff;
        assert_eq!(decode(&invalid), Err(MalformedEvaluation::InvalidUtf8));
    }

    // Offsets into the published vector, derived from ADR 0038's field table. They are constants
    // rather than searches so that a layout change fails these tests instead of silently moving
    // what they measure.
    const VERDICT_TAG_OFFSET: usize = 248;
    const FIRST_SEVERITY_TAG_OFFSET: usize = 253;
    const CONFLICTS_TAG_OFFSET: usize = 303;
    const SUPERSEDES_TAG_OFFSET: usize = 398;

    /// The offsets above are only meaningful if they land where the table says. This proves each
    /// one holds the tag it is named for in the published vector.
    #[test]
    fn the_published_vector_holds_each_tag_where_the_field_table_puts_it() {
        let bytes = encode(&published_vector_payload()).unwrap();
        assert_eq!(bytes[VERDICT_TAG_OFFSET], TAG_VERDICT_UNCERTAIN);
        assert_eq!(bytes[FIRST_SEVERITY_TAG_OFFSET], TAG_SEVERITY_BLOCKING);
        assert_eq!(bytes[CONFLICTS_TAG_OFFSET], TAG_CONFLICTS_DECLARED);
        assert_eq!(bytes[SUPERSEDES_TAG_OFFSET], TAG_SUPERSEDES_EVALUATION);
    }

    #[test]
    fn a_decoder_bound_refuses_an_over_long_rationale_finding_or_conflict() {
        // The encoder refuses first, so a caller cannot produce these bytes...
        let mut payload = published_vector_payload();
        payload.rationale = "r".repeat(MAX_RATIONALE_SCALARS + 1);
        assert_eq!(encode(&payload), Err(MalformedEvaluation::Rationale));

        let mut payload = published_vector_payload();
        payload.findings[0].summary = "s".repeat(MAX_SUMMARY_SCALARS + 1);
        assert_eq!(encode(&payload), Err(MalformedEvaluation::FindingSummary));

        let mut payload = published_vector_payload();
        payload.conflicts = Conflicts::Declared("c".repeat(MAX_CONFLICTS_SCALARS + 1));
        assert_eq!(encode(&payload), Err(MalformedEvaluation::ConflictsText));

        let mut payload = published_vector_payload();
        payload.cited_runs = (0..=MAX_CITED_RUNS)
            .map(|index| Uuid::from_u128(index as u128))
            .collect();
        assert_eq!(encode(&payload), Err(MalformedEvaluation::CitedRunCount));

        let mut payload = published_vector_payload();
        payload.findings = (0..=MAX_FINDINGS)
            .map(|_| Finding {
                severity: FindingSeverity::Note,
                summary: "n".to_owned(),
            })
            .collect();
        assert_eq!(encode(&payload), Err(MalformedEvaluation::FindingCount));

        // ...and the DECODER refuses them independently, because bytes can arrive from a store
        // rather than from this encoder. A declared count is proved against the remaining input
        // before a single entry is read, so a hostile count cannot size an allocation.
        let mut hostile = encode(&published_vector_payload()).unwrap();
        let at = MAGIC.len() + 1 + 32 + 8 + 6 + 8 + 18 + 16 + 16 + 8 + 18 + 4 + 8 + 16 + 32;
        hostile[at..at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(decode(&hostile), Err(MalformedEvaluation::CitedRunCount));
    }

    /// Cited Runs are a SET in the bytes: a permutation of the same inputs must not be able to
    /// produce a second identity for one evaluation, and a repeat must not encode at all.
    #[test]
    fn cited_runs_are_a_set_in_ascending_order() {
        let mut payload = published_vector_payload();
        payload.cited_runs.swap(0, 1);
        assert_eq!(encode(&payload), Err(MalformedEvaluation::CitedRunOrder));

        let mut payload = published_vector_payload();
        payload.cited_runs = vec![payload.cited_runs[0], payload.cited_runs[0]];
        assert_eq!(encode(&payload), Err(MalformedEvaluation::CitedRunOrder));
    }

    /// gpt2's S2: the per-field bounds admit a record the store cannot hold. 64 findings of 500
    /// four-byte scalars each is 128,576 bytes of findings alone, and the whole record reaches
    /// 145,941 -- every byte of it legal under the field table, and none of it recordable against
    /// a 65,536-byte column. A grammar whose maximum value cannot be stored is not total, so the
    /// record carries its OWN total bound and refuses before the store is asked.
    #[test]
    fn the_record_owns_a_total_bound_that_the_store_column_matches() {
        let mut payload = published_vector_payload();
        payload.findings = (0..MAX_FINDINGS)
            .map(|_| Finding {
                severity: FindingSeverity::Note,
                // Four bytes per scalar, at the per-field maximum: every value here is legal.
                summary: "\u{1F9EA}".repeat(MAX_SUMMARY_SCALARS),
            })
            .collect();
        assert_eq!(encode(&payload), Err(MalformedEvaluation::RecordTooLarge));

        // And the largest record that IS within the bound still encodes, decodes and round-trips,
        // so the refusal above is a boundary rather than a ceiling nothing can reach.
        let mut payload = published_vector_payload();
        payload.rationale = "\u{1F9EA}".repeat(MAX_RATIONALE_SCALARS);
        payload.conflicts = Conflicts::Declared("\u{1F9EA}".repeat(MAX_CONFLICTS_SCALARS));
        let bytes = encode(&payload).expect("a large but bounded record encodes");
        assert!(bytes.len() <= MAX_CANONICAL_BYTES, "{}", bytes.len());
        assert_eq!(decode(&bytes).unwrap(), payload);

        // The decoder owns the bound too, and checks it before parsing: bytes can arrive from a
        // store rather than from this encoder.
        let oversize = vec![0_u8; MAX_CANONICAL_BYTES + 1];
        assert_eq!(decode(&oversize), Err(MalformedEvaluation::RecordTooLarge));
    }

    /// The envelope the bytes may name is closed. A digest produced by another canonicalizer is
    /// refused by the CODEC, not only by a column constraint, so bytes that could never have been
    /// an evaluation of a v1 Proposal do not exist.
    #[test]
    fn the_copied_proposal_envelope_is_closed_in_the_bytes() {
        let mut payload = published_vector_payload();
        payload.subject = EvaluationSubject::Proposal(ProposalDigest {
            canonicalizer: "beacon.proposal.v2".to_owned(),
            ..proposal_subject_of(&payload).clone()
        });
        assert_eq!(
            encode(&payload),
            Err(MalformedEvaluation::ProposalDigestEnvelope)
        );

        let mut payload = published_vector_payload();
        payload.subject = EvaluationSubject::Proposal(ProposalDigest {
            algorithm: "sha512".to_owned(),
            ..proposal_subject_of(&payload).clone()
        });
        assert_eq!(
            encode(&payload),
            Err(MalformedEvaluation::ProposalDigestEnvelope)
        );
    }
}
