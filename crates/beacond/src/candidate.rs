//! Candidate v1: the post-image digests of one proposed change, bound to one exact Proposal
//! (ADR 0039).
//!
//! A Ruling names a Proposal. A Proposal names intended effects -- a [`MutationKind`] and a target
//! path -- and carries **no content at all**. Nothing in either record's identity covers the bytes
//! a dispatch would write, so without this record dispatch would apply bytes under the authority of
//! a decision that never saw them.
//!
//! A Candidate closes that with the smallest record that can: one entry per declared mutation,
//! holding the SHA-256 of the file's exact bytes before and after. **Digests only, never content.**
//! It is the output-side twin of the readable-context manifest ADR 0034 already defines for inputs,
//! and it uses the same identity -- SHA-256 over the exact file bytes, deliberately not the Git
//! object name, which is a SHA-1 over a header and the same body.
//!
//! **A Candidate selects bytes. It does not authorize them.** Recording one, and recording it
//! before a decision, proves that a determinate set of digests existed for that Proposal -- never
//! that the Operator was shown them. Authority comes from `beacon.ruling.v2`, which binds this
//! record's digest envelope after its exact diff has been presented (ADR 0039). A non-author review
//! produced the counterexample this warning exists for: record the Candidate, show the Operator
//! only the Proposal's intent, rule, dispatch -- every ordering rule holds and nobody saw the bytes.
//!
//! Two rules make the selection determinate rather than merely present:
//!
//! - **One per Proposal**, enforced structurally by `candidates_one_per_proposal`. A second set of
//!   post-image digests would leave "which bytes" ambiguous and force dispatch to choose. It does
//!   not choose: the second record is refused, and a revised change is a new Proposal -- which is
//!   what ADR 0034 already says about anything that changes.
//! - **Recorded before the Ruling**, compared against `events.sequence` -- the store's own ordering
//!   primitive, so the rule needs no clock. That comparison is a **dispatch-time gate**, not an
//!   invariant this module's store enforces: nothing here refuses a Candidate recorded later, and
//!   the Ruling that binds it is what makes the order matter.
//!
//! **Recording a Candidate creates no authority and dispatches nothing.** It opens no Gate,
//! prepares no worktree, starts no process, and touches no repository. It is a statement of what a
//! change would do, made before anyone decides whether it may.
//!
//! The canonical grammar reuses `beacon.proposal.v1`'s primitives and carries its own copy of them,
//! for the reason `ruling.rs` states: an identity grammar's whole value is that it cannot move, and
//! one shared codec would couple two frozen grammars' futures. The one predicate that is shared
//! rather than copied is `is_repository_relative_path`, because a Candidate copies its paths out of
//! a verified Proposal -- a second definition could only ever disagree with the first about which
//! file a name means.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use data_encoding::{BASE64, HEXLOWER};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::proposal::{
    MutationKind, ProposalDigest, ProposalIntentFacts, is_base_revision,
    is_repository_relative_path, proposal_intent_facts, sha256,
};
use crate::{OperatorAccountError, OperatorAccountService, ProposalError};

/// The canonicalizer identity. Any future grammar is a new canonicalizer; existing v1 bytes are
/// never reinterpreted under v2 rules.
const CANONICALIZER: &str = "beacon.candidate.v1";
const DIGEST_ALGORITHM: &str = "sha256";

/// The exact envelope a v1 Candidate may name as the Proposal it was built against. Both are
/// copied from that Proposal and both are checked by the codec, so bytes naming a canonicalizer
/// this grammar cannot have been built against are refused before any column constraint sees them.
const PROPOSAL_DIGEST_ALGORITHM: &str = "sha256";
const PROPOSAL_CANONICALIZER: &str = "beacon.proposal.v1";

/// The literal ASCII magic that starts every record, including its terminating NUL: 20 bytes.
const MAGIC: &[u8] = b"beacon.candidate.v1\0";

/// The canonical envelope, matching ADR 0034's limit for a Proposal: the record this one binds to
/// cannot be larger, and an entry list is bounded by the same ceiling as the mutation list it
/// mirrors.
const MAX_CANONICAL_BYTES: usize = 64 * 1024;
/// `sha256`, `beacon.proposal.v1`: the limit bounds a declared length before any of it is read.
const MAX_DESCRIPTOR_TOKEN_BYTES: usize = 64;
/// ADR 0034: "the pinned base revision must be a lowercase 40-character hexadecimal commit id."
const BASE_REVISION_CHARS: usize = 40;
const DIGEST_HEX_CHARS: usize = 64;
/// The smallest an entry can be on the wire: kind tag, an eight-byte length, and two image tags.
/// A declared count is multiplied by this and compared with the remaining input before the loop
/// runs, so a hostile count cannot choose an allocation.
const MIN_ENTRY_BYTES: usize = 1 + 8 + 1 + 1;
/// The same envelope as it appears in an audit Event: RFC 4648 padded base64 is four characters
/// per three bytes, rounded up.
const MAX_BASE64_ENVELOPE_BYTES: usize = 4 * MAX_CANONICAL_BYTES.div_ceil(3);

// One-byte tags. Assigned explicitly and never renumbered. They are numbered to match
// `beacon.proposal.v1`'s mutation tags because the two records describe the same three effects;
// they are still this grammar's own constants, so a future divergence is a decision rather than an
// accident.
const TAG_MUTATION_CREATE_FILE: u8 = 0x01;
const TAG_MUTATION_MODIFY_FILE: u8 = 0x02;
const TAG_MUTATION_DELETE_FILE: u8 = 0x03;
/// Absence is a value in both image positions, so it encodes as one.
const TAG_IMAGE_ABSENT: u8 = 0x00;
const TAG_IMAGE_PRESENT: u8 = 0x01;

// ---------------------------------------------------------------------------------------------
// The immutable v1 payload
// ---------------------------------------------------------------------------------------------

/// One declared mutation's content identity: what the file's bytes hash to before the change and
/// after it.
///
/// Which of the two is present is decided by the kind and never by the caller: a created file has
/// no before, a deleted file has no after, and a modified file has both. An absent image is an
/// explicit tag rather than a missing field, because "this file did not exist" and "nobody recorded
/// what it held" are different facts and only the first is expressible here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateEntry {
    pub kind: MutationKind,
    pub path: String,
    pub pre_image_sha256: Option<[u8; 32]>,
    pub post_image_sha256: Option<[u8; 32]>,
}

/// The images a caller supplies for one declared mutation. The kind and the path are copied from
/// the Proposal, so a caller cannot name an effect the Proposal did not declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateImages {
    pub pre_image_sha256: Option<[u8; 32]>,
    pub post_image_sha256: Option<[u8; 32]>,
}

/// The immutable v1 payload, exactly as its canonical bytes decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateV1 {
    /// The bound Proposal's identity, copied whole -- value, algorithm, and canonicalizer.
    pub proposal: ProposalDigest,
    /// The revision the pre-image digests are facts about. Copied from the same Proposal: a
    /// pre-image taken at another revision is a statement about a different tree.
    pub pinned_base_revision: String,
    /// One entry per declared mutation, in the Proposal's own order. Order is identity-bearing
    /// there and is preserved here rather than sorted, because it is the caller's intended
    /// sequence.
    pub entries: Vec<CandidateEntry>,
    pub recorded_at_ms: u64,
}

// ---------------------------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------------------------

/// A digest that carries its algorithm and canonicalizer identity, never a bare hex string.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateDigest {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

/// One recorded Candidate: its bytes, the identity of those bytes, and what they decode to.
///
/// The fields are private and this type derives no `Deserialize`, for the reason ADR 0034 gives: a
/// caller-supplied parsed object plus a digest permits the two to disagree. Bytes and payload here
/// were proved equal by the only constructor that exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    digest: CandidateDigest,
    canonical_bytes: Vec<u8>,
    payload: CandidateV1,
}

impl Candidate {
    pub fn digest(&self) -> &CandidateDigest {
        &self.digest
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    pub fn payload(&self) -> &CandidateV1 {
        &self.payload
    }
}

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

/// Why a value is not a Candidate.
///
/// Decided from the bytes when decoding and from the caller's arguments when encoding. Both
/// directions run the same predicates, so the encoder and the decoder cannot disagree about what is
/// well-formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedCandidate {
    Magic,
    Truncated,
    TrailingBytes,
    InvalidUtf8,
    UnknownTag,
    OverLimitLength,
    ProposalDigestValue,
    ProposalDigestEnvelope,
    PinnedBaseRevision,
    /// No entries at all. A Proposal declares at least one effect, so a Candidate for it that
    /// declares none describes a change that is not the one that was proposed.
    NoEntries,
    TargetPath,
    /// One path named by two entries. v1 dispatch applies one effect per path: two entries would
    /// leave the resulting diff with one row for two declarations, and the post-condition that
    /// proves "exactly what was declared" could not be stated.
    DuplicatePath,
    /// An image present where its kind has none, or absent where its kind requires one.
    ImageForKind,
    /// A modification whose before and after are the same bytes. That declares no change, and the
    /// mutation it is bound to declared one.
    EmptyEffect,
}

