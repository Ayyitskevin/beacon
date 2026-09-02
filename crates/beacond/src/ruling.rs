//! Ruling v1: one recorded human decision, bound to one exact Proposal digest (ADR 0037).
//!
//! A Ruling records that the Owner decided about the exact bytes a Proposal digest names, and
//! copies the Charter, Project Policy, and Repository revisions and the pinned base revision it
//! was decided under. Copying is what makes drift detectable later without trusting the Proposal
//! row to have stayed put; it is what "one policy version" names.
//!
//! **A Ruling creates no execution authority.** It is evidence that a person decided, and evidence
//! does not confer authority (`docs/BLUEPRINT.md:445-447`). Nothing here opens a Gate, prepares a
//! worktree, starts a process, invokes a model, makes a network request, or dispatches anything,
//! and no accessor in this module answers a question with a boolean: a future dispatch re-verifies
//! the binding itself and refuses otherwise.
//!
//! The command takes typed arguments rather than a byte slice. This is the one place Ruling v1
//! does not copy Proposal v1, and the reason is the difference between the two records: a
//! Proposal's bytes are the caller's, so the parsed object and the digest must never be allowed to
//! disagree; a Ruling's bytes are Beacon's own, produced from an authenticated Owner command over
//! values Beacon already holds, so accepting them from a caller would be an input surface with
//! nothing to gain. Beacon canonicalises, hashes, and stores; every read re-derives the digest
//! from the stored bytes.
//!
//! The canonical grammar reuses `beacon.proposal.v1`'s primitives exactly, and this module carries
//! its own copy of them rather than sharing one implementation. That is deliberate: an identity
//! grammar's whole value is that it cannot move, and a shared mutable codec would couple two
//! frozen grammars' futures, so a change made for one could silently restate the other's
//! identities. The primitives are small; the coupling would not be.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use data_encoding::{BASE64, HEXLOWER};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::operator_account::owner_account_exists;
use crate::project_policy::policy_revision_facts;
use crate::project_repository::latest_repository_for_preflight;
use crate::proposal::{
    ProposalBindings, ProposalDigest, UnmetPrecondition, is_base_revision, is_human_text,
    proposal_binding_facts, sha256,
};
use crate::{
    OperatorAccountError, OperatorAccountService, ProjectPolicyError, ProjectRepositoryError,
    ProposalError,
};

/// The canonicalizer identity. Any future grammar is a new canonicalizer; existing v1 bytes are
/// never reinterpreted under v2 rules.
const CANONICALIZER: &str = "beacon.ruling.v1";
const DIGEST_ALGORITHM: &str = "sha256";
const DECIDER_BASIS: &str = "owner";

/// The exact envelope a v1 Ruling may name as the Proposal it ruled on. Both are copied from the
/// Proposal and both are checked here, so bytes that name a canonicalizer this grammar cannot have
/// ruled under are refused by the codec rather than only by a column constraint.
const PROPOSAL_DIGEST_ALGORITHM: &str = "sha256";
const PROPOSAL_CANONICALIZER: &str = "beacon.proposal.v1";

/// The literal ASCII magic that starts every record, including its terminating NUL: 17 bytes.
const MAGIC: &[u8] = b"beacon.ruling.v1\0";

const MAX_REASON_SCALARS: usize = 2_000;
const MAX_REASON_BYTES: usize = MAX_REASON_SCALARS * 4;
/// `sha256` and `beacon.proposal.v1` are the only accepted values; the limit bounds a declared
/// length before any of it is read, so a hostile length cannot reach the comparison.
const MAX_DESCRIPTOR_TOKEN_BYTES: usize = 64;
/// ADR 0034: "the pinned base revision must be a lowercase 40-character hexadecimal commit id."
const BASE_REVISION_CHARS: usize = 40;
const DIGEST_HEX_CHARS: usize = 64;

// One-byte tags. Assigned explicitly and never renumbered. `supersedes` is the one position where
// 0x00 is assigned, because absence is a value there and must encode as one.
const TAG_DECISION_APPROVE: u8 = 0x01;
const TAG_DECISION_REFUSE: u8 = 0x02;
const TAG_DECIDER_OWNER: u8 = 0x01;
const TAG_SUPERSEDES_NONE: u8 = 0x00;
const TAG_SUPERSEDES_RULING: u8 = 0x01;

const DECISION_APPROVE: &str = "approve";
const DECISION_REFUSE: &str = "refuse";

// ---------------------------------------------------------------------------------------------
// The immutable v1 payload
// ---------------------------------------------------------------------------------------------

/// The closed v1 decision vocabulary.
///
/// Two arms and no third. There is no `withdraw` or `expire`, because nobody ruled in either case:
/// those are states of a Gate, and v1 models no Gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulingDecision {
    Approve,
    Refuse,
}

impl RulingDecision {
    fn tag(self) -> u8 {
        match self {
            Self::Approve => TAG_DECISION_APPROVE,
            Self::Refuse => TAG_DECISION_REFUSE,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedRuling> {
        match tag {
            TAG_DECISION_APPROVE => Ok(Self::Approve),
            TAG_DECISION_REFUSE => Ok(Self::Refuse),
            _ => Err(MalformedRuling::UnknownTag),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Approve => DECISION_APPROVE,
            Self::Refuse => DECISION_REFUSE,
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            DECISION_APPROVE => Some(Self::Approve),
            DECISION_REFUSE => Some(Self::Refuse),
            _ => None,
        }
    }
}

/// The only v1 decider basis.
///
/// There is no Operator UUID in the singleton account model, so v1 records the closed basis `owner`
/// rather than inventing an identity. The arm stays closed deliberately: BLUEPRINT `:445-446` says
/// an Agent cannot approve its own Proposal through a model judgment, and in v1 that rule has no
/// code path to exercise. A one-arm vocabulary is what keeps that honest; a check that cannot fail
/// would not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulingDecider {
    Owner,
}

/// Whether this Ruling reverses an earlier one, and which.
///
/// `None` is an explicit variant with its own tag, not a missing field and not SQL null: the store
/// holds supersession as a relation, and "no predecessor" is the absence of a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RulingSupersedes {
    None,
    /// The lowercase 64-character hexadecimal digest of the Ruling this one reverses.
    ///
    /// Carried as text rather than as thirty-two raw bytes for the reason ADR 0034 gives for the
    /// pinned base revision: a value decoded from raw bytes would re-encode `A1B2...` and
    /// `a1b2...` to one identity, and case-folding a Binding is a silent normalization this
    /// grammar refuses. The bytes on the wire are still the raw digest; the encoder refuses any
    /// text that is not already its canonical lowercase form.
    Ruling(String),
}

/// The immutable v1 payload, exactly as its canonical bytes decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulingV1 {
    /// The bound Proposal's identity, copied whole -- value, algorithm, and canonicalizer. A
    /// Ruling names the exact bytes it ruled on, and recording only the value would leave a reader
    /// to assume which function produced it: the assumption this record exists to remove.
    pub proposal: ProposalDigest,
    /// The four revisions copied from that Proposal at ruling time. This is the Proposal's own
    /// binding type because these are the Proposal's own bindings, copied rather than referenced.
    pub bindings: ProposalBindings,
    pub decision: RulingDecision,
    pub reason: String,
    pub decider: RulingDecider,
    pub supersedes: RulingSupersedes,
    pub ruled_at_ms: u64,
}

// ---------------------------------------------------------------------------------------------
// Identity and force
// ---------------------------------------------------------------------------------------------

/// A digest that carries its algorithm and canonicalizer identity, never a bare hex string.
///
/// Distinct from `ProposalDigest` because a Ruling carries two identities and neither is inferred
/// from the other: this one names the Ruling's own canonical bytes under `beacon.ruling.v1`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RulingDigest {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

/// Two facts about whether a Ruling still applies, always reported together.
///
/// A Ruling is in force exactly when `superseded_by` is `None` **and** `drift` is empty. Both are
/// returned so no caller has to learn which of "superseded" and "drifted" wins -- a Ruling can be
/// both, and saying so is more useful than choosing.
///
/// There is deliberately no `is_in_force`. A single bit read off an authority record is the thing
/// BLUEPRINT `:446` warns about: it would invite a consumer to skip the verification it is
/// required to perform itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulingForce {
    superseded_by: Option<RulingDigest>,
    drift: Vec<UnmetPrecondition>,
}

