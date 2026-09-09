//! Mission-plan Invocation, slice C1: the record's pure types, its canonical bytes, and the finite
//! transport grammar.
//!
//! An `InvocationAdmissionV1` is the immutable statement that one Operator action admitted one
//! Mission-plan model request, and an `InvocationObservationV1` is one exact thing the local
//! transport observed while that request was outstanding. Together they are the record a later
//! verified reader reduces; this slice is only the record and its language.
//!
//! **Slice C1 is pure.** No database, no service, no Event, no migration, no adapter, no permit,
//! no public Command — those are later slices, and this module deliberately exposes none of them.
//! In particular nothing here makes model calling available: there is no authority resolver, no
//! request trigger, and no transport. It follows the shape measured spend used for the same
//! problem (`crates/beacond/src/spend.rs:16-22`, `crates/beacond/tests/spend.rs:1-9`).
//!
//! # What this module does NOT prove
//!
//! - **It does not verify authority.** [`RetainedAuthority`] holds a retained *reference* and a
//!   *claimed* digest. This module never sees the authority record's bytes, so it cannot hash them
//!   and does not check them. What the codec proves is that the admission committed to this
//!   reference and this claimed digest, and nothing more. Production admission, eligibility and
//!   disclosure remain unavailable; there is no verifying authority owner yet.
//! - **It does not choose the resource envelope.** [`RetainedLimits`] are carried as *evidence* the
//!   admission recorded, supplied by whoever resolves authority. This module has no default for
//!   them and never invents one.
//! - **It does not claim durability, ownership, restart or lock behavior.** Reducing a history to
//!   a [`TransportStatus`] is a statement about the rows it is handed, not about what survived a
//!   crash, what an Event proved, or what a store returned.
//!
//! # Two different bounds, and why they are not the same bound
//!
//! A record's own [`RetainedLimits`] are **evidence**: they say what envelope the admission
//! recorded. They are not an allocation budget, because they arrive inside the very input being
//! parsed — an input that selected the only ceiling constraining its own parse would be the
//! dodgeable guard this fleet has been bitten by.
//!
//! So every decode takes a caller-supplied [`ReadCeiling`], and the caller must supply it: there is
//! no `Default`. The raw input length is compared against that ceiling **before any field is read**,
//! and every retained byte field is compared against it **before it is allocated**. Record-limit
//! coherence (`did this record's retained length fit the envelope this record itself recorded?`) is
//! checked separately, by name, and is a statement about the record rather than about this process's
//! memory.
//!
//! What the ceiling actually covers: the decoder borrows the caller's `&[u8]` and allocates only
//! what it copies out of it — the retained byte vectors and the header/reference/profile strings.
//! The caller's own buffer was allocated before this module saw it and is bounded by whoever read
//! it, not here.

use std::fmt;

use uuid::Uuid;

use crate::proposal::sha256;

/// The canonicalizer identity of an admission record. Any future grammar is a new canonicalizer;
/// existing v1 bytes are never reinterpreted under it.
pub const ADMISSION_CANONICALIZER: &str = "beacon.model_invocation.admission.v1";
/// The canonicalizer identity of a transport observation record.
pub const OBSERVATION_CANONICALIZER: &str = "beacon.model_invocation.observation.v1";
/// The one digest algorithm these records are identified by.
pub const INVOCATION_DIGEST_ALGORITHM: &str = "sha256";

const MAGIC_ADMISSION: &[u8] = b"beacon.model_invocation.admission.v1\0";
const MAGIC_OBSERVATION: &[u8] = b"beacon.model_invocation.observation.v1\0";

// ---------------------------------------------------------------------------------------------
// Identifier-shaped limits owned by this module.
//
// These bound NAMES, not payloads: a canonicalizer reference and a prompt-profile token are
// identifiers, and their length is not one of the reserved resource decisions. Every limit that
// bounds a PAYLOAD is either the caller's `ReadCeiling` or the record's own `RetainedLimits`.
// ---------------------------------------------------------------------------------------------

/// The longest authority reference token an admission may name.
pub const MAX_AUTHORITY_REFERENCE_BYTES: usize = 128;
/// The longest prompt-profile token an admission may name.
pub const MAX_PROMPT_PROFILE_BYTES: usize = 128;
/// The longest adapter-profile token an admission may name.
pub const MAX_ADAPTER_PROFILE_BYTES: usize = 128;
/// The longest activation-binding reference token an admission may name.
pub const MAX_BINDING_REFERENCE_BYTES: usize = 128;
/// The most retained response headers one `headers_observed` record may carry.
pub const MAX_RETAINED_HEADERS: usize = 32;
/// The longest retained header name.
pub const MAX_HEADER_NAME_BYTES: usize = 128;
/// The longest retained header value.
pub const MAX_HEADER_VALUE_BYTES: usize = 1_024;

/// The smallest a retained header can encode to: two `u64` lengths and one byte of name.
const MIN_HEADER_BYTES: usize = 8 + 1 + 8;

const TAG_HEADERS_OBSERVED: u8 = 0x01;
const TAG_BODY_PREFIX_OBSERVED: u8 = 0x02;
const TAG_BODY_COMPLETE: u8 = 0x03;
const TAG_BODY_INCOMPLETE: u8 = 0x04;

const TAG_OWNER_BASIS_OWNER: u8 = 0x01;

const TAG_TRANSPORT_REFUSED: u8 = 0x01;
const TAG_BODY_READ_REFUSED: u8 = 0x02;
const TAG_DEADLINE_ELAPSED: u8 = 0x03;
const TAG_CANCEL_REQUESTED: u8 = 0x04;
const TAG_RESPONSE_OVER_LIMIT: u8 = 0x05;

/// The lowest and highest HTTP status an observation may retain. A status outside this range is a
/// malformed record, never a domain value.
const MIN_HTTP_STATUS: u32 = 100;
const MAX_HTTP_STATUS: u32 = 599;

// ---------------------------------------------------------------------------------------------
// The caller's read ceiling
// ---------------------------------------------------------------------------------------------