impl MalformedCandidate {
    fn reason(self) -> &'static str {
        match self {
            Self::Magic => "the bytes do not start with the beacon.candidate.v1 magic",
            Self::Truncated => "the bytes end inside a field",
            Self::TrailingBytes => "bytes remain after the last field",
            Self::InvalidUtf8 => "a text field is not valid UTF-8",
            Self::UnknownTag => "a closed vocabulary carries an unassigned tag",
            Self::OverLimitLength => "a declared length exceeds its field's limit",
            Self::ProposalDigestValue => "the Proposal digest is not 64 lowercase hex characters",
            Self::ProposalDigestEnvelope => "the Proposal digest names another canonicalizer",
            Self::PinnedBaseRevision => {
                "the pinned base revision is not 40 lowercase hex characters"
            }
            Self::NoEntries => "a Candidate declares at least one entry",
            Self::TargetPath => "an entry names a path that is not repository-relative",
            Self::DuplicatePath => "two entries name one path",
            Self::ImageForKind => "an entry's images do not match its mutation kind",
            Self::EmptyEffect => "a modification declares the same bytes before and after",
        }
    }
}

/// Why a well-formed Candidate does not describe this Workspace's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InconsistentCandidate {
    /// No Proposal in this Workspace has that digest. Distinct from a Proposal that disagrees with
    /// its own bytes, which is invalid state.
    ProposalNotFound,
    /// A Candidate for that Proposal is already recorded. The refusal is the rule: one Proposal,
    /// one set of post-image digests.
    AlreadyRecorded,
    /// The caller supplied a different number of images than the Proposal declared mutations.
    EntryCountMismatch,
    /// The Proposal has no Candidate, so there is nothing to present.
    NotRecorded,
}

impl InconsistentCandidate {
    fn reason(self) -> &'static str {
        match self {
            Self::ProposalNotFound => "no Proposal in this Workspace has that digest",
            Self::AlreadyRecorded => "that Proposal already has a Candidate",
            Self::EntryCountMismatch => {
                "the images do not match the Proposal's declared mutations one for one"
            }
            Self::NotRecorded => "that Proposal has no Candidate to present",
        }
    }
}

/// Which fault the Candidate startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum CandidateStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One Candidate did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    Row {
        workspace_id: Uuid,
        digest_value: String,
        cause: CandidateError,
    },
    // NO `Totals` ARM. This pass has no whole-table comparison, deliberately and with its own
    // comment saying why: an Event naming a digest no row carries is refused by the replay. A
    // variant nothing can construct is a promise the type cannot keep -- evaluation taught this.
    /// A row whose Workspace column is not a UUID. Unreachable while the foreign-key check and
    /// the Workspace pass run first.
    Identity {
        workspace_id: String,
        digest_value: String,
    },
}

impl CandidateError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::Malformed(_) => "Malformed",
            Self::Inconsistent(_) => "Inconsistent",
            Self::NotFound => "NotFound",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for CandidateStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the Candidates could not be read"),
            Self::Row {
                workspace_id,
                digest_value,
                cause,
            } => write!(
                formatter,
                "Candidate {digest_value} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Identity {
                workspace_id,
                digest_value,
            } => write!(
                formatter,
                "a Candidate row's Workspace is not readable: {workspace_id}, Candidate \
                 {digest_value}"
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for CandidateStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
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

impl std::error::Error for CandidateStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            Self::Identity { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum CandidateError {
    Account(OperatorAccountError),
    Malformed(MalformedCandidate),
    Inconsistent(InconsistentCandidate),
    NotFound,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for CandidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::Malformed(reason) => {
                write!(
                    formatter,
                    "Candidate values are invalid: {}",
                    reason.reason()
                )
            }
            Self::Inconsistent(reason) => write!(
                formatter,
                "Candidate references are inconsistent: {}",
                reason.reason()
            ),
            Self::NotFound => write!(formatter, "no Candidate has that digest"),
            Self::InvalidStore => write!(formatter, "Candidate state is invalid"),
            Self::Store(error) => write!(formatter, "Candidate store failed: {error}"),
            Self::Json(error) => write!(formatter, "Candidate event is invalid: {error}"),
        }
    }
}

impl std::error::Error for CandidateError {}

impl From<OperatorAccountError> for CandidateError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for CandidateError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for CandidateError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn map_proposal_error(error: ProposalError) -> CandidateError {
    match error {
        ProposalError::Account(error) => CandidateError::Account(error),
        ProposalError::Store(error) => CandidateError::Store(error),
        _ => CandidateError::InvalidStore,
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

    fn count(&mut self, length: usize, limit: usize) -> Result<(), MalformedCandidate> {
        if length > limit {
            return Err(MalformedCandidate::OverLimitLength);
        }
        let count = u32::try_from(length).map_err(|_| MalformedCandidate::OverLimitLength)?;
        self.u32(count);
        Ok(())
    }

    fn string(&mut self, value: &str, limit: usize) -> Result<(), MalformedCandidate> {
        if value.len() > limit {
            return Err(MalformedCandidate::OverLimitLength);
        }
        let length = u64::try_from(value.len()).map_err(|_| MalformedCandidate::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value.as_bytes());
        Ok(())
    }

    /// An image, present or absent, as a tag and then its bytes. Absence encodes as a value.
    fn image(&mut self, image: Option<&[u8; 32]>) {
        match image {
            None => self.tag(TAG_IMAGE_ABSENT),
            Some(value) => {
                self.tag(TAG_IMAGE_PRESENT);
                self.sha256(value);
            }
        }
    }
}

/// Reads canonical bytes. Self-delimiting: every field proves it fits within the remaining input
/// and its own limit before any of it is consumed, and no declared count reserves capacity.
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

    fn take(&mut self, count: usize) -> Result<&'a [u8], MalformedCandidate> {
        if count > self.remaining() {
            return Err(MalformedCandidate::Truncated);
        }
        let slice = &self.input[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn magic(&mut self) -> Result<(), MalformedCandidate> {
        if self
            .take(MAGIC.len())
            .map_err(|_| MalformedCandidate::Magic)?
            == MAGIC
        {
            Ok(())
        } else {
            Err(MalformedCandidate::Magic)
        }
    }

    fn tag(&mut self) -> Result<u8, MalformedCandidate> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, MalformedCandidate> {
        let mut value = [0_u8; 4];
        value.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(value))
    }

    fn u64(&mut self) -> Result<u64, MalformedCandidate> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }

    fn sha256(&mut self) -> Result<[u8; 32], MalformedCandidate> {
        let mut value = [0_u8; 32];
        value.copy_from_slice(self.take(32)?);
        Ok(value)
    }

    fn string(&mut self, limit: usize) -> Result<String, MalformedCandidate> {
        let length = self.u64()?;
        let limit = u64::try_from(limit).map_err(|_| MalformedCandidate::OverLimitLength)?;
        if length > limit {
            return Err(MalformedCandidate::OverLimitLength);
        }
        let length = usize::try_from(length).map_err(|_| MalformedCandidate::Truncated)?;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| MalformedCandidate::InvalidUtf8)
    }

    /// A declared count, bounded before it is trusted: it is compared with its own limit and then
    /// multiplied by the smallest an entry can be, so a count that could not possibly fit in the
    /// remaining input is refused before the loop reserves anything.
    fn count(&mut self, limit: usize, entry_bytes: usize) -> Result<usize, MalformedCandidate> {
        let count =
            usize::try_from(self.u32()?).map_err(|_| MalformedCandidate::OverLimitLength)?;
        if count > limit {
            return Err(MalformedCandidate::OverLimitLength);
        }
        let smallest = count
            .checked_mul(entry_bytes)
            .ok_or(MalformedCandidate::Truncated)?;
        if smallest > self.remaining() {
            return Err(MalformedCandidate::Truncated);
        }
        Ok(count)
    }

    fn image(&mut self) -> Result<Option<[u8; 32]>, MalformedCandidate> {
        match self.tag()? {
            TAG_IMAGE_ABSENT => Ok(None),
            TAG_IMAGE_PRESENT => Ok(Some(self.sha256()?)),
            _ => Err(MalformedCandidate::UnknownTag),
        }
    }

    fn finish(self) -> Result<(), MalformedCandidate> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MalformedCandidate::TrailingBytes)
        }
    }
}

