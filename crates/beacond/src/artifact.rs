//! Artifact v1, slice 1: the canonical grammar and the identity, nothing else (ADR 0043).
//!
//! An Artifact is a versioned work product with content identity and provenance (`CONTEXT.md`) — a
//! plan, a document, a dataset, the Outcome Document's candidate Markdown. This slice is
//! deliberately pure: it owns the immutable [`ArtifactV1`] payload, its canonical bytes, the closed
//! kind and media vocabularies, and the bytes-proved [`Artifact`] wrapper. It reads no database,
//! stores no content, and records no Event; the store, the `artifact.recorded` Event, the derived
//! creation event cursor, the supersession relation, and the verified reader are slices 2-4.
//!
//! The identity is SHA-256 over the record's canonical bytes, and those bytes cover authorship,
//! scope, kind, and the content digest envelope — never the bare content digest. Two Residents
//! recording byte-identical content produce two Artifacts with two identities, because the
//! non-author Evaluation rule (`docs/BLUEPRINT.md:898`) compares the accountable Resident, and the
//! accountable Resident lives in the record.
//!
//! **Recording an Artifact creates no authority.** It opens no Gate, admits no dispatch, completes
//! no Mission, publishes no Outcome, and grants no capability. Whether an Artifact is Material is a
//! fact about its consumers, never a flag on the record, and no accessor answers a boolean.
//!
//! The canonical grammar reuses `beacon.proposal.v1`'s primitives and carries its own copy of them,
//! for the reason `ruling.rs` states: an identity grammar's whole value is that it cannot move, and
//! one shared codec would couple two frozen grammars' futures. The one function shared rather than
//! copied is `sha256`, the crate's single hashing primitive.
//!
//! Slice 2 adds the store: the `artifacts` table and its immutability triggers, one
//! `artifact.recorded` Event per record, the derived creation event cursor, the supersession
//! relation, startup reconciliation, and the verified reader. v1 is inline-only (R16/A7): content
//! lives in `content_inline` under the module's 1 MiB ceiling, and no blob directory exists.

use std::fmt;
use std::sync::Arc;

use data_encoding::{BASE64, HEXLOWER};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mission::mission_admission_facts;
use crate::operator_account::owner_account_exists;
use crate::proposal::sha256;
use crate::{OperatorAccountError, OperatorAccountService};

/// The canonicalizer identity. Any future grammar that alters the byte layout of records that
/// already exist is a new canonicalizer; assigning a value in a published tag position is not one.
/// Existing v1 bytes are never reinterpreted under v2 rules (ADR 0034's rule, kept).
pub(crate) const CANONICALIZER: &str = "beacon.artifact.v1";
/// The literal ASCII magic that starts every record, including its terminating NUL: 19 bytes.
const MAGIC: &[u8] = b"beacon.artifact.v1\0";
const DIGEST_ALGORITHM: &str = "sha256";

/// The canonicalizer identity of raw content: SHA-256 over the exact stored bytes, no
/// transformation. Closed at this one value; a canonicalized content form is a new token, never a
/// reinterpretation (ADR 0043).
const RAW_CONTENT_CANONICALIZER: &str = "beacon.raw.v1";

/// The media type `outcome_document` owns. Named in the bytes, never guessed from a filename.
const MEDIA_TYPE_MARKDOWN: &str = "text/markdown; charset=utf-8";

/// The record's own total bound. The field table admits at most ~260 bytes, so this is defensive;
/// it is stated anyway, because a grammar whose per-field limits admit a size the store refuses
/// turns a fact about the bytes into a database error surfacing through a Command (ADR 0038's
/// reason for its own bound).
const MAX_CANONICAL_BYTES: usize = 4 * 1024;
/// The v1 content ceiling: the Outcome's emitted-Markdown bound (ADR 0040), because the outcome
/// document is the only kind v1 assigns. The blob tier and its numbers arrive with the first kind
/// that needs them (R16/A7).
const MAX_CONTENT_BYTES: u64 = 1024 * 1024;
/// `sha256`, `beacon.raw.v1`, `text/markdown; charset=utf-8`: the limit bounds a declared length
/// before any of it is read.
const MAX_TOKEN_BYTES: usize = 64;
/// The most hops a supersession walk follows before refusing by name (sol1's seams read, P1):
/// the walk re-proves and retains one record per hop, so a chain with no bound is a chain-sized
/// read and allocation before any refusal. Checked BEFORE each read; the Evaluation walk owns the
/// same bound, by the same name, for the same reason.
const MAX_SUPERSESSION_HOPS: usize = 256;
const DIGEST_HEX_CHARS: usize = 64;

// One-byte tags. Assigned explicitly and never renumbered. `0x00` is assigned only where absence is
// a value — `context` and `supersedes` — and is refused everywhere else, so a zero-filled buffer
// cannot decode as any valid record.
const TAG_KIND_OUTCOME_DOCUMENT: u8 = 0x01;

const TAG_CONTEXT_NONE: u8 = 0x00;
const TAG_CONTEXT_MISSION: u8 = 0x01;

const TAG_SUPERSEDES_NONE: u8 = 0x00;
const TAG_SUPERSEDES_DIGEST: u8 = 0x01;

// ---------------------------------------------------------------------------------------------
// The immutable v1 payload
// ---------------------------------------------------------------------------------------------

/// The closed set of Artifact kinds a v1 record may carry. v1 assigns exactly one, because exactly
/// one consumer exists (R16/A4); new kinds are assigned by amendment, never renumbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// The Outcome Document's candidate Markdown (ADR 0040).
    OutcomeDocument,
}

impl ArtifactKind {
    fn tag(self) -> u8 {
        match self {
            Self::OutcomeDocument => TAG_KIND_OUTCOME_DOCUMENT,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedArtifact> {
        match tag {
            TAG_KIND_OUTCOME_DOCUMENT => Ok(Self::OutcomeDocument),
            _ => Err(MalformedArtifact::UnknownTag),
        }
    }

    /// The lowercase token this kind is projected as in the store.
    fn as_str(self) -> &'static str {
        match self {
            Self::OutcomeDocument => "outcome_document",
        }
    }

    /// The one media token this kind owns. The guard is keyed on the kind: a caller never supplies
    /// a media type the kind does not own, in either codec direction.
    fn media_type(self) -> &'static str {
        match self {
            Self::OutcomeDocument => MEDIA_TYPE_MARKDOWN,
        }
    }
}

/// What the Artifact is a work product of.
///
/// `None` is a real value: a Workspace-scoped Artifact is representable, and it is not evaluable in
/// v1 — there is no roster to check an evaluator against, which is a consequence, not a policy
/// choice. The `Mission` arm exists because accountability without a Mission has no admission to
/// name: `mission_admission_members` pins the Resident's Configuration at *a Mission's* admission,
/// so field 10 has no parent without one (R16/E1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactContext {
    None,
    Mission { project_id: Uuid, mission_id: Uuid },
}

/// A digest that carries its algorithm and canonicalizer identity, never a bare hex string.
///
/// A recorded value alone would leave a reader to assume which function produced it: the
/// assumption these records exist to remove (ADR 0034/0037).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactDigest {
    pub algorithm: String,
    pub canonicalizer: String,
    /// The lowercase 64-character hexadecimal digest.
    pub value: String,
}

/// The immutable v1 payload, exactly as its canonical bytes decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactV1 {
    /// The Workspace this Artifact belongs to. Every relation is composite over it; a
    /// cross-Workspace reference is a refusal, never a value.
    pub workspace_id: Uuid,
    pub kind: ArtifactKind,
    /// Must equal the kind's one permitted token; the codec enforces the relation in both
    /// directions.
    pub media_type: String,
    /// SHA-256 over the exact stored content bytes, under `beacon.raw.v1`. A field, never the
    /// identity: the identity covers this envelope and everything beside it.
    pub content_digest: ArtifactDigest,
    /// The stored content's byte count, bounded by `MAX_CONTENT_BYTES`.
    pub content_bytes: u64,
    /// The Resident accountable for the content, and its Configuration at the named Mission's
    /// admission. Validated at record time in slice 2; carried in the bytes here, because
    /// authorship outside the digest is provenance attached after the fact.
    pub accountable_resident_id: Uuid,
    pub accountable_configuration_id: Uuid,
    pub context: ArtifactContext,
    /// The predecessor's record digest, when this Artifact corrects one. The chain rules (cite the
    /// current tail, one predecessor each, superseded at most once) are ADR 0037/0038's and arrive
    /// with the store in slice 4.
    pub supersedes: Option<[u8; 32]>,
    /// The service clock at record time, never caller input.
    pub recorded_at_ms: u64,
}