/// How much this caller is willing to parse and allocate, supplied by the caller and never by the
/// input.
///
/// There is deliberately no `Default` and no constant: a default would be exactly the invented
/// resource value this slice is not allowed to choose. A caller that does not know its own ceiling
/// does not get to parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadCeiling {
    canonical_bytes: usize,
    retained_field_bytes: usize,
}

impl ReadCeiling {
    /// `canonical_bytes` bounds the whole record before a field is read; `retained_field_bytes`
    /// bounds each single retained byte field before it is allocated.
    pub fn new(canonical_bytes: usize, retained_field_bytes: usize) -> Self {
        Self {
            canonical_bytes,
            retained_field_bytes,
        }
    }

    pub fn canonical_bytes(&self) -> usize {
        self.canonical_bytes
    }

    pub fn retained_field_bytes(&self) -> usize {
        self.retained_field_bytes
    }
}

// ---------------------------------------------------------------------------------------------
// The record's own evidence
// ---------------------------------------------------------------------------------------------

/// The resource envelope the admission RECORDED, as evidence.
///
/// These are not this module's numbers and not an allocation budget. They arrive from whoever
/// resolved the authority, they are retained so a reader can say what envelope was admitted, and
/// they are checked only for coherence with the record that carries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetainedLimits {
    pub request_bytes: u32,
    pub response_bytes: u32,
    pub prefix_bytes: u32,
}

/// A retained reference to a resolved authority record, and the digest the admission CLAIMED for
/// it.
///
/// The bytes behind `reference` are not held here and are never hashed here, so this type verifies
/// nothing about them. It is exactly a retained name and a well-formed claimed digest, both
/// committed into the admission's own digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedAuthority {
    pub reference: String,
    pub claimed_digest: [u8; 32],
}

/// On whose authority the admission was recorded. Closed, and today a singleton.
///
/// This is the basis the record REPORTS, not a grant this module confers. `Owner` says the
/// admission was recorded under the Workspace owner's own authorization; it authorizes nothing by
/// existing, and no production path mints one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerAuthorizationBasis {
    Owner,
}

impl OwnerAuthorizationBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
        }
    }

    fn tag(self) -> u8 {
        match self {
            Self::Owner => TAG_OWNER_BASIS_OWNER,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedInvocation> {
        match tag {
            TAG_OWNER_BASIS_OWNER => Ok(Self::Owner),
            _ => Err(MalformedInvocation::UnknownOwnerBasisTag),
        }
    }
}

/// A retained reference to the exact activation binding, and the digest the admission CLAIMED for
/// it.
///
/// The eventual binding names a provider, endpoint, model name, model digest and inspection digest.
/// **None of those bytes are held here and none is hashed here**, exactly as with
/// [`RetainedAuthority`]: the future verified activation owner must supply those bytes and verify
/// them. `activation_id` beside this field carries the identity; this carries the commitment. It
/// does not verify the activation, and it selects no provider, endpoint or model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedBinding {
    pub reference: String,
    pub claimed_digest: [u8; 32],
}

/// One admitted Mission-plan Invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvocationAdmissionV1 {
    pub invocation_id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub mission_id: Uuid,
    pub resident_id: Uuid,
    /// The configuration the Mission's admission roster named. History, never current eligibility.
    pub admitted_configuration_id: Uuid,
    pub activation_id: Uuid,
    pub authority: RetainedAuthority,
    /// The commitment to the exact activation binding behind `activation_id`.
    pub binding: RetainedBinding,
    /// Which prompt grammar built the prompt. A profile identifies a grammar, never a particular
    /// prompt -- which is why the two fields below exist beside it.
    pub prompt_profile: String,
    /// The digest the admission CLAIMS for the exact prompt bytes. The bytes are not held here and
    /// are not hashed here; a future storage owner must retain them and compare.
    pub claimed_prompt_digest: [u8; 32],
    /// The length the admission CLAIMS for those same prompt bytes.
    pub claimed_prompt_bytes_len: u32,
    /// Which adapter profile serialized the provider request body. Retained because the request
    /// bytes cannot be re-read without knowing what wrote them.
    pub adapter_profile: String,
    /// The digest the admission CLAIMS for the exact serialized provider request body. Separate
    /// from the prompt commitment above: a profile turns one into the other, and this codec asserts
    /// no relationship between their lengths that no adapter profile has specified yet.
    pub claimed_request_digest: [u8; 32],
    /// The length the admission CLAIMS for those same request bytes.
    pub claimed_request_bytes_len: u32,
    pub limits: RetainedLimits,
    pub owner_basis: OwnerAuthorizationBasis,
    pub admitted_at_ms: i64,
}

// ---------------------------------------------------------------------------------------------
// Observations
// ---------------------------------------------------------------------------------------------

/// The four things the local transport can observe. Closed, and closed at every read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationKind {
    Headers,
    BodyPrefix,
    BodyComplete,
    BodyIncomplete,
}

impl ObservationKind {
    pub const ALL: [Self; 4] = [
        Self::Headers,
        Self::BodyPrefix,
        Self::BodyComplete,
        Self::BodyIncomplete,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Headers => "headers_observed",
            Self::BodyPrefix => "body_prefix_observed",
            Self::BodyComplete => "body_complete",
            Self::BodyIncomplete => "body_incomplete",
        }
    }

    /// The letter this kind spells in the transport grammar.
    pub fn letter(self) -> char {
        match self {
            Self::Headers => 'H',
            Self::BodyPrefix => 'P',
            Self::BodyComplete => 'C',
            Self::BodyIncomplete => 'I',
        }
    }

    /// Whether this kind ends the transport observations. `C` and `I` both do, locally.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::BodyComplete | Self::BodyIncomplete)
    }

    fn tag(self) -> u8 {
        match self {
            Self::Headers => TAG_HEADERS_OBSERVED,
            Self::BodyPrefix => TAG_BODY_PREFIX_OBSERVED,
            Self::BodyComplete => TAG_BODY_COMPLETE,
            Self::BodyIncomplete => TAG_BODY_INCOMPLETE,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedInvocation> {
        match tag {
            TAG_HEADERS_OBSERVED => Ok(Self::Headers),
            TAG_BODY_PREFIX_OBSERVED => Ok(Self::BodyPrefix),
            TAG_BODY_COMPLETE => Ok(Self::BodyComplete),
            TAG_BODY_INCOMPLETE => Ok(Self::BodyIncomplete),
            _ => Err(MalformedInvocation::UnknownObservationTag),
        }
    }
}

/// Why a transport attempt ended without a complete body. Closed.
///
/// None of these says the provider stopped. They say what the local adapter observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncompleteCause {
    TransportRefused,
    BodyReadRefused,
    DeadlineElapsed,
    CancelRequested,
    ResponseOverLimit,
}

