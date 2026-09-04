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

use std::fmt;

use data_encoding::HEXLOWER;
use uuid::Uuid;

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