/// A lowercase 64-character hexadecimal digest, as its thirty-two raw bytes.
///
/// Uppercase input is refused rather than folded: two spellings of one digest can never both
/// encode.
fn digest_bytes(value: &str, refusal: MalformedCandidate) -> Result<[u8; 32], MalformedCandidate> {
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

/// Which images a kind requires, and which it forbids. One function, consulted by the encoder and
/// the decoder, so a record that could be written could always be read back.
fn images_match_kind(entry: &CandidateEntry) -> bool {
    match entry.kind {
        MutationKind::CreateFile => {
            entry.pre_image_sha256.is_none() && entry.post_image_sha256.is_some()
        }
        MutationKind::ModifyFile => {
            entry.pre_image_sha256.is_some() && entry.post_image_sha256.is_some()
        }
        MutationKind::DeleteFile => {
            entry.pre_image_sha256.is_some() && entry.post_image_sha256.is_none()
        }
    }
}

fn mutation_tag(kind: MutationKind) -> u8 {
    match kind {
        MutationKind::CreateFile => TAG_MUTATION_CREATE_FILE,
        MutationKind::ModifyFile => TAG_MUTATION_MODIFY_FILE,
        MutationKind::DeleteFile => TAG_MUTATION_DELETE_FILE,
    }
}

fn parse_mutation_tag(tag: u8) -> Result<MutationKind, MalformedCandidate> {
    match tag {
        TAG_MUTATION_CREATE_FILE => Ok(MutationKind::CreateFile),
        TAG_MUTATION_MODIFY_FILE => Ok(MutationKind::ModifyFile),
        TAG_MUTATION_DELETE_FILE => Ok(MutationKind::DeleteFile),
        _ => Err(MalformedCandidate::UnknownTag),
    }
}

/// Every rule an entry list obeys, in one place: at least one entry, a repository-relative path per
/// entry, images that match the kind, no path named twice, and no modification that changes
/// nothing. Both directions call it, so the encoder cannot produce a record the decoder refuses.
fn validate_entries(entries: &[CandidateEntry]) -> Result<(), MalformedCandidate> {
    if entries.is_empty() {
        return Err(MalformedCandidate::NoEntries);
    }
    let mut paths: HashSet<&str> = HashSet::with_capacity(entries.len());
    for entry in entries {
        if !is_repository_relative_path(&entry.path) {
            return Err(MalformedCandidate::TargetPath);
        }
        if !images_match_kind(entry) {
            return Err(MalformedCandidate::ImageForKind);
        }
        if entry.kind == MutationKind::ModifyFile
            && entry.pre_image_sha256 == entry.post_image_sha256
        {
            return Err(MalformedCandidate::EmptyEffect);
        }
        if !paths.insert(entry.path.as_str()) {
            return Err(MalformedCandidate::DuplicatePath);
        }
    }
    Ok(())
}

/// The one canonical encoding of a payload. Every accepted byte difference changes the Candidate
/// identity, so alternate encodings of one parsed value do not exist: this function is the only
/// producer, and every read proves the stored bytes equal what it returns.
///
/// The field order, widths, and tag values are ADR 0039's table, and this is its executable form.
fn encode(payload: &CandidateV1) -> Result<Vec<u8>, MalformedCandidate> {
    let mut encoder = Encoder::new();

    // 2-4: the bound Proposal's identity, copied whole.
    if payload.proposal.algorithm != PROPOSAL_DIGEST_ALGORITHM
        || payload.proposal.canonicalizer != PROPOSAL_CANONICALIZER
    {
        return Err(MalformedCandidate::ProposalDigestEnvelope);
    }
    encoder.sha256(&digest_bytes(
        &payload.proposal.value,
        MalformedCandidate::ProposalDigestValue,
    )?);
    encoder.string(&payload.proposal.algorithm, MAX_DESCRIPTOR_TOKEN_BYTES)?;
    encoder.string(&payload.proposal.canonicalizer, MAX_DESCRIPTOR_TOKEN_BYTES)?;

    // 5: the revision the pre-images are facts about.
    if !is_base_revision(&payload.pinned_base_revision) {
        return Err(MalformedCandidate::PinnedBaseRevision);
    }
    encoder.string(&payload.pinned_base_revision, BASE_REVISION_CHARS)?;

    // 6-10: the entries, in the Proposal's order.
    validate_entries(&payload.entries)?;
    encoder.count(payload.entries.len(), MAX_CANONICAL_BYTES)?;
    for entry in &payload.entries {
        encoder.tag(mutation_tag(entry.kind));
        encoder.string(&entry.path, MAX_CANONICAL_BYTES)?;
        encoder.image(entry.pre_image_sha256.as_ref());
        encoder.image(entry.post_image_sha256.as_ref());
    }

    // 11.
    encoder.u64(payload.recorded_at_ms);

    // The envelope is this function's, not the store's. ADR 0039 says a Candidate stays inside the
    // 64 KiB a Proposal does, and until this line the only total-length limit was SQLite's CHECK --
    // which is a constraint on what may be persisted, not on what may be built, and it reports a
    // refusal the caller cannot act on. A per-field limit does not bound a record: 900 legal paths
    // in a legal Proposal expand into an over-envelope Candidate (gpt1's service probe).
    if encoder.out.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedCandidate::OverLimitLength);
    }
    Ok(encoder.out)
}

fn decode(input: &[u8]) -> Result<CandidateV1, MalformedCandidate> {
    // Refused before a single field is read, so an over-envelope input cannot reach the entry loop
    // and choose an allocation there. The count bound below is the second gate, not the first.
    if input.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedCandidate::OverLimitLength);
    }
    let mut decoder = Decoder::new(input);
    decoder.magic()?;

    let proposal_digest = HEXLOWER.encode(&decoder.sha256()?);
    let algorithm = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
    let canonicalizer = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
    if algorithm != PROPOSAL_DIGEST_ALGORITHM || canonicalizer != PROPOSAL_CANONICALIZER {
        return Err(MalformedCandidate::ProposalDigestEnvelope);
    }

    let pinned_base_revision = decoder.string(BASE_REVISION_CHARS)?;
    if !is_base_revision(&pinned_base_revision) {
        return Err(MalformedCandidate::PinnedBaseRevision);
    }

    let count = decoder.count(MAX_CANONICAL_BYTES, MIN_ENTRY_BYTES)?;
    let mut entries: Vec<CandidateEntry> = Vec::new();
    for _ in 0..count {
        let kind = parse_mutation_tag(decoder.tag()?)?;
        let path = decoder.string(MAX_CANONICAL_BYTES)?;
        let pre_image_sha256 = decoder.image()?;
        let post_image_sha256 = decoder.image()?;
        entries.push(CandidateEntry {
            kind,
            path,
            pre_image_sha256,
            post_image_sha256,
        });
    }
    validate_entries(&entries)?;

    let recorded_at_ms = decoder.u64()?;
    decoder.finish()?;

    Ok(CandidateV1 {
        proposal: ProposalDigest {
            algorithm,
            canonicalizer,
            value: proposal_digest,
        },
        pinned_base_revision,
        entries,
        recorded_at_ms,
    })
}

/// What an Operator is shown before ruling: the exact change, and the identity of the record that
/// carries it.
///
/// **This type is the authority precondition, made structural.** R8 requires the Ruling surface to
/// present the exact diff plus the Candidate's identity before the Operator rules; a Command that
/// accepted two digest strings could be called by anyone who knows them, and "was it shown?" would
/// be prose. Every field here is private, there is no public constructor and no `Deserialize`, so
/// the only way to hold one is to have asked [`CandidateService::present`] for it — which reads the
/// Candidate through its own proof and renders what it holds.
///
/// It does not prove a human looked. Nothing can. It proves the decision was made through the
/// surface that presents the change, rather than beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidatePresentation {
    proposal: ProposalDigest,
    candidate: CandidateDigest,
    pinned_base_revision: String,
    lines: Vec<PresentedMutation>,
}

/// One declared mutation as an Operator reads it: what it does, to which path, and the exact bytes
/// before and after as their digests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedMutation {
    pub kind: MutationKind,
    pub path: String,
    pub pre_image_sha256: Option<String>,
    pub post_image_sha256: Option<String>,
}