impl RulingForce {
    /// The identity of the Ruling that reverses this one, if any.
    ///
    /// The whole envelope, not a bare hex value: a digest carries its algorithm and canonicalizer
    /// everywhere else in this store, and reporting one without them would leave a reader to
    /// assume which function produced it. That the two are fixed in v1 makes them predictable, not
    /// implied -- the day a second canonicalizer exists, a caller that had been inferring them
    /// would be inferring wrongly.
    pub fn superseded_by(&self) -> Option<&RulingDigest> {
        self.superseded_by.as_ref()
    }

    /// Which of the four copied revisions has moved. Empty means every one still matches.
    pub fn drift(&self) -> &[UnmetPrecondition] {
        &self.drift
    }
}

/// One recorded Ruling: its bytes, the identity of those bytes, what they decode to, and the two
/// read-time facts about whether it still applies.
///
/// The fields are private and this type derives no `Deserialize`, for the reason ADR 0034 gives:
/// a caller-supplied parsed object plus digest permits the two to disagree. Bytes and payload here
/// were proved equal by the only constructor that exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ruling {
    digest: RulingDigest,
    canonical_bytes: Vec<u8>,
    payload: RulingV1,
    force: RulingForce,
}

impl Ruling {
    pub fn digest(&self) -> &RulingDigest {
        &self.digest
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    pub fn payload(&self) -> &RulingV1 {
        &self.payload
    }

    pub fn force(&self) -> &RulingForce {
        &self.force
    }
}

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

/// Why a value is not a Ruling.
///
/// Decided from the bytes when decoding and from the caller's arguments when encoding. Both
/// directions run the same predicates, so the encoder and the decoder cannot disagree about what
/// is well-formed. `Reason`, `PinnedBaseRevision`, `ProposalDigestValue`, `SupersedesDigest`, and
/// `ProposalDigestEnvelope` are reachable from either side; `Magic`, `TrailingBytes`, `Truncated`,
/// and `InvalidUtf8` describe input a decoder was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedRuling {
    Magic,
    UnknownTag,
    TrailingBytes,
    Truncated,
    OverLimitLength,
    InvalidUtf8,
    ProposalDigestValue,
    ProposalDigestEnvelope,
    PinnedBaseRevision,
    Reason,
    SupersedesDigest,
}

impl MalformedRuling {
    fn reason(self) -> &'static str {
        match self {
            Self::Magic => "the record does not start with the beacon.ruling.v1 magic",
            Self::UnknownTag => "a closed variant tag is not one this grammar assigns",
            Self::TrailingBytes => "the record does not consume its entire input",
            Self::Truncated => "a declared length does not fit the remaining input",
            Self::OverLimitLength => "a declared length exceeds its field's limit",
            Self::InvalidUtf8 => "a string field is not valid UTF-8",
            Self::ProposalDigestValue => {
                "the bound Proposal digest must be a lowercase 64-character hexadecimal value"
            }
            Self::ProposalDigestEnvelope => {
                "a v1 Ruling can only bind a sha256 digest under the beacon.proposal.v1 \
                 canonicalizer"
            }
            Self::PinnedBaseRevision => {
                "the pinned base revision must be a lowercase 40-character hexadecimal commit id"
            }
            Self::Reason => {
                "the reason must be trimmed, non-empty, at most 2,000 Unicode scalar values, and \
                 free of NUL and non-whitespace control characters"
            }
            Self::SupersedesDigest => {
                "a superseded Ruling must be named by a lowercase 64-character hexadecimal digest"
            }
        }
    }
}

/// Why a well-formed Ruling does not agree with the state it names. Every arm needs the store.
///
/// The three supersession arms stay apart on purpose. "Names no Ruling at all", "names a Ruling
/// under a different Proposal", and "names a Ruling that is no longer the end of this chain" are
/// three different facts about the world, and collapsing them is the defect AGENTS.md names:
/// absent, broken, and not-the-thing-asked-for are distinct states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InconsistentRuling {
    ProposalNotFound,
    SupersedesNotRecorded,
    SupersedesOtherProposal,
    NotTheCurrentTail,
    RuledBeforeTail,
}

impl InconsistentRuling {
    fn reason(self) -> &'static str {
        match self {
            Self::ProposalNotFound => {
                "the bound digest names no Proposal recorded in this Workspace"
            }
            Self::SupersedesNotRecorded => "the superseded digest names no recorded Ruling",
            Self::SupersedesOtherProposal => {
                "the superseded Ruling was recorded against a different Proposal"
            }
            Self::NotTheCurrentTail => {
                "a Ruling must cite the current last link of its Proposal's chain, and the first \
                 Ruling for a Proposal must cite none"
            }
            Self::RuledBeforeTail => {
                "the ruling time is earlier than the Ruling this one supersedes"
            }
        }
    }
}

#[derive(Debug)]
pub enum RulingError {
    Account(OperatorAccountError),
    Malformed(MalformedRuling),
    Inconsistent(InconsistentRuling),
    NotFound,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for RulingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::Malformed(reason) => {
                write!(formatter, "Ruling values are invalid: {}", reason.reason())
            }
            Self::Inconsistent(reason) => write!(
                formatter,
                "Ruling references are inconsistent: {}",
                reason.reason()
            ),
            Self::NotFound => write!(formatter, "no Ruling has that digest"),
            Self::InvalidStore => write!(formatter, "Ruling state is invalid"),
            Self::Store(error) => write!(formatter, "Ruling store failed: {error}"),
            Self::Json(error) => write!(formatter, "Ruling event is invalid: {error}"),
        }
    }
}

impl std::error::Error for RulingError {}

impl From<OperatorAccountError> for RulingError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for RulingError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for RulingError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
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

    fn string(&mut self, value: &str, limit: usize) -> Result<(), MalformedRuling> {
        if value.len() > limit {
            return Err(MalformedRuling::OverLimitLength);
        }
        let length = u64::try_from(value.len()).map_err(|_| MalformedRuling::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value.as_bytes());
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

    fn take(&mut self, count: usize) -> Result<&'a [u8], MalformedRuling> {
        if count > self.remaining() {
            return Err(MalformedRuling::Truncated);
        }
        let slice = &self.input[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn magic(&mut self) -> Result<(), MalformedRuling> {
        if self.take(MAGIC.len()).map_err(|_| MalformedRuling::Magic)? == MAGIC {
            Ok(())
        } else {
            Err(MalformedRuling::Magic)
        }
    }

    fn tag(&mut self) -> Result<u8, MalformedRuling> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, MalformedRuling> {
        let mut value = [0_u8; 4];
        value.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(value))
    }

    fn u64(&mut self) -> Result<u64, MalformedRuling> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }

    fn sha256(&mut self) -> Result<[u8; 32], MalformedRuling> {
        let mut value = [0_u8; 32];
        value.copy_from_slice(self.take(32)?);
        Ok(value)
    }

    fn string(&mut self, limit: usize) -> Result<String, MalformedRuling> {
        let length = self.u64()?;
        let limit = u64::try_from(limit).map_err(|_| MalformedRuling::OverLimitLength)?;
        if length > limit {
            return Err(MalformedRuling::OverLimitLength);
        }
        let length = usize::try_from(length).map_err(|_| MalformedRuling::Truncated)?;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| MalformedRuling::InvalidUtf8)
    }

    fn finish(self) -> Result<(), MalformedRuling> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MalformedRuling::TrailingBytes)
        }
    }
}