impl ArtifactV1 {
    /// The one canonical byte encoding of this record.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MalformedArtifact> {
        encode(self)
    }

    /// The record's identity: SHA-256 over its canonical bytes, carrying its own envelope.
    pub fn digest(&self) -> Result<ArtifactDigest, MalformedArtifact> {
        Ok(digest_envelope(
            &HEXLOWER.encode(&sha256(&self.canonical_bytes()?)),
        ))
    }

    /// Decode canonical bytes back into a payload. Strict: unknown tags, over-limit or invalid
    /// lengths, trailing bytes, a kind/media pairing the vocabulary does not contain, and
    /// non-canonical forms are refused.
    pub fn from_canonical_bytes(input: &[u8]) -> Result<Self, MalformedArtifact> {
        decode(input)
    }
}

/// One recorded Artifact: its bytes, the identity of those bytes, and what they decode to.
///
/// The fields are private, for the reason ADR 0034 gives: a caller-supplied parsed object plus a
/// digest permits the two to disagree. Bytes and payload here were proved equal by the only
/// constructor that exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    digest: ArtifactDigest,
    canonical_bytes: Vec<u8>,
    payload: ArtifactV1,
}

impl Artifact {
    /// The only constructor: hash the supplied slice, strictly decode that same slice, re-encode
    /// the decoded value, and require byte-for-byte equality. There is no function that accepts
    /// `(ArtifactV1, digest)`, parsed fields plus bytes, or a digest without the bytes it names.
    pub fn from_canonical_bytes(input: &[u8]) -> Result<Self, MalformedArtifact> {
        let digest = digest_envelope(&HEXLOWER.encode(&sha256(input)));
        let payload = decode(input)?;
        let reencoded = encode(&payload)?;
        if reencoded != input {
            return Err(MalformedArtifact::NonCanonicalEncoding);
        }
        Ok(Self {
            digest,
            canonical_bytes: input.to_vec(),
            payload,
        })
    }

    pub fn digest(&self) -> &ArtifactDigest {
        &self.digest
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    pub fn payload(&self) -> &ArtifactV1 {
        &self.payload
    }
}

fn digest_envelope(value: &str) -> ArtifactDigest {
    ArtifactDigest {
        algorithm: DIGEST_ALGORITHM.to_owned(),
        canonicalizer: CANONICALIZER.to_owned(),
        value: value.to_owned(),
    }
}

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

/// Bytes or field values that cannot be a v1 Artifact.
///
/// Decided from the bytes when decoding and from the caller's arguments when encoding. Both
/// directions run the same predicates, so the encoder and the decoder cannot disagree about what is
/// well-formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedArtifact {
    Magic,
    Truncated,
    TrailingBytes,
    InvalidUtf8,
    UnknownTag,
    OverLimitLength,
    /// A digest value that is not lowercase 64-character hexadecimal.
    DigestValue,
    /// A digest envelope this grammar cannot have produced: an algorithm other than `sha256`, or a
    /// canonicalizer other than `beacon.raw.v1` for content or `beacon.artifact.v1` for the record.
    DigestEnvelope,
    /// A media type the kind does not own. The relation is closed per kind, and the codec — not a
    /// caller's care — is what enforces it.
    MediaTypeForKind,
    /// A declared content byte count over the v1 ceiling.
    ContentOverBounds,
    /// The bytes decode, but re-encoding the decoded value produces different bytes.
    NonCanonicalEncoding,
}

impl MalformedArtifact {
    /// A stable lowercase reason token. Refusals are named, never spelled differently by two
    /// callers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Magic => "magic",
            Self::Truncated => "truncated",
            Self::TrailingBytes => "trailing_bytes",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::UnknownTag => "unknown_tag",
            Self::OverLimitLength => "over_limit_length",
            Self::DigestValue => "digest_value",
            Self::DigestEnvelope => "digest_envelope",
            Self::MediaTypeForKind => "media_type_for_kind",
            Self::ContentOverBounds => "content_over_bounds",
            Self::NonCanonicalEncoding => "non_canonical_encoding",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Magic => "the bytes do not start with the beacon.artifact.v1 magic",
            Self::Truncated => "the bytes end inside a field",
            Self::TrailingBytes => "bytes remain after the last field",
            Self::InvalidUtf8 => "a text field is not valid UTF-8",
            Self::UnknownTag => "a closed vocabulary carries an unassigned tag",
            Self::OverLimitLength => "a declared length exceeds its field's limit or the envelope",
            Self::DigestValue => "a digest value is not 64 lowercase hex characters",
            Self::DigestEnvelope => "a digest envelope names another algorithm or canonicalizer",
            Self::MediaTypeForKind => "the media type is not the one its kind owns",
            Self::ContentOverBounds => "the declared content byte count exceeds the v1 ceiling",
            Self::NonCanonicalEncoding => "the bytes are not the canonical encoding of their value",
        }
    }
}

impl fmt::Display for MalformedArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Artifact values are invalid: {}", self.reason())
    }
}

impl std::error::Error for MalformedArtifact {}

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

    fn u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    fn uuid(&mut self, value: &Uuid) {
        self.out.extend_from_slice(value.as_bytes());
    }

    fn sha256(&mut self, value: &[u8; 32]) {
        self.out.extend_from_slice(value);
    }

    fn string(&mut self, value: &str, limit: usize) -> Result<(), MalformedArtifact> {
        if value.len() > limit {
            return Err(MalformedArtifact::OverLimitLength);
        }
        let length = u64::try_from(value.len()).map_err(|_| MalformedArtifact::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value.as_bytes());
        Ok(())
    }
}

/// Reads canonical bytes. Self-delimiting: every field proves it fits within the remaining input
/// and its own limit before any of it is consumed, and no declared length reserves capacity.
struct Decoder<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn new(input: &'a [u8]) -> Result<Self, MalformedArtifact> {
        // The envelope is checked before the magic: an over-long input is refused as over-limit,
        // not as whatever its first bytes happen to fail.
        if input.len() > MAX_CANONICAL_BYTES {
            return Err(MalformedArtifact::OverLimitLength);
        }
        Ok(Self { input, offset: 0 })
    }

    fn remaining(&self) -> usize {
        self.input.len() - self.offset
    }

    /// Takes `count` bytes, having first proved by arithmetic that the input holds them. No
    /// declared length is ever used to reserve capacity; the check runs before any byte is copied.
    fn take(&mut self, count: usize) -> Result<&'a [u8], MalformedArtifact> {
        if count > self.remaining() {
            return Err(MalformedArtifact::Truncated);
        }
        let slice = &self.input[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn magic(&mut self) -> Result<(), MalformedArtifact> {
        if self
            .take(MAGIC.len())
            .map_err(|_| MalformedArtifact::Magic)?
            == MAGIC
        {
            Ok(())
        } else {
            Err(MalformedArtifact::Magic)
        }
    }

    fn tag(&mut self) -> Result<u8, MalformedArtifact> {
        Ok(self.take(1)?[0])
    }

    fn u64(&mut self) -> Result<u64, MalformedArtifact> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }

    fn uuid(&mut self) -> Result<Uuid, MalformedArtifact> {
        let mut value = [0_u8; 16];
        value.copy_from_slice(self.take(16)?);
        Ok(Uuid::from_bytes(value))
    }

    fn sha256(&mut self) -> Result<[u8; 32], MalformedArtifact> {
        let mut value = [0_u8; 32];
        value.copy_from_slice(self.take(32)?);
        Ok(value)
    }

    fn string(&mut self, limit: usize) -> Result<String, MalformedArtifact> {
        let length = self.u64()?;
        let limit = u64::try_from(limit).map_err(|_| MalformedArtifact::OverLimitLength)?;
        if length > limit {
            return Err(MalformedArtifact::OverLimitLength);
        }
        let length = usize::try_from(length).map_err(|_| MalformedArtifact::Truncated)?;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| MalformedArtifact::InvalidUtf8)
    }

    fn finish(self) -> Result<(), MalformedArtifact> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MalformedArtifact::TrailingBytes)
        }
    }
}