impl IncompleteCause {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TransportRefused => "transport_refused",
            Self::BodyReadRefused => "body_read_refused",
            Self::DeadlineElapsed => "deadline_elapsed",
            Self::CancelRequested => "cancel_requested",
            Self::ResponseOverLimit => "response_over_limit",
        }
    }

    fn tag(self) -> u8 {
        match self {
            Self::TransportRefused => TAG_TRANSPORT_REFUSED,
            Self::BodyReadRefused => TAG_BODY_READ_REFUSED,
            Self::DeadlineElapsed => TAG_DEADLINE_ELAPSED,
            Self::CancelRequested => TAG_CANCEL_REQUESTED,
            Self::ResponseOverLimit => TAG_RESPONSE_OVER_LIMIT,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedInvocation> {
        match tag {
            TAG_TRANSPORT_REFUSED => Ok(Self::TransportRefused),
            TAG_BODY_READ_REFUSED => Ok(Self::BodyReadRefused),
            TAG_DEADLINE_ELAPSED => Ok(Self::DeadlineElapsed),
            TAG_CANCEL_REQUESTED => Ok(Self::CancelRequested),
            TAG_RESPONSE_OVER_LIMIT => Ok(Self::ResponseOverLimit),
            _ => Err(MalformedInvocation::UnknownCauseTag),
        }
    }
}

/// One retained response header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedHeader {
    pub name: String,
    pub value: String,
}

/// One exact thing the local transport observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportObservation {
    /// The response line was read. An HTTP status is a header fact and says nothing about the body.
    Headers {
        status: u16,
        headers: Vec<RetainedHeader>,
    },
    /// The first nonempty bytes of the body.
    BodyPrefix { bytes: Vec<u8> },
    /// The whole bounded body, which may legitimately be zero bytes long.
    BodyComplete { bytes: Vec<u8>, elapsed_ms: u64 },
    /// The attempt ended locally without a complete body, retaining whatever was read.
    BodyIncomplete {
        prefix: Vec<u8>,
        cause: IncompleteCause,
    },
}

impl TransportObservation {
    pub fn kind(&self) -> ObservationKind {
        match self {
            Self::Headers { .. } => ObservationKind::Headers,
            Self::BodyPrefix { .. } => ObservationKind::BodyPrefix,
            Self::BodyComplete { .. } => ObservationKind::BodyComplete,
            Self::BodyIncomplete { .. } => ObservationKind::BodyIncomplete,
        }
    }
}

/// One observation record, sequenced within its Invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvocationObservationV1 {
    pub invocation_id: Uuid,
    /// Strictly increasing within one Invocation, starting at 1.
    pub sequence: u32,
    pub observation: TransportObservation,
}

// ---------------------------------------------------------------------------------------------
// Faults
// ---------------------------------------------------------------------------------------------

/// Why a canonical record could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MalformedInvocation {
    /// The raw input is longer than the ceiling the CALLER supplied, checked before any field.
    OverReadCeiling,
    /// A retained byte field declares more bytes than the caller's ceiling allows to be allocated.
    OverRetainedFieldCeiling,
    Magic,
    Truncated,
    TrailingBytes,
    /// A name or count exceeds one of this module's identifier-shaped limits.
    OverLimitLength,
    InvalidUtf8,
    UnknownObservationTag,
    UnknownCauseTag,
    UnknownOwnerBasisTag,
    /// A retained length disagrees with the envelope THIS RECORD recorded. A statement about the
    /// record, distinct from any ceiling.
    RetainedExceedsAdmittedLimit,
    /// A `body_prefix_observed` record with no bytes: an absent prefix is the absence of the row.
    EmptyPrefix,
    /// An authority reference or prompt profile with no bytes.
    EmptyRequiredName,
    /// A recorded envelope with a zero bound, which no admission can honestly carry.
    ZeroLimit,
    /// An HTTP status outside 100..=599.
    StatusOutOfRange,
    NegativeTimestamp,
    /// A sequence of zero: the first observation is 1.
    ZeroSequence,
}

impl MalformedInvocation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OverReadCeiling => "over_read_ceiling",
            Self::OverRetainedFieldCeiling => "over_retained_field_ceiling",
            Self::Magic => "magic",
            Self::Truncated => "truncated",
            Self::TrailingBytes => "trailing_bytes",
            Self::OverLimitLength => "over_limit_length",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::UnknownObservationTag => "unknown_observation_tag",
            Self::UnknownCauseTag => "unknown_cause_tag",
            Self::UnknownOwnerBasisTag => "unknown_owner_basis_tag",
            Self::RetainedExceedsAdmittedLimit => "retained_exceeds_admitted_limit",
            Self::EmptyPrefix => "empty_prefix",
            Self::EmptyRequiredName => "empty_required_name",
            Self::ZeroLimit => "zero_limit",
            Self::StatusOutOfRange => "status_out_of_range",
            Self::NegativeTimestamp => "negative_timestamp",
            Self::ZeroSequence => "zero_sequence",
        }
    }
}

impl fmt::Display for MalformedInvocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for MalformedInvocation {}

/// Why one observation cannot follow the ones already recorded.
///
/// This is transition validation over the grammar, and it is what refuses a crossing terminal — not
/// a uniqueness collision on an identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionRefusal {
    /// A prefix with no headers before it.
    PrefixWithoutHeaders,
    /// A complete body with no headers before it.
    CompleteWithoutHeaders,
    /// This kind is already recorded for this Invocation.
    RepeatedKind(ObservationKind),
    /// The transport observations are already ended by a terminal.
    AfterTerminal {
        terminal: ObservationKind,
        offered: ObservationKind,
    },
}

