//! Mission impact classification v1, slice 1: the record's pure types and its canonical bytes.
//!
//! A `MissionImpactClassificationV1` is the durable record that says whether one Mission is
//! high-impact. BLUEPRINT `:871` and `:990` make that bit decide whether an Outcome Document's
//! verification appendix is mandatory, and §6 item 6 of the Outcome design removed `omit` from that
//! API so a caller cannot suppress the appendix by asking. This record is what makes the answer
//! citable instead of guessed.
//!
//! **It is scoped to one Mission and it is not a field on Project Policy.** `project_policy_revisions`
//! is keyed `(workspace_id, project_id, revision)` and carries no digest at all, so a field there
//! could not express "this one Mission" and could not be cited by digest. Worse, re-classifying one
//! Mission would need a Project-wide Policy revision, and that revision would change the answer for
//! Missions that already completed -- while their published Outcomes, being immutable, could not
//! follow. So this record cites Policy by the identity Policy actually has,
//! `(project_id, policy_revision, charter_revision)`, and carries its own digest: Kevin's ruling 5
//! for Receipts, applied again.
//!
//! **There is no `unclassified` arm.** A Mission with no classification record has no record; a
//! default would let omission suppress the appendix that removing `omit` was meant to protect. An
//! honest failure -- unpublishable until someone classifies -- beats a silent one.
//!
//! Slice 1 is pure: no database, no service, no event, no migration. R13.7's contract -- the bit is
//! copied into the Outcome manifest *and* re-checked by this module's verified reader at `prepare` --
//! is the reader side, and the reader arrives with the store in slice 2. What slice 1 owes it is a
//! record whose bit cannot be read without also reading the identity and digest it belongs to.

use crate::spend::PredecessorFault;
use std::fmt;

use std::sync::Arc;

use data_encoding::{BASE64, HEXLOWER};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use uuid::Uuid;

use crate::operator_account::{OperatorAccountError, OperatorAccountService};
use crate::proposal::sha256;

/// The canonicalizer identity. Any future grammar is a new canonicalizer; existing v1 bytes are
/// never reinterpreted under v2 rules.
pub const CANONICALIZER: &str = "beacon.mission-impact.v1";
/// The literal ASCII magic that starts every record, including its terminating NUL: 25 bytes.
const MAGIC: &[u8] = b"beacon.mission-impact.v1\0";
const DIGEST_ALGORITHM: &str = "sha256";

/// The store's envelope for `mission_impact_classifications.canonical_bytes`, enforced here first so
/// an over-large record is a fact about the bytes rather than a database error surfacing through a
/// Command.
const MAX_CANONICAL_BYTES: usize = 65_536;

const MAX_RULE_ID_BYTES: usize = 64;
const MAX_TEXT_SCALARS: usize = 2_000;
const MAX_TEXT_BYTES: usize = MAX_TEXT_SCALARS * 4;
const DIGEST_HEX_CHARS: usize = 64;

// One-byte tags. Assigned explicitly and never renumbered. `supersedes` is the one position where
// 0x00 is assigned, because absence is a value there and must encode as one.
const TAG_IMPACT_HIGH: u8 = 0x01;
const TAG_IMPACT_ORDINARY: u8 = 0x02;

const TAG_BASIS_OPERATOR_DECLARED: u8 = 0x01;
const TAG_BASIS_POLICY_RULE: u8 = 0x02;

const TAG_ACTOR_OPERATOR: u8 = 0x01;

const TAG_SUPERSEDES_NONE: u8 = 0x00;
const TAG_SUPERSEDES_RECORD: u8 = 0x01;

const HIGH: &str = "high";
const ORDINARY: &str = "ordinary";

// ---------------------------------------------------------------------------------------------
// The closed vocabularies
// ---------------------------------------------------------------------------------------------

/// What the classification says about the Mission.
///
/// Two arms and deliberately no third. `unclassified` is not a value: it is the absence of a
/// record, and giving it a tag would let a caller record "we did not decide" and have the appendix
/// treat it as ordinary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpactLevel {
    /// Policy-classified high impact: the verification appendix is mandatory (`BLUEPRINT:871`).
    High,
    Ordinary,
}

impl ImpactLevel {
    fn tag(self) -> u8 {
        match self {
            Self::High => TAG_IMPACT_HIGH,
            Self::Ordinary => TAG_IMPACT_ORDINARY,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedClassification> {
        match tag {
            TAG_IMPACT_HIGH => Ok(Self::High),
            TAG_IMPACT_ORDINARY => Ok(Self::Ordinary),
            _ => Err(MalformedClassification::UnknownTag),
        }
    }

    /// The literal token this level is recorded and rendered as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => HIGH,
            Self::Ordinary => ORDINARY,
        }
    }
}

/// What decided the level.
///
/// `policy_rule` has a tag in the grammar and no path through the Command in v1, which is ADR 0034's
/// `bound`-descriptor pattern: define the tag, refuse it at submission, "so a later reviewed decision
/// needs no new tag, and existing identities are never reinterpreted". A rule engine that classifies
/// without an Operator is a new authority surface and is P-2's ruling, not this grammar's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassificationBasis {
    OperatorDeclared,
    PolicyRule,
}

impl ClassificationBasis {
    fn tag(self) -> u8 {
        match self {
            Self::OperatorDeclared => TAG_BASIS_OPERATOR_DECLARED,
            Self::PolicyRule => TAG_BASIS_POLICY_RULE,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedClassification> {
        match tag {
            TAG_BASIS_OPERATOR_DECLARED => Ok(Self::OperatorDeclared),
            TAG_BASIS_POLICY_RULE => Ok(Self::PolicyRule),
            _ => Err(MalformedClassification::UnknownTag),
        }
    }

    /// The lowercase token this basis is recorded as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OperatorDeclared => "operator_declared",
            Self::PolicyRule => "policy_rule",
        }
    }
}

/// Who recorded the classification.
///
/// One arm, for the reason `RulingDecider` has one: there is no Operator UUID in the singleton
/// account model, so v1 records the closed basis rather than inventing an identity. It is *not*
/// redundant with [`ClassificationBasis`] -- basis says what decided, actor says who recorded, and
/// the two separate the moment a rule engine exists. (Measured spend v1 dropped its own
/// `measured_by` for the opposite reason: there it would have been one-to-one with `origin`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassificationActor {
    Operator,
}

/// The Project Policy revision this classification was decided under, copied whole.
///
/// The charter revision travels with it because `applies_to_current_charter` on `main` shows a
/// Policy revision can go stale for reasons that have nothing to do with impact. Copying is ADR
/// 0037's argument for copying a Proposal's bindings into a Ruling: a copy makes later drift
/// detectable without trusting the source row to have stayed put.
///
/// Policy is cited by revision rather than by digest because Policy has no digest --
/// `project_policy_revisions` stores no canonical bytes, and minting one for somebody else's shipped
/// table is what Kevin's ruling 5 refused for Receipts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRevisionRef {
    pub project_id: Uuid,
    pub policy_revision: u32,
    pub charter_revision: u32,
}

/// Whether this record replaces an earlier classification of the same Mission, and which.
///
/// Named by the predecessor's digest, not its version number: a digest is self-proving from the
/// bytes alone while a version needs the store to say what that version was (ADR 0037, ADR 0038).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassificationSupersedes {
    None,
    Record(String),
}

/// The immutable v1 payload, exactly as its canonical bytes decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionImpactClassificationV1 {
    pub workspace_id: Uuid,
    pub mission_id: Uuid,
    /// The record's **reference** identity: which record a citation names (R15). The digest is its
    /// **content** identity. The Outcome manifest cites `(workspace_id, classification_id, version,
    /// digest)` and its verified reader refuses a pair that disagrees.
    pub classification_id: Uuid,
    pub version: u32,
    pub impact: ImpactLevel,
    pub basis: ClassificationBasis,
    pub policy: PolicyRevisionRef,
    /// The named rule, present exactly when `basis` is `policy_rule` and refused otherwise. An
    /// empty string is not "no rule": absence is the `operator_declared` arm.
    pub rule_id: Option<String>,
    pub reason: String,
    pub classified_by: ClassificationActor,
    /// A stored UTC millisecond timestamp, never a clock read at read time.
    pub classified_at_ms: u64,
    pub supersedes: ClassificationSupersedes,
}

// ---------------------------------------------------------------------------------------------
// Refusals -- each names itself
// ---------------------------------------------------------------------------------------------

/// Bytes or field values that cannot be a v1 classification record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedClassification {
    Magic,
    Truncated,
    TrailingBytes,
    UnknownTag,
    InvalidUtf8,
    RecordTooLarge,
    OverLimitLength,
    /// A `policy_rule` basis with no rule named, or an `operator_declared` basis that named one.
    RuleIdBasisMismatch,
    /// A rule id that is empty, over-long, or outside `[a-z0-9._-]`.
    RuleId,
    EmptyText,
    UntrimmedText,
    TextTooLong,
    /// A control character other than LF or TAB.
    ControlCharacter,
    /// A revision below one. Policy and Charter revisions are contiguous from 1.
    Revision,
    SupersedesDigest,
}