/// A lowercase 64-character hexadecimal digest, as its thirty-two raw bytes.
///
/// Uppercase input is refused rather than folded: two spellings of one digest can never both
/// encode.
fn digest_bytes(value: &str) -> Result<[u8; 32], MalformedArtifact> {
    if value.len() != DIGEST_HEX_CHARS {
        return Err(MalformedArtifact::DigestValue);
    }
    let decoded = HEXLOWER
        .decode(value.as_bytes())
        .map_err(|_| MalformedArtifact::DigestValue)?;
    let mut bytes = [0_u8; 32];
    if decoded.len() != bytes.len() {
        return Err(MalformedArtifact::DigestValue);
    }
    bytes.copy_from_slice(&decoded);
    Ok(bytes)
}

/// Every rule the content digest envelope obeys, in one place, consulted by the encoder and the
/// decoder: the algorithm is `sha256`, the canonicalizer is `beacon.raw.v1`, and the value is 64
/// lowercase hex characters.
fn require_content_envelope(digest: &ArtifactDigest) -> Result<(), MalformedArtifact> {
    if digest.algorithm != DIGEST_ALGORITHM || digest.canonicalizer != RAW_CONTENT_CANONICALIZER {
        return Err(MalformedArtifact::DigestEnvelope);
    }
    digest_bytes(&digest.value)?;
    Ok(())
}

fn encode(payload: &ArtifactV1) -> Result<Vec<u8>, MalformedArtifact> {
    let mut encoder = Encoder::new();
    encoder.uuid(&payload.workspace_id);
    encoder.tag(payload.kind.tag());
    if payload.media_type != payload.kind.media_type() {
        return Err(MalformedArtifact::MediaTypeForKind);
    }
    encoder.string(&payload.media_type, MAX_TOKEN_BYTES)?;
    require_content_envelope(&payload.content_digest)?;
    encoder.string(&payload.content_digest.algorithm, MAX_TOKEN_BYTES)?;
    encoder.string(&payload.content_digest.canonicalizer, MAX_TOKEN_BYTES)?;
    encoder.sha256(&digest_bytes(&payload.content_digest.value)?);
    if payload.content_bytes > MAX_CONTENT_BYTES {
        return Err(MalformedArtifact::ContentOverBounds);
    }
    encoder.u64(payload.content_bytes);
    encoder.uuid(&payload.accountable_resident_id);
    encoder.uuid(&payload.accountable_configuration_id);
    match &payload.context {
        ArtifactContext::None => encoder.tag(TAG_CONTEXT_NONE),
        ArtifactContext::Mission {
            project_id,
            mission_id,
        } => {
            encoder.tag(TAG_CONTEXT_MISSION);
            encoder.uuid(project_id);
            encoder.uuid(mission_id);
        }
    }
    match &payload.supersedes {
        None => encoder.tag(TAG_SUPERSEDES_NONE),
        Some(digest) => {
            encoder.tag(TAG_SUPERSEDES_DIGEST);
            encoder.sha256(digest);
        }
    }
    encoder.u64(payload.recorded_at_ms);
    if encoder.out.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedArtifact::OverLimitLength);
    }
    Ok(encoder.out)
}

fn decode(input: &[u8]) -> Result<ArtifactV1, MalformedArtifact> {
    let mut decoder = Decoder::new(input)?;
    decoder.magic()?;

    let workspace_id = decoder.uuid()?;
    let kind = ArtifactKind::parse(decoder.tag()?)?;
    let media_type = decoder.string(MAX_TOKEN_BYTES)?;
    if media_type != kind.media_type() {
        return Err(MalformedArtifact::MediaTypeForKind);
    }
    let content_digest = ArtifactDigest {
        algorithm: decoder.string(MAX_TOKEN_BYTES)?,
        canonicalizer: decoder.string(MAX_TOKEN_BYTES)?,
        value: HEXLOWER.encode(&decoder.sha256()?),
    };
    require_content_envelope(&content_digest)?;
    let content_bytes = decoder.u64()?;
    if content_bytes > MAX_CONTENT_BYTES {
        return Err(MalformedArtifact::ContentOverBounds);
    }
    let accountable_resident_id = decoder.uuid()?;
    let accountable_configuration_id = decoder.uuid()?;
    let context = match decoder.tag()? {
        TAG_CONTEXT_NONE => ArtifactContext::None,
        TAG_CONTEXT_MISSION => ArtifactContext::Mission {
            project_id: decoder.uuid()?,
            mission_id: decoder.uuid()?,
        },
        _ => return Err(MalformedArtifact::UnknownTag),
    };
    let supersedes = match decoder.tag()? {
        TAG_SUPERSEDES_NONE => None,
        TAG_SUPERSEDES_DIGEST => Some(decoder.sha256()?),
        _ => return Err(MalformedArtifact::UnknownTag),
    };
    let recorded_at_ms = decoder.u64()?;
    decoder.finish()?;

    Ok(ArtifactV1 {
        workspace_id,
        kind,
        media_type,
        content_digest,
        content_bytes,
        accountable_resident_id,
        accountable_configuration_id,
        context,
        supersedes,
        recorded_at_ms,
    })
}

// ---------------------------------------------------------------------------------------------
// Slice 2: the store, the Event, the cursor, and the verified reader
// ---------------------------------------------------------------------------------------------

/// Why a well-formed Artifact does not describe this Workspace's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InconsistentArtifact {
    /// The record names a Workspace other than the service's.
    WorkspaceScope,
    /// The accountable Resident does not exist in this Workspace.
    ResidentNotFound,
    /// The accountable Configuration is not a link in that Resident's configuration chain.
    ConfigurationNotFound,
    /// The named Mission does not exist in this Workspace.
    MissionNotFound,
    /// The context's Project is not the one the named Mission belongs to.
    ProjectScopeMismatch,
    /// The accountable Resident is not on the named Mission's admission roster under the named
    /// Configuration (the E3 rule: without this, the roster an Evaluation's non-authorship proof
    /// rests on would be self-chosen).
    NotAnAdmittedMember,
    /// The digest a correction names is no stored Artifact's.
    PredecessorNotFound,
    /// A correction must cite the current tail of its chain; the named predecessor is already
    /// superseded. ADR 0037's rule, under its own name.
    NotTheCurrentTail,
    /// A correction's kind differs from its predecessor's. Kind, not encoding, is the lineage
    /// property (R16/A8).
    WrongKind,
    /// A correction's context differs from its predecessor's. A document's versions stay in one
    /// Mission.
    WrongContext,
    /// A correction predating the record it supersedes. Equal times are allowed: two records can
    /// land in one millisecond.
    TimeRegresses,
    /// The supplied content does not hash to the record's content digest.
    ContentDigestMismatch,
    /// The supplied content's length is not the record's declared byte count.
    ContentLengthMismatch,
}

