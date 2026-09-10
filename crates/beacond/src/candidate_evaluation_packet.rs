//! The frozen Outcome-fidelity input packet: `beacon.outcome-fidelity.input.v1`.
//!
//! This module owns the byte table frozen at `B16-opus2-packet-byte-table.md` round 3
//! (SHA-256 `cf5cbf92…4d59`), under rulings R59, R63 and R68. It encodes; it does not read the
//! store, admit anything, or grant any authority.
//!
//! **Identity contract (R68/H2, table §A.0).** The packet is collected exactly ONCE, inside the
//! admission transaction, and retained as the attempt's `input_bytes`. Its digest is the ATTEMPT
//! identity, frozen at admission-time facts: the header's current-evaluator rows are deliberately
//! part of what the request binds. A retry or a read RE-USES the retained packet and never
//! re-collects — a later read that recomputed the header would produce a different digest for one
//! attempt. Idempotency does not rest on this digest: it rests on [`RequestKey`], a separate tuple
//! carrying no moving fact.
//!
//! **Framing (R59.2).** Frames exist at the section and evidence-entry level only
//! (`tag:u8 + u64be length`); fields inside a section are POSITIONAL. A duplicated or reordered
//! field is therefore unrepresentable rather than merely refused. No packet tag byte is `0x00`, so
//! a zero-filled buffer decodes as no valid section, entry or arm. (That is this packet's own
//! rule, not a house convention: `outcome.rs:146-147` uses `0x00` for boolean false.)

use data_encoding::HEXLOWER;
use uuid::Uuid;

use crate::evaluation::Evaluator;
use crate::outcome::SourceDigest;
use crate::proposal::sha256;
use crate::{
    BeaconCapability, LocalModelActivationState, ResidentConfigurationState, ResidentLifecycle,
};

/// The canonicalizer identity. Any future grammar is a new canonicalizer; these bytes are never
/// reinterpreted under it (ADR 0034's rule, kept).
pub const CANONICALIZER: &str = "beacon.outcome-fidelity.input.v1";
/// The literal ASCII magic that starts every packet, including its terminating NUL: 33 bytes.
const MAGIC: &[u8] = b"beacon.outcome-fidelity.input.v1\0";
const DIGEST_ALGORITHM: &str = "sha256";
/// The rubric's content canonicalizer, pinned (`candidate_evaluation.rs:368-373`).
const RAW_CANONICALIZER: &str = "beacon.raw.v1";

/// Bounds a declared token length before any of it is read (`outcome.rs:74`'s rule, kept).
const MAX_TOKEN_BYTES: usize = 64;
/// The citation bound the Outcome function already owns (`outcome.rs:66`).
const MAX_ENTRIES: u32 = 256;
/// Owner ceilings, each cited in the table's §A.2/§A.4 and repeated here because this grammar owns
/// its own bounds; a bound borrowed from another module drifts when that module changes.
const MAX_ARTIFACT_RECORD_BYTES: usize = 4 * 1024;
const MAX_ARTIFACT_CONTENT_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_OWNER_RECORD_BYTES: usize = 65_536;
const MAX_RECEIPT_JSON_BYTES: usize = 65_536;
const MAX_MISSION_TEXT_BYTES: usize = 65_536;
const MAX_MISSION_ITEMS: u32 = 64;
const MAX_RUBRIC_BYTES: usize = 65_536;
const MAX_CAPABILITIES: u32 = 7;
const MAX_GAPS: u32 = 6;

// Section tags. Assigned explicitly, never renumbered, and none is `0x00`.
const SECTION_HEADER: u8 = 0x01;
const SECTION_SUBJECT: u8 = 0x02;
const SECTION_RECIPE: u8 = 0x03;
const SECTION_EVIDENCE: u8 = 0x04;
const SECTION_RUBRIC: u8 = 0x05;

// Activation presence. Explicit, never by omission, and neither arm is `0x00`
// (`outcome.rs:133-135`'s idiom).
const TAG_ACTIVATION_NONE: u8 = 0x01;
const TAG_ACTIVATION_SOME: u8 = 0x02;

/// One frame's header: a tag byte and a big-endian length.
const FRAME_HEADER_BYTES: u64 = 1 + 8;

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

/// Why bytes are not a packet. Mirrors `MalformedArtifact`/`MalformedInvocation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedFidelityPacket {
    Magic,
    Truncated,
    TrailingBytes,
    UnknownTag,
    OverLimitLength,
    InvalidUtf8,
    DigestEnvelope,
    DigestValue,
    SectionOrder,
    SectionLengthMismatch,
    EntryCount,
    EntryOrder,
    DuplicateEntry,
    CanonicalizerNotAdmitted,
    ReceiptVersion,
}

impl MalformedFidelityPacket {
    pub fn name(self) -> &'static str {
        match self {
            Self::Magic => "Magic",
            Self::Truncated => "Truncated",
            Self::TrailingBytes => "TrailingBytes",
            Self::UnknownTag => "UnknownTag",
            Self::OverLimitLength => "OverLimitLength",
            Self::InvalidUtf8 => "InvalidUtf8",
            Self::DigestEnvelope => "DigestEnvelope",
            Self::DigestValue => "DigestValue",
            Self::SectionOrder => "SectionOrder",
            Self::SectionLengthMismatch => "SectionLengthMismatch",
            Self::EntryCount => "EntryCount",
            Self::EntryOrder => "EntryOrder",
            Self::DuplicateEntry => "DuplicateEntry",
            Self::CanonicalizerNotAdmitted => "CanonicalizerNotAdmitted",
            Self::ReceiptVersion => "ReceiptVersion",
        }
    }
}