impl MalformedClassification {
    /// A stable lowercase reason token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Magic => "magic",
            Self::Truncated => "truncated",
            Self::TrailingBytes => "trailing_bytes",
            Self::UnknownTag => "unknown_tag",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::RecordTooLarge => "record_too_large",
            Self::OverLimitLength => "over_limit_length",
            Self::RuleIdBasisMismatch => "rule_id_basis_mismatch",
            Self::RuleId => "rule_id",
            Self::EmptyText => "empty_text",
            Self::UntrimmedText => "untrimmed_text",
            Self::TextTooLong => "text_too_long",
            Self::ControlCharacter => "control_character",
            Self::Revision => "revision",
            Self::SupersedesDigest => "supersedes_digest",
        }
    }
}

impl fmt::Display for MalformedClassification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for MalformedClassification {}

/// A digest that carries its algorithm and canonicalizer identity, never a bare hex string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationDigest {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

fn require_rule_id(value: &str) -> Result<(), MalformedClassification> {
    if value.is_empty() || value.len() > MAX_RULE_ID_BYTES {
        return Err(MalformedClassification::RuleId);
    }
    if value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
    }) {
        Ok(())
    } else {
        Err(MalformedClassification::RuleId)
    }
}

/// Human text a record may carry. LF and TAB are content and are preserved; every other control
/// character is refused by name (R13.5, applied to a record rather than to a document).
fn require_text(value: &str) -> Result<(), MalformedClassification> {
    if value.is_empty() {
        return Err(MalformedClassification::EmptyText);
    }
    if value.trim() != value {
        return Err(MalformedClassification::UntrimmedText);
    }
    if value.chars().count() > MAX_TEXT_SCALARS {
        return Err(MalformedClassification::TextTooLong);
    }
    if value
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(MalformedClassification::ControlCharacter);
    }
    Ok(())
}

fn digest_bytes(value: &str) -> Result<[u8; 32], MalformedClassification> {
    if value.len() != DIGEST_HEX_CHARS {
        return Err(MalformedClassification::SupersedesDigest);
    }
    let decoded = HEXLOWER
        .decode(value.as_bytes())
        .map_err(|_| MalformedClassification::SupersedesDigest)?;
    let mut bytes = [0_u8; 32];
    if decoded.len() != bytes.len() {
        return Err(MalformedClassification::SupersedesDigest);
    }
    bytes.copy_from_slice(&decoded);
    Ok(bytes)
}

/// The rule that keeps `basis` and `rule_id` two spellings of one fact rather than two facts.
///
/// A `policy_rule` classification that names no rule cannot be audited; an `operator_declared` one
/// that names a rule claims a provenance it does not have.
fn require_rule_for_basis(
    basis: ClassificationBasis,
    rule_id: Option<&str>,
) -> Result<(), MalformedClassification> {
    match (basis, rule_id) {
        (ClassificationBasis::PolicyRule, Some(value)) => require_rule_id(value),
        (ClassificationBasis::OperatorDeclared, None) => Ok(()),
        _ => Err(MalformedClassification::RuleIdBasisMismatch),
    }
}

// ---------------------------------------------------------------------------------------------
// The canonical bytes
// ---------------------------------------------------------------------------------------------

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

    fn string(&mut self, value: &str, limit: usize) -> Result<(), MalformedClassification> {
        if value.len() > limit {
            return Err(MalformedClassification::OverLimitLength);
        }
        let length =
            u64::try_from(value.len()).map_err(|_| MalformedClassification::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value.as_bytes());
        Ok(())
    }
}

/// Reads canonical bytes. Self-delimiting: every field proves it fits within the remaining input and
/// its own limit before any of it is consumed.
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

    fn take(&mut self, count: usize) -> Result<&'a [u8], MalformedClassification> {
        if count > self.remaining() {
            return Err(MalformedClassification::Truncated);
        }
        let slice = &self.input[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn magic(&mut self) -> Result<(), MalformedClassification> {
        if self
            .take(MAGIC.len())
            .map_err(|_| MalformedClassification::Magic)?
            == MAGIC
        {
            Ok(())
        } else {
            Err(MalformedClassification::Magic)
        }
    }

    fn tag(&mut self) -> Result<u8, MalformedClassification> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, MalformedClassification> {
        let mut value = [0_u8; 4];
        value.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(value))
    }

    fn u64(&mut self) -> Result<u64, MalformedClassification> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }

    fn uuid(&mut self) -> Result<Uuid, MalformedClassification> {
        let mut value = [0_u8; 16];
        value.copy_from_slice(self.take(16)?);
        Ok(Uuid::from_bytes(value))
    }

    fn sha256_hex(&mut self) -> Result<String, MalformedClassification> {
        let mut value = [0_u8; 32];
        value.copy_from_slice(self.take(32)?);
        Ok(HEXLOWER.encode(&value))
    }

    fn string(&mut self, limit: usize) -> Result<String, MalformedClassification> {
        let length = self.u64()?;
        let limit = u64::try_from(limit).map_err(|_| MalformedClassification::OverLimitLength)?;
        if length > limit {
            return Err(MalformedClassification::OverLimitLength);
        }
        let length = usize::try_from(length).map_err(|_| MalformedClassification::Truncated)?;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| MalformedClassification::InvalidUtf8)
    }

    fn finish(self) -> Result<(), MalformedClassification> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MalformedClassification::TrailingBytes)
        }
    }
}

/// The one canonical encoding of a payload. The field order, widths, and tag values are ADR 0044's
/// table, and this is the executable form of it.
fn encode(payload: &MissionImpactClassificationV1) -> Result<Vec<u8>, MalformedClassification> {
    require_rule_for_basis(payload.basis, payload.rule_id.as_deref())?;
    require_text(&payload.reason)?;
    if payload.policy.policy_revision < 1 || payload.policy.charter_revision < 1 {
        return Err(MalformedClassification::Revision);
    }

    let mut encoder = Encoder::new();
    encoder.uuid(&payload.workspace_id);
    encoder.uuid(&payload.mission_id);
    encoder.uuid(&payload.classification_id);
    encoder.u32(payload.version);
    encoder.tag(payload.impact.tag());
    encoder.tag(payload.basis.tag());
    encoder.uuid(&payload.policy.project_id);
    encoder.u32(payload.policy.policy_revision);
    encoder.u32(payload.policy.charter_revision);
    // Absence is a value here and encodes as one, so `operator_declared` and a `policy_rule` whose
    // id happened to be empty are not one byte string.
    match &payload.rule_id {
        None => encoder.u64(0),
        Some(value) => encoder.string(value, MAX_RULE_ID_BYTES)?,
    }
    encoder.string(&payload.reason, MAX_TEXT_BYTES)?;
    match payload.classified_by {
        ClassificationActor::Operator => encoder.tag(TAG_ACTOR_OPERATOR),
    }
    encoder.u64(payload.classified_at_ms);
    match &payload.supersedes {
        ClassificationSupersedes::None => encoder.tag(TAG_SUPERSEDES_NONE),
        ClassificationSupersedes::Record(digest) => {
            encoder.tag(TAG_SUPERSEDES_RECORD);
            encoder.sha256(&digest_bytes(digest)?);
        }
    }

    if encoder.out.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedClassification::RecordTooLarge);
    }
    Ok(encoder.out)
}

fn decode(input: &[u8]) -> Result<MissionImpactClassificationV1, MalformedClassification> {
    if input.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedClassification::RecordTooLarge);
    }
    let mut decoder = Decoder::new(input);
    decoder.magic()?;

    let workspace_id = decoder.uuid()?;
    let mission_id = decoder.uuid()?;
    let classification_id = decoder.uuid()?;
    let version = decoder.u32()?;
    let impact = ImpactLevel::parse(decoder.tag()?)?;
    let basis = ClassificationBasis::parse(decoder.tag()?)?;
    let project_id = decoder.uuid()?;
    let policy_revision = decoder.u32()?;
    let charter_revision = decoder.u32()?;
    if policy_revision < 1 || charter_revision < 1 {
        return Err(MalformedClassification::Revision);
    }
    let rule_id = {
        let value = decoder.string(MAX_RULE_ID_BYTES)?;
        if value.is_empty() { None } else { Some(value) }
    };
    require_rule_for_basis(basis, rule_id.as_deref())?;
    let reason = decoder.string(MAX_TEXT_BYTES)?;
    require_text(&reason)?;
    let classified_by = match decoder.tag()? {
        TAG_ACTOR_OPERATOR => ClassificationActor::Operator,
        _ => return Err(MalformedClassification::UnknownTag),
    };
    let classified_at_ms = decoder.u64()?;
    let supersedes = match decoder.tag()? {
        TAG_SUPERSEDES_NONE => ClassificationSupersedes::None,
        TAG_SUPERSEDES_RECORD => ClassificationSupersedes::Record(decoder.sha256_hex()?),
        _ => return Err(MalformedClassification::UnknownTag),
    };
    decoder.finish()?;

    Ok(MissionImpactClassificationV1 {
        workspace_id,
        mission_id,
        classification_id,
        version,
        impact,
        basis,
        policy: PolicyRevisionRef {
            project_id,
            policy_revision,
            charter_revision,
        },
        rule_id,
        reason,
        classified_by,
        classified_at_ms,
        supersedes,
    })
}