impl InconsistentArtifact {
    fn reason(self) -> &'static str {
        match self {
            Self::WorkspaceScope => "the Artifact names a different Workspace",
            Self::ResidentNotFound => "the accountable Resident does not exist in this Workspace",
            Self::ConfigurationNotFound => {
                "the accountable Configuration is not a configuration of that Resident"
            }
            Self::MissionNotFound => "the named Mission does not exist in this Workspace",
            Self::ProjectScopeMismatch => {
                "the context Project is not the one the Mission belongs to"
            }
            Self::NotAnAdmittedMember => {
                "the accountable Resident is not on that Mission's admission roster under that \
                 Configuration"
            }
            Self::PredecessorNotFound => "the superseded digest names no stored Artifact",
            Self::NotTheCurrentTail => "the superseded Artifact is not the current tail",
            Self::WrongKind => "a correction's kind differs from its predecessor's",
            Self::WrongContext => "a correction's context differs from its predecessor's",
            Self::TimeRegresses => "a correction predates the record it supersedes",
            Self::ContentDigestMismatch => {
                "the content does not hash to the record's content digest"
            }
            Self::ContentLengthMismatch => {
                "the content's length is not the record's declared byte count"
            }
        }
    }
}

/// Which fault the Artifact startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum ArtifactStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One Artifact did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    ///
    /// MEASURED LIMIT (R41.1): the reader answers a row whose bytes do not hash to its digest with
    /// the flat `InvalidStore`, so this pass ships row-identity-only. The domain faults
    /// `ArtifactError` carries are reachable through the recording path, not through anything a
    /// startup plant can build without the codec.
    Row {
        workspace_id: Uuid,
        digest_value: String,
        cause: ArtifactError,
    },
    /// The supersession relation, which is about the store rather than one row.
    /// A COUNT across the store that does not match the log: numbers, and named as such.
    Totals(crate::TotalsMismatch),
    /// The supersession relation itself does not re-prove. Not a count, so it carries no numbers
    /// -- inventing an `expected`/`observed` for it would be the same lie as an invented row id.
    Relation(ArtifactError),
    /// A row whose Workspace column is not a UUID. Unreachable while the foreign-key check and the
    /// Workspace pass run first.
    Identity {
        workspace_id: String,
        digest_value: String,
    },
}

impl ArtifactError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::Malformed(_) => "Malformed",
            Self::Inconsistent(_) => "Inconsistent",
            Self::NotFound => "NotFound",
            Self::DigestCollision => "DigestCollision",
            Self::InvalidStore => "InvalidStore",
            Self::SupersessionTooLong { .. } => "SupersessionTooLong",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for ArtifactStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the Artifacts could not be read"),
            Self::Row {
                workspace_id,
                digest_value,
                cause,
            } => write!(
                formatter,
                "Artifact {digest_value} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Totals(totals) => {
                write!(
                    formatter,
                    "the Artifacts do not agree with the log -- {totals}"
                )
            }
            Self::Relation(cause) => write!(
                formatter,
                "the Artifact supersession relation does not re-prove: {}",
                cause.name()
            ),
            Self::Identity {
                workspace_id,
                digest_value,
            } => write!(
                formatter,
                "an Artifact row's Workspace is not readable: {workspace_id}, Artifact \
                 {digest_value}"
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for ArtifactStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Totals(totals) => formatter.debug_tuple("Totals").field(totals).finish(),
            Self::Relation(cause) => formatter.debug_tuple("Relation").field(cause).finish(),
            Self::Row {
                workspace_id,
                digest_value,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("digest_value", digest_value)
                .field("cause", cause)
                .finish(),
            Self::Identity {
                workspace_id,
                digest_value,
            } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .field("digest_value", digest_value)
                .finish(),
        }
    }
}

impl std::error::Error for ArtifactStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Relation(cause) | Self::Row { cause, .. } => Some(cause),
            // A `TotalsMismatch` is DATA, not a nested error.
            Self::Totals(_) => None,
            Self::Identity { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum ArtifactError {
    Account(OperatorAccountError),
    Malformed(MalformedArtifact),
    Inconsistent(InconsistentArtifact),
    NotFound,
    /// The digest exists and the bytes or content behind it differ. Never an idempotent match.
    DigestCollision,
    InvalidStore,
    /// A supersession walk reached `MAX_SUPERSESSION_HOPS` without finding its tail: refused
    /// there, by name, before the next record is read.
    SupersessionTooLong {
        bound: usize,
    },
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::Malformed(reason) => write!(formatter, "{reason}"),
            Self::Inconsistent(reason) => {
                write!(
                    formatter,
                    "Artifact references are inconsistent: {}",
                    reason.reason()
                )
            }
            Self::NotFound => write!(formatter, "no Artifact in this Workspace has that digest"),
            Self::DigestCollision => write!(
                formatter,
                "an Artifact with that digest exists and its bytes or content differ"
            ),
            Self::InvalidStore => write!(formatter, "Artifact state is invalid"),
            Self::SupersessionTooLong { bound } => write!(
                formatter,
                "the supersession chain is longer than {bound} hops and was not followed"
            ),
            Self::Store(error) => write!(formatter, "Artifact store failed: {error}"),
            Self::Json(error) => write!(formatter, "Artifact event is invalid: {error}"),
        }
    }
}

impl std::error::Error for ArtifactError {}

impl From<OperatorAccountError> for ArtifactError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<MalformedArtifact> for ArtifactError {
    fn from(reason: MalformedArtifact) -> Self {
        Self::Malformed(reason)
    }
}