impl TransitionRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrefixWithoutHeaders => "prefix_without_headers",
            Self::CompleteWithoutHeaders => "complete_without_headers",
            Self::RepeatedKind(_) => "repeated_kind",
            Self::AfterTerminal { .. } => "after_terminal",
        }
    }
}

impl fmt::Display for TransitionRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why an offered sequence is not in the domain the record documents: first is 1, later strictly
/// increase.
///
/// This is the documented domain and nothing more. There is no contiguity requirement here, because
/// no contract in this repository states one; a gap is legal until something says otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceFault {
    FirstIsNotOne {
        offered: u32,
    },
    NotIncreasing {
        previous: u32,
        offered: u32,
    },
    /// The convenience path was asked for the next sequence after the largest one `u32` can spell.
    /// There is no next; wrapping would re-use a sequence this history has already passed.
    NoRepresentableNext {
        previous: u32,
    },
}

impl SequenceFault {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FirstIsNotOne { .. } => "first_is_not_one",
            Self::NotIncreasing { .. } => "not_increasing",
            Self::NoRepresentableNext { .. } => "no_representable_next",
        }
    }
}

impl fmt::Display for SequenceFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why a body terminal disagrees with the prefix already observed for the same response.
///
/// A recorded `body_prefix_observed` is the FIRST bytes of that body. A later terminal that does
/// not carry those exact bytes is not a second reading of the same response — it is two records
/// claiming different things about one stream, and the later one must not be allowed to conceal
/// the earlier by being the one `status()` renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByteCoherenceFault {
    CompleteDoesNotExtendPrefix {
        observed_prefix_len: usize,
        offered_len: usize,
    },
    IncompleteDiscardsPrefix {
        observed_prefix_len: usize,
        offered_len: usize,
    },
}

impl ByteCoherenceFault {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompleteDoesNotExtendPrefix { .. } => "complete_does_not_extend_prefix",
            Self::IncompleteDiscardsPrefix { .. } => "incomplete_discards_prefix",
        }
    }
}

impl fmt::Display for ByteCoherenceFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why an append was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppendRefusal {
    /// The observation names a different Invocation.
    ForeignInvocation {
        expected: Uuid,
        offered: Uuid,
    },
    /// The same kind is already recorded with different bytes.
    IdentityConflict {
        kind: ObservationKind,
        recorded_digest: [u8; 32],
        offered_digest: [u8; 32],
    },
    Transition(TransitionRefusal),
    /// The offered sequence is outside the documented domain.
    Sequence(SequenceFault),
    /// The offered terminal contradicts the prefix already observed.
    ByteCoherence(ByteCoherenceFault),
    Malformed(MalformedInvocation),
}

impl fmt::Display for AppendRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForeignInvocation { .. } => formatter.write_str("foreign_invocation"),
            Self::IdentityConflict { .. } => formatter.write_str("observation_conflict"),
            Self::Transition(refusal) => write!(formatter, "transition:{refusal}"),
            Self::Sequence(fault) => write!(formatter, "sequence:{fault}"),
            Self::ByteCoherence(fault) => write!(formatter, "byte_coherence:{fault}"),
            Self::Malformed(fault) => write!(formatter, "malformed:{fault}"),
        }
    }
}

impl std::error::Error for AppendRefusal {}

// ---------------------------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------------------------

struct Encoder {
    out: Vec<u8>,
}

impl Encoder {
    fn new(magic: &[u8]) -> Self {
        Self {
            out: magic.to_vec(),
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

    fn i64(&mut self, value: i64) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    fn uuid(&mut self, value: &Uuid) {
        self.out.extend_from_slice(value.as_bytes());
    }

    fn sha256(&mut self, value: &[u8; 32]) {
        self.out.extend_from_slice(value);
    }

    fn string(&mut self, value: &str, limit: usize) -> Result<(), MalformedInvocation> {
        self.bytes(value.as_bytes(), limit)
    }

    fn bytes(&mut self, value: &[u8], limit: usize) -> Result<(), MalformedInvocation> {
        if value.len() > limit {
            return Err(MalformedInvocation::OverLimitLength);
        }
        let length =
            u64::try_from(value.len()).map_err(|_| MalformedInvocation::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value);
        Ok(())
    }

    fn count(&mut self, length: usize, limit: usize) -> Result<(), MalformedInvocation> {
        if length > limit {
            return Err(MalformedInvocation::OverLimitLength);
        }
        let count = u32::try_from(length).map_err(|_| MalformedInvocation::OverLimitLength)?;
        self.u32(count);
        Ok(())
    }
}

/// Reads canonical bytes. Self-delimiting: every field proves it fits within the remaining input
/// and within the CALLER's ceiling before any of it is consumed or copied.
struct Decoder<'a> {
    input: &'a [u8],
    offset: usize,
    ceiling: ReadCeiling,
}

impl<'a> Decoder<'a> {
    /// The raw length check happens HERE, before a single field is read — which is why this
    /// returns a `Result` rather than a `Decoder`.
    fn new(input: &'a [u8], ceiling: ReadCeiling) -> Result<Self, MalformedInvocation> {
        if input.len() > ceiling.canonical_bytes {
            return Err(MalformedInvocation::OverReadCeiling);
        }
        Ok(Self {
            input,
            offset: 0,
            ceiling,
        })
    }

    fn remaining(&self) -> usize {
        self.input.len() - self.offset
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], MalformedInvocation> {
        if count > self.remaining() {
            return Err(MalformedInvocation::Truncated);
        }
        let slice = &self.input[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn magic(&mut self, expected: &[u8]) -> Result<(), MalformedInvocation> {
        let magic = self
            .take(expected.len())
            .map_err(|_| MalformedInvocation::Magic)?;
        if magic == expected {
            Ok(())
        } else {
            Err(MalformedInvocation::Magic)
        }
    }

    fn tag(&mut self) -> Result<u8, MalformedInvocation> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, MalformedInvocation> {
        let mut value = [0_u8; 4];
        value.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(value))
    }

    fn u64(&mut self) -> Result<u64, MalformedInvocation> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }

    fn i64(&mut self) -> Result<i64, MalformedInvocation> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(i64::from_be_bytes(value))
    }

    fn uuid(&mut self) -> Result<Uuid, MalformedInvocation> {
        let mut value = [0_u8; 16];
        value.copy_from_slice(self.take(16)?);
        Ok(Uuid::from_bytes(value))
    }

    fn sha256(&mut self) -> Result<[u8; 32], MalformedInvocation> {
        let mut value = [0_u8; 32];
        value.copy_from_slice(self.take(32)?);
        Ok(value)
    }

    /// A name, bounded by one of this module's identifier limits.
    fn string(&mut self, limit: usize) -> Result<String, MalformedInvocation> {
        let bytes = self.length_prefixed(limit, MalformedInvocation::OverLimitLength)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| MalformedInvocation::InvalidUtf8)
    }

    /// A retained payload, bounded by the CALLER's ceiling — never by anything the input carries.
    fn retained_bytes(&mut self) -> Result<Vec<u8>, MalformedInvocation> {
        let bytes = self.length_prefixed(
            self.ceiling.retained_field_bytes,
            MalformedInvocation::OverRetainedFieldCeiling,
        )?;
        Ok(bytes.to_vec())
    }

    /// Reads a declared length and proves it fits both the bound and the remaining input, before a
    /// byte of it is copied anywhere.
    fn length_prefixed(
        &mut self,
        limit: usize,
        over: MalformedInvocation,
    ) -> Result<&'a [u8], MalformedInvocation> {
        let length = self.u64()?;
        let limit_u64 = u64::try_from(limit).map_err(|_| over)?;
        if length > limit_u64 {
            return Err(over);
        }
        let length = usize::try_from(length).map_err(|_| MalformedInvocation::Truncated)?;
        self.take(length)
    }

    /// Reads a declared entry count and proves the input could hold that many entries, before the
    /// first entry is read and before any `Vec` reserves for it.
    fn count(&mut self, limit: usize, entry_bytes: usize) -> Result<usize, MalformedInvocation> {
        let count =
            usize::try_from(self.u32()?).map_err(|_| MalformedInvocation::OverLimitLength)?;
        if count > limit {
            return Err(MalformedInvocation::OverLimitLength);
        }
        let smallest = count
            .checked_mul(entry_bytes)
            .ok_or(MalformedInvocation::Truncated)?;
        if smallest > self.remaining() {
            return Err(MalformedInvocation::Truncated);
        }
        Ok(count)
    }

    fn finish(self) -> Result<(), MalformedInvocation> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MalformedInvocation::TrailingBytes)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The admission record's bytes
// ---------------------------------------------------------------------------------------------

fn require_limits(limits: &RetainedLimits) -> Result<(), MalformedInvocation> {
    if limits.request_bytes == 0 || limits.response_bytes == 0 || limits.prefix_bytes == 0 {
        return Err(MalformedInvocation::ZeroLimit);
    }
    Ok(())
}

impl InvocationAdmissionV1 {
    /// This record's canonical bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MalformedInvocation> {
        if self.authority.reference.is_empty()
            || self.binding.reference.is_empty()
            || self.prompt_profile.is_empty()
            || self.adapter_profile.is_empty()
        {
            return Err(MalformedInvocation::EmptyRequiredName);
        }
        require_limits(&self.limits)?;
        // The REQUEST length is the one the request envelope bounds. The prompt length is
        // deliberately not compared to it: what an adapter profile does to a prompt on the way to a
        // request body is that profile's business, and no profile has specified it yet.
        if self.claimed_request_bytes_len > self.limits.request_bytes {
            return Err(MalformedInvocation::RetainedExceedsAdmittedLimit);
        }
        if self.admitted_at_ms < 0 {
            return Err(MalformedInvocation::NegativeTimestamp);
        }