impl CandidatePresentation {
    pub fn proposal(&self) -> &ProposalDigest {
        &self.proposal
    }

    pub fn candidate(&self) -> &CandidateDigest {
        &self.candidate
    }

    pub fn pinned_base_revision(&self) -> &str {
        &self.pinned_base_revision
    }

    /// The change, one line per declared mutation, in the Proposal's own order.
    pub fn lines(&self) -> &[PresentedMutation] {
        &self.lines
    }
}

// ---------------------------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------------------------

/// Owns authenticated Candidate recording and identity queries behind one deep interface.
///
/// Recording a Candidate records what a change would do. It decides nothing, authorizes nothing,
/// and a read of what it recorded is not a permission check.
pub struct CandidateService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl CandidateService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Records the one immutable Candidate for the exact bytes `proposal_digest` names.
    ///
    /// The caller supplies only the content images, one per declared mutation and in the
    /// Proposal's own order. The kind and the path of every entry are **copied from the verified
    /// Proposal**, so a caller cannot name an effect on a path the Proposal did not declare, and
    /// the entry list cannot drift from the mutation list it mirrors.
    ///
    /// The whole record is written in one immediate transaction: the uniqueness rule is read and
    /// enforced by the store's own index without releasing the write lock, so two concurrent
    /// callers cannot both believe they recorded the first Candidate.
    pub fn record(
        &self,
        proposal_digest: &str,
        images: &[CandidateImages],
    ) -> Result<Candidate, CandidateError> {
        self.accounts.with_authenticated_store(|connection, now| {
            digest_bytes(proposal_digest, MalformedCandidate::ProposalDigestValue)
                .map_err(CandidateError::Malformed)?;
            let recorded_at_ms = u64::try_from(now).map_err(|_| CandidateError::InvalidStore)?;

            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

            // The Proposal must exist in this Workspace and still agree with its own bytes. Its
            // declared effects are read out of those bytes, never off the row.
            let Some(facts) =
                proposal_intent_facts(&transaction, self.workspace_id, proposal_digest)
                    .map_err(map_proposal_error)?
            else {
                return Err(CandidateError::Inconsistent(
                    InconsistentCandidate::ProposalNotFound,
                ));
            };

            if images.len() != facts.declared_mutations.len() {
                return Err(CandidateError::Inconsistent(
                    InconsistentCandidate::EntryCountMismatch,
                ));
            }

            let entries = facts
                .declared_mutations
                .iter()
                .zip(images)
                .map(|(mutation, images)| CandidateEntry {
                    // COPIED from the verified Proposal, never taken from the caller. A Candidate
                    // that named its own kind and path could describe an effect nobody proposed.
                    kind: mutation.kind,
                    path: mutation.target.clone(),
                    pre_image_sha256: images.pre_image_sha256,
                    post_image_sha256: images.post_image_sha256,
                })
                .collect::<Vec<_>>();

            let payload = CandidateV1 {
                proposal: facts.digest,
                pinned_base_revision: facts.pinned_base_revision,
                entries,
                recorded_at_ms,
            };
            let canonical_bytes = encode(&payload).map_err(CandidateError::Malformed)?;
            let digest_value = HEXLOWER.encode(&sha256(&canonical_bytes));

            let stored = StoredCandidate {
                workspace_id: self.workspace_id,
                digest_value,
                canonical_bytes,
                proposal_digest: proposal_digest.to_owned(),
                pinned_base_revision: payload.pinned_base_revision.clone(),
                entry_count: payload.entries.len(),
                recorded_at_ms,
            };
            // The uniqueness rule is the store's, not this function's: a second Candidate for one
            // Proposal collides with `candidates_one_per_proposal` rather than passing a check
            // that a later caller could bypass. The collision is translated here so a caller reads
            // the rule rather than a SQLite constraint name.
            // ONLY a uniqueness violation is the one-per-Proposal rule. Translating every
            // constraint failure hid a different refusal behind it: an over-envelope record was
            // reported as "already recorded" against a Workspace with no Candidates at all
            // (gpt1's P0). The envelope is now refused by the codec above, and this mapping is
            // narrow so the next constraint to fire cannot borrow this refusal either.
            insert_candidate(&transaction, &stored).map_err(|error| match error {
                CandidateError::Store(rusqlite::Error::SqliteFailure(failure, _))
                    if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                        || failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY =>
                {
                    CandidateError::Inconsistent(InconsistentCandidate::AlreadyRecorded)
                }
                other => other,
            })?;
            let candidate = verified_candidate(stored)?;
            transaction.commit()?;
            Ok(candidate)
        })
    }

    /// Render the Proposal's Candidate for a decision.
    ///
    /// The Candidate is read through the same proof a dispatch will use, so what is presented is
    /// what is recorded rather than a convenience view assembled beside it. The returned value is
    /// the input `RulingService::rule_on_candidate` requires.
    pub fn present(&self, proposal_digest: &str) -> Result<CandidatePresentation, CandidateError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let Some(candidate) =
                verified_candidate_for_proposal(connection, self.workspace_id, proposal_digest)?
            else {
                return Err(CandidateError::Inconsistent(
                    InconsistentCandidate::NotRecorded,
                ));
            };
            Ok(present_candidate(&candidate))
        })
    }

    pub fn get(&self, digest: &str) -> Result<Candidate, CandidateError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let stored = load_stored_by_digest(connection, self.workspace_id, digest)?
                .ok_or(CandidateError::NotFound)?;
            verified_candidate(stored)
        })
    }

    /// The one Candidate recorded for a Proposal, or `None`.
    ///
    /// `None` means no Candidate has been recorded, which is a different fact from a Proposal that
    /// was never submitted -- that is refused. A dispatch acts differently on each: one is "nothing
    /// to apply yet", the other is "this decision names bytes this Workspace does not hold".
    pub fn for_proposal(&self, proposal_digest: &str) -> Result<Option<Candidate>, CandidateError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            verified_candidate_for_proposal(connection, self.workspace_id, proposal_digest)
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------------------------

/// One immutable Candidate row. Every column here is also in the bytes, and `columns_match` proves
/// the two agree on every read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredCandidate {
    workspace_id: Uuid,
    digest_value: String,
    canonical_bytes: Vec<u8>,
    proposal_digest: String,
    pinned_base_revision: String,
    entry_count: usize,
    recorded_at_ms: u64,
}

/// The authenticated command actor, which for a Candidate is always the Owner.
///
/// v1 has no Agent that can record one. The arm is closed for the reason `ruling.rs` closes its
/// decider: a one-arm vocabulary is what keeps "no Agent path exists" honest, where a check that
/// cannot fail would not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CandidateActor {
    Owner,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordedCandidate {
    digest: CandidateDigest,
    /// The exact canonical bytes, as canonical RFC 4648 padded base64.
    canonical_bytes_base64: String,
    actor: CandidateActor,
    recorded_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateRecordedPayload {
    candidate: RecordedCandidate,
}

fn digest_envelope(value: &str) -> CandidateDigest {
    CandidateDigest {
        algorithm: DIGEST_ALGORITHM.to_owned(),
        canonicalizer: CANONICALIZER.to_owned(),
        value: value.to_owned(),
    }
}

/// Writes the Candidate and its audit Event in the caller's one transaction, so the record and its
/// evidence land together or not at all.
fn insert_candidate(
    connection: &Connection,
    stored: &StoredCandidate,
) -> Result<(), CandidateError> {
    let recorded_at_ms =
        i64::try_from(stored.recorded_at_ms).map_err(|_| CandidateError::InvalidStore)?;
    let entry_count =
        i64::try_from(stored.entry_count).map_err(|_| CandidateError::InvalidStore)?;
    connection.execute(
        "INSERT INTO candidates (
             workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
             proposal_digest, proposal_digest_algorithm, proposal_digest_canonicalizer,
             pinned_base_revision, entry_count, recorded_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            stored.workspace_id.to_string(),
            DIGEST_ALGORITHM,
            CANONICALIZER,
            stored.digest_value.as_str(),
            stored.canonical_bytes.as_slice(),
            stored.proposal_digest.as_str(),
            PROPOSAL_DIGEST_ALGORITHM,
            PROPOSAL_CANONICALIZER,
            stored.pinned_base_revision.as_str(),
            entry_count,
            recorded_at_ms,
        ],
    )?;
    let payload = serde_json::to_string(&CandidateRecordedPayload {
        candidate: RecordedCandidate {
            digest: digest_envelope(&stored.digest_value),
            canonical_bytes_base64: BASE64.encode(&stored.canonical_bytes),
            actor: CandidateActor::Owner,
            recorded_at_ms: stored.recorded_at_ms,
        },
    })?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'proposal.candidate_recorded', ?2, ?3)",
        params![stored.workspace_id.to_string(), payload, recorded_at_ms],
    )?;
    Ok(())
}

