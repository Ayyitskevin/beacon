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

use std::fmt;

use data_encoding::HEXLOWER;
use uuid::Uuid;

use crate::proposal::sha256;

/// The canonicalizer identity. Any future grammar that alters the byte layout of records that
/// already exist is a new canonicalizer; assigning a value in a published tag position is not one.
/// Existing v1 bytes are never reinterpreted under v2 rules (ADR 0034's rule, kept).
const CANONICALIZER: &str = "beacon.artifact.v1";
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