impl MissionImpactClassificationV1 {
    /// The one canonical byte encoding of this record.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MalformedClassification> {
        encode(self)
    }

    /// The record's content identity: SHA-256 over its canonical bytes, carrying its own envelope.
    pub fn digest(&self) -> Result<ClassificationDigest, MalformedClassification> {
        Ok(ClassificationDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: CANONICALIZER.to_owned(),
            value: HEXLOWER.encode(&sha256(&self.canonical_bytes()?)),
        })
    }

    /// Decode canonical bytes back into a record. Strict: unknown tags, over-limit or invalid
    /// lengths, trailing bytes, invalid UTF-8, and a rule id that disagrees with its basis are all
    /// refused.
    pub fn from_canonical_bytes(input: &[u8]) -> Result<Self, MalformedClassification> {
        decode(input)
    }

    /// Whether the verification appendix is mandatory for the Mission this record classifies.
    ///
    /// Deliberately **not** a bare `bool` accessor on the level: a caller reaches this through the
    /// record, so it cannot get the bit without holding the identity and digest the Outcome manifest
    /// must cite beside it (R13.7). There is no `is_high_impact(mission_id)` anywhere, for the reason
    /// `RulingForce` exists instead of `is_in_force`.
    pub fn appendix_is_mandatory(&self) -> bool {
        self.impact == ImpactLevel::High
    }
}

// ---------------------------------------------------------------------------------------------
// The store and its verified reader (slice 2)
// ---------------------------------------------------------------------------------------------

/// Why a stored classification could not be returned. Every arm is an integrity failure that
/// aborts; none becomes a classification value, because there is no honest classification state for
/// "the store is wrong".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassificationStoreError {
    DigestMismatch,
    Malformed(MalformedClassification),
    /// A projection column disagrees with the canonical bytes.
    ColumnsDisagree,
    /// A citation named this digest with a different `classification_id` or `version` (R15).
    CitedIdentityMismatch,
    CrossWorkspace,
    /// R13.7: the bit a caller copied into a manifest disagrees with the record it cites.
    CopiedBitDisagrees,
    /// The policy the record cites belongs to a Project that does not own the record's Mission
    /// (sol1's cold read, #2). Both foreign keys held on their own -- the Mission exists, the
    /// Policy revision exists -- and neither says the two belong together; this is the check that
    /// does, keyed on the Mission's own row.
    PolicyProjectIsNotTheMissions,
    /// The Charter revision the record copies beside its Policy citation is not the one that Policy
    /// revision's own row records (sol1's cold read, #12). The foreign key covers `(project,
    /// policy_revision)` and the column comparison only proves the bytes and the row repeat the
    /// same copied number; this compares the number with the Policy's own record.
    PolicyRevisionHasAnotherCharter,
    /// The record it says it supersedes is not the predecessor it claims (sol1's cold read, #9):
    /// judged by the predecessor's own bytes, never by the successor's word.
    Predecessor {
        digest: String,
        fault: PredecessorFault,
    },
    Store(String),
}

impl fmt::Display for ClassificationStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DigestMismatch => formatter.write_str("digest_mismatch"),
            Self::Malformed(reason) => write!(formatter, "malformed: {reason}"),
            Self::ColumnsDisagree => formatter.write_str("columns_disagree"),
            Self::CitedIdentityMismatch => formatter.write_str("cited_identity_mismatch"),
            Self::CrossWorkspace => formatter.write_str("cross_workspace"),
            Self::CopiedBitDisagrees => formatter.write_str("copied_bit_disagrees"),
            Self::PolicyProjectIsNotTheMissions => {
                formatter.write_str("policy_project_is_not_the_missions")
            }
            Self::PolicyRevisionHasAnotherCharter => {
                formatter.write_str("policy_revision_has_another_charter")
            }
            Self::Predecessor { digest, fault } => {
                write!(formatter, "predecessor {digest}: {fault}")
            }
            // The NAME only (R28.4). The reason is built from `rusqlite::Error::to_string()` at
            // every site in this file that constructs this arm, and that text can name a column and
            // a constraint value. It stays in the value and reaches a reader through `Debug`; it
            // does not reach the one line a startup refusal prints.
            Self::Store(_) => formatter.write_str("store"),
        }
    }
}

impl std::error::Error for ClassificationStoreError {}

/// One stored classification: its bytes, their identity, and what they decode to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionImpactClassification {
    digest: ClassificationDigest,
    canonical_bytes: Vec<u8>,
    payload: MissionImpactClassificationV1,
}

impl MissionImpactClassification {
    pub fn digest(&self) -> &ClassificationDigest {
        &self.digest
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    pub fn payload(&self) -> &MissionImpactClassificationV1 {
        &self.payload
    }

    /// R13.7's re-check, and the reason this method exists rather than a bare getter.
    ///
    /// `prepare` copies the impact bit into the Outcome manifest **and** re-checks it against this
    /// exact record. Handing a caller a `bool` would let it copy one and check nothing; handing it
    /// this makes the copy and the check one call, and the refusal names what disagreed.
    pub fn recheck_copied_bit(
        &self,
        copied_high_impact: bool,
    ) -> Result<(), ClassificationStoreError> {
        if copied_high_impact == self.payload.appendix_is_mandatory() {
            Ok(())
        } else {
            Err(ClassificationStoreError::CopiedBitDisagrees)
        }
    }
}

struct StoredClassificationRow {
    digest_value: String,
    canonical_bytes: Vec<u8>,
    classification_id: Uuid,
    mission_id: Uuid,
    project_id: Uuid,
    version: u32,
    impact: String,
    basis: String,
    policy_revision: u32,
    charter_revision: u32,
    classified_at_ms: u64,
}

fn classification_columns_match(
    row: &StoredClassificationRow,
    payload: &MissionImpactClassificationV1,
) -> bool {
    row.classification_id == payload.classification_id
        && row.mission_id == payload.mission_id
        && row.project_id == payload.policy.project_id
        && row.version == payload.version
        && row.impact == payload.impact.as_str()
        && row.basis == payload.basis.as_str()
        && row.policy_revision == payload.policy.policy_revision
        && row.charter_revision == payload.policy.charter_revision
        && row.classified_at_ms == payload.classified_at_ms
}

fn load_classification_row(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<StoredClassificationRow>, ClassificationStoreError> {
    connection
        .query_row(
            "SELECT digest_value, canonical_bytes, classification_id, mission_id, project_id,
                    version, impact, basis, policy_revision, charter_revision, classified_at_ms
             FROM mission_impact_classifications
             WHERE workspace_id = ?1 AND digest_value = ?2",
            rusqlite::params![workspace_id.to_string(), digest_value],
            |row| {
                Ok(StoredClassificationRow {
                    digest_value: row.get(0)?,
                    canonical_bytes: row.get(1)?,
                    classification_id: uuid_column("classification_id", row.get::<_, String>(2)?)?,
                    mission_id: uuid_column("mission_id", row.get::<_, String>(3)?)?,
                    project_id: uuid_column("project_id", row.get::<_, String>(4)?)?,
                    version: row.get::<_, i64>(5)? as u32,
                    impact: row.get(6)?,
                    basis: row.get(7)?,
                    policy_revision: row.get::<_, i64>(8)? as u32,
                    charter_revision: row.get::<_, i64>(9)? as u32,
                    classified_at_ms: row.get::<_, i64>(10)? as u64,
                })
            },
        )
        .optional()
        .map_err(|error| ClassificationStoreError::Store(error.to_string()))
}

/// Parse an id column, or refuse with the text that was actually there (R39.2).
///
/// It used to be `Uuid::parse_str(&value).unwrap_or(Uuid::nil())`. A corrupt column then became a
/// well-formed UUID -- and nil is a value a later reader can carry forward, compare, and store as
/// though the row had said it. Reporting what was found costs one `?` and invents nothing.
///
/// The error is a `rusqlite` conversion failure because the caller is inside a `query_row` closure:
/// it propagates through the caller's existing
/// `ClassificationStoreError::Store(error.to_string())` mapping, where the raw text reaches a reader
/// through `Debug` and never through the one-line Display (R28.4).
fn uuid_column(column: &'static str, value: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&value).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::other(format!(
                "the {column} column is not a UUID: {value:?}"
            ))),
        )
    })
}

/// One classification, verified, by digest within a Workspace.
/// The Project that owns a Mission, from the Mission's own row. `None` is "no such Mission", which
/// the foreign key already refuses on write and which a reader treats as a corrupt store.
fn mission_project_of(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Option<Uuid>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT project_id FROM missions WHERE workspace_id = ?1 AND id = ?2",
            rusqlite::params![workspace_id.to_string(), mission_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map(|found| found.and_then(|value| Uuid::parse_str(&value).ok()))
}

/// The Charter revision a Policy revision's own row records. `None` is "no such Policy revision",
/// which the composite foreign key refuses on write and a reader treats as a corrupt store.
fn policy_charter_revision(
    connection: &Connection,
    workspace_id: Uuid,
    policy: &PolicyRevisionRef,
) -> Result<Option<u32>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT charter_revision FROM project_policy_revisions
             WHERE workspace_id = ?1 AND project_id = ?2 AND revision = ?3",
            rusqlite::params![
                workspace_id.to_string(),
                policy.project_id.to_string(),
                i64::from(policy.policy_revision)
            ],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|found| found.and_then(|value| u32::try_from(value).ok()))
}