/// Why a packet was not collected. Raised before any packet byte is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FidelityPacketRefusal {
    Malformed(MalformedFidelityPacket),
    OwnerRefused {
        kind: &'static str,
    },
    EssentialSourceAbsent {
        kind: &'static str,
    },
    SourceOverOwnerCeiling {
        kind: &'static str,
    },
    /// R59.4: the refusal names charged AND cap.
    AggregateOverCap {
        cap: u64,
        charged: u64,
    },
    ReceiptOwnerUnregistered {
        run: Uuid,
    },
    MissionAbsent,
    ScopeMismatch,
}

impl FidelityPacketRefusal {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "Malformed",
            Self::OwnerRefused { .. } => "OwnerRefused",
            Self::EssentialSourceAbsent { .. } => "EssentialSourceAbsent",
            Self::SourceOverOwnerCeiling { .. } => "SourceOverOwnerCeiling",
            Self::AggregateOverCap { .. } => "AggregateOverCap",
            Self::ReceiptOwnerUnregistered { .. } => "ReceiptOwnerUnregistered",
            Self::MissionAbsent => "MissionAbsent",
            Self::ScopeMismatch => "ScopeMismatch",
        }
    }
}

impl From<MalformedFidelityPacket> for FidelityPacketRefusal {
    fn from(error: MalformedFidelityPacket) -> Self {
        Self::Malformed(error)
    }
}

// ---------------------------------------------------------------------------------------------
// The aggregate cap: a caller-supplied input with no default (R59.1)
// ---------------------------------------------------------------------------------------------

/// How many bytes this caller will let one packet reach.
///
/// There is deliberately no `Default` and no constant. R59.1: the value is Kevin's F3 policy
/// decision, and a default here would be exactly the invented resource value this slice may not
/// choose. C2's `ReadCeiling` (`model_invocation.rs:113-118`) rules the same way for the same
/// reason: a caller that does not know its own cap does not get to build a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketCap(u64);

impl PacketCap {
    pub fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    pub fn bytes(self) -> u64 {
        self.0
    }
}

/// The pre-dedup charge (R59.4, intended and conservative): every selected occurrence is charged,
/// including byte-equal duplicates the encoder will emit once.
#[derive(Debug, Clone, Copy)]
struct Charge {
    cap: u64,
    charged: u64,
}

impl Charge {
    fn new(cap: PacketCap) -> Self {
        Self {
            cap: cap.bytes(),
            charged: 0,
        }
    }

    fn add(&mut self, bytes: u64) -> Result<(), FidelityPacketRefusal> {
        self.charged =
            self.charged
                .checked_add(bytes)
                .ok_or(FidelityPacketRefusal::AggregateOverCap {
                    cap: self.cap,
                    charged: u64::MAX,
                })?;
        if self.charged > self.cap {
            return Err(FidelityPacketRefusal::AggregateOverCap {
                cap: self.cap,
                charged: self.charged,
            });
        }
        Ok(())
    }

    fn add_len_prefixed(&mut self, len: usize) -> Result<(), FidelityPacketRefusal> {
        self.add(8)?;
        self.add(len as u64)
    }
}

// ---------------------------------------------------------------------------------------------
// The request key (R68/H2): idempotency, carrying no moving fact
// ---------------------------------------------------------------------------------------------

/// The S3 idempotency key. Deliberately NOT the packet digest: it is a pure function of facts that
/// cannot move between two admissions of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestKey {
    pub workspace_id: Uuid,
    pub manifest_digest: [u8; 32],
    pub candidate_record_digest: [u8; 32],
    pub evaluator: Evaluator,
    pub rubric_digest: [u8; 32],
    pub purpose: String,
}

impl RequestKey {
    /// A stable 32-byte identity for the tuple, for storage and comparison.
    pub fn digest(&self) -> [u8; 32] {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(b"beacon.outcome-fidelity.request-key.v1\0");
        buffer.extend_from_slice(self.workspace_id.as_bytes());
        buffer.extend_from_slice(&self.manifest_digest);
        buffer.extend_from_slice(&self.candidate_record_digest);
        buffer.extend_from_slice(self.evaluator.resident_id.as_bytes());
        buffer.extend_from_slice(self.evaluator.configuration_id.as_bytes());
        buffer.extend_from_slice(&self.rubric_digest);
        buffer.extend_from_slice(&(self.purpose.len() as u64).to_be_bytes());
        buffer.extend_from_slice(self.purpose.as_bytes());
        sha256(&buffer)
    }
}

// ---------------------------------------------------------------------------------------------
// Section inputs
// ---------------------------------------------------------------------------------------------

/// A digest envelope on the wire: algorithm, canonicalizer, and the raw 32 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireDigest {
    pub canonicalizer: String,
    pub value: [u8; 32],
}