const SELECT_COLUMNS: &str = "digest_algorithm, digest_canonicalizer, digest_value, \
     canonical_bytes, proposal_digest, proposal_digest_algorithm, \
     proposal_digest_canonicalizer, pinned_base_revision, entry_count, recorded_at_ms";

fn read_stored(row: &rusqlite::Row<'_>, workspace_id: Uuid) -> rusqlite::Result<StoredCandidate> {
    Ok(StoredCandidate {
        workspace_id,
        digest_value: row.get::<_, String>(2)?,
        canonical_bytes: row.get::<_, Vec<u8>>(3)?,
        proposal_digest: row.get::<_, String>(4)?,
        pinned_base_revision: row.get::<_, String>(7)?,
        entry_count: usize::try_from(row.get::<_, i64>(8)?).unwrap_or(usize::MAX),
        recorded_at_ms: u64::try_from(row.get::<_, i64>(9)?).unwrap_or(u64::MAX),
    })
}

/// The three descriptor columns are checked here rather than trusted: a row whose algorithm or
/// canonicalizer is not this module's is invalid state, never a Candidate with a caveat.
fn check_descriptors(row: &rusqlite::Row<'_>) -> rusqlite::Result<bool> {
    Ok(row.get::<_, String>(0)? == DIGEST_ALGORITHM
        && row.get::<_, String>(1)? == CANONICALIZER
        && row.get::<_, String>(5)? == PROPOSAL_DIGEST_ALGORITHM
        && row.get::<_, String>(6)? == PROPOSAL_CANONICALIZER)
}

fn load_stored_by_digest(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<StoredCandidate>, CandidateError> {
    let row = connection
        .query_row(
            &format!(
                "SELECT {SELECT_COLUMNS} FROM candidates
                 WHERE workspace_id = ?1 AND digest_value = ?2"
            ),
            params![workspace_id.to_string(), digest_value],
            |row| Ok((check_descriptors(row)?, read_stored(row, workspace_id)?)),
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((false, _)) => Err(CandidateError::InvalidStore),
        Some((true, stored)) => Ok(Some(stored)),
    }
}

fn load_stored_for_proposal(
    connection: &Connection,
    workspace_id: Uuid,
    proposal_digest: &str,
) -> Result<Option<StoredCandidate>, CandidateError> {
    let row = connection
        .query_row(
            &format!(
                "SELECT {SELECT_COLUMNS} FROM candidates
                 WHERE workspace_id = ?1 AND proposal_digest = ?2"
            ),
            params![workspace_id.to_string(), proposal_digest],
            |row| Ok((check_descriptors(row)?, read_stored(row, workspace_id)?)),
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((false, _)) => Err(CandidateError::InvalidStore),
        Some((true, stored)) => Ok(Some(stored)),
    }
}

/// Rebuilds a Candidate from its stored bytes, proving on every read what recording proved once:
/// the digest names these bytes, the bytes decode, they re-encode identically, and every indexed
/// column repeats what the bytes say.
fn verified_candidate(stored: StoredCandidate) -> Result<Candidate, CandidateError> {
    if HEXLOWER.encode(&sha256(&stored.canonical_bytes)) != stored.digest_value {
        return Err(CandidateError::InvalidStore);
    }
    let payload = decode(&stored.canonical_bytes).map_err(|_| CandidateError::InvalidStore)?;
    // ADR 0039's discipline, and ADR 0037's before it: strictly decode AND re-encode. No sequence
    // this grammar can decode re-encodes differently -- every field is fixed-width or exactly
    // length-prefixed -- so no test can reach this refusal today. It stays because the day a field
    // becomes optional or a second spelling appears, this is the line that refuses it before it
    // becomes a permanent second identity.
    if encode(&payload).map_err(|_| CandidateError::InvalidStore)? != stored.canonical_bytes {
        return Err(CandidateError::InvalidStore);
    }
    if !columns_match(&stored, &payload) {
        return Err(CandidateError::InvalidStore);
    }
    Ok(Candidate {
        digest: digest_envelope(&stored.digest_value),
        canonical_bytes: stored.canonical_bytes,
        payload,
    })
}

/// Every indexed column against the parsed value. Checked on every read rather than only at
/// startup, so a row and its bytes cannot disagree even between two restarts.
fn columns_match(stored: &StoredCandidate, payload: &CandidateV1) -> bool {
    stored.proposal_digest == payload.proposal.value
        && stored.pinned_base_revision == payload.pinned_base_revision
        && stored.entry_count == payload.entries.len()
        && stored.recorded_at_ms == payload.recorded_at_ms
}

/// Exactly one `proposal.candidate_recorded` Event names this Candidate.
///
/// A record whose evidence is missing, or duplicated, is not a Candidate with a caveat. Checked at
/// startup over the whole table and again by [`verified_candidate_for_proposal`] for the one record
/// a consumer is about to act on, because a consumer that acts inside its own transaction cannot
/// borrow the last startup's conclusion.
fn require_one_event(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<(), CandidateError> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM events
         WHERE workspace_id = ?1 AND kind = 'proposal.candidate_recorded'
           AND json_extract(payload_json, '$.candidate.digest.value') = ?2",
        params![workspace_id.to_string(), digest_value],
        |row| row.get(0),
    )?;
    if count == 1 {
        Ok(())
    } else {
        Err(CandidateError::InvalidStore)
    }
}