pub fn verified_classification_within(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<MissionImpactClassification>, ClassificationStoreError> {
    let Some(row) = load_classification_row(connection, workspace_id, digest_value)? else {
        return Ok(None);
    };
    if HEXLOWER.encode(&sha256(&row.canonical_bytes)) != row.digest_value {
        return Err(ClassificationStoreError::DigestMismatch);
    }
    let payload = decode(&row.canonical_bytes).map_err(ClassificationStoreError::Malformed)?;
    if encode(&payload).map_err(ClassificationStoreError::Malformed)? != row.canonical_bytes {
        return Err(ClassificationStoreError::Malformed(
            MalformedClassification::TrailingBytes,
        ));
    }
    if !classification_columns_match(&row, &payload) {
        return Err(ClassificationStoreError::ColumnsDisagree);
    }
    if payload.workspace_id != workspace_id {
        return Err(ClassificationStoreError::CrossWorkspace);
    }
    // The policy's Project must OWN the Mission. Both foreign keys hold on their own and neither
    // ties the two together, so a Mission of Project A classified under Project B's valid policy
    // read back as verified until this check existed.
    let owner = mission_project_of(connection, workspace_id, payload.mission_id)
        .map_err(|error| ClassificationStoreError::Store(error.to_string()))?;
    if owner != Some(payload.policy.project_id) {
        return Err(ClassificationStoreError::PolicyProjectIsNotTheMissions);
    }
    // The Charter revision copied beside the Policy citation must be the one that Policy
    // revision's own row records (sol1's cold read, #12). A missing row is not "another charter":
    // the foreign key refused it on write, so here it is a corrupt store.
    match policy_charter_revision(connection, workspace_id, &payload.policy)
        .map_err(|error| ClassificationStoreError::Store(error.to_string()))?
    {
        Some(charter) if charter == payload.policy.charter_revision => {}
        Some(_) => return Err(ClassificationStoreError::PolicyRevisionHasAnotherCharter),
        None => {
            return Err(ClassificationStoreError::Store(
                "the cited Policy revision is not stored".to_owned(),
            ));
        }
    }
    // The record it says it supersedes is a classification of this Mission, in this Workspace,
    // by the predecessor's OWN bytes (sol1's cold read, #9) -- on every read.
    if let Some((digest, fault)) = predecessor_fault(connection, &payload)? {
        return Err(ClassificationStoreError::Predecessor { digest, fault });
    }
    Ok(Some(MissionImpactClassification {
        digest: ClassificationDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: CANONICALIZER.to_owned(),
            value: row.digest_value,
        },
        canonical_bytes: row.canonical_bytes,
        payload,
    }))
}

/// The fault, if any, in a classification's `supersedes` link, judged by the predecessor's own
/// bytes -- looked up by digest alone, hashed and decoded rather than trusted, exactly as
/// `spend::predecessor_fault` does for the other ladder.
fn predecessor_fault(
    connection: &Connection,
    payload: &MissionImpactClassificationV1,
) -> Result<Option<(String, PredecessorFault)>, ClassificationStoreError> {
    let ClassificationSupersedes::Record(digest_value) = &payload.supersedes else {
        return Ok(None);
    };
    let stored: Option<Vec<u8>> = connection
        .query_row(
            "SELECT canonical_bytes FROM mission_impact_classifications WHERE digest_value = ?1",
            rusqlite::params![digest_value],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| ClassificationStoreError::Store(error.to_string()))?;
    let fault = match stored {
        None => PredecessorFault::NotStored,
        Some(bytes) if HEXLOWER.encode(&sha256(&bytes)) != *digest_value => {
            PredecessorFault::Unverifiable(ClassificationStoreError::DigestMismatch.to_string())
        }
        Some(bytes) => match decode(&bytes) {
            Err(reason) => PredecessorFault::Unverifiable(
                ClassificationStoreError::Malformed(reason).to_string(),
            ),
            Ok(predecessor) if predecessor.workspace_id != payload.workspace_id => {
                PredecessorFault::InAnotherWorkspace
            }
            Ok(predecessor) if predecessor.mission_id != payload.mission_id => {
                PredecessorFault::ForAnotherMission
            }
            Ok(_) => return Ok(None),
        },
    };
    Ok(Some((digest_value.clone(), fault)))
}

/// The reader a citation uses: `(workspace_id, classification_id, version, digest)` (R15).
pub fn verified_classification_for_citation(
    connection: &Connection,
    workspace_id: Uuid,
    classification_id: Uuid,
    version: u32,
    digest_value: &str,
) -> Result<Option<MissionImpactClassification>, ClassificationStoreError> {
    let Some(record) = verified_classification_within(connection, workspace_id, digest_value)?
    else {
        return Ok(None);
    };
    if record.payload.classification_id != classification_id || record.payload.version != version {
        return Err(ClassificationStoreError::CitedIdentityMismatch);
    }
    Ok(Some(record))
}

// ---------------------------------------------------------------------------------------------
// The write path: one Command, one record, one Event
// ---------------------------------------------------------------------------------------------

/// Why a classification could not be recorded. Same shape, and the same reasons, as
/// `SpendRecordError`.
#[derive(Debug)]
pub enum ClassificationRecordError {
    Account(OperatorAccountError),
    Malformed(MalformedClassification),
    /// The bytes decode but do not re-encode to themselves (ADR 0034's submit contract).
    NotCanonical,
    CrossWorkspace,
    /// `(workspace, mission, version)` or `classification_id` is already taken. A reclassification
    /// is a new record at the next version, never an edit.
    IdentityTaken,
    /// The composite foreign key refused: this Workspace's Project has no such Policy revision.
    /// The store answers it, so it cannot be a check a caller forgets.
    PolicyRevisionNotFound,
    /// The cited policy's Project does not own the Mission being classified. The Command refuses
    /// it before the row exists, and the reader refuses it on every read, for the same reason in
    /// the same words.
    PolicyProjectIsNotTheMissions,
    /// The Charter revision copied beside the Policy citation is not the one that Policy revision
    /// records. Refused by the verified read-back inside the recording transaction, like
    /// `Predecessor`, and for the same reason without a Command-side twin.
    PolicyRevisionHasAnotherCharter,
    /// The record it says it supersedes is not the predecessor it claims: refused by the verified
    /// read-back inside the recording transaction and by the reader on every read after. No
    /// separate Command-side copy, for the reason `SpendRecordError::Predecessor` gives.
    Predecessor {
        digest: String,
        fault: PredecessorFault,
    },
    Store(String),
}

impl From<OperatorAccountError> for ClassificationRecordError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for ClassificationRecordError {
    fn from(error: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(inner, _) = &error
            && inner.code == rusqlite::ErrorCode::ConstraintViolation
        {
            // A foreign-key refusal and a uniqueness refusal are two different mistakes, and
            // collapsing them would tell a caller to bump a version when the Policy revision is
            // what is missing.
            return match inner.extended_code {
                rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY => Self::PolicyRevisionNotFound,
                _ => Self::IdentityTaken,
            };
        }
        Self::Store(error.to_string())
    }
}

impl From<ClassificationStoreError> for ClassificationRecordError {
    fn from(error: ClassificationStoreError) -> Self {
        match error {
            ClassificationStoreError::Malformed(reason) => Self::Malformed(reason),
            ClassificationStoreError::PolicyRevisionHasAnotherCharter => {
                Self::PolicyRevisionHasAnotherCharter
            }
            ClassificationStoreError::Predecessor { digest, fault } => {
                Self::Predecessor { digest, fault }
            }
            other => Self::Store(other.to_string()),
        }
    }
}

/// The account fault's NAME, for this caller's outward line (R42).
///
/// `OperatorAccountError`'s own `Display` is not closed -- its `Store` arm carries a driver error
/// whose text names columns and paths -- and its `Debug` is worse. A caller that must say *which*
/// account fault it was therefore projects the name itself, and a caller-owned exhaustive match is
/// what makes a new account variant a compile error here rather than a silent new disclosure.
///
/// Deliberately duplicated in `outcome_publication.rs` rather than shared: each caller owns the
/// projection it emits, and one shared helper would let a change made for one caller's audience
/// travel to the other's without anyone reading the second line (R42 selector ownership).
/// `AuthenticationThrottled` keeps its `retry_after_ms` in the typed value and in `Debug`; this
/// projection is the name alone, and the wire's retry handling is unchanged.
fn account_tag(error: &OperatorAccountError) -> &'static str {
    match error {
        OperatorAccountError::PasswordTooShort => "PasswordTooShort",
        OperatorAccountError::PasswordTooLong => "PasswordTooLong",
        OperatorAccountError::PasswordConfirmationMismatch => "PasswordConfirmationMismatch",
        OperatorAccountError::RecoveryNotConfirmed => "RecoveryNotConfirmed",
        OperatorAccountError::SetupNotStarted => "SetupNotStarted",
        OperatorAccountError::AccountAlreadyExists => "AccountAlreadyExists",
        OperatorAccountError::AccountMissing => "AccountMissing",
        OperatorAccountError::AuthenticationFailed => "AuthenticationFailed",
        OperatorAccountError::AuthenticationThrottled { .. } => "AuthenticationThrottled",
        OperatorAccountError::CredentialDisabled => "CredentialDisabled",
        OperatorAccountError::RemotePasswordNotAllowed => "RemotePasswordNotAllowed",
        OperatorAccountError::NotAuthenticated => "NotAuthenticated",
        OperatorAccountError::InvalidSessionTimeout => "InvalidSessionTimeout",
        OperatorAccountError::CredentialProcessing => "CredentialProcessing",
        OperatorAccountError::InvalidStore => "InvalidStore",
        OperatorAccountError::ClockBeforeUnixEpoch => "ClockBeforeUnixEpoch",
        OperatorAccountError::Store(_) => "Store",
        OperatorAccountError::StateUnavailable => "StateUnavailable",
    }
}