/// A lowercase 64-character hexadecimal digest, as its thirty-two raw bytes.
///
/// Uppercase input is refused rather than folded: `HEXLOWER` accepts only the canonical form, so
/// two spellings of one digest can never both encode.
fn digest_bytes(value: &str, refusal: MalformedRuling) -> Result<[u8; 32], MalformedRuling> {
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

/// The one canonical encoding of a payload. Every accepted byte difference changes the Ruling
/// identity, so alternate encodings of the same parsed value do not exist: this function is the
/// only producer, and every read proves the stored bytes equal what it returns.
///
/// The field order, widths, and tag values are ADR 0037's table, and this is the executable form
/// of it.
fn encode(payload: &RulingV1) -> Result<Vec<u8>, MalformedRuling> {
    let mut encoder = Encoder::new();

    // 2-4: the bound Proposal's identity, copied whole.
    if payload.proposal.algorithm != PROPOSAL_DIGEST_ALGORITHM
        || payload.proposal.canonicalizer != PROPOSAL_CANONICALIZER
    {
        return Err(MalformedRuling::ProposalDigestEnvelope);
    }
    encoder.sha256(&digest_bytes(
        &payload.proposal.value,
        MalformedRuling::ProposalDigestValue,
    )?);
    encoder.string(&payload.proposal.algorithm, MAX_DESCRIPTOR_TOKEN_BYTES)?;
    encoder.string(&payload.proposal.canonicalizer, MAX_DESCRIPTOR_TOKEN_BYTES)?;

    // 5-8: the four copied revisions.
    encoder.u32(payload.bindings.charter_revision);
    encoder.u32(payload.bindings.project_policy_revision);
    encoder.u32(payload.bindings.repository_revision);
    if !is_base_revision(&payload.bindings.pinned_base_revision) {
        return Err(MalformedRuling::PinnedBaseRevision);
    }
    encoder.string(&payload.bindings.pinned_base_revision, BASE_REVISION_CHARS)?;

    // 9-11: the decision, why, and on what basis.
    encoder.tag(payload.decision.tag());
    if !is_human_text(&payload.reason, MAX_REASON_SCALARS) {
        return Err(MalformedRuling::Reason);
    }
    encoder.string(&payload.reason, MAX_REASON_BYTES)?;
    match payload.decider {
        RulingDecider::Owner => encoder.tag(TAG_DECIDER_OWNER),
    }

    // 12: absence is a value here, so it encodes as one.
    match &payload.supersedes {
        RulingSupersedes::None => encoder.tag(TAG_SUPERSEDES_NONE),
        RulingSupersedes::Ruling(digest) => {
            encoder.tag(TAG_SUPERSEDES_RULING);
            encoder.sha256(&digest_bytes(digest, MalformedRuling::SupersedesDigest)?);
        }
    }

    // 13.
    encoder.u64(payload.ruled_at_ms);
    Ok(encoder.out)
}

fn decode(input: &[u8]) -> Result<RulingV1, MalformedRuling> {
    let mut decoder = Decoder::new(input);
    decoder.magic()?;

    let proposal_digest = HEXLOWER.encode(&decoder.sha256()?);
    let algorithm = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
    let canonicalizer = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
    if algorithm != PROPOSAL_DIGEST_ALGORITHM || canonicalizer != PROPOSAL_CANONICALIZER {
        return Err(MalformedRuling::ProposalDigestEnvelope);
    }

    let charter_revision = decoder.u32()?;
    let project_policy_revision = decoder.u32()?;
    let repository_revision = decoder.u32()?;
    let pinned_base_revision = decoder.string(BASE_REVISION_CHARS)?;
    if !is_base_revision(&pinned_base_revision) {
        return Err(MalformedRuling::PinnedBaseRevision);
    }

    let decision = RulingDecision::parse(decoder.tag()?)?;
    let reason = decoder.string(MAX_REASON_BYTES)?;
    if !is_human_text(&reason, MAX_REASON_SCALARS) {
        return Err(MalformedRuling::Reason);
    }
    let decider = match decoder.tag()? {
        TAG_DECIDER_OWNER => RulingDecider::Owner,
        _ => return Err(MalformedRuling::UnknownTag),
    };
    let supersedes = match decoder.tag()? {
        TAG_SUPERSEDES_NONE => RulingSupersedes::None,
        TAG_SUPERSEDES_RULING => RulingSupersedes::Ruling(HEXLOWER.encode(&decoder.sha256()?)),
        _ => return Err(MalformedRuling::UnknownTag),
    };
    let ruled_at_ms = decoder.u64()?;
    decoder.finish()?;

    Ok(RulingV1 {
        proposal: ProposalDigest {
            algorithm,
            canonicalizer,
            value: proposal_digest,
        },
        bindings: ProposalBindings {
            charter_revision,
            project_policy_revision,
            repository_revision,
            pinned_base_revision,
        },
        decision,
        reason,
        decider,
        supersedes,
        ruled_at_ms,
    })
}

// ---------------------------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------------------------

/// Owns authenticated Ruling recording and identity queries behind one deep interface.
///
/// Recording a Ruling records that the Owner decided. It grants nothing, and a read of what it
/// recorded is not a permission check.
pub struct RulingService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl RulingService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Records one immutable Ruling about the exact bytes `proposal_digest` names.
    ///
    /// The Owner is authenticated before anything else happens, so an unauthenticated caller
    /// receives only the account refusal whatever else is wrong with the request. The caller's own
    /// values are then checked with the predicates the encoder uses, so a malformed argument never
    /// reaches the store; the encoder stays the authority, and because both call the same
    /// predicate they cannot disagree.
    ///
    /// The chain is extended inside one immediate transaction: the current tail is read, compared
    /// with `supersedes`, and the new link inserted without releasing the write lock, so two
    /// concurrent rulings cannot both believe they extend it.
    pub fn rule(
        &self,
        proposal_digest: &str,
        decision: RulingDecision,
        reason: &str,
        supersedes: RulingSupersedes,
    ) -> Result<Ruling, RulingError> {
        self.accounts.with_authenticated_store(|connection, now| {
            digest_bytes(proposal_digest, MalformedRuling::ProposalDigestValue)
                .map_err(RulingError::Malformed)?;
            if !is_human_text(reason, MAX_REASON_SCALARS) {
                return Err(RulingError::Malformed(MalformedRuling::Reason));
            }
            if let RulingSupersedes::Ruling(digest) = &supersedes {
                digest_bytes(digest, MalformedRuling::SupersedesDigest)
                    .map_err(RulingError::Malformed)?;
            }
            let ruled_at_ms = u64::try_from(now).map_err(|_| RulingError::InvalidStore)?;

            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

            // The Proposal must exist in this Workspace and still agree with its own bytes. Its
            // bindings are read out of those bytes, never off the row, so what a Ruling copies is
            // what the Proposal says rather than what its index happens to hold.
            let Some(facts) =
                proposal_binding_facts(&transaction, self.workspace_id, proposal_digest)
                    .map_err(map_proposal_error)?
            else {
                return Err(RulingError::Inconsistent(
                    InconsistentRuling::ProposalNotFound,
                ));
            };

            let chain = ruling_chain(&transaction, self.workspace_id, proposal_digest)?;
            let tail = chain.last();
            match (tail, &supersedes) {
                (None, RulingSupersedes::None) => {}
                (Some(tail), RulingSupersedes::Ruling(cited)) if tail == cited => {}
                (_, RulingSupersedes::Ruling(cited)) => {
                    // Say which of the three things is wrong rather than one refusal for all
                    // three. A digest naming nothing, a digest naming another Proposal's Ruling,
                    // and a digest naming an already-superseded link of this chain are different
                    // mistakes and lead a caller to different corrections.
                    return Err(RulingError::Inconsistent(
                        match recorded_proposal_of(&transaction, cited)? {
                            None => InconsistentRuling::SupersedesNotRecorded,
                            Some(other) if other != proposal_digest => {
                                InconsistentRuling::SupersedesOtherProposal
                            }
                            Some(_) => InconsistentRuling::NotTheCurrentTail,
                        },
                    ));
                }
                (Some(_), RulingSupersedes::None) => {
                    return Err(RulingError::Inconsistent(
                        InconsistentRuling::NotTheCurrentTail,
                    ));
                }
            }
            if let Some(tail) = tail {
                let tail_ruled_at_ms = ruled_at_ms_of(&transaction, tail)?;
                if ruled_at_ms < tail_ruled_at_ms {
                    return Err(RulingError::Inconsistent(
                        InconsistentRuling::RuledBeforeTail,
                    ));
                }
            }

            let payload = RulingV1 {
                // COPIED from the verified Proposal, never rebuilt from this module's constants.
                // The store's CHECK constraints make the two equal today, but equality is not
                // provenance: a Ruling that asserted an algorithm and a canonicalizer it never
                // read would be inferring exactly what recording the envelope exists to stop.
                proposal: facts.digest,
                bindings: facts.bindings,
                decision,
                reason: reason.to_owned(),
                decider: RulingDecider::Owner,
                supersedes,
                ruled_at_ms,
            };
            let canonical_bytes = encode(&payload).map_err(RulingError::Malformed)?;
            let digest_value = HEXLOWER.encode(&sha256(&canonical_bytes));

            let stored = StoredRuling {
                workspace_id: self.workspace_id,
                digest_value,
                canonical_bytes,
                proposal_digest: proposal_digest.to_owned(),
                charter_revision: payload.bindings.charter_revision,
                project_policy_revision: payload.bindings.project_policy_revision,
                repository_revision: payload.bindings.repository_revision,
                pinned_base_revision: payload.bindings.pinned_base_revision.clone(),
                decision: payload.decision,
                reason: payload.reason.clone(),
                supersedes: match &payload.supersedes {
                    RulingSupersedes::None => None,
                    RulingSupersedes::Ruling(digest) => Some(digest.clone()),
                },
                ruled_at_ms,
            };
            insert_ruling(&transaction, &stored)?;
            let ruling = verified_ruling(&transaction, stored)?;
            transaction.commit()?;
            Ok(ruling)
        })
    }

    pub fn get(&self, digest: &str) -> Result<Ruling, RulingError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let stored =
                load_stored(connection, self.workspace_id, digest)?.ok_or(RulingError::NotFound)?;
            verified_ruling(connection, stored)
        })
    }

    /// Every Ruling recorded against one Proposal, in **chain order, oldest first**.
    ///
    /// Chain order, never timestamp order: a timestamp is evidence about when, and the chain is
    /// the record of what followed what. The effective Ruling is the last element.
    ///
    /// A Proposal that was never recorded is refused rather than answered with an empty list. "No
    /// such Proposal" and "no decision yet" are different facts and a caller acts differently on
    /// each.
    pub fn list_for_proposal(&self, proposal_digest: &str) -> Result<Vec<Ruling>, RulingError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            if proposal_binding_facts(connection, self.workspace_id, proposal_digest)
                .map_err(map_proposal_error)?
                .is_none()
            {
                return Err(RulingError::Inconsistent(
                    InconsistentRuling::ProposalNotFound,
                ));
            }
            ruling_chain(connection, self.workspace_id, proposal_digest)?
                .into_iter()
                .map(|digest| {
                    let stored = load_stored(connection, self.workspace_id, &digest)?
                        .ok_or(RulingError::InvalidStore)?;
                    verified_ruling(connection, stored)
                })
                .collect()
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The supersession chain
// ---------------------------------------------------------------------------------------------