impl From<rusqlite::Error> for ArtifactError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for ArtifactError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Who recorded the Artifact. One arm: the singleton account model has no Operator UUID, so v1
/// records the closed basis rather than inventing an identity (RulingDecider's reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactActor {
    Owner,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordedArtifact {
    digest: ArtifactDigest,
    canonical_bytes_base64: String,
    content_digest: ArtifactDigest,
    content_bytes: u64,
    actor: ArtifactActor,
    recorded_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArtifactRecordedPayload {
    artifact: RecordedArtifact,
}

/// One stored Artifact row: the bytes, the identity, the content, and the projections.
///
/// Every column past `content_inline` is a projection for querying. Every claim is re-derived
/// from the bytes on every verified read and at startup, and a row that disagrees with its own
/// bytes is invalid state, not a different record.
struct StoredArtifact {
    workspace_id: Uuid,
    digest_value: String,
    canonical_bytes: Vec<u8>,
    content_inline: Vec<u8>,
    kind: String,
    media_type: String,
    content_bytes: u64,
    accountable_resident_id: Uuid,
    accountable_configuration_id: Uuid,
    context_kind: String,
    context_project_id: Option<Uuid>,
    context_mission_id: Option<Uuid>,
    has_predecessor: bool,
    supersedes_digest: Option<String>,
    recorded_at_ms: u64,
    creation_event_seq: u64,
}

fn parse_uuid(value: &str) -> Result<Uuid, ArtifactError> {
    Uuid::parse_str(value).map_err(|_| ArtifactError::InvalidStore)
}

fn parse_optional_uuid(value: Option<String>) -> Result<Option<Uuid>, ArtifactError> {
    value.map(|inner| parse_uuid(&inner)).transpose()
}

fn nonnegative_u64(value: i64) -> Result<u64, ArtifactError> {
    u64::try_from(value).map_err(|_| ArtifactError::InvalidStore)
}

fn load_stored(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<StoredArtifact>, ArtifactError> {
    connection
        .query_row(
            "SELECT workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                    canonical_bytes, content_inline, kind, media_type, content_bytes,
                    accountable_resident_id, accountable_configuration_id, context_kind,
                    context_project_id, context_mission_id, has_predecessor, supersedes_digest,
                    recorded_at_ms, creation_event_seq
             FROM artifacts
             WHERE workspace_id = ?1 AND digest_value = ?2",
            params![workspace_id.to_string(), digest_value],
            |row| {
                let digest_algorithm: String = row.get(1)?;
                let digest_canonicalizer: String = row.get(2)?;
                // A row naming another algorithm or canonicalizer is not this module's record at
                // all: refuse it here so no reader ever re-hashes bytes under the wrong function.
                if digest_algorithm != DIGEST_ALGORITHM || digest_canonicalizer != CANONICALIZER {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok(StoredArtifact {
                    workspace_id: parse_uuid(&row.get::<_, String>(0)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    digest_value: row.get(3)?,
                    canonical_bytes: row.get(4)?,
                    content_inline: row.get(5)?,
                    kind: row.get(6)?,
                    media_type: row.get(7)?,
                    content_bytes: nonnegative_u64(row.get(8)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    accountable_resident_id: parse_uuid(&row.get::<_, String>(9)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    accountable_configuration_id: parse_uuid(&row.get::<_, String>(10)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    context_kind: row.get(11)?,
                    context_project_id: parse_optional_uuid(row.get(12)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    context_mission_id: parse_optional_uuid(row.get(13)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    has_predecessor: row.get::<_, i64>(14)? != 0,
                    supersedes_digest: row.get(15)?,
                    recorded_at_ms: nonnegative_u64(row.get(16)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    creation_event_seq: nonnegative_u64(row.get(17)?)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            },
        )
        .optional()
        .map_err(ArtifactError::from)
}

/// Every indexed column repeats what the bytes say. A row that disagrees with its own bytes is
/// refused, on every verified read and at startup.
fn columns_match(stored: &StoredArtifact, payload: &ArtifactV1) -> bool {
    let (context_kind, context_project_id, context_mission_id) = match &payload.context {
        ArtifactContext::None => ("none", None, None),
        ArtifactContext::Mission {
            project_id,
            mission_id,
        } => ("mission", Some(*project_id), Some(*mission_id)),
    };
    stored.workspace_id == payload.workspace_id
        && stored.kind == payload.kind.as_str()
        && stored.media_type == payload.media_type
        && stored.content_bytes == payload.content_bytes
        && stored.accountable_resident_id == payload.accountable_resident_id
        && stored.accountable_configuration_id == payload.accountable_configuration_id
        && stored.context_kind == context_kind
        && stored.context_project_id == context_project_id
        && stored.context_mission_id == context_mission_id
        && stored.has_predecessor == payload.supersedes.is_some()
        && stored.supersedes_digest.as_deref()
            == payload
                .supersedes
                .map(|digest| HEXLOWER.encode(&digest))
                .as_deref()
        && stored.recorded_at_ms == payload.recorded_at_ms
}

/// The content proof: the stored bytes hash to the record's content digest and measure its
/// declared count. Recomputed on every verified read; never trusted from a column.
fn content_matches(stored: &StoredArtifact, payload: &ArtifactV1) -> bool {
    stored.content_inline.len() as u64 == payload.content_bytes
        && payload.content_digest.value == HEXLOWER.encode(&sha256(&stored.content_inline))
}

/// One stored Artifact, proved: the digest names these bytes, the bytes decode strictly and
/// re-encode identically, every indexed column repeats what they say, and the content hashes to
/// the digest the bytes carry. The supersession relation is checked against the bytes too: the
/// row a successor cites must be the digest the successor's bytes name.
fn verified_payload(
    connection: &Connection,
    stored: &StoredArtifact,
) -> Result<ArtifactV1, ArtifactError> {
    if HEXLOWER.encode(&sha256(&stored.canonical_bytes)) != stored.digest_value {
        return Err(ArtifactError::InvalidStore);
    }
    let payload = decode(&stored.canonical_bytes).map_err(|_| ArtifactError::InvalidStore)?;
    if encode(&payload).map_err(|_| ArtifactError::InvalidStore)? != stored.canonical_bytes {
        return Err(ArtifactError::InvalidStore);
    }
    if !columns_match(stored, &payload) || !content_matches(stored, &payload) {
        return Err(ArtifactError::InvalidStore);
    }
    if let Some(predecessor) = &stored.supersedes_digest {
        let cited: bool = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM artifact_supersessions
                     WHERE workspace_id = ?1 AND artifact_digest = ?2 AND supersedes_digest = ?3)",
                params![
                    stored.workspace_id.to_string(),
                    stored.digest_value,
                    predecessor
                ],
                |row| row.get(0),
            )
            .map_err(ArtifactError::Store)?;
        if !cited {
            return Err(ArtifactError::InvalidStore);
        }
    }
    Ok(payload)
}

/// The Event paired with one Artifact: exactly one, its decoded byte field exactly equal to the
/// stored record bytes, and its sequence is the record's creation cursor. Re-derived on every
/// verified read, never trusted from the column.
fn paired_event_sequence(
    connection: &Connection,
    stored: &StoredArtifact,
) -> Result<u64, ArtifactError> {
    let mut statement = connection.prepare(
        "SELECT sequence, payload_json, occurred_at_ms FROM events
         WHERE workspace_id = ?1 AND kind = 'artifact.recorded'
           AND json_extract(payload_json, '$.artifact.digest.value') = ?2",
    )?;
    let rows = statement
        .query_map(
            params![stored.workspace_id.to_string(), stored.digest_value],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if rows.len() != 1 {
        return Err(ArtifactError::InvalidStore);
    }
    let (sequence, payload_json, occurred_at_ms) = rows.into_iter().next().expect("one row");
    verify_artifact_event(
        connection,
        stored.workspace_id,
        nonnegative_u64(occurred_at_ms)?,
        &payload_json,
    )?;
    nonnegative_u64(sequence)
}

/// Preflight the directly selected context Artifact before its owning reader transfers blobs.
/// This does not verify the Artifact; the caller must still use the verified owner in the same
/// snapshot. The Event byte cap is a read-allocation bound, not a resource or inference budget.
pub(crate) fn artifact_within_context_read_limits(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<bool, ArtifactError> {
    let lengths: Option<(i64, i64)> = connection
        .query_row(
            "SELECT length(canonical_bytes), length(content_inline) FROM artifacts
         WHERE workspace_id = ?1 AND digest_value = ?2",
            params![workspace_id.to_string(), digest_value],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((canonical_bytes, content_bytes)) = lengths
        && (canonical_bytes > MAX_CANONICAL_BYTES as i64
            || content_bytes > MAX_CONTENT_BYTES as i64)
    {
        return Ok(false);
    }
    let (events, event_bytes): (i64, i64) = connection.query_row(
        "SELECT count(*), coalesce(max(length(CAST(payload_json AS BLOB))), 0) FROM events
         WHERE workspace_id = ?1 AND kind = 'artifact.recorded'
           AND json_extract(payload_json, '$.artifact.digest.value') = ?2",
        params![workspace_id.to_string(), digest_value],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(events <= 1 && event_bytes <= 64 * 1024)
}

/// One stored Artifact, fully proved, inside the caller's transaction.
///
/// Returns `Ok(None)` when this Workspace has no such Artifact -- a different fact from one that
/// disagrees with itself, which is `InvalidStore`, and from a digest that names another
/// Workspace's Artifact, which the caller never learns about.
pub(crate) fn verified_artifact_within(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<VerifiedArtifactRecord>, ArtifactError> {
    let Some(stored) = load_stored(connection, workspace_id, digest_value)? else {
        return Ok(None);
    };
    let payload = verified_payload(connection, &stored)?;
    let event_sequence = paired_event_sequence(connection, &stored)?;
    if event_sequence != stored.creation_event_seq {
        return Err(ArtifactError::InvalidStore);
    }
    let superseded_by = connection
        .query_row(
            "SELECT artifact_digest FROM artifact_supersessions
             WHERE workspace_id = ?1 AND supersedes_digest = ?2",
            params![stored.workspace_id.to_string(), stored.digest_value],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(Some(VerifiedArtifactRecord {
        digest: digest_envelope(&stored.digest_value),
        payload,
        content: stored.content_inline,
        creation_event_seq: stored.creation_event_seq,
        superseded_by,
    }))
}

/// One Artifact, verified: the record, its content, its creation cursor, and its supersession
/// state.
///
/// The record's own fields are proved at read time (bytes, digest, columns, content). Two fields
/// are different in kind: `payload.supersedes` is a fact of the record (boundary-time, inside the
/// bytes), while `superseded_by` is derived from the relation **now** -- read-time state, labelled
/// so by the accessor's documentation, and never silently merged into the cited record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedArtifactRecord {
    digest: ArtifactDigest,
    payload: ArtifactV1,
    content: Vec<u8>,
    creation_event_seq: u64,
    superseded_by: Option<String>,
}

impl VerifiedArtifactRecord {
    /// The record's identity envelope (algorithm, canonicalizer, value), proved at read time.
    pub fn digest(&self) -> &ArtifactDigest {
        &self.digest
    }

    /// The decoded record, proved against its own bytes at read time.
    pub fn payload(&self) -> &ArtifactV1 {
        &self.payload
    }

    /// The exact stored content bytes, proved against the record's content digest at read time.
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// The sequence of this record's `artifact.recorded` Event: where this record entered the
    /// ledger. Re-derived against the Event on every read; a position, not a claim about anything
    /// else.
    pub fn creation_event_seq(&self) -> u64 {
        self.creation_event_seq
    }

    /// The digest of the Artifact that supersedes this one, if one exists -- derived from the
    /// supersession relation at read time. This is read-time state: it moves as corrections land,
    /// and it is never a property of this record's bytes.
    pub fn superseded_by(&self) -> Option<&str> {
        self.superseded_by.as_deref()
    }
}

/// The digest chain from one Artifact to its current tail, in chain order, cited digest first.
///
/// Chain order, never timestamp order. Every hop is re-proved: the successor's bytes must name the
/// predecessor the relation records. This is the both-links rule's instrument (R12 item 10): a
/// consumer shows the cited digest and, beside it, the current tail -- never the tail silently in
/// place of the link.
pub(crate) fn supersession_tail(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Vec<ArtifactDigest>, ArtifactError> {
    let mut chain = Vec::new();
    let mut current = digest_value.to_owned();
    loop {
        // `chain` holds the records read so far, the cited one included; a walk about to read past
        // `MAX_SUPERSESSION_HOPS` hops is refused by name BEFORE that read (sol1's seams read, P1).
        if chain.len() > MAX_SUPERSESSION_HOPS {
            return Err(ArtifactError::SupersessionTooLong {
                bound: MAX_SUPERSESSION_HOPS,
            });
        }
        let Some(verified) = verified_artifact_within(connection, workspace_id, &current)? else {
            return Err(ArtifactError::InvalidStore);
        };
        let next = verified.superseded_by.clone();
        chain.push(verified.digest.clone());
        match next {
            None => return Ok(chain),
            Some(successor) => current = successor,
        }
    }
}

/// Proves the record's references exist and agree, at record time and again at startup.
/// Lifecycle is deliberately not consulted: accountability is a record-time fact, and a retired
/// Resident's Artifacts stay attributable (`BLUEPRINT:343`).
fn validate_references(
    connection: &Connection,
    workspace_id: Uuid,
    payload: &ArtifactV1,
) -> Result<(), ArtifactError> {
    let inconsistent = |reason| Err(ArtifactError::Inconsistent(reason));

    if payload.workspace_id != workspace_id {
        return inconsistent(InconsistentArtifact::WorkspaceScope);
    }

    let resident_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE workspace_id = ?1 AND id = ?2)",
        params![
            workspace_id.to_string(),
            payload.accountable_resident_id.to_string()
        ],
        |row| row.get(0),
    )?;
    if !resident_exists {
        return inconsistent(InconsistentArtifact::ResidentNotFound);
    }
    let configuration_exists: bool = connection.query_row(
        "SELECT EXISTS(
                 SELECT 1 FROM resident_configuration_decisions
                 WHERE workspace_id = ?1 AND resident_id = ?2 AND id = ?3)",
        params![
            workspace_id.to_string(),
            payload.accountable_resident_id.to_string(),
            payload.accountable_configuration_id.to_string()
        ],
        |row| row.get(0),
    )?;
    if !configuration_exists {
        return inconsistent(InconsistentArtifact::ConfigurationNotFound);
    }

    if let ArtifactContext::Mission {
        project_id,
        mission_id,
    } = &payload.context
    {
        let Some(facts) = mission_admission_facts(
            connection,
            workspace_id,
            *mission_id,
            payload.accountable_resident_id,
        )
        .map_err(|_| ArtifactError::InvalidStore)?
        else {
            return inconsistent(InconsistentArtifact::MissionNotFound);
        };
        if &facts.project_id != project_id {
            return inconsistent(InconsistentArtifact::ProjectScopeMismatch);
        }
        if facts.member_configuration_id != Some(payload.accountable_configuration_id) {
            return inconsistent(InconsistentArtifact::NotAnAdmittedMember);
        }
    }
    Ok(())
}

/// The supersession rules, checked at record time inside the writer's transaction so two
/// concurrent corrections cannot both extend one tail: cite the current tail, same kind, same
/// context, and no step back in time.
fn validate_supersession(
    connection: &Connection,
    workspace_id: Uuid,
    payload: &ArtifactV1,
) -> Result<(), ArtifactError> {
    let Some(predecessor) = payload.supersedes else {
        return Ok(());
    };
    let predecessor_hex = HEXLOWER.encode(&predecessor);
    let inconsistent = |reason| Err(ArtifactError::Inconsistent(reason));

    let Some(stored) = load_stored(connection, workspace_id, &predecessor_hex)? else {
        return inconsistent(InconsistentArtifact::PredecessorNotFound);
    };
    let predecessor_payload = verified_payload(connection, &stored)?;
    if predecessor_payload.kind != payload.kind {
        return inconsistent(InconsistentArtifact::WrongKind);
    }
    if predecessor_payload.context != payload.context {
        return inconsistent(InconsistentArtifact::WrongContext);
    }
    if payload.recorded_at_ms < predecessor_payload.recorded_at_ms {
        return inconsistent(InconsistentArtifact::TimeRegresses);
    }
    let already_superseded: bool = connection.query_row(
        "SELECT EXISTS(
                 SELECT 1 FROM artifact_supersessions
                 WHERE workspace_id = ?1 AND supersedes_digest = ?2)",
        params![workspace_id.to_string(), predecessor_hex],
        |row| row.get(0),
    )?;
    if already_superseded {
        return inconsistent(InconsistentArtifact::NotTheCurrentTail);
    }
    Ok(())
}

/// Writes the Event, then the row carrying the Event's sequence as its creation cursor, then the
/// supersession relation. The Event goes FIRST because the cursor is the Event's own sequence and
/// `artifacts_no_update` makes a later correction impossible -- a row written with a provisional
/// cursor could never be repaired. One transaction, opened by the caller, so a failure partway
/// rolls all three back together.
fn insert_artifact(
    connection: &Connection,
    stored: &StoredArtifact,
    content_digest: &ArtifactDigest,
) -> Result<(), ArtifactError> {
    let payload = serde_json::to_string(&ArtifactRecordedPayload {
        artifact: RecordedArtifact {
            digest: digest_envelope(&stored.digest_value),
            canonical_bytes_base64: BASE64.encode(&stored.canonical_bytes),
            content_digest: content_digest.clone(),
            content_bytes: stored.content_bytes,
            actor: ArtifactActor::Owner,
            recorded_at_ms: stored.recorded_at_ms,
        },
    })?;
    crate::event_chain::append_event(
        connection,
        &stored.workspace_id.to_string(),
        "artifact.recorded",
        &payload,
        i64::try_from(stored.recorded_at_ms).map_err(|_| ArtifactError::InvalidStore)?,
    )?;
    let sequence = connection.last_insert_rowid();
    connection.execute(
        "INSERT INTO artifacts (
             workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
             canonical_bytes, content_inline, kind, media_type, content_bytes,
             accountable_resident_id, accountable_configuration_id, context_kind,
             context_project_id, context_mission_id, has_predecessor, supersedes_digest,
             recorded_at_ms, creation_event_seq
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                   ?17, ?18)",
        params![
            stored.workspace_id.to_string(),
            DIGEST_ALGORITHM,
            CANONICALIZER,
            stored.digest_value.as_str(),
            stored.canonical_bytes.as_slice(),
            stored.content_inline.as_slice(),
            stored.kind.as_str(),
            stored.media_type.as_str(),
            i64::try_from(stored.content_bytes).map_err(|_| ArtifactError::InvalidStore)?,
            stored.accountable_resident_id.to_string(),
            stored.accountable_configuration_id.to_string(),
            stored.context_kind.as_str(),
            stored.context_project_id.map(|id| id.to_string()),
            stored.context_mission_id.map(|id| id.to_string()),
            i64::from(stored.has_predecessor),
            stored.supersedes_digest.as_deref(),
            i64::try_from(stored.recorded_at_ms).map_err(|_| ArtifactError::InvalidStore)?,
            sequence,
        ],
    )?;
    if let Some(predecessor) = &stored.supersedes_digest {
        connection.execute(
            "INSERT INTO artifact_supersessions (workspace_id, artifact_digest, supersedes_digest)
             VALUES (?1, ?2, ?3)",
            params![
                stored.workspace_id.to_string(),
                stored.digest_value.as_str(),
                predecessor
            ],
        )?;
    }
    Ok(())
}

/// Owns authenticated Artifact recording and identity reads behind one deep interface.
///
/// Recording an Artifact creates no authority. It opens no Gate, admits no dispatch, completes no
/// Mission, publishes no Outcome, invokes no model, and makes no network request.
pub struct ArtifactService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl ArtifactService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Records one immutable Artifact and its content. The Owner is authenticated before anything
    /// is inspected. The time is the service clock, stamped into the bytes the service
    /// canonicalizes -- the typed-argument pattern (Ruling v1, Candidate v1): there is no second
    /// party whose exact encoding must be preserved, so no untrusted byte slice is accepted.
    /// Identical content recorded twice is two Artifacts, because the accountability event
    /// differs; idempotence across a crash belongs to the caller, who can first read by the digest
    /// it would have produced.
    pub fn record(
        &self,
        kind: ArtifactKind,
        content: &[u8],
        context: ArtifactContext,
        accountable_resident_id: Uuid,
        accountable_configuration_id: Uuid,
        supersedes: Option<[u8; 32]>,
    ) -> Result<VerifiedArtifactRecord, ArtifactError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let recorded_at_ms = u64::try_from(now).map_err(|_| ArtifactError::InvalidStore)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let verified = record_artifact_within(
                &transaction,
                self.workspace_id,
                recorded_at_ms,
                kind,
                content,
                context,
                accountable_resident_id,
                accountable_configuration_id,
                supersedes,
            )?;
            transaction.commit()?;
            Ok(verified)
        })
    }

    /// The exact-digest read: the record, its content, its cursor, and its read-time supersession
    /// state -- or a refusal. A digest that names nothing in this Workspace is `NotFound`, never
    /// an empty or filtered answer.
    pub fn get(&self, digest: &str) -> Result<VerifiedArtifactRecord, ArtifactError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            verified_artifact_within(connection, self.workspace_id, digest)?
                .ok_or(ArtifactError::NotFound)
        })
    }

    /// The digest chain from one Artifact to its current tail, chain order, cited digest first.
    /// Read-time state, derived on every call.
    pub fn list_supersession_tail(
        &self,
        digest: &str,
    ) -> Result<Vec<ArtifactDigest>, ArtifactError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            if load_stored(connection, self.workspace_id, digest)?.is_none() {
                return Err(ArtifactError::NotFound);
            }
            supersession_tail(connection, self.workspace_id, digest)
        })
    }
}

/// Records one Artifact INSIDE a caller's transaction: the row, its `artifact.recorded` Event, and
/// its supersession link, or nothing at all.
///
/// `ArtifactService::record` is this function plus an authenticated store and a commit. It is
/// split out because a caller that must write an Artifact and its own record in ONE transaction
/// cannot go through the service: `with_authenticated_store` holds the account mutex, so calling
/// the service from inside another `with_authenticated_store` closure would deadlock, and calling
/// it outside one would commit the Artifact before the record that names it. `prepare` is that
/// caller (ADR 0040): a candidate Artifact with no manifest is an Outcome Document nothing can be
/// verified against.
///
/// `recorded_at_ms` is the caller's authenticated service clock, never caller input -- the same
/// value `record` stamps, passed rather than re-read so both writes in one transaction carry one
/// time.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_artifact_within(
    transaction: &Transaction<'_>,
    workspace_id: Uuid,
    recorded_at_ms: u64,
    kind: ArtifactKind,
    content: &[u8],
    context: ArtifactContext,
    accountable_resident_id: Uuid,
    accountable_configuration_id: Uuid,
    supersedes: Option<[u8; 32]>,
) -> Result<VerifiedArtifactRecord, ArtifactError> {
    let media_type = kind.media_type().to_owned();
    let payload = ArtifactV1 {
        workspace_id,
        kind,
        media_type,
        content_digest: ArtifactDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: RAW_CONTENT_CANONICALIZER.to_owned(),
            value: HEXLOWER.encode(&sha256(content)),
        },
        content_bytes: u64::try_from(content.len()).map_err(|_| ArtifactError::InvalidStore)?,
        accountable_resident_id,
        accountable_configuration_id,
        context,
        supersedes,
        recorded_at_ms,
    };
    // The codec's own refusals, run on the value the service just built: the encoder is the only
    // producer of these bytes, and its bounds are the module's.
    let canonical_bytes = payload.canonical_bytes()?;

    if let Some(stored) = load_stored(transaction, workspace_id, &payload.digest()?.value)? {
        if stored.canonical_bytes != canonical_bytes || stored.content_inline != content {
            return Err(ArtifactError::DigestCollision);
        }
        return verified_artifact_within(transaction, workspace_id, &stored.digest_value)?
            .ok_or(ArtifactError::InvalidStore);
    }

    validate_references(transaction, workspace_id, &payload)?;
    validate_supersession(transaction, workspace_id, &payload)?;

    let content_digest = payload.content_digest.clone();
    let stored = StoredArtifact {
        workspace_id,
        digest_value: payload.digest()?.value,
        canonical_bytes,
        content_inline: content.to_vec(),
        kind: kind.as_str().to_owned(),
        media_type: kind.media_type().to_owned(),
        content_bytes: payload.content_bytes,
        accountable_resident_id,
        accountable_configuration_id,
        context_kind: match &payload.context {
            ArtifactContext::None => "none".to_owned(),
            ArtifactContext::Mission { .. } => "mission".to_owned(),
        },
        context_project_id: match &payload.context {
            ArtifactContext::Mission { project_id, .. } => Some(*project_id),
            ArtifactContext::None => None,
        },
        context_mission_id: match &payload.context {
            ArtifactContext::Mission { mission_id, .. } => Some(*mission_id),
            ArtifactContext::None => None,
        },
        has_predecessor: payload.supersedes.is_some(),
        supersedes_digest: payload.supersedes.map(|digest| HEXLOWER.encode(&digest)),
        recorded_at_ms,
        // The cursor IS the paired Event's position. The Event is written first and the row
        // carries its sequence; `artifacts_no_update` makes any later correction impossible, which
        // is exactly the guarantee reconciliation relies on.
        creation_event_seq: 1,
    };
    let digest_value = stored.digest_value.clone();
    insert_artifact(transaction, &stored, &content_digest)?;
    verified_artifact_within(transaction, workspace_id, &digest_value)?
        .ok_or(ArtifactError::InvalidStore)
}