impl WireDigest {
    fn encoded_len(&self) -> u64 {
        8 + DIGEST_ALGORITHM.len() as u64 + 8 + self.canonicalizer.len() as u64 + 32
    }
}

/// The current activation facts, present only when the configuration carries a binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationFacts {
    pub configured_activation_id: Uuid,
    pub latest_activation_id: Uuid,
    pub latest_state: LocalModelActivationState,
}

/// §A.1. Rows 10-17 are ADMISSION-TIME facts (R68/H2): they are read once, inside the admission
/// transaction, and never recomputed for a retry or a read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderFacts {
    pub workspace_id: Uuid,
    pub manifest_digest: [u8; 32],
    pub candidate_record_digest: [u8; 32],
    pub rubric_id: String,
    pub rubric_version: u32,
    pub rubric_digest: [u8; 32],
    /// The ONE selected non-author evaluator, a held request input — never the whole admitted
    /// vector `candidate_evaluation.rs:315-325` returns (astra Q1).
    pub admitted: Evaluator,
    pub current_lifecycle: ResidentLifecycle,
    pub current_configuration_id: Uuid,
    pub current_configuration_state: ResidentConfigurationState,
    pub capabilities: Vec<BeaconCapability>,
    pub activation: Option<ActivationFacts>,
    pub gaps: Vec<PacketGap>,
}

/// The six closed gap arms, in the order this grammar sorts them — never the order the context
/// happens to push them (astra Q1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PacketGap {
    ResidentNotActive,
    ConfigurationCleared,
    ConfigurationChanged,
    MissingArtifactEvaluate,
    ActivationSuperseded,
    ActivationInactive,
}

impl PacketGap {
    fn tag(self) -> u8 {
        match self {
            Self::ResidentNotActive => 0x01,
            Self::ConfigurationCleared => 0x02,
            Self::ConfigurationChanged => 0x03,
            Self::MissingArtifactEvaluate => 0x04,
            Self::ActivationSuperseded => 0x05,
            Self::ActivationInactive => 0x06,
        }
    }
}

/// §A.2. `superseded_by` is deliberately absent: `artifact.rs:1192-1195` labels it read-time state
/// that "moves as corrections land", so embedding it would give one candidate two packet digests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectFacts {
    pub record_digest: WireDigest,
    pub record_bytes: Vec<u8>,
    pub content_digest: WireDigest,
    pub content_bytes: Vec<u8>,
    pub creation_event_seq: u64,
}

/// §A.3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeFacts {
    pub manifest_bytes: Vec<u8>,
    pub manifest_digest: WireDigest,
    pub renderer_version: String,
}

/// §A.5. The rubric's digest envelope is NOT repeated here — it is header rows 6-7.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubricFacts {
    pub bytes: Vec<u8>,
    pub criterion_ids: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// §A.4 evidence entries
// ---------------------------------------------------------------------------------------------

/// Kind tags reuse `SourceKind`'s registry byte-for-byte (`outcome.rs:115-122`) so the packet
/// cannot fork the owner's vocabulary. `0x09` Receipt is packet-local because Receipt v1/v2
/// defines no content digest (`outcome.rs:249-250`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    Mission,
    Completion,
    Artifact,
    Proposal,
    Ruling,
    Evaluation,
    Policy,
    Spend,
    Receipt,
}

impl EntryKind {
    pub fn tag(self) -> u8 {
        match self {
            Self::Mission => 0x01,
            Self::Completion => 0x02,
            Self::Artifact => 0x03,
            Self::Proposal => 0x04,
            Self::Ruling => 0x05,
            Self::Evaluation => 0x06,
            Self::Policy => 0x07,
            Self::Spend => 0x08,
            Self::Receipt => 0x09,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mission => "mission",
            Self::Completion => "completion",
            Self::Artifact => "artifact",
            Self::Proposal => "proposal",
            Self::Ruling => "ruling",
            Self::Evaluation => "evaluation",
            Self::Policy => "policy",
            Self::Spend => "spend",
            Self::Receipt => "receipt",
        }
    }
}

/// One evidence entry's body. Every arm carries exact owner bytes; a digest alone is never
/// sufficient provenance (R68/H1 — the READER that supplies each arm is named in the collector).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryBody {
    /// Kind `0x01`. No digest: a Mission has no canonical bytes (`outcome.rs:603-608`). Carries
    /// constraints and prohibitions (R59.5) because `limitations_and_authority` cannot be judged
    /// without them.
    Mission {
        mission_id: Uuid,
        project_id: Uuid,
        desired_outcome: String,
        success_conditions: Vec<String>,
        constraints: Vec<String>,
        prohibitions: Vec<String>,
    },
    /// Kinds `0x02`, `0x04`, `0x05`, `0x06`, `0x07`, `0x08`.
    Record { digest: WireDigest, bytes: Vec<u8> },
    /// Kind `0x03`: the record AND its content, both with envelopes.
    Artifact {
        record_digest: WireDigest,
        record_bytes: Vec<u8>,
        content_digest: WireDigest,
        content_bytes: Vec<u8>,
    },
    /// Kind `0x09`. Identified by `(run UUID, version)` plus the Outcome checksum, because Receipt
    /// v1/v2 has no content digest; the checksum label lives in the bytes so no later reader can
    /// mistake it for a Receipt domain identity (`outcome.rs:57-62`).
    Receipt {
        run_uuid: Uuid,
        version: u32,
        checksum_label: String,
        checksum: [u8; 32],
        receipt_json: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceEntry {
    pub kind: EntryKind,
    pub body: EntryBody,
}

/// The frozen sort and duplicate identity (R68/H3). Mission sorts on its UUID and Receipt on the
/// run UUID; neither has a digest and neither gets a synthetic one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum EntryKey {
    Uuid([u8; 16]),
    Digest {
        canonicalizer: Vec<u8>,
        value: [u8; 32],
    },
}