/// Re-derives one Proposal's Ruling chain from the relation, oldest first.
///
/// The store makes a fork hard -- each Ruling cites at most one predecessor (primary key) and is
/// superseded at most once (unique index) -- but neither constraint forbids two independent chains
/// under one Proposal, nor a cycle among links that each satisfy both. So this walk is the check:
/// exactly one head, and a walk that reaches every Ruling. A store that fails either is invalid,
/// not a chain with a caveat, and every caller -- recording, listing, and reconciliation -- asks
/// this one function so they cannot drift apart about what the chain is.
fn ruling_chain(
    connection: &Connection,
    workspace_id: Uuid,
    proposal_digest: &str,
) -> Result<Vec<String>, RulingError> {
    let mut statement = connection.prepare(
        "SELECT digest_value FROM rulings
         WHERE workspace_id = ?1 AND proposal_digest = ?2",
    )?;
    let all = statement
        .query_map(params![workspace_id.to_string(), proposal_digest], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<HashSet<_>, _>>()?;
    drop(statement);
    if all.is_empty() {
        return Ok(Vec::new());
    }

    let mut statement = connection.prepare(
        "SELECT supersedes_ruling_digest, ruling_digest FROM ruling_supersessions
         WHERE workspace_id = ?1 AND proposal_digest = ?2",
    )?;
    let links = statement
        .query_map(params![workspace_id.to_string(), proposal_digest], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    let mut successor: HashMap<String, String> = HashMap::new();
    let mut cited: HashSet<String> = HashSet::new();
    for (superseded, successor_digest) in links {
        if successor
            .insert(superseded.clone(), successor_digest.clone())
            .is_some()
            || !cited.insert(successor_digest)
            || !all.contains(&superseded)
        {
            return Err(RulingError::InvalidStore);
        }
    }

    let Some(head) = all.iter().find(|digest| !cited.contains(*digest)).cloned() else {
        // Every Ruling cites a predecessor, so the links form a cycle and there is no first link.
        return Err(RulingError::InvalidStore);
    };

    let mut chain = Vec::with_capacity(all.len());
    let mut cursor = Some(head);
    while let Some(digest) = cursor {
        cursor = successor.get(&digest).cloned();
        chain.push(digest);
        // A bound on the walk, not a check on the store: in-edges are unique and the head has
        // none, so the walk is a simple path and cannot revisit. If a store ever violated that,
        // this returns rather than hanging, which is the only failure mode worse than an error.
        if chain.len() > all.len() {
            return Err(RulingError::InvalidStore);
        }
    }
    // The one comparison that refuses a fork. There is deliberately no separate "more than one
    // head" branch: a second head means at least one Ruling is unreachable from the first, so the
    // walk always comes up short, and a guard no store state can reach alone is not a guard.
    // Removing this line is what turns `a_forked_chain_has_no_last_link_and_is_refused` red.
    if chain.len() != all.len() {
        return Err(RulingError::InvalidStore);
    }
    Ok(chain)
}

/// Which Proposal a Ruling digest was recorded against, anywhere in the store.
///
/// Deliberately unscoped by Workspace: it exists to tell "names no Ruling at all" apart from
/// "names a Ruling recorded elsewhere", and a Workspace-scoped lookup would report the second as
/// the first.
fn recorded_proposal_of(
    connection: &Connection,
    digest: &str,
) -> Result<Option<String>, RulingError> {
    Ok(connection
        .query_row(
            "SELECT proposal_digest FROM rulings WHERE digest_value = ?1",
            params![digest],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

fn ruled_at_ms_of(connection: &Connection, digest: &str) -> Result<u64, RulingError> {
    let value: i64 = connection.query_row(
        "SELECT ruled_at_ms FROM rulings WHERE digest_value = ?1",
        params![digest],
        |row| row.get(0),
    )?;
    u64::try_from(value).map_err(|_| RulingError::InvalidStore)
}

// ---------------------------------------------------------------------------------------------
// Force, derived at read time and never stored
// ---------------------------------------------------------------------------------------------

/// A Ruling is in force only while every revision it copied still equals the current one.
///
/// Exactly the four bound revisions and nothing else. The Proposal's own readiness -- undeclared
/// constraint families, an unverified readable context, a Resident whose lifecycle or
/// Configuration moved -- is a fact about the Proposal, not about whether the decision still
/// applies to the bytes it named, and folding it in here would report a Ruling as drifted for
/// reasons its Blueprint rule does not name (`BLUEPRINT:906`).
///
/// Nothing derived here is ever stored, so a historical Ruling does not become corrupt because the
/// world moved.
fn derive_force(
    connection: &Connection,
    workspace_id: Uuid,
    stored: &StoredRuling,
    payload: &RulingV1,
) -> Result<RulingForce, RulingError> {
    let superseded_by = connection
        .query_row(
            "SELECT ruling_digest FROM ruling_supersessions
             WHERE workspace_id = ?1 AND proposal_digest = ?2 AND supersedes_ruling_digest = ?3",
            params![
                workspace_id.to_string(),
                stored.proposal_digest.as_str(),
                stored.digest_value.as_str()
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|value| digest_envelope(&value));

    let Some(facts) = proposal_binding_facts(connection, workspace_id, &stored.proposal_digest)
        .map_err(map_proposal_error)?
    else {
        // The foreign key makes this unreachable while the store is sound; a Ruling whose Proposal
        // is gone is an invalid store, never a Ruling with a caveat.
        return Err(RulingError::InvalidStore);
    };

    let mut drift = Vec::new();
    let (current_charter, latest_repository) =
        latest_repository_for_preflight(connection, workspace_id, facts.project_id)
            .map_err(map_repository_error)?;
    if current_charter != payload.bindings.charter_revision {
        drift.push(UnmetPrecondition::CharterRevisionMoved);
    }
    let policy = policy_revision_facts(
        connection,
        workspace_id,
        facts.project_id,
        payload.bindings.project_policy_revision,
    )
    .map_err(map_policy_error)?;
    if !policy.is_some_and(|facts| facts.is_current) {
        drift.push(UnmetPrecondition::ProjectPolicyRevisionMoved);
    }
    // Repository and base are separate facts, so a rebind that keeps the same pinned commit
    // reports only the revision as moved -- one drift, not two. This is the shape the round-two
    // `BaseRevisionMoved` bug had in Proposal readiness, and it is repeated here rather than
    // shared because the two records answer different questions about the same columns.
    match latest_repository {
        Some(binding) if binding.revision == payload.bindings.repository_revision => {
            if binding.base_revision != payload.bindings.pinned_base_revision {
                drift.push(UnmetPrecondition::BaseRevisionMoved);
            }
        }
        Some(binding) => {
            drift.push(UnmetPrecondition::RepositoryRevisionMoved);
            if binding.base_revision != payload.bindings.pinned_base_revision {
                drift.push(UnmetPrecondition::BaseRevisionMoved);
            }
        }
        None => {
            drift.push(UnmetPrecondition::RepositoryRevisionMoved);
            drift.push(UnmetPrecondition::BaseRevisionMoved);
        }
    }
    Ok(RulingForce {
        superseded_by,
        drift,
    })
}

// ---------------------------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------------------------

/// One immutable Ruling row. Every state discriminator is a stored value; the one `Option` here is
/// the supersession **relation**, where `None` means "no row" rather than a null column.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredRuling {
    workspace_id: Uuid,
    digest_value: String,
    canonical_bytes: Vec<u8>,
    proposal_digest: String,
    charter_revision: u32,
    project_policy_revision: u32,
    repository_revision: u32,
    pinned_base_revision: String,
    decision: RulingDecision,
    reason: String,
    supersedes: Option<String>,
    ruled_at_ms: u64,
}

/// The authenticated command actor, which for a Ruling is always the Owner.
///
/// Recorded in the audit Event as well as in the bytes' `decider`: the Event says who issued the
/// Command, the bytes say on what basis the decision was made. In v1 they are the same person and
/// both are closed one-arm vocabularies, which is exactly why recording only one of them would be
/// a claim that stops being true the moment either widens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RulingActor {
    Owner,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordedRuling {
    digest: RulingDigest,
    /// The exact canonical bytes, as canonical RFC 4648 padded base64.
    canonical_bytes_base64: String,
    actor: RulingActor,
    ruled_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RulingRecordedPayload {
    ruling: RecordedRuling,
}

fn digest_envelope(value: &str) -> RulingDigest {
    RulingDigest {
        algorithm: DIGEST_ALGORITHM.to_owned(),
        canonicalizer: CANONICALIZER.to_owned(),
        value: value.to_owned(),
    }
}

/// Writes the Ruling, its supersession link if it has one, and its audit Event -- in the caller's
/// one transaction, so the domain change and its evidence land together or not at all.
///
/// Identical re-ruling needs no check here: two Rulings that differ in nothing produce one digest,
/// and `rulings.digest_value` is the primary key, so the second insert collides. What a caller
/// actually hits first is the chain rule -- a second `rule` call that cites nothing is refused as
/// `NotTheCurrentTail` -- because a deliberate reversal differs by its `supersedes` link and
/// therefore by its digest, so two identical Rulings are not expressible while that rule holds.
fn insert_ruling(connection: &Connection, stored: &StoredRuling) -> Result<(), RulingError> {
    let ruled_at_ms = i64::try_from(stored.ruled_at_ms).map_err(|_| RulingError::InvalidStore)?;
    connection.execute(
        "INSERT INTO rulings (
             workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
             proposal_digest, proposal_digest_algorithm, proposal_digest_canonicalizer,
             charter_revision, project_policy_revision, repository_revision, pinned_base_revision,
             decision, reason, decider_basis, ruled_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            stored.workspace_id.to_string(),
            DIGEST_ALGORITHM,
            CANONICALIZER,
            stored.digest_value.as_str(),
            stored.canonical_bytes.as_slice(),
            stored.proposal_digest.as_str(),
            PROPOSAL_DIGEST_ALGORITHM,
            PROPOSAL_CANONICALIZER,
            i64::from(stored.charter_revision),
            i64::from(stored.project_policy_revision),
            i64::from(stored.repository_revision),
            stored.pinned_base_revision.as_str(),
            stored.decision.as_str(),
            stored.reason.as_str(),
            DECIDER_BASIS,
            ruled_at_ms,
        ],
    )?;
    if let Some(superseded) = &stored.supersedes {
        connection.execute(
            "INSERT INTO ruling_supersessions (
                 workspace_id, proposal_digest, ruling_digest, supersedes_ruling_digest
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                stored.workspace_id.to_string(),
                stored.proposal_digest.as_str(),
                stored.digest_value.as_str(),
                superseded.as_str(),
            ],
        )?;
    }
    let payload = serde_json::to_string(&RulingRecordedPayload {
        ruling: RecordedRuling {
            digest: digest_envelope(&stored.digest_value),
            canonical_bytes_base64: BASE64.encode(&stored.canonical_bytes),
            actor: RulingActor::Owner,
            ruled_at_ms: stored.ruled_at_ms,
        },
    })?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'ruling.recorded', ?2, ?3)",
        params![stored.workspace_id.to_string(), payload, ruled_at_ms],
    )?;
    Ok(())
}

fn load_stored(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<StoredRuling>, RulingError> {
    let row = connection
        .query_row(
            "SELECT digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
                    proposal_digest, proposal_digest_algorithm, proposal_digest_canonicalizer,
                    charter_revision, project_policy_revision, repository_revision,
                    pinned_base_revision, decision, reason, decider_basis, ruled_at_ms
             FROM rulings WHERE workspace_id = ?1 AND digest_value = ?2",
            params![workspace_id.to_string(), digest_value],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, i64>(14)?,
                ))
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.0 != DIGEST_ALGORITHM
        || row.1 != CANONICALIZER
        || row.5 != PROPOSAL_DIGEST_ALGORITHM
        || row.6 != PROPOSAL_CANONICALIZER
        || row.13 != DECIDER_BASIS
    {
        return Err(RulingError::InvalidStore);
    }
    let supersedes = connection
        .query_row(
            "SELECT supersedes_ruling_digest FROM ruling_supersessions
             WHERE workspace_id = ?1 AND proposal_digest = ?2 AND ruling_digest = ?3",
            params![workspace_id.to_string(), row.4.as_str(), digest_value],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(Some(StoredRuling {
        workspace_id,
        digest_value: row.2,
        canonical_bytes: row.3,
        proposal_digest: row.4,
        charter_revision: positive_u32(row.7)?,
        project_policy_revision: positive_u32(row.8)?,
        repository_revision: positive_u32(row.9)?,
        pinned_base_revision: row.10,
        decision: RulingDecision::from_str(&row.11).ok_or(RulingError::InvalidStore)?,
        reason: row.12,
        supersedes,
        ruled_at_ms: u64::try_from(row.14).map_err(|_| RulingError::InvalidStore)?,
    }))
}

/// Rebuilds a Ruling from its stored bytes, proving on every read what recording proved once: the
/// digest names these bytes, the bytes decode, they re-encode identically, and every indexed
/// column and the supersession relation repeat what the bytes say. A store that disagrees with its
/// own bytes is invalid, not a Ruling with a caveat.
fn verified_ruling(connection: &Connection, stored: StoredRuling) -> Result<Ruling, RulingError> {
    if HEXLOWER.encode(&sha256(&stored.canonical_bytes)) != stored.digest_value {
        return Err(RulingError::InvalidStore);
    }
    let payload = decode(&stored.canonical_bytes).map_err(|_| RulingError::InvalidStore)?;
    // ADR 0037's stated discipline: "strictly decodes and re-encodes them". No byte sequence this
    // grammar can decode re-encodes differently -- every field is fixed-width or exactly
    // length-prefixed, so there is no second spelling of one value and no test can reach this
    // refusal. It stays because the day a field becomes optional or a list appears, this is the
    // line that refuses the second spelling before it becomes a permanent second identity.
    if encode(&payload).map_err(|_| RulingError::InvalidStore)? != stored.canonical_bytes {
        return Err(RulingError::InvalidStore);
    }
    if !columns_match(&stored, &payload) {
        return Err(RulingError::InvalidStore);
    }
    let force = derive_force(connection, stored.workspace_id, &stored, &payload)?;
    Ok(Ruling {
        digest: digest_envelope(&stored.digest_value),
        canonical_bytes: stored.canonical_bytes,
        payload,
        force,
    })
}

/// Every indexed column against the parsed value -- including the supersession relation against
/// the byte tag, which is the agreement ADR 0037 promises reconciliation will check. It is checked
/// here instead of only at startup because every read goes through this function, so a row and its
/// relation cannot disagree even between two restarts.
fn columns_match(stored: &StoredRuling, payload: &RulingV1) -> bool {
    let supersedes_agrees = match (&stored.supersedes, &payload.supersedes) {
        (None, RulingSupersedes::None) => true,
        (Some(recorded), RulingSupersedes::Ruling(encoded)) => recorded == encoded,
        _ => false,
    };
    supersedes_agrees
        && stored.proposal_digest == payload.proposal.value
        && stored.charter_revision == payload.bindings.charter_revision
        && stored.project_policy_revision == payload.bindings.project_policy_revision
        && stored.repository_revision == payload.bindings.repository_revision
        && stored.pinned_base_revision == payload.bindings.pinned_base_revision
        && stored.decision == payload.decision
        && stored.reason == payload.reason
        && stored.ruled_at_ms == payload.ruled_at_ms
}

// ---------------------------------------------------------------------------------------------
// Startup reconciliation
// ---------------------------------------------------------------------------------------------

/// Recomputes every Ruling from its stored bytes, re-verifies what it is bound to, re-derives
/// every supersession chain, and requires exactly one Event per Ruling with no orphan.
///
/// Force is derived along the way and never fails startup by itself: drift populates a list, it
/// does not raise an error, so a historical Ruling does not become corrupt because the world moved.
/// Everything that *does* refuse here is a store disagreeing with its own bytes.
pub(crate) fn verify_ruling_state(connection: &Connection) -> Result<(), RulingError> {
    let mut statement = connection.prepare(
        "SELECT workspace_id, digest_value, proposal_digest FROM rulings
         ORDER BY workspace_id, digest_value",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    // A record naming `owner` as its decider basis is only truthful while that account exists.
    if !rows.is_empty() && !owner_account_exists(connection)? {
        return Err(RulingError::InvalidStore);
    }

    let mut chains: Vec<(Uuid, String)> = Vec::new();
    for (workspace_id, digest_value, proposal_digest) in &rows {
        let workspace_id = parse_uuid(workspace_id)?;
        let stored = load_stored(connection, workspace_id, digest_value)?
            .ok_or(RulingError::InvalidStore)?;
        // Digest recomputed from the bytes, bytes decoded and re-encoded, every indexed column and
        // the supersession relation compared to the parsed value: `verified_ruling` is the same
        // proof every read performs, run here over the whole table.
        let ruling = verified_ruling(connection, stored)?;

        // What the Ruling copied must still be what the Proposal recorded. This is the one check a
        // read does not perform, and it is the whole point of copying: a Ruling whose copies
        // disagree with its Proposal would report drift against the wrong baseline, and would make
        // "ruled under this policy version" a claim nothing supports.
        let facts = proposal_binding_facts(connection, workspace_id, proposal_digest)
            .map_err(map_proposal_error)?
            .ok_or(RulingError::InvalidStore)?;
        if facts.bindings != ruling.payload().bindings {
            return Err(RulingError::InvalidStore);
        }

        let event_count: i64 = connection.query_row(
            "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = 'ruling.recorded'
               AND json_extract(payload_json, '$.ruling.digest.value') = ?2",
            params![workspace_id.to_string(), digest_value.as_str()],
            |row| row.get(0),
        )?;
        if event_count != 1 {
            return Err(RulingError::InvalidStore);
        }

        let chain_key = (workspace_id, proposal_digest.clone());
        if !chains.contains(&chain_key) {
            chains.push(chain_key);
        }
    }

    let total_events: i64 = connection.query_row(
        "SELECT count(*) FROM events WHERE kind = 'ruling.recorded'",
        [],
        |row| row.get(0),
    )?;
    if usize::try_from(total_events) != Ok(rows.len()) {
        return Err(RulingError::InvalidStore);
    }

    // Every chain re-derived rather than trusted: one head, one path, every Ruling on it. The
    // store forbids citing two predecessors and being superseded twice, but nothing in SQL forbids
    // two independent chains under one Proposal or a cycle whose every link satisfies both.
    for (workspace_id, proposal_digest) in &chains {
        let chain = ruling_chain(connection, *workspace_id, proposal_digest)?;
        let mut previous = 0_u64;
        for digest in &chain {
            let ruled_at_ms = ruled_at_ms_of(connection, digest)?;
            // Non-decreasing, equal allowed: two rulings can land in the same millisecond, and
            // refusing that would invent a constraint the clock does not support. The service
            // refuses a backwards step at write time; nothing else would notice a store that
            // holds one, so it is re-derived here.
            if ruled_at_ms < previous {
                return Err(RulingError::InvalidStore);
            }
            previous = ruled_at_ms;
        }
    }
    Ok(())
}

/// Requires one Event per Ruling whose decoded byte field exactly equals the stored Ruling bytes,
/// and whose encoding is the canonical padded base64 of them -- a differently encoded payload
/// carrying the same bytes is different evidence, not the same evidence reformatted.
///
/// This is the per-Event half of reconciliation; `verify_ruling_state` is the whole-table half.
/// An orphan Event is refused here rather than there, because an Event naming a digest no row
/// carries cannot be matched against a row at all.
pub(crate) fn verify_ruling_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), RulingError> {
    let payload: RulingRecordedPayload = serde_json::from_str(payload_json)?;
    let recorded = payload.ruling;
    if recorded.digest.algorithm != DIGEST_ALGORITHM
        || recorded.digest.canonicalizer != CANONICALIZER
        || recorded.ruled_at_ms != occurred_at_ms
    {
        return Err(RulingError::InvalidStore);
    }
    let canonical_bytes = BASE64
        .decode(recorded.canonical_bytes_base64.as_bytes())
        .map_err(|_| RulingError::InvalidStore)?;
    if BASE64.encode(&canonical_bytes) != recorded.canonical_bytes_base64 {
        return Err(RulingError::InvalidStore);
    }
    if HEXLOWER.encode(&sha256(&canonical_bytes)) != recorded.digest.value {
        return Err(RulingError::InvalidStore);
    }
    let stored = load_stored(connection, workspace_id, &recorded.digest.value)?
        .ok_or(RulingError::InvalidStore)?;
    if stored.canonical_bytes != canonical_bytes || stored.ruled_at_ms != occurred_at_ms {
        return Err(RulingError::InvalidStore);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------------------------

fn parse_uuid(value: &str) -> Result<Uuid, RulingError> {
    Uuid::parse_str(value).map_err(|_| RulingError::InvalidStore)
}

fn positive_u32(value: i64) -> Result<u32, RulingError> {
    let value = u32::try_from(value).map_err(|_| RulingError::InvalidStore)?;
    if value == 0 {
        return Err(RulingError::InvalidStore);
    }
    Ok(value)
}

/// A Proposal that cannot be read is an invalid store from a Ruling's point of view: the Ruling
/// binds it, so there is no version of "the Proposal is broken" that leaves this Ruling merely
/// unready. Only a store failure stays a store failure.
fn map_proposal_error(error: ProposalError) -> RulingError {
    match error {
        ProposalError::Store(error) => RulingError::Store(error),
        _ => RulingError::InvalidStore,
    }
}

fn map_policy_error(error: ProjectPolicyError) -> RulingError {
    match error {
        ProjectPolicyError::Store(error) => RulingError::Store(error),
        _ => RulingError::InvalidStore,
    }
}

fn map_repository_error(error: ProjectRepositoryError) -> RulingError {
    match error {
        ProjectRepositoryError::Store(error) => RulingError::Store(error),
        _ => RulingError::InvalidStore,
    }
}

// ---------------------------------------------------------------------------------------------
// Codec tests
//
// These need no store: they are about the bytes alone, which is where a canonicalizer's mistakes
// become permanent. Behaviour through the Command, Query, and Event interfaces lives in
// `tests/ruling.rs`, where the record is assembled by a builder that follows ADR 0037's published
// table rather than calling this encoder.
// ---------------------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    const PROPOSAL: &str = "3b1f8c0e5d4a2b697f8e1d0c3a5b7e9d2f4a6c8e0b1d3f5a7c9e1b3d5f7a9c1e";
    const OTHER_RULING: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";
    const BASE: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

    /// The store's own envelope for `rulings.canonical_bytes`, mirrored from migration 0019.
    /// Nothing this grammar can encode comes near it, which is why there is no size refusal on the
    /// command path; `the_largest_legal_record_fits_the_stores_envelope` is what makes that a
    /// measured fact rather than an assumption, so the constant lives where that measurement is.
    const STORE_CANONICAL_BYTES_CEILING: usize = 64 * 1024;

    fn fixture() -> RulingV1 {
        RulingV1 {
            proposal: ProposalDigest {
                algorithm: PROPOSAL_DIGEST_ALGORITHM.to_owned(),
                canonicalizer: PROPOSAL_CANONICALIZER.to_owned(),
                value: PROPOSAL.to_owned(),
            },
            bindings: ProposalBindings {
                charter_revision: 1,
                project_policy_revision: 2,
                repository_revision: 3,
                pinned_base_revision: BASE.to_owned(),
            },
            decision: RulingDecision::Approve,
            reason: "The declared effects stay inside the Repository and spend nothing.".to_owned(),
            decider: RulingDecider::Owner,
            supersedes: RulingSupersedes::None,
            ruled_at_ms: 1_756_800_000_000,
        }
    }

    #[test]
    fn the_record_starts_with_the_seventeen_byte_canonicalizer_magic() {
        // The wrong answer this kills is the design's own: it said the magic was 19 bytes, which
        // it inherited from `beacon.proposal.v1\0`. A codec built to that number would read every
        // field two bytes late.
        assert_eq!(MAGIC.len(), 17);
        assert_eq!(MAGIC, b"beacon.ruling.v1\0");
        let bytes = encode(&fixture()).unwrap();
        assert_eq!(&bytes[..MAGIC.len()], MAGIC);
        assert_ne!(MAGIC, b"beacon.proposal.v1\0");
    }

    #[test]
    fn a_known_fixture_reproduces_its_digest_from_the_bytes_alone() {
        // Reproduced by hand from ADR 0037's table, so this asserts the published grammar rather
        // than whatever the encoder happens to do.
        let mut expected = Vec::new();
        expected.extend_from_slice(b"beacon.ruling.v1\0");
        expected.extend_from_slice(&HEXLOWER.decode(PROPOSAL.as_bytes()).unwrap());
        expected.extend_from_slice(&6_u64.to_be_bytes());
        expected.extend_from_slice(b"sha256");
        expected.extend_from_slice(&18_u64.to_be_bytes());
        expected.extend_from_slice(b"beacon.proposal.v1");
        expected.extend_from_slice(&1_u32.to_be_bytes());
        expected.extend_from_slice(&2_u32.to_be_bytes());
        expected.extend_from_slice(&3_u32.to_be_bytes());
        expected.extend_from_slice(&40_u64.to_be_bytes());
        expected.extend_from_slice(BASE.as_bytes());
        expected.push(0x01);
        let reason = fixture().reason;
        expected.extend_from_slice(&(reason.len() as u64).to_be_bytes());
        expected.extend_from_slice(reason.as_bytes());
        expected.push(0x01);
        expected.push(0x00);
        expected.extend_from_slice(&1_756_800_000_000_u64.to_be_bytes());

        let bytes = encode(&fixture()).unwrap();
        assert_eq!(bytes, expected);
        assert_eq!(
            HEXLOWER.encode(&sha256(&bytes)),
            HEXLOWER.encode(&sha256(&expected))
        );
    }

    #[test]
    fn every_single_byte_of_the_fixture_is_load_bearing() {
        // A field the grammar reads but the record does not carry, or a byte no field consumes,
        // would let two different decisions share one identity.
        let bytes = encode(&fixture()).unwrap();
        for index in 0..bytes.len() {
            let mut altered = bytes.clone();
            altered[index] ^= 0xff;
            match decode(&altered) {
                Err(_) => {}
                Ok(payload) => assert_ne!(
                    encode(&payload).unwrap(),
                    bytes,
                    "byte {index} changed nothing"
                ),
            }
        }
    }

    #[test]
    fn a_trailing_byte_is_refused() {
        let mut bytes = encode(&fixture()).unwrap();
        bytes.push(0x00);
        assert_eq!(decode(&bytes), Err(MalformedRuling::TrailingBytes));
    }

    #[test]
    fn a_truncated_record_is_refused() {
        let bytes = encode(&fixture()).unwrap();
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length]).is_err(), "prefix {length} decoded");
        }
    }

    #[test]
    fn decode_then_encode_is_the_identity() {
        for supersedes in [
            RulingSupersedes::None,
            RulingSupersedes::Ruling(OTHER_RULING.to_owned()),
        ] {
            for decision in [RulingDecision::Approve, RulingDecision::Refuse] {
                let payload = RulingV1 {
                    decision,
                    supersedes: supersedes.clone(),
                    ..fixture()
                };
                let bytes = encode(&payload).unwrap();
                assert_eq!(decode(&bytes).unwrap(), payload);
                assert_eq!(encode(&decode(&bytes).unwrap()).unwrap(), bytes);
            }
        }
    }

    #[test]
    fn two_same_width_values_swapped_are_a_different_record_rather_than_a_refusal() {
        // Named for what it proves, because the earlier name claimed more than a positional
        // grammar can deliver (gpt2, SP2). A record whose `charter_revision` and
        // `project_policy_revision` are swapped IS a legitimate record whose revisions are those
        // numbers -- no predicate can tell the two apart without knowing an intent the bytes do
        // not carry. What holds is that the swap changes the identity, and that is asserted here.
        let bytes = encode(&fixture()).unwrap();
        let swapped = encode(&RulingV1 {
            bindings: ProposalBindings {
                charter_revision: 2,
                project_policy_revision: 1,
                ..fixture().bindings
            },
            ..fixture()
        })
        .unwrap();
        assert_ne!(swapped, bytes);
        assert_ne!(sha256(&swapped), sha256(&bytes));
        // And it decodes, as a different record. Saying so is the point: the refusal a caller
        // might expect does not exist, and a test that implied otherwise was hiding that.
        let decoded = decode(&swapped).unwrap();
        assert_eq!(decoded.bindings.charter_revision, 2);
        assert_eq!(decoded.bindings.project_policy_revision, 1);
    }

    #[test]
    fn a_reordering_the_grammar_can_see_is_refused() {
        // The other half of the same ruling: where a reordering IS structurally visible -- here
        // the two adjacent length-prefixed strings of the Proposal envelope -- it is refused
        // rather than read as some other record.
        let bytes = encode(&fixture()).unwrap();
        let mut swapped = Vec::new();
        swapped.extend_from_slice(MAGIC);
        swapped.extend_from_slice(&HEXLOWER.decode(PROPOSAL.as_bytes()).unwrap());
        swapped.extend_from_slice(&18_u64.to_be_bytes());
        swapped.extend_from_slice(b"beacon.proposal.v1");
        swapped.extend_from_slice(&6_u64.to_be_bytes());
        swapped.extend_from_slice(b"sha256");
        swapped.extend_from_slice(&bytes[MAGIC.len() + 32 + 8 + 6 + 8 + 18..]);
        assert_eq!(
            decode(&swapped),
            Err(MalformedRuling::ProposalDigestEnvelope)
        );
    }

    #[test]
    fn an_unassigned_tag_is_refused_in_every_closed_position() {
        let bytes = encode(&fixture()).unwrap();
        // decision, decider, and supersedes, located by walking the record the way the grammar
        // does rather than by a hand-counted constant that could drift.
        let reason_length = fixture().reason.len();
        let decision = MAGIC.len() + 32 + 8 + 6 + 8 + 18 + 4 + 4 + 4 + 8 + 40;
        let decider = decision + 1 + 8 + reason_length;
        let supersedes = decider + 1;
        assert_eq!(bytes[decision], TAG_DECISION_APPROVE);
        assert_eq!(bytes[decider], TAG_DECIDER_OWNER);
        assert_eq!(bytes[supersedes], TAG_SUPERSEDES_NONE);

        for (offset, unassigned) in [
            (decision, 0x00),
            (decision, 0x03),
            (decider, 0x00),
            (decider, 0x02),
            (supersedes, 0x02),
        ] {
            let mut altered = bytes.clone();
            altered[offset] = unassigned;
            assert_eq!(
                decode(&altered),
                Err(MalformedRuling::UnknownTag),
                "tag {unassigned:#04x} at {offset} was accepted"
            );
        }
    }

    #[test]
    fn absence_is_a_value_so_the_two_supersedes_states_have_different_identities() {
        // The wrong answer: omitting the field when there is no predecessor, which would make
        // "reverses nothing" and "reverses R" collide the moment a future grammar widened.
        let none = encode(&fixture()).unwrap();
        let cites = encode(&RulingV1 {
            supersedes: RulingSupersedes::Ruling(OTHER_RULING.to_owned()),
            ..fixture()
        })
        .unwrap();
        assert_ne!(none, cites);
        assert_eq!(cites.len(), none.len() + 32);
        assert_ne!(sha256(&none), sha256(&cites));
    }

    #[test]
    fn each_identity_bearing_field_changes_the_digest() {
        let base = sha256(&encode(&fixture()).unwrap());
        let variants = [
            RulingV1 {
                proposal: ProposalDigest {
                    value: OTHER_RULING.to_owned(),
                    ..fixture().proposal
                },
                ..fixture()
            },
            RulingV1 {
                bindings: ProposalBindings {
                    charter_revision: 9,
                    ..fixture().bindings
                },
                ..fixture()
            },
            RulingV1 {
                bindings: ProposalBindings {
                    project_policy_revision: 9,
                    ..fixture().bindings
                },
                ..fixture()
            },
            RulingV1 {
                bindings: ProposalBindings {
                    repository_revision: 9,
                    ..fixture().bindings
                },
                ..fixture()
            },
            RulingV1 {
                bindings: ProposalBindings {
                    pinned_base_revision: "f".repeat(40),
                    ..fixture().bindings
                },
                ..fixture()
            },
            RulingV1 {
                decision: RulingDecision::Refuse,
                ..fixture()
            },
            RulingV1 {
                reason: "A different reason entirely.".to_owned(),
                ..fixture()
            },
            RulingV1 {
                supersedes: RulingSupersedes::Ruling(OTHER_RULING.to_owned()),
                ..fixture()
            },
            RulingV1 {
                ruled_at_ms: 1_756_800_000_001,
                ..fixture()
            },
        ];
        for variant in variants {
            assert_ne!(sha256(&encode(&variant).unwrap()), base);
        }
    }

    #[test]
    fn the_reason_must_be_trimmed_bounded_and_free_of_control_characters() {
        for reason in [
            String::new(),
            "   ".to_owned(),
            " leading space".to_owned(),
            "trailing space ".to_owned(),
            "a NUL\u{0}inside".to_owned(),
            "a bell\u{7}inside".to_owned(),
            "x".repeat(MAX_REASON_SCALARS + 1),
        ] {
            let payload = RulingV1 {
                reason,
                ..fixture()
            };
            assert_eq!(encode(&payload), Err(MalformedRuling::Reason));
        }
        // Whitespace controls stay legal, and the boundary itself is accepted, so the refusal is
        // the rule rather than an off-by-one that would also refuse a legitimate reason.
        for reason in [
            "two\nlines\tapart".to_owned(),
            "x".repeat(MAX_REASON_SCALARS),
        ] {
            assert!(
                encode(&RulingV1 {
                    reason,
                    ..fixture()
                })
                .is_ok()
            );
        }
    }

    #[test]
    fn digests_and_the_base_revision_are_refused_rather_than_case_folded() {
        // Folding would let `A1B2...` and `a1b2...` name one Ruling, which is the silent
        // normalization of a Binding this grammar exists to refuse.
        assert_eq!(
            encode(&RulingV1 {
                proposal: ProposalDigest {
                    value: PROPOSAL.to_uppercase(),
                    ..fixture().proposal
                },
                ..fixture()
            }),
            Err(MalformedRuling::ProposalDigestValue)
        );
        assert_eq!(
            encode(&RulingV1 {
                supersedes: RulingSupersedes::Ruling(OTHER_RULING.to_uppercase()),
                ..fixture()
            }),
            Err(MalformedRuling::SupersedesDigest)
        );
        assert_eq!(
            encode(&RulingV1 {
                bindings: ProposalBindings {
                    pinned_base_revision: BASE.to_uppercase(),
                    ..fixture().bindings
                },
                ..fixture()
            }),
            Err(MalformedRuling::PinnedBaseRevision)
        );
        for wrong_length in [PROPOSAL[1..].to_owned(), format!("{PROPOSAL}00")] {
            assert_eq!(
                encode(&RulingV1 {
                    proposal: ProposalDigest {
                        value: wrong_length,
                        ..fixture().proposal
                    },
                    ..fixture()
                }),
                Err(MalformedRuling::ProposalDigestValue)
            );
        }
    }

    #[test]
    fn only_a_sha256_beacon_proposal_v1_envelope_is_accepted() {
        // A v1 Ruling that named another canonicalizer would claim to have ruled under a grammar
        // that did not exist when it was written.
        for (algorithm, canonicalizer) in [
            ("sha512", PROPOSAL_CANONICALIZER),
            (PROPOSAL_DIGEST_ALGORITHM, "beacon.proposal.v2"),
        ] {
            let payload = RulingV1 {
                proposal: ProposalDigest {
                    algorithm: algorithm.to_owned(),
                    canonicalizer: canonicalizer.to_owned(),
                    value: PROPOSAL.to_owned(),
                },
                ..fixture()
            };
            assert_eq!(
                encode(&payload),
                Err(MalformedRuling::ProposalDigestEnvelope)
            );
        }
    }

    #[test]
    fn the_largest_legal_record_fits_the_stores_envelope() {
        // `rulings.canonical_bytes` is CHECKed BETWEEN 17 AND 65536, and this goes red if a
        // future limit raise outgrows that column.
        let largest = RulingV1 {
            reason: "\u{10348}".repeat(MAX_REASON_SCALARS),
            supersedes: RulingSupersedes::Ruling(OTHER_RULING.to_owned()),
            ..fixture()
        };
        let bytes = encode(&largest).unwrap();
        assert_eq!(largest.reason.len(), MAX_REASON_BYTES);
        assert!(bytes.len() > MAX_REASON_BYTES);
        assert!(
            bytes.len() < STORE_CANONICAL_BYTES_CEILING,
            "{}",
            bytes.len()
        );
        assert_eq!(decode(&bytes).unwrap(), largest);
    }
}