/// Requires one Event per Artifact whose decoded byte field exactly equals the stored record
/// bytes, whose content digest envelope is the content's own, and whose encoding is the canonical
/// padded base64 -- a differently encoded payload carrying the same bytes is different evidence,
/// not the same evidence reformatted.
pub(crate) fn verify_artifact_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), ArtifactError> {
    let payload: ArtifactRecordedPayload = serde_json::from_str(payload_json)?;
    let recorded = payload.artifact;
    // Whole envelopes, not values alone (R15): a forged algorithm or canonicalizer beside a
    // matching value is different evidence, and the content digest's envelope is the content's
    // own -- sha256 under beacon.raw.v1 -- not whatever the Event claims.
    if recorded.digest.algorithm != DIGEST_ALGORITHM
        || recorded.digest.canonicalizer != CANONICALIZER
        || recorded.content_digest.algorithm != DIGEST_ALGORITHM
        || recorded.content_digest.canonicalizer != RAW_CONTENT_CANONICALIZER
        || recorded.recorded_at_ms != occurred_at_ms
    {
        return Err(ArtifactError::InvalidStore);
    }
    let canonical_bytes = BASE64
        .decode(recorded.canonical_bytes_base64.as_bytes())
        .map_err(|_| ArtifactError::InvalidStore)?;
    if BASE64.encode(&canonical_bytes) != recorded.canonical_bytes_base64 {
        return Err(ArtifactError::InvalidStore);
    }
    if HEXLOWER.encode(&sha256(&canonical_bytes)) != recorded.digest.value {
        return Err(ArtifactError::InvalidStore);
    }
    let stored = load_stored(connection, workspace_id, &recorded.digest.value)?
        .ok_or(ArtifactError::InvalidStore)?;
    if stored.canonical_bytes != canonical_bytes
        || stored.recorded_at_ms != occurred_at_ms
        || recorded.content_bytes != stored.content_bytes
        || recorded.content_digest.value != HEXLOWER.encode(&sha256(&stored.content_inline))
    {
        return Err(ArtifactError::InvalidStore);
    }
    Ok(())
}