impl EvidenceEntry {
    fn key(&self) -> Result<EntryKey, MalformedFidelityPacket> {
        Ok(match (&self.kind, &self.body) {
            (EntryKind::Mission, EntryBody::Mission { mission_id, .. }) => {
                EntryKey::Uuid(*mission_id.as_bytes())
            }
            (EntryKind::Receipt, EntryBody::Receipt { run_uuid, .. }) => {
                EntryKey::Uuid(*run_uuid.as_bytes())
            }
            (EntryKind::Artifact, EntryBody::Artifact { record_digest, .. }) => EntryKey::Digest {
                canonicalizer: record_digest.canonicalizer.as_bytes().to_vec(),
                value: record_digest.value,
            },
            (
                EntryKind::Completion
                | EntryKind::Proposal
                | EntryKind::Ruling
                | EntryKind::Evaluation
                | EntryKind::Policy
                | EntryKind::Spend,
                EntryBody::Record { digest, .. },
            ) => EntryKey::Digest {
                canonicalizer: digest.canonicalizer.as_bytes().to_vec(),
                value: digest.value,
            },
            // A kind paired with a body shape it does not own is a grammar violation, never a
            // value: the two spellings of one fact must agree in both directions.
            _ => return Err(MalformedFidelityPacket::UnknownTag),
        })
    }

    fn charge(&self) -> Result<u64, MalformedFidelityPacket> {
        let bytes = match &self.body {
            EntryBody::Mission {
                desired_outcome,
                success_conditions,
                constraints,
                prohibitions,
                ..
            } => {
                let mut total: u64 = 16 + 16 + 8 + desired_outcome.len() as u64 + 4 + 4 + 4;
                for list in [success_conditions, constraints, prohibitions] {
                    for item in list {
                        total = total
                            .checked_add(8 + item.len() as u64)
                            .ok_or(MalformedFidelityPacket::OverLimitLength)?;
                    }
                }
                total
            }
            EntryBody::Record { digest, bytes } => digest.encoded_len() + 8 + bytes.len() as u64,
            EntryBody::Artifact {
                record_digest,
                record_bytes,
                content_digest,
                content_bytes,
            } => {
                record_digest.encoded_len()
                    + 8
                    + record_bytes.len() as u64
                    + content_digest.encoded_len()
                    + 8
                    + content_bytes.len() as u64
            }
            EntryBody::Receipt {
                checksum_label,
                receipt_json,
                ..
            } => 16 + 4 + 8 + checksum_label.len() as u64 + 32 + 8 + receipt_json.len() as u64,
        };
        Ok(bytes)
    }
}

// ---------------------------------------------------------------------------------------------
// The verified packet
// ---------------------------------------------------------------------------------------------

/// One evidence entry's position in the frozen order, and the label the projection must use.
///
/// R59.3: the evidence entry ID is the zero-based ordinal in the §A.4 sort, and the provider
/// message must present evidence in packet order labelled with that ordinal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryIndexRow {
    pub ordinal: u32,
    pub kind: EntryKind,
    /// The ordinal as the decimal string a reply must cite.
    pub label: String,
}

/// The collected packet: exact bytes, its identity envelope, and the ordinal index.
///
/// Sealed: constructed only by [`encode_packet`] in this module, so no other seat can mint one
/// from bytes nobody verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEvaluationPacket {
    bytes: Vec<u8>,
    digest: SourceDigest,
    entries: Vec<EntryIndexRow>,
}

impl VerifiedEvaluationPacket {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The identity envelope (algorithm, canonicalizer, value), never a bare hex string.
    pub fn digest(&self) -> &SourceDigest {
        &self.digest
    }

    /// The ordinal-labelled entry index, in packet order.
    pub fn entries(&self) -> &[EntryIndexRow] {
        &self.entries
    }

    /// Whether a reply's evidence reference names an entry this packet actually carries.
    pub fn resolves(&self, label: &str) -> bool {
        self.entries.iter().any(|entry| entry.label == label)
    }
}