/// Exactly one Event names this Candidate **and that Event verifies**: its actor, its time, its
/// envelope, and its canonical bytes, through the same function startup replay uses.
fn require_verified_event(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<(), CandidateError> {
    require_one_event(connection, workspace_id, digest_value)?;
    let (payload_json, occurred_at_ms) = connection.query_row(
        "SELECT payload_json, occurred_at_ms FROM events
         WHERE workspace_id = ?1 AND kind = 'proposal.candidate_recorded'
           AND json_extract(payload_json, '$.candidate.digest.value') = ?2",
        params![workspace_id.to_string(), digest_value],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let occurred_at_ms = u64::try_from(occurred_at_ms).map_err(|_| CandidateError::InvalidStore)?;
    verify_candidate_event(connection, workspace_id, occurred_at_ms, &payload_json)
}

/// The `events.sequence` of the one Event that recorded a Candidate. See
/// [`crate::ruling::ruling_event_sequence`] for why sequence rather than time.
pub(crate) fn candidate_event_sequence(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<i64>, CandidateError> {
    connection
        .query_row(
            "SELECT sequence FROM events
             WHERE workspace_id = ?1 AND kind = 'proposal.candidate_recorded'
               AND json_extract(payload_json, '$.candidate.digest.value') = ?2",
            params![workspace_id.to_string(), digest_value],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(CandidateError::Store)
}

/// The one Candidate for a Proposal, proved from bytes **inside the caller's transaction**.
///
/// This exists because a snapshot is not a fact about the moment it is used. `CandidateService`
/// methods open their own authenticated store closure and return a value derived at that instant;
/// a consumer that must decide inside one serialized transaction -- dispatch, when it lands --
/// cannot treat such a value as current, and must not assemble the record by joining tables
/// itself. So the proof lives here, once, and runs against the connection the caller is already
/// holding: the digest names these bytes, they decode, they re-encode identically, every indexed
/// column repeats them, exactly one Event carries them, and the entries still match the Proposal's
/// declared mutations in order.
///
/// `Ok(None)` means the Proposal has no Candidate, which is a different fact from a Proposal this
/// Workspace never recorded -- that is `ProposalNotFound` -- and from a Candidate that disagrees
/// with itself, which is `InvalidStore`. A consumer acts differently on each.
pub(crate) fn verified_candidate_for_proposal(
    connection: &Connection,
    workspace_id: Uuid,
    proposal_digest: &str,
) -> Result<Option<Candidate>, CandidateError> {
    let Some(facts) = proposal_intent_facts(connection, workspace_id, proposal_digest)
        .map_err(map_proposal_error)?
    else {
        return Err(CandidateError::Inconsistent(
            InconsistentCandidate::ProposalNotFound,
        ));
    };
    let Some(stored) = load_stored_for_proposal(connection, workspace_id, proposal_digest)? else {
        return Ok(None);
    };
    let digest_value = stored.digest_value.clone();
    let candidate = verified_candidate(stored)?;
    // The owning Event verifier, against the exact row -- not a count of matching digests. A probe
    // changed only `$.candidate.actor` to a value the closed vocabulary does not have and this
    // reader still handed the Candidate out, because counting rows proves a row exists and nothing
    // about what it says (gpt1).
    require_verified_event(connection, workspace_id, &digest_value)?;
    if !agrees_with_proposal(&facts, candidate.payload()) {
        return Err(CandidateError::InvalidStore);
    }
    Ok(Some(candidate))
}

/// Render one verified Candidate. `pub(crate)` so the Ruling service can build the same value from
/// the record it just proved, rather than re-reading it through a second path.
pub(crate) fn present_candidate(candidate: &Candidate) -> CandidatePresentation {
    CandidatePresentation {
        proposal: candidate.payload().proposal.clone(),
        candidate: candidate.digest().clone(),
        pinned_base_revision: candidate.payload().pinned_base_revision.clone(),
        lines: candidate
            .payload()
            .entries
            .iter()
            .map(|entry| PresentedMutation {
                kind: entry.kind,
                path: entry.path.clone(),
                pre_image_sha256: entry.pre_image_sha256.as_ref().map(|v| HEXLOWER.encode(v)),
                post_image_sha256: entry.post_image_sha256.as_ref().map(|v| HEXLOWER.encode(v)),
            })
            .collect(),
    }
}

/// The copy rule: a Candidate's entries are the Proposal's declared mutations, in order, at the
/// Proposal's own pinned base. Checked wherever a Candidate is read for use, not only at startup.
fn agrees_with_proposal(facts: &ProposalIntentFacts, payload: &CandidateV1) -> bool {
    facts.pinned_base_revision == payload.pinned_base_revision
        && facts.declared_mutations.len() == payload.entries.len()
        && facts
            .declared_mutations
            .iter()
            .zip(&payload.entries)
            .all(|(mutation, entry)| mutation.kind == entry.kind && mutation.target == entry.path)
}

// ---------------------------------------------------------------------------------------------
// Startup reconciliation
// ---------------------------------------------------------------------------------------------

/// Recomputes every Candidate from its stored bytes, re-verifies what it is bound to, and requires
/// exactly one Event per Candidate with no orphan.
///
/// What it deliberately does not do is re-derive anything about the world: a Candidate records what
/// a change would do to a tree at one revision, and that record does not become invalid because the
/// tree moved. Dispatch re-establishes every live fact itself, at dispatch time.
/// The rows `verify_candidate_state` enumerates, in the order it enumerates them.
///
/// EXTRACTED SO A BAR CAN CALL IT (R44.6): a placement assertion that COPIES this query agrees
/// with itself, so flipping the pass's own `ORDER BY` leaves it decorative. One function, called
/// by both, moves the pass and the bar's view of it together.
pub fn enumerate_candidates(
    connection: &Connection,
) -> rusqlite::Result<Vec<(String, String, String)>> {
    let mut statement = connection.prepare(
        "SELECT workspace_id, digest_value, proposal_digest FROM candidates
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
    Ok(rows)
}

pub(crate) fn verify_candidate_state(connection: &Connection) -> Result<(), CandidateStateFault> {
    let rows = enumerate_candidates(connection).map_err(CandidateStateFault::Store)?;

    for (workspace_id, digest_value, proposal_digest) in &rows {
        let identity = || CandidateStateFault::Identity {
            workspace_id: workspace_id.clone(),
            digest_value: digest_value.clone(),
        };
        let parsed = Uuid::parse_str(workspace_id).map_err(|_| identity())?;
        verify_one_candidate(connection, parsed, digest_value, proposal_digest).map_err(
            |cause| CandidateStateFault::Row {
                workspace_id: parsed,
                digest_value: digest_value.clone(),
                cause,
            },
        )?;
    }
    // No whole-table population comparison here, deliberately -- which is why this pass has no
    // `Totals` arm. An Event naming a digest no row carries is refused by
    // `verify_candidate_event` during the startup replay, which walks every event row, so a second
    // count could not be reached by any store this code can produce, and a guard nothing can
    // exercise is a claim rather than a check (gpt1, twice).
    // `an_event_whose_digest_names_no_candidate_is_refused_by_the_replay` pins the guard that does
    // the work.
    Ok(())
}

/// One stored Candidate, re-proved. Unchanged from what the loop held inline.
fn verify_one_candidate(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
    proposal_digest: &str,
) -> Result<(), CandidateError> {
    {
        let stored = load_stored_by_digest(connection, workspace_id, digest_value)?
            .ok_or(CandidateError::InvalidStore)?;
        let candidate = verified_candidate(stored)?;

        // What the Candidate copied must still be what the Proposal declared. This is the check no
        // read performs, and it is the whole point of copying: a Candidate whose entries disagree
        // with its Proposal's mutations would describe an effect nobody proposed while every
        // digest still verified.
        let facts = proposal_intent_facts(connection, workspace_id, proposal_digest)
            .map_err(map_proposal_error)?
            .ok_or(CandidateError::InvalidStore)?;
        if !agrees_with_proposal(&facts, candidate.payload()) {
            return Err(CandidateError::InvalidStore);
        }

        require_one_event(connection, workspace_id, digest_value)?;
    }
    Ok(())
}

/// Requires one Event per Candidate whose decoded byte field exactly equals the stored bytes, and
/// whose encoding is the canonical padded base64 of them -- a differently encoded payload carrying
/// the same bytes is different evidence, not the same evidence reformatted.
pub(crate) fn verify_candidate_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), CandidateError> {
    let payload: CandidateRecordedPayload = serde_json::from_str(payload_json)?;
    let recorded = payload.candidate;
    if recorded.digest.algorithm != DIGEST_ALGORITHM
        || recorded.digest.canonicalizer != CANONICALIZER
        || recorded.recorded_at_ms != occurred_at_ms
    {
        return Err(CandidateError::InvalidStore);
    }
    // Bounded before it is decoded: `events.payload_json` carries no length constraint, so an
    // Event is an unbounded string until something says otherwise, and base64 of a record that
    // cannot exist is not worth allocating (gpt1's P0).
    if recorded.canonical_bytes_base64.len() > MAX_BASE64_ENVELOPE_BYTES {
        return Err(CandidateError::InvalidStore);
    }
    let stored = load_stored_by_digest(connection, workspace_id, &recorded.digest.value)?
        .ok_or(CandidateError::InvalidStore)?;
    let bytes = BASE64
        .decode(recorded.canonical_bytes_base64.as_bytes())
        .map_err(|_| CandidateError::InvalidStore)?;
    if bytes != stored.canonical_bytes
        || BASE64.encode(&stored.canonical_bytes) != recorded.canonical_bytes_base64
        || stored.recorded_at_ms != recorded.recorded_at_ms
    {
        return Err(CandidateError::InvalidStore);
    }
    verified_candidate(stored).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fabricated Proposal digest. Patterned rather than random so no reader mistakes it for
    /// something that exists.
    const PROPOSAL_DIGEST: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const BASE_REVISION: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
    const RECORDED_AT_MS: u64 = 1_756_000_000_000;

    /// The exact canonical bytes of [`fixture`], recorded once and pinned here.
    ///
    /// Independence matters more than convenience: this constant and [`FIXTURE_DIGEST`] were
    /// produced outside the code under test, by writing ADR 0039's table out by hand, so an encoder
    /// that silently changed its layout would fail against them rather than agree with itself.
    /// `tests/candidate.rs` builds the same grammar a third time, through the service.
    const FIXTURE_RECORD_HEX: &str = concat!(
        "626561636f6e2e63616e6469646174652e763100aaaaaaaaaaaaaaaaaaaaaaaa",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa000000000000000673686132",
        "35360000000000000012626561636f6e2e70726f706f73616c2e763100000000",
        "0000002861316232633364346535663661316232633364346535663661316232",
        "6333643465356636613162320000000302000000000000000a7372632f6c6962",
        "2e72730111111111111111111111111111111111111111111111111111111111",
        "1111111101222222222222222222222222222222222222222222222222222222",
        "222222222203000000000000000a7372632f6f6c642e72730133333333333333",
        "3333333333333333333333333333333333333333333333333300010000000000",
        "00000b646f63732f6e65772e6d64000144444444444444444444444444444444",
        "4444444444444444444444444444444400000198d9c19800",
    );
    const FIXTURE_DIGEST: &str = "fb7c33826e705f1d91b1a607f089561d77d997e502385393ff4b44bbca1349d4";

    fn entry(kind: MutationKind, path: &str, pre: Option<u8>, post: Option<u8>) -> CandidateEntry {
        CandidateEntry {
            kind,
            path: path.to_owned(),
            pre_image_sha256: pre.map(|byte| [byte; 32]),
            post_image_sha256: post.map(|byte| [byte; 32]),
        }
    }

    /// One Candidate carrying all three kinds, in an order the sorted path list does not reproduce.
    fn fixture() -> CandidateV1 {
        CandidateV1 {
            proposal: ProposalDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.proposal.v1".to_owned(),
                value: PROPOSAL_DIGEST.to_owned(),
            },
            pinned_base_revision: BASE_REVISION.to_owned(),
            entries: vec![
                entry(
                    MutationKind::ModifyFile,
                    "src/lib.rs",
                    Some(0x11),
                    Some(0x22),
                ),
                entry(MutationKind::DeleteFile, "src/old.rs", Some(0x33), None),
                entry(MutationKind::CreateFile, "docs/new.md", None, Some(0x44)),
            ],
            recorded_at_ms: RECORDED_AT_MS,
        }
    }

    fn fixture_bytes() -> Vec<u8> {
        encode(&fixture()).unwrap()
    }

    #[test]
    fn a_known_fixture_reproduces_its_digest_from_the_bytes_alone() {
        // The encoder must produce exactly the recorded bytes...
        assert_eq!(HEXLOWER.encode(&fixture_bytes()), FIXTURE_RECORD_HEX);
        // ...and the recorded digest must be the digest of the recorded bytes, computed without
        // going through the encoder at all. Two claims: an encoder that drifted fails the first,
        // and a digest taken over the wrong preimage fails the second.
        let recorded = HEXLOWER.decode(FIXTURE_RECORD_HEX.as_bytes()).unwrap();
        assert_eq!(HEXLOWER.encode(&sha256(&recorded)), FIXTURE_DIGEST);
    }

    /// gpt2's cross-lab vector, re-derived here.
    ///
    /// A non-author reviewer wrote an encoder for this grammar from ADR 0039's table alone, in
    /// another lab, and published the bytes it produced for the values below. Its record ends at
    /// the last entry; this grammar closes with `recorded_at_ms`, which is the one field the
    /// design's summary did not name and the ADR now does. Everything before that trailing u64 is
    /// byte-identical, and this test pins both facts: the 334-byte prefix is exactly what the
    /// independent encoder produced, and the whole 342-byte record is what this one does.
    ///
    /// A fixture written by the code under test can only prove self-consistency. This one cannot:
    /// nothing in this repository produced the constants below.
    const CROSS_LAB_PREFIX_LEN: usize = 334;
    const CROSS_LAB_PREFIX_DIGEST: &str =
        "05d7e247e3285b18cf2579b3486e5f12244fa6ebdb532ea1f20d6c7417b01ceb";
    const CROSS_LAB_DIGEST: &str =
        "532c2311082d3eb98d0bde53a9f276fd02ad52f12985096d7c6f64dce9d36ff1";
    const CROSS_LAB_RECORDED_AT_MS: u64 = 1_756_000_000_000;

    fn ranged(start: u8) -> [u8; 32] {
        let mut value = [0_u8; 32];
        for (offset, byte) in value.iter_mut().enumerate() {
            *byte = start.wrapping_add(u8::try_from(offset).unwrap());
        }
        value
    }

    #[test]
    fn the_cross_lab_vector_reproduces_byte_for_byte() {
        let payload = CandidateV1 {
            proposal: ProposalDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.proposal.v1".to_owned(),
                value: HEXLOWER.encode(&ranged(0x00)),
            },
            pinned_base_revision: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            entries: vec![
                CandidateEntry {
                    kind: MutationKind::CreateFile,
                    path: "docs/new.txt".to_owned(),
                    pre_image_sha256: None,
                    post_image_sha256: Some(ranged(0x20)),
                },
                CandidateEntry {
                    kind: MutationKind::ModifyFile,
                    path: "src/lib.rs".to_owned(),
                    pre_image_sha256: Some(ranged(0x40)),
                    post_image_sha256: Some(ranged(0x60)),
                },
                CandidateEntry {
                    kind: MutationKind::DeleteFile,
                    path: "old.txt".to_owned(),
                    pre_image_sha256: Some(ranged(0x80)),
                    post_image_sha256: None,
                },
            ],
            recorded_at_ms: CROSS_LAB_RECORDED_AT_MS,
        };

        let bytes = encode(&payload).unwrap();
        assert_eq!(
            bytes.len(),
            CROSS_LAB_PREFIX_LEN + 8,
            "the record is the independent one plus its trailing recorded_at_ms"
        );
        assert_eq!(
            HEXLOWER.encode(&sha256(&bytes[..CROSS_LAB_PREFIX_LEN])),
            CROSS_LAB_PREFIX_DIGEST,
            "the bytes before recorded_at_ms are not the independently produced ones"
        );
        assert_eq!(HEXLOWER.encode(&sha256(&bytes)), CROSS_LAB_DIGEST);
        assert_eq!(decode(&bytes).unwrap(), payload);
    }

    #[test]
    fn the_record_starts_with_the_canonicalizer_magic() {
        let bytes = fixture_bytes();
        assert_eq!(&bytes[..MAGIC.len()], MAGIC);
        assert_eq!(MAGIC, b"beacon.candidate.v1\0");

        let mut wrong = bytes.clone();
        wrong[0] = b'B';
        assert_eq!(decode(&wrong), Err(MalformedCandidate::Magic));
        assert_eq!(decode(b"short"), Err(MalformedCandidate::Magic));
    }

    #[test]
    fn decode_then_encode_is_the_identity_for_every_mutation_kind() {
        // Round-tripping the fixture covers all three kinds and both image positions at once: a
        // grammar with a second spelling of one value would re-encode to different bytes here.
        let bytes = fixture_bytes();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, fixture());
        assert_eq!(encode(&decoded).unwrap(), bytes);

        // And each kind alone, so a defect in one arm cannot hide behind the other two.
        for entry in [
            entry(MutationKind::CreateFile, "a.txt", None, Some(0x01)),
            entry(MutationKind::ModifyFile, "b.txt", Some(0x02), Some(0x03)),
            entry(MutationKind::DeleteFile, "c.txt", Some(0x04), None),
        ] {
            let mut payload = fixture();
            payload.entries = vec![entry];
            let bytes = encode(&payload).unwrap();
            assert_eq!(decode(&bytes).unwrap(), payload);
            assert_eq!(encode(&decode(&bytes).unwrap()).unwrap(), bytes);
        }
    }

    #[test]
    fn every_single_byte_of_the_fixture_is_load_bearing() {
        let bytes = fixture_bytes();
        let payload = fixture();
        for offset in 0..bytes.len() {
            let mut mutated = bytes.clone();
            mutated[offset] ^= 0x01;
            match decode(&mutated) {
                // Refused: the byte cannot be changed at all.
                Err(_) => {}
                Ok(decoded) => {
                    // Accepted: it must name a different Candidate, and re-encode to exactly the
                    // bytes it was decoded from. A byte that changes nothing would be a second
                    // spelling of one identity.
                    assert_ne!(decoded, payload, "byte {offset} changed nothing");
                    assert_eq!(encode(&decoded).unwrap(), mutated, "byte {offset}");
                }
            }
        }
    }

    #[test]
    fn a_delete_carries_no_post_image_and_a_create_carries_no_pre_image() {
        // Which images a kind carries is a fact about the kind, not a caller's choice: a created
        // file has no before, a deleted file has no after. Both directions refuse the four wrong
        // combinations, so a record that could be written could always be read back.
        let cases = [
            entry(MutationKind::CreateFile, "a.txt", Some(0x01), Some(0x02)),
            entry(MutationKind::CreateFile, "a.txt", None, None),
            entry(MutationKind::DeleteFile, "a.txt", Some(0x01), Some(0x02)),
            entry(MutationKind::DeleteFile, "a.txt", None, None),
            entry(MutationKind::ModifyFile, "a.txt", None, Some(0x02)),
            entry(MutationKind::ModifyFile, "a.txt", Some(0x01), None),
        ];
        for case in cases {
            let mut payload = fixture();
            payload.entries = vec![case.clone()];
            assert_eq!(
                encode(&payload),
                Err(MalformedCandidate::ImageForKind),
                "{case:?} encoded"
            );
        }

        // And the decoder refuses the same shapes, built by hand so the encoder cannot be the
        // thing that produced them: a create carrying a pre-image.
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&[0xaa; 32]);
        for token in ["sha256", "beacon.proposal.v1", BASE_REVISION] {
            bytes.extend_from_slice(&(token.len() as u64).to_be_bytes());
            bytes.extend_from_slice(token.as_bytes());
        }
        bytes.extend_from_slice(&1_u32.to_be_bytes());
        bytes.push(TAG_MUTATION_CREATE_FILE);
        bytes.extend_from_slice(&5_u64.to_be_bytes());
        bytes.extend_from_slice(b"a.txt");
        bytes.push(TAG_IMAGE_PRESENT);
        bytes.extend_from_slice(&[0x01; 32]);
        bytes.push(TAG_IMAGE_PRESENT);
        bytes.extend_from_slice(&[0x02; 32]);
        bytes.extend_from_slice(&RECORDED_AT_MS.to_be_bytes());
        assert_eq!(decode(&bytes), Err(MalformedCandidate::ImageForKind));
    }

    #[test]
    fn two_entries_naming_one_path_are_refused() {
        // v1 dispatch applies one effect per path. Two entries would leave the resulting diff with
        // one row for two declarations, and the post-condition that proves "exactly what was
        // declared" could not be stated. Both spellings of the collision are refused: the same
        // kind twice, and two kinds on one path.
        for second in [
            entry(
                MutationKind::ModifyFile,
                "src/lib.rs",
                Some(0x55),
                Some(0x66),
            ),
            entry(MutationKind::DeleteFile, "src/lib.rs", Some(0x55), None),
        ] {
            let mut payload = fixture();
            payload.entries = vec![
                entry(
                    MutationKind::ModifyFile,
                    "src/lib.rs",
                    Some(0x11),
                    Some(0x22),
                ),
                second,
            ];
            assert_eq!(encode(&payload), Err(MalformedCandidate::DuplicatePath));
        }
    }

    #[test]
    fn a_modification_that_changes_nothing_is_refused() {
        // The mutation it mirrors declared an effect. Identical before and after declares none,
        // and a dispatch of it would produce a commit whose diff omits the path.
        let mut payload = fixture();
        payload.entries = vec![entry(
            MutationKind::ModifyFile,
            "src/lib.rs",
            Some(0x11),
            Some(0x11),
        )];
        assert_eq!(encode(&payload), Err(MalformedCandidate::EmptyEffect));

        // The positive control: one byte different and the same record encodes.
        payload.entries[0].post_image_sha256 = Some([0x12; 32]);
        assert!(encode(&payload).is_ok());
    }

    #[test]
    fn a_candidate_with_no_entries_is_refused() {
        let mut payload = fixture();
        payload.entries.clear();
        assert_eq!(encode(&payload), Err(MalformedCandidate::NoEntries));
    }

    #[test]
    fn an_unassigned_tag_is_refused_in_every_closed_position() {
        // Every closed vocabulary refuses what it was not assigned, rather than defaulting to an
        // arm. The offsets are computed from the fixture rather than hard-coded, so this test does
        // not quietly stop pointing at the tag it means when a field moves.
        let bytes = fixture_bytes();
        let kind_offset = MAGIC.len() + 32 + (8 + 6) + (8 + 18) + (8 + 40) + 4;
        assert_eq!(bytes[kind_offset], TAG_MUTATION_MODIFY_FILE);
        let image_offset = kind_offset + 1 + 8 + "src/lib.rs".len();
        assert_eq!(bytes[image_offset], TAG_IMAGE_PRESENT);

        for offset in [kind_offset, image_offset] {
            let mut mutated = bytes.clone();
            mutated[offset] = 0x7f;
            assert_eq!(
                decode(&mutated),
                Err(MalformedCandidate::UnknownTag),
                "an unassigned tag at {offset} was accepted"
            );
        }
    }

    #[test]
    fn an_uppercase_digest_or_base_revision_is_refused_rather_than_folded() {
        // Two spellings of one value would be two identities for one Candidate. The encoder
        // refuses both rather than lowercasing either.
        let mut payload = fixture();
        payload.proposal.value = PROPOSAL_DIGEST.to_uppercase();
        assert_eq!(
            encode(&payload),
            Err(MalformedCandidate::ProposalDigestValue)
        );

        let mut payload = fixture();
        payload.pinned_base_revision = BASE_REVISION.to_uppercase();
        assert_eq!(
            encode(&payload),
            Err(MalformedCandidate::PinnedBaseRevision)
        );

        // And bytes that carry an uppercase revision are refused by the decoder, not folded: the
        // encoder is not the only way bytes reach this store.
        let bytes = fixture_bytes();
        let revision_offset = MAGIC.len() + 32 + (8 + 6) + (8 + 18) + 8;
        let mut mutated = bytes.clone();
        mutated[revision_offset] = b'A';
        assert_eq!(
            decode(&mutated),
            Err(MalformedCandidate::PinnedBaseRevision)
        );
    }

    #[test]
    fn a_proposal_digest_naming_another_canonicalizer_is_refused() {
        let mut payload = fixture();
        payload.proposal.canonicalizer = "beacon.proposal.v2".to_owned();
        assert_eq!(
            encode(&payload),
            Err(MalformedCandidate::ProposalDigestEnvelope)
        );

        let mut payload = fixture();
        payload.proposal.algorithm = "sha512".to_owned();
        assert_eq!(
            encode(&payload),
            Err(MalformedCandidate::ProposalDigestEnvelope)
        );
    }

    #[test]
    fn a_declared_count_is_bounded_before_the_loop_reads_anything() {
        // The bound is the function's own and is applied before allocation: a count of four
        // billion entries cannot fit in the remaining bytes, and is refused by arithmetic rather
        // than by running out of memory.
        let bytes = fixture_bytes();
        let count_offset = MAGIC.len() + 32 + (8 + 6) + (8 + 18) + (8 + 40);
        let mut mutated = bytes.clone();
        mutated[count_offset..count_offset + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(decode(&mutated), Err(MalformedCandidate::OverLimitLength));

        // One short of the limit still fails on the input it does not have, rather than on the
        // limit: the two refusals are different facts.
        let mut mutated = bytes.clone();
        mutated[count_offset..count_offset + 4].copy_from_slice(&64_u32.to_be_bytes());
        assert_eq!(decode(&mutated), Err(MalformedCandidate::Truncated));
    }

    #[test]
    fn trailing_bytes_and_truncation_are_different_refusals() {
        let bytes = fixture_bytes();
        let mut longer = bytes.clone();
        longer.push(0x00);
        assert_eq!(decode(&longer), Err(MalformedCandidate::TrailingBytes));
        assert_eq!(
            decode(&bytes[..bytes.len() - 1]),
            Err(MalformedCandidate::Truncated)
        );
    }

    #[test]
    fn a_path_that_is_not_repository_relative_is_refused() {
        for path in ["/etc/passwd", "../escape", "", "a/../b", "a//b"] {
            let mut payload = fixture();
            payload.entries = vec![entry(MutationKind::CreateFile, path, None, Some(0x01))];
            assert_eq!(
                encode(&payload),
                Err(MalformedCandidate::TargetPath),
                "{path:?} encoded"
            );
        }
    }
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()` (the fold at 30d2a20): an exhaustive `name()` catches an
    /// added arm, only a second table catches a wrong or duplicated name.
    fn expected_name(error: &CandidateError) -> &'static str {
        match error {
            CandidateError::Account(_) => "Account",
            CandidateError::Malformed(_) => "Malformed",
            CandidateError::Inconsistent(_) => "Inconsistent",
            CandidateError::NotFound => "NotFound",
            CandidateError::InvalidStore => "InvalidStore",
            CandidateError::Store(_) => "Store",
            CandidateError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_candidate_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            CandidateError::Account(
                crate::operator_account::OperatorAccountError::PasswordTooShort,
            ),
            CandidateError::Malformed(MalformedCandidate::Magic),
            CandidateError::Inconsistent(InconsistentCandidate::ProposalNotFound),
            CandidateError::NotFound,
            CandidateError::InvalidStore,
            CandidateError::Store(rusqlite::Error::QueryReturnedNoRows),
            CandidateError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            7,
            "an arm was added to `CandidateError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(CandidateError::name).collect();
        assert_eq!(
            names.len(),
            7,
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