/// Startup reconciliation: every Artifact row re-proved against its bytes, its content, its Event,
/// its cursor, its references, and its chain. The world is not re-derived -- a later Mission or a
/// retired Resident does not corrupt a recorded Artifact.
/// The rows `verify_artifact_state` enumerates, in the order it enumerates them.
///
/// EXTRACTED SO A BAR CAN CALL IT (R44.6). A startup bar that asserts where its plant sits was
/// COPYING this query, so flipping the pass's own `ORDER BY` left the assertion decorative: it
/// agreed with a copy that no longer described the pass. One function, called by both, is what
/// makes the assertion load-bearing -- a mutant on the ordering moves the pass AND the bar's view
/// of it, and the bar fails.
pub fn enumerate_artifacts(connection: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "SELECT workspace_id, digest_value FROM artifacts ORDER BY workspace_id, digest_value",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn verify_artifact_state(connection: &Connection) -> Result<(), ArtifactStateFault> {
    let rows = enumerate_artifacts(connection).map_err(ArtifactStateFault::Store)?;

    if !rows.is_empty() && !owner_account_exists(connection).map_err(ArtifactStateFault::Store)? {
        return Err(ArtifactStateFault::Totals(crate::TotalsMismatch {
            table: "operator_accounts (an owner for the stored Artifacts)",
            expected: 1,
            observed: 0,
        }));
    }

    for (workspace_id, digest_value) in &rows {
        let identity = || ArtifactStateFault::Identity {
            workspace_id: workspace_id.clone(),
            digest_value: digest_value.clone(),
        };
        let parsed = Uuid::parse_str(workspace_id).map_err(|_| identity())?;
        verify_one_artifact(connection, parsed, digest_value).map_err(|cause| {
            ArtifactStateFault::Row {
                workspace_id: parsed,
                digest_value: digest_value.clone(),
                cause,
            }
        })?;
    }
    verify_artifact_supersessions(connection, rows.len()).map_err(|cause| match cause {
        ArtifactError::Store(error) => ArtifactStateFault::Store(error),
        other => ArtifactStateFault::Relation(other),
    })?;
    Ok(())
}

/// One stored Artifact, re-proved. Unchanged from what the loop held inline.
fn verify_one_artifact(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<(), ArtifactError> {
    let stored =
        load_stored(connection, workspace_id, digest_value)?.ok_or(ArtifactError::InvalidStore)?;
    let payload = verified_payload(connection, &stored)?;
    validate_references(connection, workspace_id, &payload)?;
    let event_sequence = paired_event_sequence(connection, &stored)?;
    if event_sequence != stored.creation_event_seq {
        return Err(ArtifactError::InvalidStore);
    }
    Ok(())
}

/// The supersession relation: about the store, not one row.
fn verify_artifact_supersessions(
    connection: &Connection,
    artifact_count: usize,
) -> Result<(), ArtifactError> {
    // The relation re-derived against the bytes: every supersession row's successor must name that
    // predecessor in its canonical bytes, no Artifact may be superseded twice (the store's UNIQUE
    // is re-proved here, reconciliation being complete on its own terms), and no chain may cycle.
    let mut statement = connection.prepare(
        "SELECT workspace_id, artifact_digest, supersedes_digest
         FROM artifact_supersessions ORDER BY workspace_id, artifact_digest",
    )?;
    let links: Vec<(String, String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let mut superseded_once: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    for (workspace, successor, predecessor) in &links {
        if !superseded_once.insert((workspace.clone(), predecessor.clone())) {
            return Err(ArtifactError::InvalidStore);
        }
        let stored = load_stored(connection, parse_uuid(workspace)?, successor)?
            .ok_or(ArtifactError::InvalidStore)?;
        let payload = verified_payload(connection, &stored)?;
        let Some(named) = payload.supersedes else {
            return Err(ArtifactError::InvalidStore);
        };
        if HEXLOWER.encode(&named) != *predecessor {
            return Err(ArtifactError::InvalidStore);
        }
        let predecessor_stored = load_stored(connection, parse_uuid(workspace)?, predecessor)?
            .ok_or(ArtifactError::InvalidStore)?;
        let predecessor_payload = verified_payload(connection, &predecessor_stored)?;
        if payload.recorded_at_ms < predecessor_payload.recorded_at_ms {
            return Err(ArtifactError::InvalidStore);
        }
    }
    // A cycle in the relation whose bytes agreed with it would be a cycle of SHA-256 preimages;
    // what a store can actually hold is a cycle the bytes contradict, refused above, or one whose
    // walk never ends, refused here.
    for (workspace, successor, _) in &links {
        let mut walked = std::collections::HashSet::new();
        let mut current = successor.clone();
        while let Some(next) = links
            .iter()
            .find(|link| link.1 == current && &link.0 == workspace)
            .map(|link| link.2.clone())
        {
            if !walked.insert(current.clone()) {
                return Err(ArtifactError::InvalidStore);
            }
            current = next;
        }
    }

    let total_events: i64 = connection.query_row(
        "SELECT count(*) FROM events WHERE kind = 'artifact.recorded'",
        [],
        |row| row.get(0),
    )?;
    if usize::try_from(total_events) != Ok(artifact_count) {
        return Err(ArtifactError::InvalidStore);
    }
    Ok(())
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()` (the fold at 30d2a20).
    fn expected_name(error: &ArtifactError) -> &'static str {
        match error {
            ArtifactError::Account(_) => "Account",
            ArtifactError::Malformed(_) => "Malformed",
            ArtifactError::Inconsistent(_) => "Inconsistent",
            ArtifactError::NotFound => "NotFound",
            ArtifactError::DigestCollision => "DigestCollision",
            ArtifactError::InvalidStore => "InvalidStore",
            ArtifactError::SupersessionTooLong { .. } => "SupersessionTooLong",
            ArtifactError::Store(_) => "Store",
            ArtifactError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_artifact_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            ArtifactError::Account(crate::operator_account::OperatorAccountError::PasswordTooShort),
            ArtifactError::Malformed(MalformedArtifact::Magic),
            ArtifactError::Inconsistent(InconsistentArtifact::WorkspaceScope),
            ArtifactError::NotFound,
            ArtifactError::DigestCollision,
            ArtifactError::InvalidStore,
            ArtifactError::SupersessionTooLong { bound: 1 },
            ArtifactError::Store(rusqlite::Error::QueryReturnedNoRows),
            ArtifactError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            9,
            "an arm was added to `ArtifactError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(ArtifactError::name).collect();
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