// ---------------------------------------------------------------------------------------------
// The encoder
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

    fn uuid(&mut self, value: &Uuid) {
        self.out.extend_from_slice(value.as_bytes());
    }

    fn raw32(&mut self, value: &[u8; 32]) {
        self.out.extend_from_slice(value);
    }

    fn text(&mut self, value: &str, limit: usize) -> Result<(), MalformedFidelityPacket> {
        self.blob(value.as_bytes(), limit)
    }

    fn blob(&mut self, value: &[u8], limit: usize) -> Result<(), MalformedFidelityPacket> {
        if value.len() > limit {
            return Err(MalformedFidelityPacket::OverLimitLength);
        }
        let length =
            u64::try_from(value.len()).map_err(|_| MalformedFidelityPacket::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value);
        Ok(())
    }

    fn digest(&mut self, digest: &WireDigest) -> Result<(), MalformedFidelityPacket> {
        if digest.canonicalizer.is_empty() {
            return Err(MalformedFidelityPacket::DigestEnvelope);
        }
        self.text(DIGEST_ALGORITHM, MAX_TOKEN_BYTES)?;
        self.text(&digest.canonicalizer, MAX_TOKEN_BYTES)?;
        self.raw32(&digest.value);
        Ok(())
    }

    /// Opens a frame and returns the offset its length must be back-patched into.
    fn open_frame(&mut self, tag: u8) -> usize {
        self.tag(tag);
        let offset = self.out.len();
        self.u64(0);
        offset
    }

    fn close_frame(&mut self, offset: usize) -> Result<(), MalformedFidelityPacket> {
        let body = self.out.len() - offset - 8;
        let length = u64::try_from(body).map_err(|_| MalformedFidelityPacket::OverLimitLength)?;
        self.out[offset..offset + 8].copy_from_slice(&length.to_be_bytes());
        Ok(())
    }
}

fn capability_tag(capability: BeaconCapability) -> u8 {
    match capability {
        BeaconCapability::WorkCreateMission => 0x01,
        BeaconCapability::WorkFormTeam => 0x02,
        BeaconCapability::WorkCoordinateMission => 0x03,
        BeaconCapability::WorkAssignWork => 0x04,
        BeaconCapability::ArtifactProduce => 0x05,
        BeaconCapability::ArtifactEvaluate => 0x06,
        BeaconCapability::EvidenceDevelop => 0x07,
    }
}

fn lifecycle_tag(lifecycle: ResidentLifecycle) -> u8 {
    match lifecycle {
        ResidentLifecycle::Inactive => 0x01,
        ResidentLifecycle::Active => 0x02,
        ResidentLifecycle::Suspended => 0x03,
        ResidentLifecycle::Retired => 0x04,
    }
}

fn configuration_state_tag(state: ResidentConfigurationState) -> u8 {
    match state {
        ResidentConfigurationState::Configured => 0x01,
        ResidentConfigurationState::Cleared => 0x02,
    }
}

fn activation_state_tag(state: LocalModelActivationState) -> u8 {
    match state {
        LocalModelActivationState::Active => 0x01,
        LocalModelActivationState::Inactive => 0x02,
    }
}

/// Everything one packet is built from. The collector fills this; this module encodes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketInputs {
    pub header: HeaderFacts,
    pub subject: SubjectFacts,
    pub recipe: RecipeFacts,
    /// Unsorted and pre-dedup, exactly as collected. This grammar sorts and deduplicates.
    pub evidence: Vec<EvidenceEntry>,
    pub rubric: RubricFacts,
}

/// Charge the whole packet BEFORE any owner byte is appended, then encode.
///
/// R59.4: the charge counts every selected occurrence pre-dedup, so a packet can be refused at a
/// charged size the encoded bytes would never have reached. Conservative by construction, and
/// intended. No packet byte is returned on any refusal.
pub fn encode_packet(
    inputs: &PacketInputs,
    cap: PacketCap,
) -> Result<VerifiedEvaluationPacket, FidelityPacketRefusal> {
    charge_packet(inputs, cap)?;
    encode_charged(inputs).map_err(FidelityPacketRefusal::from)
}

fn charge_packet(inputs: &PacketInputs, cap: PacketCap) -> Result<(), FidelityPacketRefusal> {
    let mut charge = Charge::new(cap);
    charge.add(MAGIC.len() as u64)?;
    charge.add(5 * FRAME_HEADER_BYTES)?;

    // S1: every fixed scalar and pinned token.
    charge.add(16 + 32 + 32)?;
    charge.add_len_prefixed(inputs.header.rubric_id.len())?;
    charge.add(4)?;
    charge.add_len_prefixed(RAW_CANONICALIZER.len())?;
    charge.add(32 + 16 + 16 + 1 + 16 + 1)?;
    charge.add(4 + inputs.header.capabilities.len() as u64)?;
    charge.add(1)?;
    if inputs.header.activation.is_some() {
        charge.add(16 + 16 + 1)?;
    }
    charge.add(4 + inputs.header.gaps.len() as u64)?;
    charge.add(1)?;

    // S2.
    charge.add(inputs.subject.record_digest.encoded_len())?;
    charge.add_len_prefixed(inputs.subject.record_bytes.len())?;
    charge.add(inputs.subject.content_digest.encoded_len())?;
    charge.add_len_prefixed(inputs.subject.content_bytes.len())?;
    charge.add(8)?;

    // S3.
    charge.add_len_prefixed(inputs.recipe.manifest_bytes.len())?;
    charge.add(inputs.recipe.manifest_digest.encoded_len())?;
    charge.add_len_prefixed(inputs.recipe.renderer_version.len())?;

    // S4: the count, then every occurrence including byte-equal duplicates.
    charge.add(4)?;
    for entry in &inputs.evidence {
        charge.add(FRAME_HEADER_BYTES)?;
        charge.add(entry.charge().map_err(FidelityPacketRefusal::from)?)?;
    }

    // S5.
    charge.add_len_prefixed(inputs.rubric.bytes.len())?;
    charge.add(4)?;
    for id in &inputs.rubric.criterion_ids {
        charge.add_len_prefixed(id.len())?;
    }
    Ok(())
}