        let mut encoder = Encoder::new(MAGIC_ADMISSION);
        encoder.uuid(&self.invocation_id);
        encoder.uuid(&self.workspace_id);
        encoder.uuid(&self.project_id);
        encoder.uuid(&self.mission_id);
        encoder.uuid(&self.resident_id);
        encoder.uuid(&self.admitted_configuration_id);
        encoder.uuid(&self.activation_id);
        encoder.string(&self.authority.reference, MAX_AUTHORITY_REFERENCE_BYTES)?;
        encoder.sha256(&self.authority.claimed_digest);
        encoder.string(&self.binding.reference, MAX_BINDING_REFERENCE_BYTES)?;
        encoder.sha256(&self.binding.claimed_digest);
        encoder.string(&self.prompt_profile, MAX_PROMPT_PROFILE_BYTES)?;
        encoder.sha256(&self.claimed_prompt_digest);
        encoder.u32(self.claimed_prompt_bytes_len);
        encoder.string(&self.adapter_profile, MAX_ADAPTER_PROFILE_BYTES)?;
        encoder.sha256(&self.claimed_request_digest);
        encoder.u32(self.claimed_request_bytes_len);
        encoder.u32(self.limits.request_bytes);
        encoder.u32(self.limits.response_bytes);
        encoder.u32(self.limits.prefix_bytes);
        encoder.tag(self.owner_basis.tag());
        encoder.i64(self.admitted_at_ms);
        Ok(encoder.out)
    }

    /// This record's digest identity.
    pub fn digest(&self) -> Result<[u8; 32], MalformedInvocation> {
        Ok(sha256(&self.canonical_bytes()?))
    }

    /// Read one admission record, bounded by the caller's ceiling.
    pub fn from_canonical_bytes(
        input: &[u8],
        ceiling: ReadCeiling,
    ) -> Result<Self, MalformedInvocation> {
        let mut decoder = Decoder::new(input, ceiling)?;
        decoder.magic(MAGIC_ADMISSION)?;
        let invocation_id = decoder.uuid()?;
        let workspace_id = decoder.uuid()?;
        let project_id = decoder.uuid()?;
        let mission_id = decoder.uuid()?;
        let resident_id = decoder.uuid()?;
        let admitted_configuration_id = decoder.uuid()?;
        let activation_id = decoder.uuid()?;
        let reference = decoder.string(MAX_AUTHORITY_REFERENCE_BYTES)?;
        let claimed_digest = decoder.sha256()?;
        let binding_reference = decoder.string(MAX_BINDING_REFERENCE_BYTES)?;
        let binding_claimed_digest = decoder.sha256()?;
        let prompt_profile = decoder.string(MAX_PROMPT_PROFILE_BYTES)?;
        let claimed_prompt_digest = decoder.sha256()?;
        let claimed_prompt_bytes_len = decoder.u32()?;
        let adapter_profile = decoder.string(MAX_ADAPTER_PROFILE_BYTES)?;
        let claimed_request_digest = decoder.sha256()?;
        let claimed_request_bytes_len = decoder.u32()?;
        let limits = RetainedLimits {
            request_bytes: decoder.u32()?,
            response_bytes: decoder.u32()?,
            prefix_bytes: decoder.u32()?,
        };
        // Closed BEFORE the timestamp it precedes: an unknown basis is named as itself rather than
        // renamed by whatever the following field turns out to be.
        let owner_basis = OwnerAuthorizationBasis::parse(decoder.tag()?)?;
        let admitted_at_ms = decoder.i64()?;
        decoder.finish()?;

        if reference.is_empty()
            || binding_reference.is_empty()
            || prompt_profile.is_empty()
            || adapter_profile.is_empty()
        {
            return Err(MalformedInvocation::EmptyRequiredName);
        }
        require_limits(&limits)?;
        if claimed_request_bytes_len > limits.request_bytes {
            return Err(MalformedInvocation::RetainedExceedsAdmittedLimit);
        }
        if admitted_at_ms < 0 {
            return Err(MalformedInvocation::NegativeTimestamp);
        }

        Ok(Self {
            invocation_id,
            workspace_id,
            project_id,
            mission_id,
            resident_id,
            admitted_configuration_id,
            activation_id,
            authority: RetainedAuthority {
                reference,
                claimed_digest,
            },
            binding: RetainedBinding {
                reference: binding_reference,
                claimed_digest: binding_claimed_digest,
            },
            prompt_profile,
            claimed_prompt_digest,
            claimed_prompt_bytes_len,
            adapter_profile,
            claimed_request_digest,
            claimed_request_bytes_len,
            limits,
            owner_basis,
            admitted_at_ms,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The observation record's bytes
// ---------------------------------------------------------------------------------------------

/// Check one observation against the envelope THIS ADMISSION recorded.
///
/// Coherence with the record, not a bound on this process: the ceiling did the bounding already.
fn require_within_admitted_limits(
    observation: &TransportObservation,
    limits: &RetainedLimits,
) -> Result<(), MalformedInvocation> {
    let (length, allowed) = match observation {
        TransportObservation::Headers { .. } => return Ok(()),
        TransportObservation::BodyPrefix { bytes } => (bytes.len(), limits.prefix_bytes),
        TransportObservation::BodyComplete { bytes, .. } => (bytes.len(), limits.response_bytes),
        // A partially read body is bounded by the response envelope, not by the first-prefix bound:
        // `response_over_limit` retains what was read before the stop, which is past the prefix.
        TransportObservation::BodyIncomplete { prefix, .. } => {
            (prefix.len(), limits.response_bytes)
        }
    };
    let allowed = usize::try_from(allowed).map_err(|_| MalformedInvocation::OverLimitLength)?;
    if length > allowed {
        return Err(MalformedInvocation::RetainedExceedsAdmittedLimit);
    }
    Ok(())
}

impl InvocationObservationV1 {
    pub fn kind(&self) -> ObservationKind {
        self.observation.kind()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MalformedInvocation> {
        if self.sequence == 0 {
            return Err(MalformedInvocation::ZeroSequence);
        }
        let mut encoder = Encoder::new(MAGIC_OBSERVATION);
        encoder.uuid(&self.invocation_id);
        encoder.u32(self.sequence);
        encoder.tag(self.kind().tag());
        match &self.observation {
            TransportObservation::Headers { status, headers } => {
                let status = u32::from(*status);
                if !(MIN_HTTP_STATUS..=MAX_HTTP_STATUS).contains(&status) {
                    return Err(MalformedInvocation::StatusOutOfRange);
                }
                encoder.u32(status);
                encoder.count(headers.len(), MAX_RETAINED_HEADERS)?;
                for header in headers {
                    if header.name.is_empty() {
                        return Err(MalformedInvocation::EmptyRequiredName);
                    }
                    encoder.string(&header.name, MAX_HEADER_NAME_BYTES)?;
                    encoder.string(&header.value, MAX_HEADER_VALUE_BYTES)?;
                }
            }
            TransportObservation::BodyPrefix { bytes } => {
                if bytes.is_empty() {
                    return Err(MalformedInvocation::EmptyPrefix);
                }
                encoder.bytes(bytes, usize::MAX)?;
            }
            TransportObservation::BodyComplete { bytes, elapsed_ms } => {
                encoder.bytes(bytes, usize::MAX)?;
                encoder.u64(*elapsed_ms);
            }
            TransportObservation::BodyIncomplete { prefix, cause } => {
                encoder.bytes(prefix, usize::MAX)?;
                encoder.tag(cause.tag());
            }
        }
        Ok(encoder.out)
    }

    pub fn digest(&self) -> Result<[u8; 32], MalformedInvocation> {
        Ok(sha256(&self.canonical_bytes()?))
    }

    /// Read one observation record, bounded by the caller's ceiling.
    pub fn from_canonical_bytes(
        input: &[u8],
        ceiling: ReadCeiling,
    ) -> Result<Self, MalformedInvocation> {
        let mut decoder = Decoder::new(input, ceiling)?;
        decoder.magic(MAGIC_OBSERVATION)?;
        let invocation_id = decoder.uuid()?;
        let sequence = decoder.u32()?;
        if sequence == 0 {
            return Err(MalformedInvocation::ZeroSequence);
        }
        // The kind tag is closed HERE, before any field it governs is read: an unknown tag is
        // named before a later over-limit field can rename the fault.
        let kind = ObservationKind::parse(decoder.tag()?)?;
        let observation = match kind {
            ObservationKind::Headers => {
                let status = decoder.u32()?;
                if !(MIN_HTTP_STATUS..=MAX_HTTP_STATUS).contains(&status) {
                    return Err(MalformedInvocation::StatusOutOfRange);
                }
                let count = decoder.count(MAX_RETAINED_HEADERS, MIN_HEADER_BYTES)?;
                let mut headers = Vec::with_capacity(count);
                for _ in 0..count {
                    let name = decoder.string(MAX_HEADER_NAME_BYTES)?;
                    if name.is_empty() {
                        return Err(MalformedInvocation::EmptyRequiredName);
                    }
                    let value = decoder.string(MAX_HEADER_VALUE_BYTES)?;
                    headers.push(RetainedHeader { name, value });
                }
                TransportObservation::Headers {
                    status: u16::try_from(status)
                        .map_err(|_| MalformedInvocation::StatusOutOfRange)?,
                    headers,
                }
            }
            ObservationKind::BodyPrefix => {
                let bytes = decoder.retained_bytes()?;
                if bytes.is_empty() {
                    return Err(MalformedInvocation::EmptyPrefix);
                }
                TransportObservation::BodyPrefix { bytes }
            }
            ObservationKind::BodyComplete => {
                let bytes = decoder.retained_bytes()?;
                let elapsed_ms = decoder.u64()?;
                TransportObservation::BodyComplete { bytes, elapsed_ms }
            }
            ObservationKind::BodyIncomplete => {
                let prefix = decoder.retained_bytes()?;
                let cause = IncompleteCause::parse(decoder.tag()?)?;
                TransportObservation::BodyIncomplete { prefix, cause }
            }
        };
        decoder.finish()?;
        Ok(Self {
            invocation_id,
            sequence,
            observation,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The finite transport grammar
// ---------------------------------------------------------------------------------------------

/// What one Invocation's transport observations say, reduced from the rows in hand.
///
/// This is a statement about the rows it was given. It is NOT a durability, ownership, restart or
/// pending claim: those need the store and runtime seams, which this slice does not build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportStatus {
    /// Admitted, nothing observed. Not "not sent".
    PotentiallyAttempted,
    /// Headers observed and no body terminal. Never rendered as "no response".
    ResponseStarted { status: u16 },
    /// Headers and an exact first prefix, and no terminal.
    ResponsePartial { status: u16, prefix: Vec<u8> },
    /// A complete response of zero bytes. Distinct from both of the above.
    CompleteEmpty { status: u16 },
    /// A complete response of exactly these bytes.
    CompleteRetained { status: u16, bytes: Vec<u8> },
    /// The attempt ended locally. `status` is absent when the terminal preceded any headers.
    Incomplete {
        status: Option<u16>,
        cause: IncompleteCause,
        prefix: Vec<u8>,
    },
}

/// What an append did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Appended {
    /// A new row, at this sequence.
    Recorded { sequence: u32 },
    /// The identical record is already present: idempotent, and not a new row.
    AlreadyPresent { sequence: u32 },
}

/// One Invocation's admitted transport observations, in the order they were recorded.
///
/// The history owns the grammar: `A · H? · P? · (C|I)?`, with `P` and `C` requiring `H`, `I`
/// permitted as the whole transport, each kind at most once, and a terminal ending the
/// observations. Materialization is not part of this slice, so at most three rows exist.
#[derive(Clone, Debug)]
pub struct TransportHistory {
    invocation_id: Uuid,
    limits: RetainedLimits,
    rows: Vec<(InvocationObservationV1, [u8; 32])>,
}

impl TransportHistory {
    /// An admitted Invocation with nothing observed yet.
    pub fn admitted(admission: &InvocationAdmissionV1) -> Self {
        Self {
            invocation_id: admission.invocation_id,
            limits: admission.limits,
            rows: Vec::new(),
        }
    }

    pub fn rows(&self) -> impl Iterator<Item = &InvocationObservationV1> {
        self.rows.iter().map(|(row, _)| row)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The word this history spells, in the grammar's own letters.
    pub fn word(&self) -> String {
        self.rows
            .iter()
            .map(|(row, _)| row.kind().letter())
            .collect()
    }

    fn recorded(&self, kind: ObservationKind) -> Option<&(InvocationObservationV1, [u8; 32])> {
        self.rows.iter().find(|(row, _)| row.kind() == kind)
    }

    /// The sequence of the most recently recorded row. Rows are appended in strictly increasing
    /// sequence, so the last one carries the maximum.
    fn last_sequence(&self) -> Option<u32> {
        self.rows.last().map(|(row, _)| row.sequence)
    }

    /// The exact bytes of a recorded first prefix, if one was observed.
    fn observed_prefix(&self) -> Option<&[u8]> {
        self.recorded(ObservationKind::BodyPrefix)
            .and_then(|(row, _)| match &row.observation {
                TransportObservation::BodyPrefix { bytes } => Some(bytes.as_slice()),
                _ => None,
            })
    }

    /// Whether one more observation's bytes agree with the prefix already observed.
    fn byte_coherence(&self, observation: &TransportObservation) -> Result<(), ByteCoherenceFault> {
        let Some(prefix) = self.observed_prefix() else {
            return Ok(());
        };
        match observation {
            TransportObservation::BodyComplete { bytes, .. } if !bytes.starts_with(prefix) => {
                Err(ByteCoherenceFault::CompleteDoesNotExtendPrefix {
                    observed_prefix_len: prefix.len(),
                    offered_len: bytes.len(),
                })
            }
            TransportObservation::BodyIncomplete {
                prefix: retained, ..
            } if !retained.starts_with(prefix) => {
                Err(ByteCoherenceFault::IncompleteDiscardsPrefix {
                    observed_prefix_len: prefix.len(),
                    offered_len: retained.len(),
                })
            }
            _ => Ok(()),
        }
    }

    /// Whether an offered sequence is in the documented domain, given what is recorded.
    fn sequence_domain(&self, offered: u32) -> Result<(), SequenceFault> {
        match self.last_sequence() {
            None if offered != 1 => Err(SequenceFault::FirstIsNotOne { offered }),
            Some(previous) if offered <= previous => {
                Err(SequenceFault::NotIncreasing { previous, offered })
            }
            _ => Ok(()),
        }
    }

    fn terminal(&self) -> Option<ObservationKind> {
        self.rows
            .iter()
            .map(|(row, _)| row.kind())
            .find(|kind| kind.is_terminal())
    }

    /// Whether one more observation of this kind could legally follow what is recorded.
    ///
    /// Pure transition validation over the kinds alone. A crossing terminal is refused HERE, by
    /// name, and never left to collide on an identity.
    pub fn transition(&self, offered: ObservationKind) -> Result<(), TransitionRefusal> {
        if let Some(terminal) = self.terminal() {
            return Err(TransitionRefusal::AfterTerminal { terminal, offered });
        }
        if self.recorded(offered).is_some() {
            return Err(TransitionRefusal::RepeatedKind(offered));
        }
        let headers = self.recorded(ObservationKind::Headers).is_some();
        match offered {
            ObservationKind::Headers | ObservationKind::BodyIncomplete => Ok(()),
            ObservationKind::BodyPrefix if !headers => Err(TransitionRefusal::PrefixWithoutHeaders),
            ObservationKind::BodyComplete if !headers => {
                Err(TransitionRefusal::CompleteWithoutHeaders)
            }
            _ => Ok(()),
        }
    }

    /// Append one observation.
    ///
    /// Re-appending the identical record is idempotent and adds no row. The same kind with
    /// different bytes is a conflict, named as one.
    pub fn append(&mut self, observation: TransportObservation) -> Result<Appended, AppendRefusal> {
        // The LAST recorded sequence plus one, not the row count: sequences may legally skip, so a
        // count would eventually offer a sequence that has already been passed.
        let previous = self.last_sequence().unwrap_or(0);
        let sequence = previous.checked_add(1).ok_or(AppendRefusal::Sequence(
            SequenceFault::NoRepresentableNext { previous },
        ))?;
        self.append_record(InvocationObservationV1 {
            invocation_id: self.invocation_id,
            sequence,
            observation,
        })
    }

    /// Append one already-sequenced record.
    pub fn append_record(
        &mut self,
        record: InvocationObservationV1,
    ) -> Result<Appended, AppendRefusal> {
        if record.invocation_id != self.invocation_id {
            return Err(AppendRefusal::ForeignInvocation {
                expected: self.invocation_id,
                offered: record.invocation_id,
            });
        }
        require_within_admitted_limits(&record.observation, &self.limits)
            .map_err(AppendRefusal::Malformed)?;
        let offered_digest = record.digest().map_err(AppendRefusal::Malformed)?;

        // Identity is (kind, sequence), so this arm answers only "is this the SAME record again?".
        // A repeated kind at a NEW sequence is a grammar violation, not a byte disagreement, and it
        // falls through to transition validation below so it is refused by the right name.
        let kind = record.kind();
        let same_identity = self
            .rows
            .iter()
            .find(|(row, _)| row.kind() == kind && row.sequence == record.sequence);
        if let Some((recorded, recorded_digest)) = same_identity {
            if *recorded_digest == offered_digest {
                return Ok(Appended::AlreadyPresent {
                    sequence: recorded.sequence,
                });
            }
            return Err(AppendRefusal::IdentityConflict {
                kind,
                recorded_digest: *recorded_digest,
                offered_digest,
            });
        }

        // Order: the grammar first, then the bytes, then the sequence domain. A crossing terminal
        // is a grammar fault whatever its bytes say, and a terminal that contradicts the observed
        // prefix is a byte fault whatever its sequence says.
        self.transition(kind).map_err(AppendRefusal::Transition)?;
        self.byte_coherence(&record.observation)
            .map_err(AppendRefusal::ByteCoherence)?;
        self.sequence_domain(record.sequence)
            .map_err(AppendRefusal::Sequence)?;
        let sequence = record.sequence;
        self.rows.push((record, offered_digest));
        Ok(Appended::Recorded { sequence })
    }

    /// Reduce the recorded rows to one status.
    ///
    /// Every arm comes from a row that is present. Nothing here invents a terminal, and nothing
    /// here reads a stored state flag, because there is none.
    pub fn status(&self) -> TransportStatus {
        let status = self
            .recorded(ObservationKind::Headers)
            .and_then(|(row, _)| match &row.observation {
                TransportObservation::Headers { status, .. } => Some(*status),
                _ => None,
            });
        let prefix = self
            .recorded(ObservationKind::BodyPrefix)
            .and_then(|(row, _)| match &row.observation {
                TransportObservation::BodyPrefix { bytes } => Some(bytes.clone()),
                _ => None,
            });

        if let Some((row, _)) = self.recorded(ObservationKind::BodyIncomplete)
            && let TransportObservation::BodyIncomplete { prefix, cause } = &row.observation
        {
            return TransportStatus::Incomplete {
                status,
                cause: *cause,
                prefix: prefix.clone(),
            };
        }
        // The status is REQUIRED here rather than defaulted: transition validation refuses a
        // complete body with no headers, so a `None` would mean the history is not what this
        // grammar admits -- and inventing a status 0 would be this record answering a question it
        // was never told the answer to.
        if let Some((row, _)) = self.recorded(ObservationKind::BodyComplete)
            && let TransportObservation::BodyComplete { bytes, .. } = &row.observation
            && let Some(status) = status
        {
            if bytes.is_empty() {
                return TransportStatus::CompleteEmpty { status };
            }
            return TransportStatus::CompleteRetained {
                status,
                bytes: bytes.clone(),
            };
        }
        match (status, prefix) {
            (Some(status), Some(prefix)) => TransportStatus::ResponsePartial { status, prefix },
            (Some(status), None) => TransportStatus::ResponseStarted { status },
            _ => TransportStatus::PotentiallyAttempted,
        }
    }
}