impl fmt::Display for ClassificationRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "account: {}", account_tag(error)),
            Self::Malformed(reason) => write!(formatter, "malformed: {reason}"),
            Self::NotCanonical => formatter.write_str("not_canonical"),
            Self::CrossWorkspace => formatter.write_str("cross_workspace"),
            Self::IdentityTaken => formatter.write_str("identity_taken"),
            Self::PolicyRevisionNotFound => formatter.write_str("policy_revision_not_found"),
            Self::PolicyProjectIsNotTheMissions => {
                formatter.write_str("policy_project_is_not_the_missions")
            }
            Self::PolicyRevisionHasAnotherCharter => {
                formatter.write_str("policy_revision_has_another_charter")
            }
            Self::Predecessor { digest, fault } => {
                write!(formatter, "predecessor {digest}: {fault}")
            }
            Self::Store(_) => formatter.write_str("store"),
        }
    }
}

impl std::error::Error for ClassificationRecordError {}

/// The `mission.impact_classified` Event payload.
///
/// It carries the impact level as the record's own literal, and the Policy revision the
/// classification was made under -- so a reader of the ledger can tell *which* Policy said this
/// without resolving "the current one", which is a different question with a different answer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MissionImpactClassifiedPayload {
    pub workspace_id: String,
    pub classification_id: String,
    pub mission_id: String,
    pub project_id: String,
    pub version: u32,
    pub impact: String,
    pub basis: String,
    pub policy_revision: u32,
    pub charter_revision: u32,
    pub digest: ClassificationDigestPayload,
    pub canonical_bytes_base64: String,
    pub classified_at_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClassificationDigestPayload {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

/// Records Mission impact classifications, and reads them back verified.
///
/// There is deliberately **no** `is_high_impact(mission_id) -> bool` here. A caller that wants the
/// level reads a record and gets its identity, its version, its Policy revision and its digest with
/// it; a bare boolean is exactly the shortcut that lets a renderer print "high impact" without
/// being able to say which classification said so (ADR 0044).
pub struct MissionImpactService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl MissionImpactService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Records one immutable classification from the exact bytes supplied.
    pub fn classify(
        &self,
        canonical_bytes: &[u8],
    ) -> Result<MissionImpactClassification, ClassificationRecordError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let payload = decode(canonical_bytes).map_err(ClassificationRecordError::Malformed)?;
            if encode(&payload).map_err(ClassificationRecordError::Malformed)? != canonical_bytes {
                return Err(ClassificationRecordError::NotCanonical);
            }
            if payload.workspace_id != self.workspace_id {
                return Err(ClassificationRecordError::CrossWorkspace);
            }
            let digest_value = HEXLOWER.encode(&sha256(canonical_bytes));
            // The Command says it first, so a caller gets the reason rather than a read-back
            // refusal after a row that the reader would never return.
            let owner = mission_project_of(connection, self.workspace_id, payload.mission_id)?;
            if owner != Some(payload.policy.project_id) {
                return Err(ClassificationRecordError::PolicyProjectIsNotTheMissions);
            }

            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            insert_classification(&transaction, &digest_value, canonical_bytes, &payload)?;
            let recorded =
                verified_classification_within(&transaction, self.workspace_id, &digest_value)?
                    .ok_or_else(|| {
                        ClassificationRecordError::Store("record vanished".to_owned())
                    })?;
            transaction.commit()?;
            Ok(recorded)
        })
    }

    /// One classification by its content identity, verified.
    pub fn get(
        &self,
        digest_value: &str,
    ) -> Result<Option<MissionImpactClassification>, ClassificationRecordError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            Ok(verified_classification_within(
                connection,
                self.workspace_id,
                digest_value,
            )?)
        })
    }

    /// Every classification of one Mission, each verified, in version order.
    ///
    /// The caller is handed the ladder rather than a "current" answer, because *which* record is
    /// current depends on the event cursor the caller is resolving at -- a question this store
    /// cannot answer and must not appear to.
    pub fn history(
        &self,
        mission_id: Uuid,
    ) -> Result<Vec<MissionImpactClassification>, ClassificationRecordError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let mut statement = connection.prepare(
                "SELECT digest_value FROM mission_impact_classifications
                 WHERE workspace_id = ?1 AND mission_id = ?2
                 ORDER BY version",
            )?;
            let digests = statement
                .query_map(
                    rusqlite::params![self.workspace_id.to_string(), mission_id.to_string()],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let mut records = Vec::with_capacity(digests.len());
            for digest_value in digests {
                let record =
                    verified_classification_within(connection, self.workspace_id, &digest_value)?
                        .ok_or_else(|| {
                        ClassificationRecordError::Store("record vanished".to_owned())
                    })?;
                records.push(record);
            }
            Ok(records)
        })
    }
}

/// The row and its Event, written together in one transaction, every projection column derived
/// from `payload` in this one place.
fn insert_classification(
    connection: &Connection,
    digest_value: &str,
    canonical_bytes: &[u8],
    payload: &MissionImpactClassificationV1,
) -> Result<(), ClassificationRecordError> {
    connection.execute(
        "INSERT INTO mission_impact_classifications (
             workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
             classification_id, mission_id, project_id, version, impact, basis,
             policy_revision, charter_revision, classified_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        rusqlite::params![
            payload.workspace_id.to_string(),
            DIGEST_ALGORITHM,
            CANONICALIZER,
            digest_value,
            canonical_bytes,
            payload.classification_id.to_string(),
            payload.mission_id.to_string(),
            payload.policy.project_id.to_string(),
            i64::from(payload.version),
            payload.impact.as_str(),
            payload.basis.as_str(),
            i64::from(payload.policy.policy_revision),
            i64::from(payload.policy.charter_revision),
            i64::try_from(payload.classified_at_ms).map_err(|_| {
                ClassificationRecordError::Store("timestamp out of range".to_owned())
            })?,
        ],
    )?;
    let event = serde_json::to_string(&MissionImpactClassifiedPayload {
        workspace_id: payload.workspace_id.to_string(),
        classification_id: payload.classification_id.to_string(),
        mission_id: payload.mission_id.to_string(),
        project_id: payload.policy.project_id.to_string(),
        version: payload.version,
        impact: payload.impact.as_str().to_owned(),
        basis: payload.basis.as_str().to_owned(),
        policy_revision: payload.policy.policy_revision,
        charter_revision: payload.policy.charter_revision,
        digest: ClassificationDigestPayload {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: CANONICALIZER.to_owned(),
            value: digest_value.to_owned(),
        },
        canonical_bytes_base64: BASE64.encode(canonical_bytes),
        classified_at_ms: payload.classified_at_ms,
    })
    .map_err(|error| ClassificationRecordError::Store(error.to_string()))?;
    crate::event_chain::append_event(
        connection,
        &payload.workspace_id.to_string(),
        "mission.impact_classified",
        &event,
        i64::try_from(payload.classified_at_ms)
            .map_err(|_| ClassificationRecordError::Store("timestamp out of range".to_owned()))?,
    )?;
    Ok(())
}

/// Why the classification startup pass refused, with the row that caused it (R27.1).
///
/// The pass used to return `ClassificationStoreError` and startup flattened it with
/// `.map_err(|_| ..)` onto a unit variant, so a refused reopen said "a classification does not
/// re-prove" and nothing else -- not which row, and not whether the cause was malformed bytes or a
/// predecessor of another Mission. An operator could not tell a corrupt store from a forged link
/// from a bug in the reader (sol1's P1).
pub(crate) enum ClassificationStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One row did not re-prove. `cause` is the owning reader's OWN error, which already carries
    /// `PredecessorFault` -- carried whole rather than re-described, so startup and a direct read
    /// give the same answer to the same question.
    Row {
        workspace_id: Uuid,
        digest: String,
        cause: ClassificationStoreError,
    },
}

impl fmt::Display for ClassificationStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // No row to name -- the enumeration itself failed, so none was reached -- and no store
            // text (R28.4). `source()` carries the `rusqlite::Error` for anyone who needs it.
            Self::Store(_) => {
                formatter.write_str("the classifications could not be read: store (see source)")
            }
            Self::Row {
                workspace_id,
                digest,
                cause,
            } => write!(
                formatter,
                "classification {digest} in Workspace {workspace_id} does not re-prove: {cause}"
            ),
        }
    }
}