fn encode_charged(
    inputs: &PacketInputs,
) -> Result<VerifiedEvaluationPacket, MalformedFidelityPacket> {
    let mut encoder = Encoder::new();

    // ---- S1 header -------------------------------------------------------------------------
    let frame = encoder.open_frame(SECTION_HEADER);
    let header = &inputs.header;
    encoder.uuid(&header.workspace_id);
    encoder.raw32(&header.manifest_digest);
    encoder.raw32(&header.candidate_record_digest);
    encoder.text(&header.rubric_id, MAX_TOKEN_BYTES)?;
    encoder.u32(header.rubric_version);
    encoder.text(RAW_CANONICALIZER, MAX_TOKEN_BYTES)?;
    encoder.raw32(&header.rubric_digest);
    encoder.uuid(&header.admitted.resident_id);
    encoder.uuid(&header.admitted.configuration_id);
    encoder.tag(lifecycle_tag(header.current_lifecycle));
    encoder.uuid(&header.current_configuration_id);
    encoder.tag(configuration_state_tag(header.current_configuration_state));

    let mut capabilities: Vec<u8> = header
        .capabilities
        .iter()
        .map(|c| capability_tag(*c))
        .collect();
    capabilities.sort_unstable();
    if capabilities.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(MalformedFidelityPacket::EntryOrder);
    }
    if capabilities.len() as u32 > MAX_CAPABILITIES {
        return Err(MalformedFidelityPacket::EntryCount);
    }
    encoder.u32(capabilities.len() as u32);
    for tag in &capabilities {
        encoder.tag(*tag);
    }

    match &header.activation {
        None => encoder.tag(TAG_ACTIVATION_NONE),
        Some(activation) => {
            encoder.tag(TAG_ACTIVATION_SOME);
            encoder.uuid(&activation.configured_activation_id);
            encoder.uuid(&activation.latest_activation_id);
            encoder.tag(activation_state_tag(activation.latest_state));
        }
    }

    // Gaps sort by TAG, never in the order the context pushed them (astra Q1).
    let mut gaps: Vec<u8> = header.gaps.iter().map(|gap| gap.tag()).collect();
    gaps.sort_unstable();
    if gaps.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(MalformedFidelityPacket::EntryOrder);
    }
    if gaps.len() as u32 > MAX_GAPS {
        return Err(MalformedFidelityPacket::EntryCount);
    }
    encoder.u32(gaps.len() as u32);
    for tag in &gaps {
        encoder.tag(*tag);
    }

    // Row 18: `NotMeasured` is the only arm today. Encoding it reserves the position so a later
    // measured arm cannot arrive as a silent change of meaning.
    encoder.tag(0x01);
    encoder.close_frame(frame)?;

    // ---- S2 subject ------------------------------------------------------------------------
    let frame = encoder.open_frame(SECTION_SUBJECT);
    encoder.digest(&inputs.subject.record_digest)?;
    encoder.blob(&inputs.subject.record_bytes, MAX_ARTIFACT_RECORD_BYTES)?;
    encoder.digest(&inputs.subject.content_digest)?;
    encoder.blob(&inputs.subject.content_bytes, MAX_ARTIFACT_CONTENT_BYTES)?;
    encoder.u64(inputs.subject.creation_event_seq);
    encoder.close_frame(frame)?;

    // ---- S3 frozen recipe ------------------------------------------------------------------
    let frame = encoder.open_frame(SECTION_RECIPE);
    encoder.blob(&inputs.recipe.manifest_bytes, MAX_MANIFEST_BYTES)?;
    if inputs.recipe.manifest_digest.canonicalizer != crate::outcome::CANONICALIZER {
        // The retired v1 grammar is refused BY NAME, never reinterpreted (`outcome.rs:38-41`).
        return Err(MalformedFidelityPacket::CanonicalizerNotAdmitted);
    }
    encoder.digest(&inputs.recipe.manifest_digest)?;
    encoder.text(&inputs.recipe.renderer_version, MAX_TOKEN_BYTES)?;
    encoder.close_frame(frame)?;

    // ---- S4 evidence -----------------------------------------------------------------------
    let frame = encoder.open_frame(SECTION_EVIDENCE);
    let ordered = order_entries(&inputs.evidence)?;
    if ordered.len() as u32 > MAX_ENTRIES {
        return Err(MalformedFidelityPacket::EntryCount);
    }
    encoder.u32(ordered.len() as u32);
    let mut index = Vec::with_capacity(ordered.len());
    for (ordinal, entry) in ordered.iter().enumerate() {
        let ordinal = u32::try_from(ordinal).map_err(|_| MalformedFidelityPacket::EntryCount)?;
        let entry_frame = encoder.open_frame(entry.kind.tag());
        encode_entry_body(&mut encoder, entry)?;
        encoder.close_frame(entry_frame)?;
        index.push(EntryIndexRow {
            ordinal,
            kind: entry.kind,
            label: ordinal.to_string(),
        });
    }
    encoder.close_frame(frame)?;

    // ---- S5 rubric -------------------------------------------------------------------------
    let frame = encoder.open_frame(SECTION_RUBRIC);
    encoder.blob(&inputs.rubric.bytes, MAX_RUBRIC_BYTES)?;
    encoder.u32(
        u32::try_from(inputs.rubric.criterion_ids.len())
            .map_err(|_| MalformedFidelityPacket::EntryCount)?,
    );
    for id in &inputs.rubric.criterion_ids {
        encoder.text(id, MAX_TOKEN_BYTES)?;
    }
    encoder.close_frame(frame)?;

    let bytes = encoder.out;
    let digest = SourceDigest {
        algorithm: DIGEST_ALGORITHM.to_owned(),
        canonicalizer: CANONICALIZER.to_owned(),
        value: HEXLOWER.encode(&sha256(&bytes)),
    };
    Ok(VerifiedEvaluationPacket {
        bytes,
        digest,
        entries: index,
    })
}

/// Sort by `(kind tag, per-kind key)` and drop byte-equal duplicates. Database or insertion order
/// never reaches packet bytes.
fn order_entries(entries: &[EvidenceEntry]) -> Result<Vec<EvidenceEntry>, MalformedFidelityPacket> {
    let mut keyed: Vec<(u8, EntryKey, &EvidenceEntry)> = Vec::with_capacity(entries.len());
    for entry in entries {
        keyed.push((entry.kind.tag(), entry.key()?, entry));
    }
    keyed.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let mut ordered: Vec<EvidenceEntry> = Vec::with_capacity(keyed.len());
    for (tag, key, entry) in keyed {
        if let Some(previous) = ordered.last()
            && previous.kind.tag() == tag
            && previous.key()? == key
        {
            // Identical identity deduplicates ONLY after byte equality; a mismatch means two
            // records claim one identity, which is a refusal rather than a choice.
            if previous == entry {
                continue;
            }
            return Err(MalformedFidelityPacket::DuplicateEntry);
        }
        ordered.push(entry.clone());
    }
    Ok(ordered)
}

fn encode_entry_body(
    encoder: &mut Encoder,
    entry: &EvidenceEntry,
) -> Result<(), MalformedFidelityPacket> {
    match &entry.body {
        EntryBody::Mission {
            mission_id,
            project_id,
            desired_outcome,
            success_conditions,
            constraints,
            prohibitions,
        } => {
            encoder.uuid(mission_id);
            encoder.uuid(project_id);
            encoder.text(desired_outcome, MAX_MISSION_TEXT_BYTES)?;
            for list in [success_conditions, constraints, prohibitions] {
                let count =
                    u32::try_from(list.len()).map_err(|_| MalformedFidelityPacket::EntryCount)?;
                if count > MAX_MISSION_ITEMS {
                    return Err(MalformedFidelityPacket::EntryCount);
                }
                encoder.u32(count);
                for item in list {
                    encoder.text(item, MAX_MISSION_TEXT_BYTES)?;
                }
            }
        }
        EntryBody::Record { digest, bytes } => {
            encoder.digest(digest)?;
            encoder.blob(bytes, MAX_OWNER_RECORD_BYTES)?;
        }
        EntryBody::Artifact {
            record_digest,
            record_bytes,
            content_digest,
            content_bytes,
        } => {
            encoder.digest(record_digest)?;
            encoder.blob(record_bytes, MAX_ARTIFACT_RECORD_BYTES)?;
            encoder.digest(content_digest)?;
            encoder.blob(content_bytes, MAX_ARTIFACT_CONTENT_BYTES)?;
        }
        EntryBody::Receipt {
            run_uuid,
            version,
            checksum_label,
            checksum,
            receipt_json,
        } => {
            if !(1..=4).contains(version) {
                return Err(MalformedFidelityPacket::ReceiptVersion);
            }
            if checksum_label != crate::outcome::RECEIPT_CHECKSUM_LABEL {
                return Err(MalformedFidelityPacket::CanonicalizerNotAdmitted);
            }
            encoder.uuid(run_uuid);
            encoder.u32(*version);
            encoder.text(checksum_label, MAX_TOKEN_BYTES)?;
            encoder.raw32(checksum);
            encoder.blob(receipt_json, MAX_RECEIPT_JSON_BYTES)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// C8: the measured minima
// ---------------------------------------------------------------------------------------------

/// The smallest number of bytes one entry of each kind can occupy, INCLUDING its frame header.
///
/// MEASURED, not derived: `tests::min_entry_bytes_is_the_measured_minimum` encodes the smallest
/// legal entry of every kind through the real encoder and asserts each figure equals the result,
/// so a change to the encoder moves the test before it can move the guard. The hand-computed
/// figures this codebase replaced were all too LOW (`outcome.rs:88-95`).
pub const MIN_ENTRY_BYTES: [u64; 9] = [61, 72, 135, 72, 72, 72, 72, 72, 111];

/// The smallest an entry of `kind` can be, for the count-before-reserve guard.
pub fn min_entry_bytes(kind: EntryKind) -> u64 {
    MIN_ENTRY_BYTES[(kind.tag() - 1) as usize]
}

/// A declared entry count is checked against the remaining input using the SMALLEST possible entry
/// before anything reserves for it (`model_invocation.rs:799-812`'s rule, kept). A cap alone leaves
/// a hostile count buying 256 slots and one decode attempt.
pub fn admits_entry_count(declared: u32, remaining: u64) -> Result<(), MalformedFidelityPacket> {
    if declared > MAX_ENTRIES {
        return Err(MalformedFidelityPacket::EntryCount);
    }
    let smallest = MIN_ENTRY_BYTES
        .iter()
        .copied()
        .min()
        .expect("nine kinds")
        .checked_mul(u64::from(declared))
        .ok_or(MalformedFidelityPacket::EntryCount)?;
    if smallest > remaining {
        return Err(MalformedFidelityPacket::Truncated);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(canonicalizer: &str) -> WireDigest {
        WireDigest {
            canonicalizer: canonicalizer.to_owned(),
            value: [0_u8; 32],
        }
    }

    pub(crate) fn inputs(evidence: Vec<EvidenceEntry>) -> PacketInputs {
        PacketInputs {
            header: HeaderFacts {
                workspace_id: Uuid::nil(),
                manifest_digest: [1_u8; 32],
                candidate_record_digest: [2_u8; 32],
                rubric_id: "outcome-fidelity-draft".to_owned(),
                rubric_version: 1,
                rubric_digest: [3_u8; 32],
                admitted: Evaluator {
                    resident_id: Uuid::nil(),
                    configuration_id: Uuid::nil(),
                },
                current_lifecycle: ResidentLifecycle::Active,
                current_configuration_id: Uuid::nil(),
                current_configuration_state: ResidentConfigurationState::Configured,
                capabilities: vec![BeaconCapability::ArtifactEvaluate],
                activation: None,
                gaps: Vec::new(),
            },
            subject: SubjectFacts {
                record_digest: digest("beacon.artifact.v1"),
                record_bytes: b"record".to_vec(),
                content_digest: digest("beacon.raw.v1"),
                content_bytes: b"# candidate".to_vec(),
                creation_event_seq: 7,
            },
            recipe: RecipeFacts {
                manifest_bytes: b"manifest".to_vec(),
                manifest_digest: digest(crate::outcome::CANONICALIZER),
                renderer_version: crate::outcome::RENDERER_V1.to_owned(),
            },
            evidence,
            rubric: RubricFacts {
                bytes: b"rubric".to_vec(),
                criterion_ids: crate::candidate_evaluation_result::CRITERION_IDS
                    .iter()
                    .map(|id| (*id).to_owned())
                    .collect(),
            },
        }
    }

    fn cap() -> PacketCap {
        PacketCap::new(64 * 1024 * 1024)
    }

    /// The smallest legal entry of each kind, for the measurement below.
    fn smallest(kind: EntryKind) -> EvidenceEntry {
        let body = match kind {
            EntryKind::Mission => EntryBody::Mission {
                mission_id: Uuid::nil(),
                project_id: Uuid::nil(),
                desired_outcome: String::new(),
                success_conditions: Vec::new(),
                constraints: Vec::new(),
                prohibitions: Vec::new(),
            },
            EntryKind::Artifact => EntryBody::Artifact {
                record_digest: digest("x"),
                record_bytes: Vec::new(),
                content_digest: digest("x"),
                content_bytes: Vec::new(),
            },
            EntryKind::Receipt => EntryBody::Receipt {
                run_uuid: Uuid::nil(),
                version: 1,
                checksum_label: crate::outcome::RECEIPT_CHECKSUM_LABEL.to_owned(),
                checksum: [0_u8; 32],
                receipt_json: Vec::new(),
            },
            _ => EntryBody::Record {
                digest: digest("x"),
                bytes: Vec::new(),
            },
        };
        EvidenceEntry { kind, body }
    }

    /// C8. MEASURED, not derived: each figure is what the real encoder produces for the smallest
    /// legal entry of its kind, so a change to the encoder moves this test before it can move the
    /// guard. The figures this replaced were hand-computed and two of the nine were wrong -- one
    /// of them LOW, the direction that lets a hostile count through (`outcome.rs:88-95`).
    #[test]
    fn min_entry_bytes_is_the_measured_minimum() {
        let base = encode_packet(&inputs(Vec::new()), cap()).expect("base packet encodes");
        let kinds = [
            EntryKind::Mission,
            EntryKind::Completion,
            EntryKind::Artifact,
            EntryKind::Proposal,
            EntryKind::Ruling,
            EntryKind::Evaluation,
            EntryKind::Policy,
            EntryKind::Spend,
            EntryKind::Receipt,
        ];
        let mut measured = Vec::new();
        for kind in kinds {
            let one = encode_packet(&inputs(vec![smallest(kind)]), cap()).expect("entry encodes");
            measured.push((one.bytes().len() - base.bytes().len()) as u64);
        }
        assert_eq!(
            measured,
            MIN_ENTRY_BYTES.to_vec(),
            "measured minima disagree with the guard constants"
        );
    }
}