/// `Debug` is the channel R28.4 leaves open: it carries the store's own text, the cause's payload
/// and everything else the value holds. Deliberately NOT `Display` -- that was the previous shape,
/// and it left nowhere to look once the one line stopped saying enough.
impl fmt::Debug for ClassificationStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Row {
                workspace_id,
                digest,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("digest", digest)
                .field("cause", cause)
                .finish(),
        }
    }
}

/// The other channel R28.4 leaves open. `Store` carries a real `rusqlite::Error`, so a caller that
/// wants the driver's own words asks for them here rather than reading them off a message.
impl std::error::Error for ClassificationStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
        }
    }
}

/// Re-prove every stored classification at startup (R26.1), the shape `verify_manifest_state` uses.
///
/// **It walks every ROW, in every Workspace**, not each chain's newest version and not one
/// Workspace. The Outcome path reads the newest classification at or before a completion's
/// boundary, and `history()` is called from tests only, so a superseded version is a row nothing in
/// the product reads again -- which is exactly where a forged supersession link sits undisturbed. A
/// pass that visited only the newest version of each Mission, or only one Workspace, would skip the
/// only rows it exists for. Both shapes are pinned by tests, because both were live mutants that
/// the suite could not see until someone wrote one.
///
/// It re-proves by CALLING `verified_classification_within`, never by restating its rules: the
/// digest over the stored bytes, the codec round trip, every projection column against the bytes,
/// the Policy revision's Charter, the Project that owns the Mission, and the predecessor judged by
/// its own bytes (`PredecessorFault`: absent, unverifiable, another Workspace, another Mission --
/// sol1's #9). A second copy of those rules here would be a second thing to keep true.
///
/// It CARRIES the reader's fault rather than flattening it (R27.1) -- see
/// [`ClassificationStateFault`].
///
/// Where it runs: `inspect_store`, with its siblings, which is AFTER the startup transaction has
/// committed. It holds no write lock (PR #77 / FF-49 moved the expensive arithmetic out of that
/// lock; this pass was never in it).
/// The rows `verify_classification_state` enumerates, in the order it enumerates them.
///
/// EXTRACTED SO A BAR CAN CALL IT (R44.6): a placement assertion that COPIES this query agrees
/// with itself, so flipping the pass's own `ORDER BY` leaves it decorative. One function, called
/// by both, moves the pass and the bar's view of it together.
pub fn enumerate_classifications(
    connection: &Connection,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "SELECT workspace_id, digest_value FROM mission_impact_classifications
         ORDER BY workspace_id, digest_value",
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn verify_classification_state(
    connection: &Connection,
) -> Result<(), ClassificationStateFault> {
    let rows: Vec<(String, String)> =
        enumerate_classifications(connection).map_err(ClassificationStateFault::Store)?;

    for (workspace_id, digest_value) in &rows {
        // Not `Row { workspace_id: Uuid::nil(), .. }` (R39.2). A row whose Workspace id will not
        // parse has no Workspace to name, and nil is a well-formed UUID that a reader could carry
        // forward as one. `Store` is the arm for "the pass could not proceed", and both raw values
        // -- the Workspace text and the digest -- travel in the error rather than being invented
        // into typed fields.
        let parsed = Uuid::parse_str(workspace_id).map_err(|_| {
            ClassificationStateFault::Store(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(format!(
                    "the workspace_id column is not a UUID: {workspace_id:?} \
                     (classification {digest_value})"
                ))),
            ))
        })?;
        // `Ok(None)` cannot happen for a digest this query just read, and it is not treated as
        // success: a row that vanished between the two statements is a store changing under a pass
        // that is supposed to be reading a fixed set.
        match verified_classification_within(connection, parsed, digest_value) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(ClassificationStateFault::Row {
                    workspace_id: parsed,
                    digest: digest_value.clone(),
                    cause: ClassificationStoreError::Store(
                        "the row was enumerated and then could not be read".to_owned(),
                    ),
                });
            }
            Err(cause) => {
                return Err(ClassificationStateFault::Row {
                    workspace_id: parsed,
                    digest: digest_value.clone(),
                    cause,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod display_tests {
    use super::*;

    /// Text only a store error carries: a constraint name and the column it names. R28.4 is about
    /// exactly this reaching a one-line refusal an operator sees.
    const STORE_TEXT: &str =
        "UNIQUE constraint failed: mission_impact_classifications.digest_value";

    /// R28.4: the one line names the row and the fault, and nothing the store said.
    ///
    /// The needle is text only a store error carries. It must not be in the message an operator
    /// sees, and it must still be reachable -- through `Debug`, and for the `Store` arm through
    /// `source()`, which carries the `rusqlite::Error` itself.
    #[test]
    fn a_classification_refusal_names_the_row_and_the_fault_and_not_the_store_text() {
        let workspace_id = Uuid::new_v4();
        let digest = "a".repeat(64);
        let fault = ClassificationStateFault::Row {
            workspace_id,
            digest: digest.clone(),
            cause: ClassificationStoreError::Store(STORE_TEXT.to_owned()),
        };

        let line = fault.to_string();
        assert!(
            line.contains(&workspace_id.to_string()),
            "no Workspace: {line}"
        );
        assert!(line.contains(&digest), "no row key: {line}");
        assert!(line.contains("store"), "no fault name: {line}");
        assert!(
            !line.contains(STORE_TEXT),
            "the store's own text is in the one-line message: {line}"
        );
        assert!(
            !line.contains("constraint"),
            "part of the store's text survived: {line}"
        );

        // And it is still reachable, which is the other half of the rule.
        assert!(
            format!("{fault:?}").contains(STORE_TEXT),
            "Debug lost the store text, so there is nowhere to look: {fault:?}"
        );
        assert_ne!(
            format!("{fault:?}"),
            line,
            "Debug is Display again, so the two channels are one"
        );
    }

    /// The name each arm must put in the message.
    ///
    /// Exhaustive and wildcard-free ON PURPOSE: a new arm on `ClassificationStoreError` is a
    /// compile error here, so "every named fault" stays a claim this test can keep. kimi's P2 was
    /// that the first version named five of ten -- true of the five, and silent about the rest.
    fn expected_name(error: &ClassificationStoreError) -> &'static str {
        match error {
            ClassificationStoreError::DigestMismatch => "digest_mismatch",
            ClassificationStoreError::Malformed(_) => "malformed",
            ClassificationStoreError::ColumnsDisagree => "columns_disagree",
            ClassificationStoreError::CitedIdentityMismatch => "cited_identity_mismatch",
            ClassificationStoreError::CrossWorkspace => "cross_workspace",
            ClassificationStoreError::CopiedBitDisagrees => "copied_bit_disagrees",
            ClassificationStoreError::PolicyProjectIsNotTheMissions => {
                "policy_project_is_not_the_missions"
            }
            ClassificationStoreError::PolicyRevisionHasAnotherCharter => {
                "policy_revision_has_another_charter"
            }
            ClassificationStoreError::Predecessor { .. } => "predecessor",
            ClassificationStoreError::Store(_) => "store",
        }
    }

    /// Every named fault reaches the message -- all ten arms, not the five that had a fixture.
    ///
    /// Closing the store's text did not close the vocabulary, and this is where that is checked.
    /// Both directions are covered: `expected_name` fails to COMPILE if an arm is added, and the
    /// distinct-name count below fails if an arm is added there without a fixture here.
    ///
    /// `PredecessorFault::NotStored` is the deliberate choice of payload: `Unverifiable(reason)`
    /// interpolates a reason of its own (spend.rs), which is R37's question and tier 1's to answer.
    /// Using it here would quietly make this bar assert something it was not given.
    #[test]
    fn a_classification_refusal_still_names_which_fault_it_was() {
        let every_arm = [
            ClassificationStoreError::DigestMismatch,
            ClassificationStoreError::Malformed(MalformedClassification::Magic),
            ClassificationStoreError::ColumnsDisagree,
            ClassificationStoreError::CitedIdentityMismatch,
            ClassificationStoreError::CrossWorkspace,
            ClassificationStoreError::CopiedBitDisagrees,
            ClassificationStoreError::PolicyProjectIsNotTheMissions,
            ClassificationStoreError::PolicyRevisionHasAnotherCharter,
            ClassificationStoreError::Predecessor {
                digest: "c".repeat(64),
                fault: PredecessorFault::NotStored,
            },
            ClassificationStoreError::Store("anything at all".to_owned()),
        ];

        let names: std::collections::BTreeSet<&str> = every_arm.iter().map(expected_name).collect();
        assert_eq!(
            names.len(),
            10,
            "an arm was added to `ClassificationStoreError` without a fixture here, so this bar \
             would have gone on claiming to cover the whole vocabulary: {names:?}"
        );

        for cause in every_arm {
            let name = expected_name(&cause);
            let line = ClassificationStateFault::Row {
                workspace_id: Uuid::new_v4(),
                digest: "b".repeat(64),
                cause,
            }
            .to_string();
            assert!(line.contains(name), "the fault is not named: {line}");
        }
    }

    /// The enumeration itself failing has no row to name, and still must not print the driver's
    /// words. `source()` is where they live.
    #[test]
    fn an_enumeration_failure_names_no_row_and_carries_its_error_in_source() {
        use std::error::Error;

        let fault = ClassificationStateFault::Store(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(19),
            Some(STORE_TEXT.to_owned()),
        ));
        let line = fault.to_string();
        assert!(line.contains("store"), "no fault name: {line}");
        assert!(
            !line.contains(STORE_TEXT),
            "the driver's words are in the one-line message: {line}"
        );
        assert!(
            format!("{fault:?}").contains(STORE_TEXT),
            "Debug lost the driver's words: {fault:?}"
        );
        assert!(
            fault
                .source()
                .expect("the enumeration failure carries its error")
                .to_string()
                .contains(STORE_TEXT),
            "source() did not carry the driver's words"
        );
    }

    // ---------------------------------------------------------------------------------------
    // R42 slice 1, commit 1: the recorder's outward projection is closed.
    // ---------------------------------------------------------------------------------------

    /// Two fixed synthetic markers. Every diagnostic payload a case carries holds one, so "did
    /// this line disclose the payload" is a substring search rather than an eyeball.
    const NEEDLE_A: &str = "R42S1_PRIVATE_A";
    const NEEDLE_B: &str = "R42S1_PRIVATE_B";

    fn payload_a() -> String {
        format!("{NEEDLE_A}|column=private_column|path=/home/example-user/private.txt\nsecond-line")
    }

    fn payload_b() -> String {
        format!(
            "{NEEDLE_B}|column=other_column|path=/home/example-user/other-note.txt\nsecond-line"
        )
    }

    /// A real driver error carrying a marker. `InvalidParameterName` is a rusqlite constructor,
    /// so this is the driver's own `Display`/`Debug`, not a string dressed up as one.
    fn driver_error(needle: &str) -> rusqlite::Error {
        rusqlite::Error::InvalidParameterName(format!("{needle}|:private_binding"))
    }

    /// One declarative row table, expanded into BOTH the exhaustive expected match and the
    /// constructed fixtures.
    ///
    /// The match is wildcard-free, so a new production variant fails to compile here until its row
    /// exists -- and a row cannot be added without supplying its constructor, its independently
    /// written expected literal, and the markers its payload carries. Nothing in a table is read
    /// from production: `account_tag`, `MalformedClassification::as_str` and the child `Display`s
    /// are never called to build an expectation.
    macro_rules! closed_table {
        (
            $expected_fn:ident / $rows_fn:ident : $ty:ty,
            $( $case:literal | $pattern:pat => $line:expr , $build:expr , $markers:expr ; )+
        ) => {
            fn $expected_fn(value: &$ty) -> (&'static str, String) {
                match value {
                    $( $pattern => ($case, String::from($line)), )+
                }
            }

            fn $rows_fn() -> Vec<(&'static str, $ty, &'static [&'static str])> {
                vec![$(($case, $build, $markers)),+]
            }
        };
    }

    closed_table! {
        expected_account / account_rows: OperatorAccountError,
        "PASSWORD_TOO_SHORT" | OperatorAccountError::PasswordTooShort
            => "PasswordTooShort", OperatorAccountError::PasswordTooShort, &[];
        "PASSWORD_TOO_LONG" | OperatorAccountError::PasswordTooLong
            => "PasswordTooLong", OperatorAccountError::PasswordTooLong, &[];
        "PASSWORD_CONFIRMATION_MISMATCH" | OperatorAccountError::PasswordConfirmationMismatch
            => "PasswordConfirmationMismatch",
            OperatorAccountError::PasswordConfirmationMismatch, &[];
        "RECOVERY_NOT_CONFIRMED" | OperatorAccountError::RecoveryNotConfirmed
            => "RecoveryNotConfirmed", OperatorAccountError::RecoveryNotConfirmed, &[];
        "SETUP_NOT_STARTED" | OperatorAccountError::SetupNotStarted
            => "SetupNotStarted", OperatorAccountError::SetupNotStarted, &[];
        "ACCOUNT_ALREADY_EXISTS" | OperatorAccountError::AccountAlreadyExists
            => "AccountAlreadyExists", OperatorAccountError::AccountAlreadyExists, &[];
        "ACCOUNT_MISSING" | OperatorAccountError::AccountMissing
            => "AccountMissing", OperatorAccountError::AccountMissing, &[];
        "AUTHENTICATION_FAILED" | OperatorAccountError::AuthenticationFailed
            => "AuthenticationFailed", OperatorAccountError::AuthenticationFailed, &[];
        "AUTHENTICATION_THROTTLED" | OperatorAccountError::AuthenticationThrottled { .. }
            => "AuthenticationThrottled",
            OperatorAccountError::AuthenticationThrottled { retry_after_ms: 1001 }, &[];
        "CREDENTIAL_DISABLED" | OperatorAccountError::CredentialDisabled
            => "CredentialDisabled", OperatorAccountError::CredentialDisabled, &[];
        "REMOTE_PASSWORD_NOT_ALLOWED" | OperatorAccountError::RemotePasswordNotAllowed
            => "RemotePasswordNotAllowed", OperatorAccountError::RemotePasswordNotAllowed, &[];
        "NOT_AUTHENTICATED" | OperatorAccountError::NotAuthenticated
            => "NotAuthenticated", OperatorAccountError::NotAuthenticated, &[];
        "INVALID_SESSION_TIMEOUT" | OperatorAccountError::InvalidSessionTimeout
            => "InvalidSessionTimeout", OperatorAccountError::InvalidSessionTimeout, &[];
        "CREDENTIAL_PROCESSING" | OperatorAccountError::CredentialProcessing
            => "CredentialProcessing", OperatorAccountError::CredentialProcessing, &[];
        "INVALID_STORE" | OperatorAccountError::InvalidStore
            => "InvalidStore", OperatorAccountError::InvalidStore, &[];
        "CLOCK_BEFORE_UNIX_EPOCH" | OperatorAccountError::ClockBeforeUnixEpoch
            => "ClockBeforeUnixEpoch", OperatorAccountError::ClockBeforeUnixEpoch, &[];
        "STORE" | OperatorAccountError::Store(_)
            => "Store", OperatorAccountError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "STATE_UNAVAILABLE" | OperatorAccountError::StateUnavailable
            => "StateUnavailable", OperatorAccountError::StateUnavailable, &[];
    }

    closed_table! {
        expected_malformed / malformed_rows: MalformedClassification,
        "MAGIC" | MalformedClassification::Magic => "magic", MalformedClassification::Magic, &[];
        "TRUNCATED" | MalformedClassification::Truncated
            => "truncated", MalformedClassification::Truncated, &[];
        "TRAILING_BYTES" | MalformedClassification::TrailingBytes
            => "trailing_bytes", MalformedClassification::TrailingBytes, &[];
        "UNKNOWN_TAG" | MalformedClassification::UnknownTag
            => "unknown_tag", MalformedClassification::UnknownTag, &[];
        "INVALID_UTF8" | MalformedClassification::InvalidUtf8
            => "invalid_utf8", MalformedClassification::InvalidUtf8, &[];
        "RECORD_TOO_LARGE" | MalformedClassification::RecordTooLarge
            => "record_too_large", MalformedClassification::RecordTooLarge, &[];
        "OVER_LIMIT_LENGTH" | MalformedClassification::OverLimitLength
            => "over_limit_length", MalformedClassification::OverLimitLength, &[];
        "RULE_ID_BASIS_MISMATCH" | MalformedClassification::RuleIdBasisMismatch
            => "rule_id_basis_mismatch", MalformedClassification::RuleIdBasisMismatch, &[];
        "RULE_ID" | MalformedClassification::RuleId
            => "rule_id", MalformedClassification::RuleId, &[];
        "EMPTY_TEXT" | MalformedClassification::EmptyText
            => "empty_text", MalformedClassification::EmptyText, &[];
        "UNTRIMMED_TEXT" | MalformedClassification::UntrimmedText
            => "untrimmed_text", MalformedClassification::UntrimmedText, &[];
        "TEXT_TOO_LONG" | MalformedClassification::TextTooLong
            => "text_too_long", MalformedClassification::TextTooLong, &[];
        "CONTROL_CHARACTER" | MalformedClassification::ControlCharacter
            => "control_character", MalformedClassification::ControlCharacter, &[];
        "REVISION" | MalformedClassification::Revision
            => "revision", MalformedClassification::Revision, &[];
        "SUPERSEDES_DIGEST" | MalformedClassification::SupersedesDigest
            => "supersedes_digest", MalformedClassification::SupersedesDigest, &[];
    }

    closed_table! {
        expected_predecessor / predecessor_rows: PredecessorFault,
        "NOT_STORED" | PredecessorFault::NotStored
            => "not_stored", PredecessorFault::NotStored, &[];
        "UNVERIFIABLE" | PredecessorFault::Unverifiable(_)
            => "unverifiable", PredecessorFault::Unverifiable(payload_a()), &[NEEDLE_A];
        "IN_ANOTHER_WORKSPACE" | PredecessorFault::InAnotherWorkspace
            => "in_another_workspace", PredecessorFault::InAnotherWorkspace, &[];
        "FOR_ANOTHER_MISSION" | PredecessorFault::ForAnotherMission
            => "for_another_mission", PredecessorFault::ForAnotherMission, &[];
    }

    /// The two predecessor digests, chosen in fixture metadata before any error is built. The
    /// digest is approved outward context (R42's allowed-context list), so it is asserted
    /// PRESENT -- an approved field is not a needle.
    fn digest_a() -> String {
        "a".repeat(64)
    }

    fn digest_b() -> String {
        "b".repeat(64)
    }

    closed_table! {
        expected_recorder / recorder_rows: ClassificationRecordError,
        "R42S1.RECORD.ACCOUNT" | ClassificationRecordError::Account(error)
            => format!("account: {}", expected_account(error).1),
            ClassificationRecordError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))),
            &[NEEDLE_A];
        "R42S1.RECORD.MALFORMED" | ClassificationRecordError::Malformed(reason)
            => format!("malformed: {}", expected_malformed(reason).1),
            ClassificationRecordError::Malformed(MalformedClassification::Magic), &[];
        "R42S1.RECORD.NOT_CANONICAL" | ClassificationRecordError::NotCanonical
            => "not_canonical", ClassificationRecordError::NotCanonical, &[];
        "R42S1.RECORD.CROSS_WORKSPACE" | ClassificationRecordError::CrossWorkspace
            => "cross_workspace", ClassificationRecordError::CrossWorkspace, &[];
        "R42S1.RECORD.IDENTITY_TAKEN" | ClassificationRecordError::IdentityTaken
            => "identity_taken", ClassificationRecordError::IdentityTaken, &[];
        "R42S1.RECORD.POLICY_REVISION_NOT_FOUND" | ClassificationRecordError::PolicyRevisionNotFound
            => "policy_revision_not_found", ClassificationRecordError::PolicyRevisionNotFound, &[];
        "R42S1.RECORD.POLICY_PROJECT_IS_NOT_THE_MISSIONS"
            | ClassificationRecordError::PolicyProjectIsNotTheMissions
            => "policy_project_is_not_the_missions",
            ClassificationRecordError::PolicyProjectIsNotTheMissions, &[];
        "R42S1.RECORD.POLICY_REVISION_HAS_ANOTHER_CHARTER"
            | ClassificationRecordError::PolicyRevisionHasAnotherCharter
            => "policy_revision_has_another_charter",
            ClassificationRecordError::PolicyRevisionHasAnotherCharter, &[];
        "R42S1.RECORD.PREDECESSOR" | ClassificationRecordError::Predecessor { digest, fault }
            => format!("predecessor {digest}: {}", expected_predecessor(fault).1),
            ClassificationRecordError::Predecessor {
                digest: digest_a(),
                fault: PredecessorFault::Unverifiable(payload_a()),
            },
            &[NEEDLE_A];
        "R42S1.RECORD.STORE" | ClassificationRecordError::Store(_)
            => "store", ClassificationRecordError::Store(payload_a()), &[NEEDLE_A];
    }

    /// One semantic case: an id nothing else shares, the value, and the markers its payload was
    /// DECLARED to carry -- never the markers its Debug happens to contain.
    struct Case {
        id: String,
        value: ClassificationRecordError,
        markers: &'static [&'static str],
    }

    /// The three needle-bearing cases lead, so a mutant's FIRST failing label is predictable.
    const LEAD_CASES: [&str; 3] = [
        "R42S1.RECORD.STORE",
        "R42S1.RECORD.ACCOUNT.STORE",
        "R42S1.RECORD.PREDECESSOR.UNVERIFIABLE",
    ];

    /// The 44 cases: the seven leaf roots, plus every child variant of the three nested roots.
    ///
    /// The root rows drive which stems exist (and are themselves checked for pattern alignment);
    /// the nested stems expand over their own tables, so a new child variant is a new case without
    /// anyone remembering to add one.
    fn recorder_cases() -> Vec<Case> {
        let mut cases = Vec::new();
        for (stem, value, markers) in recorder_rows() {
            match stem {
                "R42S1.RECORD.ACCOUNT" => {
                    for (child, error, child_markers) in account_rows() {
                        cases.push(Case {
                            id: format!("{stem}.{child}"),
                            value: ClassificationRecordError::Account(error),
                            markers: child_markers,
                        });
                    }
                }
                "R42S1.RECORD.MALFORMED" => {
                    for (child, reason, child_markers) in malformed_rows() {
                        cases.push(Case {
                            id: format!("{stem}.{child}"),
                            value: ClassificationRecordError::Malformed(reason),
                            markers: child_markers,
                        });
                    }
                }
                "R42S1.RECORD.PREDECESSOR" => {
                    for (child, fault, child_markers) in predecessor_rows() {
                        let carries_marker = !child_markers.is_empty();
                        cases.push(Case {
                            id: format!("{stem}.{child}"),
                            value: ClassificationRecordError::Predecessor {
                                digest: if carries_marker {
                                    digest_a()
                                } else {
                                    digest_b()
                                },
                                fault,
                            },
                            markers: child_markers,
                        });
                    }
                }
                leaf => cases.push(Case {
                    id: leaf.to_owned(),
                    value,
                    markers,
                }),
            }
        }
        cases.sort_by_key(|case| {
            LEAD_CASES
                .iter()
                .position(|lead| *lead == case.id)
                .unwrap_or(usize::MAX)
        });
        cases
    }

    /// R42 slice 1: every `ClassificationRecordError` line is the closed projection, and every
    /// diagnostic payload it was built from is still in `Debug`.
    ///
    /// The two census sites are `Account` (which printed the account error's whole `Debug`) and
    /// `Store` (which printed the store's text). The other eight arms are pinned here as
    /// regressions: closing two lines must not quietly reword the eight that were already safe.
    #[test]
    fn r42_recorder_projection_is_closed() {
        let cases = recorder_cases();

        // Cardinality is reviewed, not read from production: 18 account + 15 malformed
        // + 4 predecessor + 6 unit roots + Store.
        assert_eq!(account_rows().len(), 18, "R42S1.RECORD.CARDINALITY.ACCOUNT");
        assert_eq!(
            malformed_rows().len(),
            15,
            "R42S1.RECORD.CARDINALITY.MALFORMED"
        );
        assert_eq!(
            predecessor_rows().len(),
            4,
            "R42S1.RECORD.CARDINALITY.PREDECESSOR"
        );
        assert_eq!(recorder_rows().len(), 10, "R42S1.RECORD.CARDINALITY.ROOT");
        assert_eq!(cases.len(), 44, "R42S1.RECORD.CARDINALITY.CASES");
        let ids: std::collections::BTreeSet<&str> =
            cases.iter().map(|case| case.id.as_str()).collect();
        assert_eq!(ids.len(), cases.len(), "R42S1.RECORD.CASE_IDS_UNIQUE");
        for lead in LEAD_CASES {
            assert!(ids.contains(lead), "R42S1.RECORD.LEAD_CASE_PRESENT: {lead}");
        }

        // The driver fixture must really carry its marker, or every NO_NEEDLE below is vacuous.
        assert!(
            format!("{:?}", driver_error(NEEDLE_A)).contains(NEEDLE_A),
            "R42S1.FIXTURE.DRIVER_NEEDLE"
        );

        for case in &cases {
            let Case { id, value, markers } = case;
            let (stem, expected) = expected_recorder(value);
            assert!(
                id == stem || id.starts_with(&format!("{stem}.")),
                "R42S1.RECORD.PATTERN_ALIGNS: {id} was built as {stem}"
            );

            let line = value.to_string();
            for marker in *markers {
                assert!(!line.contains(marker), "{id}.NO_NEEDLE: {line}");
            }
            assert!(
                !line.contains('\n') && !line.contains('\r'),
                "{id}.ONE_LINE: {line:?}"
            );

            let debug = format!("{value:?}");
            for marker in *markers {
                assert!(debug.contains(marker), "{id}.DEBUG_RETAINS: {debug}");
            }

            assert_eq!(line, expected, "{id}.PROJECTION");
        }

        // Approved context: the predecessor digest is the submitted supersedes reference, and it
        // is deliberately IN the line. A permitted field is asserted present, never absent.
        let predecessor = ClassificationRecordError::Predecessor {
            digest: digest_b(),
            fault: PredecessorFault::Unverifiable(payload_b()),
        };
        let line = predecessor.to_string();
        assert_eq!(
            line,
            format!("predecessor {}: unverifiable", digest_b()),
            "R42S1.RECORD.PREDECESSOR.CONTEXT"
        );
        assert!(
            !line.contains(NEEDLE_B),
            "R42S1.RECORD.PREDECESSOR.NO_NEEDLE"
        );
        assert!(
            format!("{predecessor:?}").contains(NEEDLE_B),
            "R42S1.RECORD.PREDECESSOR.DEBUG_RETAINS"
        );

        // A second payload proves the projection is independent of the text it was handed.
        let second = ClassificationRecordError::Store(payload_b());
        assert_eq!(
            second.to_string(),
            "store",
            "R42S1.RECORD.STORE.SECOND_SEED"
        );
        assert!(
            format!("{second:?}").contains(NEEDLE_B),
            "R42S1.RECORD.STORE.SECOND_SEED_DEBUG_RETAINS"
        );
    }
}
