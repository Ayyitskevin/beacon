//! Immutable Proposal v1, bound to the exact bytes Beacon stores (ADR 0034).
//!
//! A Proposal is an immutable description of intended effects, inputs, bounds, checks, and budget,
//! identified by its digest. This module is the whole of that identity: one fixed binary
//! canonicalizer, one authenticated `submit(canonical_bytes)`, one immutable row, one audit Event.
//!
//! It deliberately creates no authority. Nothing here opens a Gate, records a Ruling, consults an
//! execution capability, prepares a worktree, starts a process, invokes a model, makes a network
//! request, populates a Receipt, or dispatches anything. `submit` records that an Owner asked for
//! this description to be reviewed; it does not decide that the description may proceed.
//!
//! The bytes are the record. There is no public function that accepts a parsed value and a digest
//! as independent arguments, because two such arguments can disagree.

use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use data_encoding::{BASE64, HEXLOWER};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::git_object_store::{ObjectStore, ObjectStoreError};
use crate::mission::mission_admission_facts;
use crate::operator_account::owner_account_exists;
use crate::project::current_charter_revision;
use crate::project_policy::policy_revision_facts;
use crate::project_repository::{
    DeclaredCheck, RepositorySourceKind, latest_repository_for_preflight,
    stored_repository_for_preflight,
};
use crate::resident_configuration::current_configuration;
use crate::resident_lifecycle::{ResidentLifecycle, load_resident};
use crate::{
    MissionError, OperatorAccountError, OperatorAccountService, ProjectError, ProjectPolicyError,
    ProjectRepositoryError, ResidentConfigurationError, ResidentLifecycleError,
};

/// The canonicalizer identity. Any future grammar is a new canonicalizer; existing v1 bytes are
/// never reinterpreted under v2 rules.
const CANONICALIZER: &str = "beacon.proposal.v1";
const DIGEST_ALGORITHM: &str = "sha256";
const BOOTSTRAP_BASIS: &str = "owner";

/// The literal ASCII magic that starts every record, including its terminating NUL.
const MAGIC: &[u8] = b"beacon.proposal.v1\0";

/// `submit` refuses a slice larger than this before parsing or making any size-derived allocation.
const MAX_CANONICAL_BYTES: usize = 64 * 1024;

const MAX_SUMMARY_SCALARS: usize = 2_000;
const MAX_BOOTSTRAP_REASON_SCALARS: usize = 500;
const MAX_SUMMARY_BYTES: usize = MAX_SUMMARY_SCALARS * 4;
const MAX_BOOTSTRAP_REASON_BYTES: usize = MAX_BOOTSTRAP_REASON_SCALARS * 4;
// A string or list field's own limit is the canonical envelope, not an invented per-field number
// (S4, gpt2 non-author review of PR #32). ADR 0034 states exactly two size limits: the 64 KiB
// whole-record envelope (`MAX_CANONICAL_BYTES`) and the 40-character pinned base revision
// (`BASE_REVISION_CHARS`). Where a field must exact-byte-match another module's Binding --
// declared-check names, arguments, and working directories -- the limit is that module's own,
// never a tighter one this grammar would invent: `MAX_CHECK_NAME_BYTES` mirrors
// `project_repository.rs`'s `MAX_CHECK_NAME_CHARS`, and Repository Binding places no length limit
// at all on an argument or a working directory, so none is placed here either. Every other string
// and list below is bounded by `MAX_CANONICAL_BYTES` alone: a declared length or count that could
// not possibly fit the record is refused before any entry is decoded (Decoder::string,
// Decoder::count), which is what "the decoder first proves it fits within the remaining input and
// the field's own limit" requires -- it does not require a second, narrower limit to exist.
const MAX_CHECK_NAME_BYTES: usize = 64;
/// ADR 0034: "`kind` and `canonicalizer` are lowercase ASCII tokens of at most 64 bytes."
const MAX_DESCRIPTOR_TOKEN_BYTES: usize = 64;
/// ADR 0034: "the pinned base revision must be a lowercase 40-character hexadecimal commit id."
const BASE_REVISION_CHARS: usize = 40;

/// How long the readable-context comparison may spend inspecting an object store before it
/// reports that it does not know. Mirrors Check Preflight's budget, because it is the same
/// inspection against the same store; an elapsed budget is never a negative answer.
const CONTEXT_INSPECTION_BUDGET_MS: u128 = 5_000;

/// The smallest number of bytes one entry of a list can occupy. A declared count is proved against
/// the remaining input using these before a single entry is decoded, and no count is ever used to
/// reserve capacity.
const MIN_STRING_ENTRY_BYTES: usize = 8;
const MIN_CONTEXT_ENTRY_BYTES: usize = 8 + 32;
const MIN_MUTATION_ENTRY_BYTES: usize = 1 + 8;
const MIN_DECLARED_CHECK_ENTRY_BYTES: usize = 8 + 4 + 8;

// One-byte tags. Assigned explicitly and never renumbered. 0x00 is unassigned in every position,
// so a zero-filled buffer cannot decode as a valid variant.
const TAG_BOOTSTRAP_OWNER: u8 = 0x01;
const TAG_CONTEXT_NOT_DECLARED: u8 = 0x01;
const TAG_CONTEXT_DECLARED: u8 = 0x02;
const TAG_MUTATION_CREATE_FILE: u8 = 0x01;
const TAG_MUTATION_MODIFY_FILE: u8 = 0x02;
const TAG_MUTATION_DELETE_FILE: u8 = 0x03;
const TAG_CONSTRAINT_NOT_DECLARED: u8 = 0x01;
const TAG_CONSTRAINT_BOUND: u8 = 0x02;
const TAG_BUDGET_NO_SPEND: u8 = 0x01;
const TAG_ROLLBACK_DISCARD_DISPOSABLE_WORKTREE: u8 = 0x01;

// ---------------------------------------------------------------------------------------------
// The immutable v1 payload
// ---------------------------------------------------------------------------------------------

/// The Workspace, Project, and Mission a Proposal belongs to.
///
/// Project and Mission are both present even though one can be found through the other. The
/// redundant scope makes a cross-Workspace or cross-Project substitution independently detectable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalScope {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub mission_id: Uuid,
}

/// The only v1 authorization basis.
///
/// There is no Operator UUID in the current singleton account model, so v1 records the closed
/// basis `owner` rather than an invented identity, and has no `none` or `policy_selected` variant:
/// accepting either before the corresponding evidence path exists would assert a check Beacon
/// cannot perform. The reason lives inside the canonical bytes, so it cannot become ambient
/// authority or silently apply to another Proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerBootstrap {
    Owner { reason: String },
}

impl OwnerBootstrap {
    fn reason(&self) -> &str {
        match self {
            Self::Owner { reason } => reason,
        }
    }
}

/// The Resident that asked, and the exact Configuration it was admitted under.
///
/// Beacon does not claim that an unauthenticated Resident issued the command: the audit Event
/// separately names the authenticated command actor as `owner`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalRequester {
    pub resident_id: Uuid,
    pub resident_configuration_id: Uuid,
    pub bootstrap: OwnerBootstrap,
}

/// The exact immutable revisions this Proposal is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalBindings {
    pub charter_revision: u32,
    pub project_policy_revision: u32,
    pub repository_revision: u32,
    /// The pinned base revision, carried as its 40-character lowercase hexadecimal text.
    ///
    /// Not twenty raw bytes: a commit id decoded from raw bytes would re-encode `A1B2…` and
    /// `a1b2…` to one identity, and case-folding a Binding is exactly the silent normalization
    /// this grammar refuses.
    pub pinned_base_revision: String,
}

/// One file presented as readable context, and the identity of the exact bytes presented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextEntry {
    pub path: String,
    pub content_sha256: [u8; 32],
}

/// Readable context is mandatory and has two distinct states.
///
/// `NotDeclared` is not ambient read access, a default, or an empty manifest; it makes the
/// Proposal incomplete for Ruling or dispatch. `Declared` with no entries is the different,
/// positive assertion that no Repository file is readable. The two produce different canonical
/// bytes and different Proposal digests.
///
/// Internal manifest consistency does not prove that a path/digest pair existed at the pinned base
/// revision, and the current sandbox still permits broad host reads. A declared readable context
/// records intended scope only; no surface may present it as enforced or verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadableContext {
    NotDeclared,
    Declared {
        entries: Vec<ContextEntry>,
        manifest_sha256: [u8; 32],
    },
}

/// The closed v1 mutation vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationKind {
    CreateFile,
    ModifyFile,
    DeleteFile,
}

impl MutationKind {
    fn tag(self) -> u8 {
        match self {
            Self::CreateFile => TAG_MUTATION_CREATE_FILE,
            Self::ModifyFile => TAG_MUTATION_MODIFY_FILE,
            Self::DeleteFile => TAG_MUTATION_DELETE_FILE,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedProposal> {
        match tag {
            TAG_MUTATION_CREATE_FILE => Ok(Self::CreateFile),
            TAG_MUTATION_MODIFY_FILE => Ok(Self::ModifyFile),
            TAG_MUTATION_DELETE_FILE => Ok(Self::DeleteFile),
            _ => Err(MalformedProposal::UnknownTag),
        }
    }
}

/// One intended effect. An intended sequence, not authorization and not an assertion that anything
/// occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredMutation {
    pub kind: MutationKind,
    pub target: String,
}

/// What the Proposal intends: the human summary, the readable context, the targets it may touch,
/// the effects it declares, and the Declared Checks it selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalIntent {
    pub summary: String,
    pub readable_context: ReadableContext,
    pub target_paths: Vec<String>,
    pub declared_mutations: Vec<DeclaredMutation>,
    pub declared_check_runs: Vec<String>,
    pub declared_checks: Vec<DeclaredCheck>,
}

/// One undeveloped constraint family.
///
/// The Blueprint requires sandbox, network, model/provider, command/tool, Evaluation, Policy, and
/// budget constraints to participate in Proposal identity, but several of those domains have no
/// executable Beacon contract yet. Guessing their internal shape in an irreversible v1 grammar
/// would turn design intent into false authority, so each is an explicit binding instead.
///
/// The `Bound` form identifies an immutable descriptor and contains no secret value, credential,
/// URL, command text, or capability. Until the content and owning-module surfaces exist, the only
/// accepted value is `NotDeclared`: the tag is defined so a later reviewed decision needs no new
/// tag, and submission refuses it so no unknown kind is silently treated as supported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintBinding {
    NotDeclared,
    Bound {
        kind: String,
        version: u32,
        canonicalizer: String,
        sha256: [u8; 32],
    },
}

impl ConstraintBinding {
    fn is_declared(&self) -> bool {
        matches!(self, Self::Bound { .. })
    }
}

/// The constraint families a future readiness check must resolve before dispatch is conceivable.
///
/// The exact Project Policy revision is not one of these: it is real persisted state and stays a
/// first-class binding. `execution_policy` is separate because the current Project Policy does not
/// define execution admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalRequirements {
    pub execution_policy: ConstraintBinding,
    pub sandbox: ConstraintBinding,
    pub network: ConstraintBinding,
    pub model_provider: ConstraintBinding,
    pub command_allowlist: ConstraintBinding,
    pub tool_allowlist: ConstraintBinding,
    pub evaluations: ConstraintBinding,
}

/// The measured ceilings a later Run would be held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProposalBounds {
    pub wall_clock_ms: u64,
    pub cpu_seconds: u64,
    pub address_space_bytes: u64,
    pub file_size_bytes: u64,
    pub output_bytes: u64,
    pub token_ceiling: u64,
}

/// V1 supports only `NoSpend`. Wider Budget Envelopes and paid providers require a later versioned
/// contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetEnvelope {
    NoSpend,
}

/// V1 supports only `DiscardDisposableWorktree`. A different rollback strategy requires a later
/// versioned contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackStrategy {
    DiscardDisposableWorktree,
}

/// The immutable v1 payload, exactly as its canonical bytes decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalV1 {
    pub scope: ProposalScope,
    pub requester: ProposalRequester,
    pub bindings: ProposalBindings,
    pub intent: ProposalIntent,
    pub requirements: ProposalRequirements,
    pub bounds: ProposalBounds,
    pub budget: BudgetEnvelope,
    pub rollback: RollbackStrategy,
}

// ---------------------------------------------------------------------------------------------
// Identity, actor, and readiness
// ---------------------------------------------------------------------------------------------

/// A digest that carries its algorithm and canonicalizer identity, never a bare hex string.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProposalDigest {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

/// The authenticated command actor. Truthfully the Owner, never the requesting Resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalActor {
    Owner,
}

/// Why a declared readable context could not be compared against the pinned base revision.
///
/// ADR 0034: "Internal manifest consistency does not prove that a path/digest pair existed at the
/// pinned base revision. A later readiness diagnostic independently compares every entry with that
/// revision when the bound Repository is locally available; otherwise it reports that comparison as
/// not verified. No surface may label the context verified from the manifest's self-consistency
/// alone."
///
/// Every arm is a distinct reason the comparison did not happen, decided by actually inspecting
/// the bound Repository rather than assumed. None of them says the context is wrong -- that is a
/// different fact, and claiming it without having read the revision would be the exact error the
/// ADR forbids in reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextComparison {
    /// The bound Repository is a Git URL. There is nothing local to compare against, and reading
    /// it would need a network fetch this slice does not perform.
    RepositorySourceNotLocal,
    /// The bound local path could not be opened as a Git object store: missing, unreadable, or not
    /// a repository. The store is broken, and that is a different fact from an inspection that
    /// errored or one that ran out of time -- Check Preflight draws exactly this distinction with
    /// `RepositoryUnreadable`, `PreflightFailed` and `PreflightTimedOut`, and AGENTS.md requires
    /// it: "Terminal, absent, broken, and not evaluated are distinct states."
    RepositoryUnreadable,
    /// The store opened and an inspection step errored -- in practice, a pack index that cannot be
    /// parsed as a version 2 index. An error, never a negative answer. Mirrors `PreflightFailed`.
    ///
    /// A `.git` indirection chain is NOT an example of this: `IndirectionTooDeep` is produced only
    /// by `ObjectStore::open`, whose failure this module reports as `RepositoryUnreadable` before
    /// any inspection begins. `inspection_error` still maps it, because that mapping is exhaustive
    /// over the store's error enum and must stay so, but no production path reaches it.
    InspectionFailed,
    /// The inspection did not finish inside its budget. Not evaluated: distinct from both a
    /// negative answer and an error. Mirrors `PreflightTimedOut`.
    InspectionTimedOut,
    /// The object store opened, but the pinned base revision is not present in it. The revision
    /// the entries would be compared against is the thing that is missing.
    PinnedRevisionAbsent,
    /// The object store opened and the pinned revision is present, but Beacon cannot read file
    /// content at a revision: resolving a path to its blob means decompressing the commit and tree
    /// objects, `beacond` carries no decompressor, and ADR 0027 forbids spawning `git`. This is the
    /// terminal state today -- see `docs/implementation/phase-2-proposal-v1.md`.
    ContentUnreadableAtRevision,
}

/// One precondition a Proposal has not met. Derived at read time, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnmetPrecondition {
    ExecutionPolicyNotDeclared,
    SandboxNotDeclared,
    NetworkNotDeclared,
    ModelProviderNotDeclared,
    CommandAllowlistNotDeclared,
    ToolAllowlistNotDeclared,
    EvaluationsNotDeclared,
    ReadableContextNotDeclared,
    /// The context is declared, and the comparison ADR 0034 requires against the pinned base
    /// revision did not happen. Never silence, and never a claim that the entries are wrong.
    ReadableContextNotVerified {
        reason: ContextComparison,
    },
    CharterRevisionMoved,
    ProjectPolicyRevisionMoved,
    RepositoryRevisionMoved,
    BaseRevisionMoved,
    ResidentLifecycleChanged,
    ConfigurationChanged,
}

/// A diagnostic list of unmet preconditions, never a boolean authorization.
///
/// There is deliberately no `is_ready`. Readiness is not permission to proceed, and an interface
/// that could ask one question and get one bit would present it as permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalReadiness {
    unmet: Vec<UnmetPrecondition>,
}

impl ProposalReadiness {
    pub fn unmet(&self) -> &[UnmetPrecondition] {
        &self.unmet
    }
}

/// One recorded Proposal: its bytes, the identity of those bytes, and what they decode to.
///
/// The fields are private and this type derives no `Deserialize`. Every comparable view in this
/// crate exposes public fields, and this one deliberately does not: a caller-supplied parsed
/// object plus digest permits the values to disagree, which is the alternative ADR 0034 rejects.
/// Bytes and payload here were proved equal by the only constructor that exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    digest: ProposalDigest,
    canonical_bytes: Vec<u8>,
    payload: ProposalV1,
    actor: ProposalActor,
    submitted_at_ms: u64,
    readiness: ProposalReadiness,
}

impl Proposal {
    pub fn digest(&self) -> &ProposalDigest {
        &self.digest
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    pub fn payload(&self) -> &ProposalV1 {
        &self.payload
    }

    pub fn actor(&self) -> ProposalActor {
        self.actor
    }

    pub fn submitted_at_ms(&self) -> u64 {
        self.submitted_at_ms
    }

    pub fn readiness(&self) -> &ProposalReadiness {
        &self.readiness
    }
}

// ---------------------------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------------------------

/// Why a byte slice is not a Proposal. Every arm is decided from the bytes alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedProposal {
    Magic,
    UnknownTag,
    TrailingBytes,
    Truncated,
    OverLimitLength,
    InvalidUtf8,
    NonCanonicalOrder,
    DuplicateEntry,
    Summary,
    BootstrapReason,
    PinnedBaseRevision,
    TargetPath,
    MutationTarget,
    CheckRunUnresolved,
    DeclaredCheck,
    DescriptorIdentity,
    UnsupportedBoundDescriptor,
    ManifestMismatch,
}

impl MalformedProposal {
    fn reason(self) -> &'static str {
        match self {
            Self::Magic => "the record does not start with the beacon.proposal.v1 magic",
            Self::UnknownTag => "a closed variant tag is not one this grammar assigns",
            Self::TrailingBytes => "the record does not consume its entire input",
            Self::Truncated => "a declared length or count does not fit the remaining input",
            Self::OverLimitLength => "a declared length or count exceeds its field's limit",
            Self::InvalidUtf8 => "a string field is not valid UTF-8",
            Self::NonCanonicalOrder => {
                "a list is not in its canonical order; Beacon refuses unsorted input rather than \
                 silently changing the submitted Proposal"
            }
            Self::DuplicateEntry => "a list that must be distinct repeats an entry",
            Self::Summary => {
                "the summary must be trimmed, non-empty, at most 2,000 Unicode scalar values, and \
                 free of NUL and non-whitespace control characters"
            }
            Self::BootstrapReason => {
                "the Owner bootstrap reason must be trimmed, non-empty, at most 500 Unicode scalar \
                 values, and free of NUL and non-whitespace control characters"
            }
            Self::PinnedBaseRevision => {
                "the pinned base revision must be a lowercase 40-character hexadecimal commit id"
            }
            Self::TargetPath => {
                "target paths must be distinct repository-relative POSIX paths in ascending raw \
                 UTF-8 byte order, never absolute and never containing a backslash, NUL, empty \
                 component, `.` component, or `..` component"
            }
            Self::MutationTarget => {
                "every declared mutation must name one target path, and every target path must be \
                 named by at least one mutation"
            }
            Self::CheckRunUnresolved => "a declared check run names no declared check",
            Self::DeclaredCheck => {
                "declared checks must name distinct commands with a non-empty argument vector and \
                 a repository-relative working directory"
            }
            Self::DescriptorIdentity => {
                "a bound descriptor identity must carry lowercase ASCII kind and canonicalizer \
                 tokens of at most 64 bytes and a positive version"
            }
            Self::UnsupportedBoundDescriptor => {
                "no owning module can resolve a bound descriptor yet, so the only accepted \
                 constraint value is not_declared"
            }
            Self::ManifestMismatch => {
                "the readable context manifest digest does not match its encoded entry list"
            }
        }
    }
}

/// Why a well-formed Proposal does not agree with the state it names. Every arm needs the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InconsistentProposal {
    WorkspaceScope,
    ProjectNotFound,
    MissionNotInProject,
    ResidentNotAdmitted,
    ConfigurationNotAdmission,
    PolicyNotForCharter,
    RepositoryNotForCharter,
    PinnedBaseRevisionMismatch,
    DeclaredChecksNotBound,
}

impl InconsistentProposal {
    fn reason(self) -> &'static str {
        match self {
            Self::WorkspaceScope => "the Proposal names a different Workspace",
            Self::ProjectNotFound => "the named Project was not found in this Workspace",
            Self::MissionNotInProject => "the named Mission does not belong to the named Project",
            Self::ResidentNotAdmitted => {
                "the requesting Resident is not an admitted member of this Mission"
            }
            Self::ConfigurationNotAdmission => {
                "the named Configuration is not that member's immutable admission Configuration"
            }
            Self::PolicyNotForCharter => {
                "the named Project Policy revision does not bind the named Charter revision"
            }
            Self::RepositoryNotForCharter => {
                "the named Repository revision does not bind the named Charter revision"
            }
            Self::PinnedBaseRevisionMismatch => {
                "the pinned base revision is not the one the named Repository revision pinned"
            }
            Self::DeclaredChecksNotBound => {
                "a declared check does not match the bound Repository revision exactly"
            }
        }
    }
}

#[derive(Debug)]
pub enum ProposalError {
    Account(OperatorAccountError),
    Oversize,
    Malformed(MalformedProposal),
    NotCanonical,
    Inconsistent(InconsistentProposal),
    NotFound,
    DigestCollision,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ProposalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::Oversize => write!(
                formatter,
                "a Proposal submission cannot exceed {MAX_CANONICAL_BYTES} bytes"
            ),
            Self::Malformed(reason) => {
                write!(formatter, "Proposal bytes are invalid: {}", reason.reason())
            }
            Self::NotCanonical => write!(
                formatter,
                "Proposal bytes are not the canonical encoding of the value they decode to"
            ),
            Self::Inconsistent(reason) => write!(
                formatter,
                "Proposal references are inconsistent: {}",
                reason.reason()
            ),
            Self::NotFound => write!(formatter, "no Proposal has that digest"),
            Self::DigestCollision => write!(
                formatter,
                "a stored Proposal has this digest but different bytes"
            ),
            Self::InvalidStore => write!(formatter, "Proposal state is invalid"),
            Self::Store(error) => write!(formatter, "Proposal store failed: {error}"),
            Self::Json(error) => write!(formatter, "Proposal event is invalid: {error}"),
        }
    }
}

impl std::error::Error for ProposalError {}

impl From<OperatorAccountError> for ProposalError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for ProposalError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for ProposalError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

// ---------------------------------------------------------------------------------------------
// The canonical bytes
// ---------------------------------------------------------------------------------------------

/// Appends canonical bytes. Every length and count is checked against its field's limit here too,
/// so the encoder and the decoder cannot disagree about what fits.
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

    fn uuid(&mut self, value: Uuid) {
        self.out.extend_from_slice(value.as_bytes());
    }

    fn sha256(&mut self, value: &[u8; 32]) {
        self.out.extend_from_slice(value);
    }

    fn string(&mut self, value: &str, limit: usize) -> Result<(), MalformedProposal> {
        if value.len() > limit {
            return Err(MalformedProposal::OverLimitLength);
        }
        let length = u64::try_from(value.len()).map_err(|_| MalformedProposal::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn count(&mut self, length: usize, limit: usize) -> Result<(), MalformedProposal> {
        if length > limit {
            return Err(MalformedProposal::OverLimitLength);
        }
        let count = u32::try_from(length).map_err(|_| MalformedProposal::OverLimitLength)?;
        self.u32(count);
        Ok(())
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

    fn take(&mut self, count: usize) -> Result<&'a [u8], MalformedProposal> {
        if count > self.remaining() {
            return Err(MalformedProposal::Truncated);
        }
        let slice = &self.input[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn magic(&mut self) -> Result<(), MalformedProposal> {
        if self
            .take(MAGIC.len())
            .map_err(|_| MalformedProposal::Magic)?
            == MAGIC
        {
            Ok(())
        } else {
            Err(MalformedProposal::Magic)
        }
    }

    fn tag(&mut self) -> Result<u8, MalformedProposal> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, MalformedProposal> {
        let mut value = [0_u8; 4];
        value.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(value))
    }

    fn u64(&mut self) -> Result<u64, MalformedProposal> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }

    fn uuid(&mut self) -> Result<Uuid, MalformedProposal> {
        let mut value = [0_u8; 16];
        value.copy_from_slice(self.take(16)?);
        Ok(Uuid::from_bytes(value))
    }

    fn sha256(&mut self) -> Result<[u8; 32], MalformedProposal> {
        let mut value = [0_u8; 32];
        value.copy_from_slice(self.take(32)?);
        Ok(value)
    }

    fn string(&mut self, limit: usize) -> Result<String, MalformedProposal> {
        let length = self.u64()?;
        let limit = u64::try_from(limit).map_err(|_| MalformedProposal::OverLimitLength)?;
        if length > limit {
            return Err(MalformedProposal::OverLimitLength);
        }
        let length = usize::try_from(length).map_err(|_| MalformedProposal::Truncated)?;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| MalformedProposal::InvalidUtf8)
    }

    /// A declared count, proved against both its field's limit and the remaining input before a
    /// single entry is decoded. The caller appends entries as it decodes them; nothing here or in
    /// the caller reserves capacity from this value.
    fn count(&mut self, limit: usize, entry_bytes: usize) -> Result<usize, MalformedProposal> {
        let count = usize::try_from(self.u32()?).map_err(|_| MalformedProposal::OverLimitLength)?;
        if count > limit {
            return Err(MalformedProposal::OverLimitLength);
        }
        let smallest = count
            .checked_mul(entry_bytes)
            .ok_or(MalformedProposal::Truncated)?;
        if smallest > self.remaining() {
            return Err(MalformedProposal::Truncated);
        }
        Ok(count)
    }

    fn finish(self) -> Result<(), MalformedProposal> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MalformedProposal::TrailingBytes)
        }
    }
}

/// The one canonical encoding of a payload. Every accepted byte difference changes the Proposal
/// identity, so alternate encodings of the same parsed value do not exist: this function is the
/// only producer, and `submit` proves the caller's bytes equal what it returns.
fn encode(payload: &ProposalV1) -> Result<Vec<u8>, MalformedProposal> {
    let mut encoder = Encoder::new();

    encoder.uuid(payload.scope.workspace_id);
    encoder.uuid(payload.scope.project_id);
    encoder.uuid(payload.scope.mission_id);

    encoder.uuid(payload.requester.resident_id);
    encoder.uuid(payload.requester.resident_configuration_id);
    match &payload.requester.bootstrap {
        OwnerBootstrap::Owner { reason } => {
            encoder.tag(TAG_BOOTSTRAP_OWNER);
            encoder.string(reason, MAX_BOOTSTRAP_REASON_BYTES)?;
        }
    }

    encoder.u32(payload.bindings.charter_revision);
    encoder.u32(payload.bindings.project_policy_revision);
    encoder.u32(payload.bindings.repository_revision);
    encoder.string(&payload.bindings.pinned_base_revision, BASE_REVISION_CHARS)?;

    encoder.string(&payload.intent.summary, MAX_SUMMARY_BYTES)?;
    match &payload.intent.readable_context {
        ReadableContext::NotDeclared => encoder.tag(TAG_CONTEXT_NOT_DECLARED),
        ReadableContext::Declared {
            entries,
            manifest_sha256,
        } => {
            encoder.tag(TAG_CONTEXT_DECLARED);
            encode_context_entries(&mut encoder, entries)?;
            encoder.sha256(manifest_sha256);
        }
    }
    encoder.count(payload.intent.target_paths.len(), MAX_CANONICAL_BYTES)?;
    for target in &payload.intent.target_paths {
        encoder.string(target, MAX_CANONICAL_BYTES)?;
    }
    encoder.count(payload.intent.declared_mutations.len(), MAX_CANONICAL_BYTES)?;
    for mutation in &payload.intent.declared_mutations {
        encoder.tag(mutation.kind.tag());
        encoder.string(&mutation.target, MAX_CANONICAL_BYTES)?;
    }
    encoder.count(
        payload.intent.declared_check_runs.len(),
        MAX_CANONICAL_BYTES,
    )?;
    for name in &payload.intent.declared_check_runs {
        encoder.string(name, MAX_CHECK_NAME_BYTES)?;
    }
    encoder.count(payload.intent.declared_checks.len(), MAX_CANONICAL_BYTES)?;
    for check in &payload.intent.declared_checks {
        encoder.string(&check.name, MAX_CHECK_NAME_BYTES)?;
        encoder.count(check.argv.len(), MAX_CANONICAL_BYTES)?;
        for argument in &check.argv {
            encoder.string(argument, MAX_CANONICAL_BYTES)?;
        }
        encoder.string(&check.working_directory, MAX_CANONICAL_BYTES)?;
    }

    for constraint in constraints(&payload.requirements) {
        match constraint {
            ConstraintBinding::NotDeclared => encoder.tag(TAG_CONSTRAINT_NOT_DECLARED),
            ConstraintBinding::Bound {
                kind,
                version,
                canonicalizer,
                sha256,
            } => {
                encoder.tag(TAG_CONSTRAINT_BOUND);
                encoder.string(kind, MAX_DESCRIPTOR_TOKEN_BYTES)?;
                encoder.u32(*version);
                encoder.string(canonicalizer, MAX_DESCRIPTOR_TOKEN_BYTES)?;
                encoder.sha256(sha256);
            }
        }
    }

    encoder.u64(payload.bounds.wall_clock_ms);
    encoder.u64(payload.bounds.cpu_seconds);
    encoder.u64(payload.bounds.address_space_bytes);
    encoder.u64(payload.bounds.file_size_bytes);
    encoder.u64(payload.bounds.output_bytes);
    encoder.u64(payload.bounds.token_ceiling);

    match payload.budget {
        BudgetEnvelope::NoSpend => encoder.tag(TAG_BUDGET_NO_SPEND),
    }
    match payload.rollback {
        RollbackStrategy::DiscardDisposableWorktree => {
            encoder.tag(TAG_ROLLBACK_DISCARD_DISPOSABLE_WORKTREE);
        }
    }

    Ok(encoder.out)
}

/// The requirement families, in their canonical order. One place, so the encoder, the decoder, and
/// readiness cannot drift apart about which families exist or what order they are in.
fn constraints(requirements: &ProposalRequirements) -> [&ConstraintBinding; 7] {
    [
        &requirements.execution_policy,
        &requirements.sandbox,
        &requirements.network,
        &requirements.model_provider,
        &requirements.command_allowlist,
        &requirements.tool_allowlist,
        &requirements.evaluations,
    ]
}

fn encode_context_entries(
    encoder: &mut Encoder,
    entries: &[ContextEntry],
) -> Result<(), MalformedProposal> {
    encoder.count(entries.len(), MAX_CANONICAL_BYTES)?;
    for entry in entries {
        encoder.string(&entry.path, MAX_CANONICAL_BYTES)?;
        encoder.sha256(&entry.content_sha256);
    }
    Ok(())
}

/// The canonical encoding of a readable-context entry list on its own, which is what
/// `manifest_sha256` is the digest of: the exact bytes the list occupies inside the record,
/// count prefix included.
fn encoded_context_entries(entries: &[ContextEntry]) -> Result<Vec<u8>, MalformedProposal> {
    let mut encoder = Encoder { out: Vec::new() };
    encode_context_entries(&mut encoder, entries)?;
    Ok(encoder.out)
}

fn decode(input: &[u8]) -> Result<ProposalV1, MalformedProposal> {
    let mut decoder = Decoder::new(input);
    decoder.magic()?;

    let scope = ProposalScope {
        workspace_id: decoder.uuid()?,
        project_id: decoder.uuid()?,
        mission_id: decoder.uuid()?,
    };

    let resident_id = decoder.uuid()?;
    let resident_configuration_id = decoder.uuid()?;
    let bootstrap = match decoder.tag()? {
        TAG_BOOTSTRAP_OWNER => {
            let reason = decoder.string(MAX_BOOTSTRAP_REASON_BYTES)?;
            if !is_human_text(&reason, MAX_BOOTSTRAP_REASON_SCALARS) {
                return Err(MalformedProposal::BootstrapReason);
            }
            OwnerBootstrap::Owner { reason }
        }
        _ => return Err(MalformedProposal::UnknownTag),
    };
    let requester = ProposalRequester {
        resident_id,
        resident_configuration_id,
        bootstrap,
    };

    let charter_revision = decoder.u32()?;
    let project_policy_revision = decoder.u32()?;
    let repository_revision = decoder.u32()?;
    let pinned_base_revision = decoder.string(BASE_REVISION_CHARS)?;
    if !is_base_revision(&pinned_base_revision) {
        return Err(MalformedProposal::PinnedBaseRevision);
    }
    let bindings = ProposalBindings {
        charter_revision,
        project_policy_revision,
        repository_revision,
        pinned_base_revision,
    };

    let summary = decoder.string(MAX_SUMMARY_BYTES)?;
    if !is_human_text(&summary, MAX_SUMMARY_SCALARS) {
        return Err(MalformedProposal::Summary);
    }
    let readable_context = decode_readable_context(&mut decoder)?;
    let target_paths = decode_target_paths(&mut decoder)?;
    let declared_mutations = decode_mutations(&mut decoder, &target_paths)?;
    let declared_checks_runs = decode_check_runs(&mut decoder)?;
    let declared_checks = decode_declared_checks(&mut decoder)?;
    for name in &declared_checks_runs {
        if !declared_checks.iter().any(|check| &check.name == name) {
            return Err(MalformedProposal::CheckRunUnresolved);
        }
    }

    let requirements = ProposalRequirements {
        execution_policy: decode_constraint(&mut decoder)?,
        sandbox: decode_constraint(&mut decoder)?,
        network: decode_constraint(&mut decoder)?,
        model_provider: decode_constraint(&mut decoder)?,
        command_allowlist: decode_constraint(&mut decoder)?,
        tool_allowlist: decode_constraint(&mut decoder)?,
        evaluations: decode_constraint(&mut decoder)?,
    };

    let bounds = ProposalBounds {
        wall_clock_ms: decoder.u64()?,
        cpu_seconds: decoder.u64()?,
        address_space_bytes: decoder.u64()?,
        file_size_bytes: decoder.u64()?,
        output_bytes: decoder.u64()?,
        token_ceiling: decoder.u64()?,
    };

    let budget = match decoder.tag()? {
        TAG_BUDGET_NO_SPEND => BudgetEnvelope::NoSpend,
        _ => return Err(MalformedProposal::UnknownTag),
    };
    let rollback = match decoder.tag()? {
        TAG_ROLLBACK_DISCARD_DISPOSABLE_WORKTREE => RollbackStrategy::DiscardDisposableWorktree,
        _ => return Err(MalformedProposal::UnknownTag),
    };

    decoder.finish()?;

    Ok(ProposalV1 {
        scope,
        requester,
        bindings,
        intent: ProposalIntent {
            summary,
            readable_context,
            target_paths,
            declared_mutations,
            declared_check_runs: declared_checks_runs,
            declared_checks,
        },
        requirements,
        bounds,
        budget,
        rollback,
    })
}

fn decode_readable_context(
    decoder: &mut Decoder<'_>,
) -> Result<ReadableContext, MalformedProposal> {
    match decoder.tag()? {
        TAG_CONTEXT_NOT_DECLARED => Ok(ReadableContext::NotDeclared),
        TAG_CONTEXT_DECLARED => {
            let count = decoder.count(MAX_CANONICAL_BYTES, MIN_CONTEXT_ENTRY_BYTES)?;
            let mut entries: Vec<ContextEntry> = Vec::new();
            for _ in 0..count {
                let path = decoder.string(MAX_CANONICAL_BYTES)?;
                if !is_repository_relative_path(&path) {
                    return Err(MalformedProposal::TargetPath);
                }
                check_ascending(entries.last().map(|entry| entry.path.as_str()), &path)?;
                let content_sha256 = decoder.sha256()?;
                entries.push(ContextEntry {
                    path,
                    content_sha256,
                });
            }
            let manifest_sha256 = decoder.sha256()?;
            if sha256(&encoded_context_entries(&entries)?) != manifest_sha256 {
                return Err(MalformedProposal::ManifestMismatch);
            }
            Ok(ReadableContext::Declared {
                entries,
                manifest_sha256,
            })
        }
        _ => Err(MalformedProposal::UnknownTag),
    }
}

fn decode_target_paths(decoder: &mut Decoder<'_>) -> Result<Vec<String>, MalformedProposal> {
    let count = decoder.count(MAX_CANONICAL_BYTES, MIN_STRING_ENTRY_BYTES)?;
    let mut paths: Vec<String> = Vec::new();
    for _ in 0..count {
        let path = decoder.string(MAX_CANONICAL_BYTES)?;
        if !is_repository_relative_path(&path) {
            return Err(MalformedProposal::TargetPath);
        }
        check_ascending(paths.last().map(String::as_str), &path)?;
        paths.push(path);
    }
    Ok(paths)
}

fn decode_mutations(
    decoder: &mut Decoder<'_>,
    target_paths: &[String],
) -> Result<Vec<DeclaredMutation>, MalformedProposal> {
    let count = decoder.count(MAX_CANONICAL_BYTES, MIN_MUTATION_ENTRY_BYTES)?;
    let mut mutations: Vec<DeclaredMutation> = Vec::new();
    let mut seen = HashSet::new();
    for _ in 0..count {
        let kind = MutationKind::parse(decoder.tag()?)?;
        let target = decoder.string(MAX_CANONICAL_BYTES)?;
        if !target_paths.iter().any(|path| path == &target) {
            return Err(MalformedProposal::MutationTarget);
        }
        if !seen.insert((kind, target.clone())) {
            return Err(MalformedProposal::DuplicateEntry);
        }
        mutations.push(DeclaredMutation { kind, target });
    }
    // Order is the caller's intended sequence and is identity-bearing, so it is not checked; the
    // reverse coverage rule is, because a target nothing acts on is not an intended effect.
    if target_paths
        .iter()
        .any(|path| !mutations.iter().any(|mutation| &mutation.target == path))
    {
        return Err(MalformedProposal::MutationTarget);
    }
    Ok(mutations)
}

fn decode_check_runs(decoder: &mut Decoder<'_>) -> Result<Vec<String>, MalformedProposal> {
    let count = decoder.count(MAX_CANONICAL_BYTES, MIN_STRING_ENTRY_BYTES)?;
    let mut names: Vec<String> = Vec::new();
    for _ in 0..count {
        let name = decoder.string(MAX_CHECK_NAME_BYTES)?;
        if !is_check_name(&name) {
            return Err(MalformedProposal::DeclaredCheck);
        }
        check_ascending(names.last().map(String::as_str), &name)?;
        names.push(name);
    }
    Ok(names)
}

fn decode_declared_checks(
    decoder: &mut Decoder<'_>,
) -> Result<Vec<DeclaredCheck>, MalformedProposal> {
    let count = decoder.count(MAX_CANONICAL_BYTES, MIN_DECLARED_CHECK_ENTRY_BYTES)?;
    let mut checks: Vec<DeclaredCheck> = Vec::new();
    for _ in 0..count {
        let name = decoder.string(MAX_CHECK_NAME_BYTES)?;
        if !is_check_name(&name) {
            return Err(MalformedProposal::DeclaredCheck);
        }
        check_ascending(checks.last().map(|check| check.name.as_str()), &name)?;
        let argument_count = decoder.count(MAX_CANONICAL_BYTES, MIN_STRING_ENTRY_BYTES)?;
        let mut argv: Vec<String> = Vec::new();
        for _ in 0..argument_count {
            let argument = decoder.string(MAX_CANONICAL_BYTES)?;
            if argument.is_empty() || argument.contains('\0') {
                return Err(MalformedProposal::DeclaredCheck);
            }
            argv.push(argument);
        }
        if argv.is_empty() {
            return Err(MalformedProposal::DeclaredCheck);
        }
        let working_directory = decoder.string(MAX_CANONICAL_BYTES)?;
        if !is_working_directory(&working_directory) {
            return Err(MalformedProposal::DeclaredCheck);
        }
        checks.push(DeclaredCheck {
            name,
            argv,
            working_directory,
        });
    }
    Ok(checks)
}

fn decode_constraint(decoder: &mut Decoder<'_>) -> Result<ConstraintBinding, MalformedProposal> {
    match decoder.tag()? {
        TAG_CONSTRAINT_NOT_DECLARED => Ok(ConstraintBinding::NotDeclared),
        TAG_CONSTRAINT_BOUND => {
            let kind = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
            let version = decoder.u32()?;
            let canonicalizer = decoder.string(MAX_DESCRIPTOR_TOKEN_BYTES)?;
            let sha256 = decoder.sha256()?;
            if !is_descriptor_token(&kind) || !is_descriptor_token(&canonicalizer) || version == 0 {
                return Err(MalformedProposal::DescriptorIdentity);
            }
            Ok(ConstraintBinding::Bound {
                kind,
                version,
                canonicalizer,
                sha256,
            })
        }
        _ => Err(MalformedProposal::UnknownTag),
    }
}

/// The canonical order for every ordered list in this grammar: ascending raw UTF-8 bytes, strictly
/// increasing so that equal neighbours are duplicates rather than a tie.
fn check_ascending(previous: Option<&str>, current: &str) -> Result<(), MalformedProposal> {
    match previous {
        None => Ok(()),
        Some(previous) if previous.as_bytes() < current.as_bytes() => Ok(()),
        Some(previous) if previous == current => Err(MalformedProposal::DuplicateEntry),
        Some(_) => Err(MalformedProposal::NonCanonicalOrder),
    }
}

/// Trimmed, non-empty, bounded human text, free of NUL and non-whitespace control characters.
/// `char::is_control` covers NUL, and whitespace controls such as newline and tab stay legal.
fn is_human_text(value: &str, max_scalars: usize) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= max_scalars
        && !value
            .chars()
            .any(|character| character.is_control() && !character.is_whitespace())
}

/// A distinct UTF-8 POSIX path relative to the Repository root. Never Unicode-normalized and never
/// case-folded: those transformations can name a different filesystem object.
fn is_repository_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\0')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

/// A Declared Check's working directory: `.` or a path inside the repository.
fn is_working_directory(working_directory: &str) -> bool {
    working_directory == "." || is_repository_relative_path(working_directory)
}

fn is_check_name(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    name.len() <= MAX_CHECK_NAME_BYTES
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '.'
                || character == '_'
                || character == '-'
        })
}

fn is_descriptor_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_DESCRIPTOR_TOKEN_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-'
        })
}

fn is_base_revision(value: &str) -> bool {
    value.len() == BASE_REVISION_CHARS
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let output = digest.finalize();
    let mut value = [0_u8; 32];
    value.copy_from_slice(&output[..]);
    value
}

// ---------------------------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------------------------

/// Owns authenticated Proposal submission and identity queries behind one deep interface.
///
/// Submission records an Operator-attributed, Resident-requested description for review. It opens
/// no Gate, records no Ruling, prepares no worktree, starts no process, invokes no model, makes no
/// network request, and dispatches nothing.
pub struct ProposalService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl ProposalService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Records one immutable Proposal from the exact bytes that name it.
    ///
    /// The Owner is authenticated before anything looks at `canonical_bytes`: an unauthenticated
    /// caller gets the account refusal and no parser oracle. The bytes are then hashed, strictly
    /// decoded, re-encoded, and required to be byte-for-byte equal to what was supplied.
    pub fn submit(&self, canonical_bytes: &[u8]) -> Result<Proposal, ProposalError> {
        self.accounts.with_authenticated_store(|connection, now| {
            if canonical_bytes.len() > MAX_CANONICAL_BYTES {
                return Err(ProposalError::Oversize);
            }
            let digest_value = HEXLOWER.encode(&sha256(canonical_bytes));
            let payload = decode(canonical_bytes).map_err(ProposalError::Malformed)?;
            if constraints(&payload.requirements)
                .iter()
                .any(|constraint| constraint.is_declared())
            {
                return Err(ProposalError::Malformed(
                    MalformedProposal::UnsupportedBoundDescriptor,
                ));
            }
            if encode(&payload).map_err(ProposalError::Malformed)? != canonical_bytes {
                return Err(ProposalError::NotCanonical);
            }

            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(stored) = load_stored(&transaction, self.workspace_id, &digest_value)? {
                if stored.canonical_bytes != canonical_bytes {
                    return Err(ProposalError::DigestCollision);
                }
                let proposal = verified_proposal(&transaction, stored)?;
                transaction.commit()?;
                return Ok(proposal);
            }
            validate_references(&transaction, self.workspace_id, &payload)?;

            let submitted_at_ms = u64::try_from(now).map_err(|_| ProposalError::InvalidStore)?;
            let stored = StoredProposal {
                workspace_id: self.workspace_id,
                digest_value,
                canonical_bytes: canonical_bytes.to_vec(),
                project_id: payload.scope.project_id,
                mission_id: payload.scope.mission_id,
                charter_revision: payload.bindings.charter_revision,
                project_policy_revision: payload.bindings.project_policy_revision,
                repository_revision: payload.bindings.repository_revision,
                pinned_base_revision: payload.bindings.pinned_base_revision.clone(),
                requester_resident_id: payload.requester.resident_id,
                requester_configuration_id: payload.requester.resident_configuration_id,
                bootstrap_reason: payload.requester.bootstrap.reason().to_owned(),
                submitted_at_ms,
            };
            insert_proposal(&transaction, &stored)?;
            let proposal = verified_proposal(&transaction, stored)?;
            transaction.commit()?;
            Ok(proposal)
        })
    }

    pub fn get(&self, digest: &str) -> Result<Proposal, ProposalError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let stored = load_stored(connection, self.workspace_id, digest)?
                .ok_or(ProposalError::NotFound)?;
            verified_proposal(connection, stored)
        })
    }

    pub fn list_for_mission(&self, mission_id: Uuid) -> Result<Vec<Proposal>, ProposalError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let mut statement = connection.prepare(
                "SELECT digest_value FROM proposals
                 WHERE workspace_id = ?1 AND mission_id = ?2
                 ORDER BY submitted_at_ms, digest_value",
            )?;
            let digests = statement
                .query_map(
                    params![self.workspace_id.to_string(), mission_id.to_string()],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            digests
                .into_iter()
                .map(|digest| {
                    let stored = load_stored(connection, self.workspace_id, &digest)?
                        .ok_or(ProposalError::InvalidStore)?;
                    verified_proposal(connection, stored)
                })
                .collect()
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Referent existence and scope consistency
// ---------------------------------------------------------------------------------------------

/// Proves the Proposal's references exist and agree with each other, without comparing any binding
/// to the current revision. Staleness is a read-time diagnostic, not a submission failure.
fn validate_references(
    connection: &Connection,
    workspace_id: Uuid,
    payload: &ProposalV1,
) -> Result<(), ProposalError> {
    let inconsistent = |reason| Err(ProposalError::Inconsistent(reason));

    if payload.scope.workspace_id != workspace_id {
        return inconsistent(InconsistentProposal::WorkspaceScope);
    }
    if current_charter_revision(connection, workspace_id, payload.scope.project_id)
        .map_err(map_project_error)?
        .is_none()
    {
        return inconsistent(InconsistentProposal::ProjectNotFound);
    }

    // A Mission that does not exist is, truthfully, not in the named Project.
    let Some(mission) = mission_admission_facts(
        connection,
        workspace_id,
        payload.scope.mission_id,
        payload.requester.resident_id,
    )
    .map_err(map_mission_error)?
    else {
        return inconsistent(InconsistentProposal::MissionNotInProject);
    };
    if mission.project_id != payload.scope.project_id {
        return inconsistent(InconsistentProposal::MissionNotInProject);
    }
    let Some(admission_configuration_id) = mission.member_configuration_id else {
        return inconsistent(InconsistentProposal::ResidentNotAdmitted);
    };
    if admission_configuration_id != payload.requester.resident_configuration_id {
        return inconsistent(InconsistentProposal::ConfigurationNotAdmission);
    }

    // A Policy revision that does not exist does not bind the named Charter revision.
    let policy = policy_revision_facts(
        connection,
        workspace_id,
        payload.scope.project_id,
        payload.bindings.project_policy_revision,
    )
    .map_err(map_policy_error)?;
    if policy.map(|facts| facts.charter_revision) != Some(payload.bindings.charter_revision) {
        return inconsistent(InconsistentProposal::PolicyNotForCharter);
    }

    let Some(repository) = stored_repository_for_preflight(
        connection,
        workspace_id,
        payload.scope.project_id,
        payload.bindings.repository_revision,
    )
    .map_err(map_repository_error)?
    else {
        return inconsistent(InconsistentProposal::RepositoryNotForCharter);
    };
    if repository.charter_revision != payload.bindings.charter_revision {
        return inconsistent(InconsistentProposal::RepositoryNotForCharter);
    }
    if repository.base_revision != payload.bindings.pinned_base_revision {
        return inconsistent(InconsistentProposal::PinnedBaseRevisionMismatch);
    }
    if payload.intent.declared_checks.iter().any(|declared| {
        !repository
            .declared_checks
            .iter()
            .any(|bound| bound == declared)
    }) {
        return inconsistent(InconsistentProposal::DeclaredChecksNotBound);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Readiness, derived at read time and never stored
// ---------------------------------------------------------------------------------------------

/// A Proposal is not ready while any required constraint or its readable context is not declared,
/// or a current Project, Policy, Repository, base, Resident lifecycle, or Configuration no longer
/// matches. In v1 every requirement family can only be `NotDeclared`, so every v1 Proposal is
/// unready by construction — which is what "inert" means here, stated rather than implied.
fn derive_readiness(
    connection: &Connection,
    workspace_id: Uuid,
    payload: &ProposalV1,
) -> Result<ProposalReadiness, ProposalError> {
    let mut unmet = Vec::new();
    let families = [
        UnmetPrecondition::ExecutionPolicyNotDeclared,
        UnmetPrecondition::SandboxNotDeclared,
        UnmetPrecondition::NetworkNotDeclared,
        UnmetPrecondition::ModelProviderNotDeclared,
        UnmetPrecondition::CommandAllowlistNotDeclared,
        UnmetPrecondition::ToolAllowlistNotDeclared,
        UnmetPrecondition::EvaluationsNotDeclared,
    ];
    for (constraint, precondition) in constraints(&payload.requirements).iter().zip(families) {
        if !constraint.is_declared() {
            unmet.push(precondition);
        }
    }
    match payload.intent.readable_context {
        ReadableContext::NotDeclared => {
            unmet.push(UnmetPrecondition::ReadableContextNotDeclared);
        }
        ReadableContext::Declared { .. } => {
            if let Some(reason) = compare_readable_context(connection, workspace_id, payload)? {
                unmet.push(UnmetPrecondition::ReadableContextNotVerified { reason });
            }
        }
    }

    let (current_charter, latest_repository) =
        latest_repository_for_preflight(connection, workspace_id, payload.scope.project_id)
            .map_err(map_repository_error)?;
    if current_charter != payload.bindings.charter_revision {
        unmet.push(UnmetPrecondition::CharterRevisionMoved);
    }
    let policy = policy_revision_facts(
        connection,
        workspace_id,
        payload.scope.project_id,
        payload.bindings.project_policy_revision,
    )
    .map_err(map_policy_error)?;
    if !policy.is_some_and(|facts| facts.is_current) {
        unmet.push(UnmetPrecondition::ProjectPolicyRevisionMoved);
    }
    // Repository and base are separate facts (ADR 0034:220-228), so a rebind that keeps the
    // same pinned commit must report only the revision as moved -- not a false base-drift
    // diagnosis riding on top of a true one. Both are unmet only when there is no current
    // binding at all to compare against, which never happens for a Proposal that passed
    // submission (Repository Bindings are append-only, so a bound revision never disappears);
    // it stays the honest answer for that state if it is ever reached some other way.
    match latest_repository {
        Some(binding) if binding.revision == payload.bindings.repository_revision => {
            if binding.base_revision != payload.bindings.pinned_base_revision {
                unmet.push(UnmetPrecondition::BaseRevisionMoved);
            }
        }
        Some(binding) => {
            unmet.push(UnmetPrecondition::RepositoryRevisionMoved);
            if binding.base_revision != payload.bindings.pinned_base_revision {
                unmet.push(UnmetPrecondition::BaseRevisionMoved);
            }
        }
        None => {
            unmet.push(UnmetPrecondition::RepositoryRevisionMoved);
            unmet.push(UnmetPrecondition::BaseRevisionMoved);
        }
    }

    let resident = load_resident(connection, workspace_id, payload.requester.resident_id)
        .map_err(map_lifecycle_error)?;
    if !resident.is_some_and(|resident| resident.lifecycle == ResidentLifecycle::Active) {
        unmet.push(UnmetPrecondition::ResidentLifecycleChanged);
    }
    let configuration =
        current_configuration(connection, workspace_id, payload.requester.resident_id)
            .map_err(map_configuration_error)?;
    if configuration.map(|configuration| configuration.id)
        != Some(payload.requester.resident_configuration_id)
    {
        unmet.push(UnmetPrecondition::ConfigurationChanged);
    }
    Ok(ProposalReadiness { unmet })
}

/// Compare every declared readable-context entry against the pinned base revision.
///
/// Returns `None` when the comparison was performed and every entry agreed, and `Some(reason)`
/// when it could not be performed at all.
///
/// **Today this function never returns `None`.** Each step below is a real inspection of the bound
/// Repository -- the source kind, whether the object store opens, whether the pinned revision is
/// present -- and any of them can end the walk with its own reason. When they all succeed, the
/// comparison still cannot run: resolving `path` to its blob at a revision means decompressing the
/// commit object, walking its tree, and reading the blob, and `beacond` has no decompressor
/// (`git_object_store` is deliberately presence-only) while ADR 0027 forbids spawning `git`. So the
/// walk ends at `ContentUnreadableAtRevision`, which is the honest answer and satisfies the ADR's
/// actual requirement: "No surface may label the context verified from the manifest's
/// self-consistency alone."
///
/// The alternative -- reading the entries out of the working tree at `source` -- is refused on
/// purpose. That compares against whatever is checked out now, not against the pinned revision, so
/// it would manufacture both false agreement and false disagreement from the wrong evidence.
///
/// When a decompressor arrives, the comparison replaces the final step and gains its own outcome
/// for entries that disagree; every step before it stays exactly as it is.
fn compare_readable_context(
    connection: &Connection,
    workspace_id: Uuid,
    payload: &ProposalV1,
) -> Result<Option<ContextComparison>, ProposalError> {
    let started = Instant::now();
    let mut deadline = move || started.elapsed().as_millis() > CONTEXT_INSPECTION_BUDGET_MS;
    compare_readable_context_within(connection, workspace_id, payload, &mut deadline)
}

/// The comparison, driven by a caller-supplied deadline.
///
/// The deadline is consulted on entry and between every object-store step, so the work this
/// function itself schedules stops at the budget. It is passed in rather than read from a constant
/// for the same reason `ObjectStore::contains` takes one: it is the seam that makes the elapsed
/// outcome reachable by a test, and it lets a future caller hold one budget across many Proposals
/// instead of granting each its own.
///
/// **The bound is best-effort, not hard.** Between two checks sits one call into
/// `git_object_store`, and a single such call can overrun: `ObjectStore::open` reads a `.git` file
/// with `read_to_string` before this function regains control, and `contains` reads a whole pack
/// index with `fs::read` between its own deadline checks. Bounding those would mean changing that
/// module's I/O, which this slice does not own. The residual is named in
/// `docs/implementation/phase-2-proposal-v1.md`.
fn compare_readable_context_within(
    connection: &Connection,
    workspace_id: Uuid,
    payload: &ProposalV1,
    deadline: &mut impl FnMut() -> bool,
) -> Result<Option<ContextComparison>, ProposalError> {
    if deadline() {
        return Ok(Some(ContextComparison::InspectionTimedOut));
    }
    let Some(binding) = stored_repository_for_preflight(
        connection,
        workspace_id,
        payload.scope.project_id,
        payload.bindings.repository_revision,
    )
    .map_err(map_repository_error)?
    else {
        // The bound revision is immutable and was proved to exist at submission, so this is not
        // reachable through the store's own rules -- but a binding we cannot load is a broken
        // referent, not a missing revision.
        return Ok(Some(ContextComparison::RepositoryUnreadable));
    };
    if binding.source_kind != RepositorySourceKind::LocalPath {
        return Ok(Some(ContextComparison::RepositorySourceNotLocal));
    }
    if deadline() {
        return Ok(Some(ContextComparison::InspectionTimedOut));
    }
    let Ok(store) = ObjectStore::open(Path::new(&binding.source)) else {
        return Ok(Some(ContextComparison::RepositoryUnreadable));
    };
    if deadline() {
        return Ok(Some(ContextComparison::InspectionTimedOut));
    }
    match store.contains(&payload.bindings.pinned_base_revision, deadline) {
        Ok(true) => Ok(Some(ContextComparison::ContentUnreadableAtRevision)),
        Ok(false) => Ok(Some(ContextComparison::PinnedRevisionAbsent)),
        Err(error) => Ok(Some(inspection_error(error))),
    }
}

/// How an object-store failure is reported. The lower layer keeps these apart; so does this one.
///
/// An elapsed inspection was not evaluated. Any other error means a step broke. Neither is a
/// negative answer, and neither may be reported as one -- that is the whole distinction
/// AGENTS.md:152 requires, in one place so it cannot drift between call sites.
fn inspection_error(error: ObjectStoreError) -> ContextComparison {
    match error {
        ObjectStoreError::Elapsed => ContextComparison::InspectionTimedOut,
        ObjectStoreError::NotARepository
        | ObjectStoreError::IndirectionTooDeep
        | ObjectStoreError::UnreadableIndex => ContextComparison::InspectionFailed,
    }
}

// ---------------------------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------------------------

/// One immutable Proposal row. Every state discriminator is a stored value; meaningful absence is
/// an explicit parsed variant inside the bytes, never SQL null.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredProposal {
    workspace_id: Uuid,
    digest_value: String,
    canonical_bytes: Vec<u8>,
    project_id: Uuid,
    mission_id: Uuid,
    charter_revision: u32,
    project_policy_revision: u32,
    repository_revision: u32,
    pinned_base_revision: String,
    requester_resident_id: Uuid,
    requester_configuration_id: Uuid,
    bootstrap_reason: String,
    submitted_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubmittedProposal {
    digest: ProposalDigest,
    /// The exact canonical bytes, as canonical RFC 4648 padded base64.
    canonical_bytes_base64: String,
    actor: ProposalActor,
    submitted_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProposalSubmittedPayload {
    proposal: SubmittedProposal,
}

fn digest_envelope(value: &str) -> ProposalDigest {
    ProposalDigest {
        algorithm: DIGEST_ALGORITHM.to_owned(),
        canonicalizer: CANONICALIZER.to_owned(),
        value: value.to_owned(),
    }
}

fn insert_proposal(connection: &Connection, stored: &StoredProposal) -> Result<(), ProposalError> {
    let submitted_at_ms =
        i64::try_from(stored.submitted_at_ms).map_err(|_| ProposalError::InvalidStore)?;
    connection.execute(
        "INSERT INTO proposals (
             workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
             project_id, mission_id, charter_revision, project_policy_revision,
             repository_revision, pinned_base_revision, requester_resident_id,
             requester_configuration_id, bootstrap_basis, bootstrap_reason, submitted_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            stored.workspace_id.to_string(),
            DIGEST_ALGORITHM,
            CANONICALIZER,
            stored.digest_value.as_str(),
            stored.canonical_bytes.as_slice(),
            stored.project_id.to_string(),
            stored.mission_id.to_string(),
            i64::from(stored.charter_revision),
            i64::from(stored.project_policy_revision),
            i64::from(stored.repository_revision),
            stored.pinned_base_revision.as_str(),
            stored.requester_resident_id.to_string(),
            stored.requester_configuration_id.to_string(),
            BOOTSTRAP_BASIS,
            stored.bootstrap_reason.as_str(),
            submitted_at_ms,
        ],
    )?;
    let payload = serde_json::to_string(&ProposalSubmittedPayload {
        proposal: SubmittedProposal {
            digest: digest_envelope(&stored.digest_value),
            canonical_bytes_base64: BASE64.encode(&stored.canonical_bytes),
            actor: ProposalActor::Owner,
            submitted_at_ms: stored.submitted_at_ms,
        },
    })?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'proposal.submitted', ?2, ?3)",
        params![stored.workspace_id.to_string(), payload, submitted_at_ms],
    )?;
    Ok(())
}

fn load_stored(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<StoredProposal>, ProposalError> {
    let row = connection
        .query_row(
            "SELECT digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
                    project_id, mission_id, charter_revision, project_policy_revision,
                    repository_revision, pinned_base_revision, requester_resident_id,
                    requester_configuration_id, bootstrap_basis, bootstrap_reason, submitted_at_ms
             FROM proposals WHERE workspace_id = ?1 AND digest_value = ?2",
            params![workspace_id.to_string(), digest_value],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
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
    if row.0 != DIGEST_ALGORITHM || row.1 != CANONICALIZER || row.12 != BOOTSTRAP_BASIS {
        return Err(ProposalError::InvalidStore);
    }
    Ok(Some(StoredProposal {
        workspace_id,
        digest_value: row.2,
        canonical_bytes: row.3,
        project_id: parse_uuid(&row.4)?,
        mission_id: parse_uuid(&row.5)?,
        charter_revision: positive_u32(row.6)?,
        project_policy_revision: positive_u32(row.7)?,
        repository_revision: positive_u32(row.8)?,
        pinned_base_revision: row.9,
        requester_resident_id: parse_uuid(&row.10)?,
        requester_configuration_id: parse_uuid(&row.11)?,
        bootstrap_reason: row.13,
        submitted_at_ms: u64::try_from(row.14).map_err(|_| ProposalError::InvalidStore)?,
    }))
}

/// Rebuilds a Proposal from its stored bytes, proving on every read what submission proved once:
/// the digest names these bytes, the bytes decode, they re-encode identically, and every indexed
/// column repeats what the bytes say. A store that disagrees with its own bytes is invalid, not a
/// Proposal with a caveat.
fn verified_proposal(
    connection: &Connection,
    stored: StoredProposal,
) -> Result<Proposal, ProposalError> {
    if HEXLOWER.encode(&sha256(&stored.canonical_bytes)) != stored.digest_value {
        return Err(ProposalError::InvalidStore);
    }
    let payload = decode(&stored.canonical_bytes).map_err(|_| ProposalError::InvalidStore)?;
    if encode(&payload).map_err(|_| ProposalError::InvalidStore)? != stored.canonical_bytes {
        return Err(ProposalError::InvalidStore);
    }
    if !columns_match(&stored, &payload) {
        return Err(ProposalError::InvalidStore);
    }
    // ADR 0034: "Until those content and owning-module surfaces exist, the only accepted value
    // is not_declared." `submit` refuses a `Bound` constraint on the way in; this is the same
    // rule enforced on the way out, so a row that reached the store some other way -- or one a
    // future migration widens the grammar under -- cannot present a supported constraint that no
    // owning module can resolve. Every caller of this function (submit's idempotent-return path,
    // get, list_for_mission, and startup) goes through this check.
    if constraints(&payload.requirements)
        .iter()
        .any(|constraint| constraint.is_declared())
    {
        return Err(ProposalError::InvalidStore);
    }
    let readiness = derive_readiness(connection, stored.workspace_id, &payload)?;
    Ok(Proposal {
        digest: digest_envelope(&stored.digest_value),
        canonical_bytes: stored.canonical_bytes,
        payload,
        actor: ProposalActor::Owner,
        submitted_at_ms: stored.submitted_at_ms,
        readiness,
    })
}

fn columns_match(stored: &StoredProposal, payload: &ProposalV1) -> bool {
    stored.workspace_id == payload.scope.workspace_id
        && stored.project_id == payload.scope.project_id
        && stored.mission_id == payload.scope.mission_id
        && stored.charter_revision == payload.bindings.charter_revision
        && stored.project_policy_revision == payload.bindings.project_policy_revision
        && stored.repository_revision == payload.bindings.repository_revision
        && stored.pinned_base_revision == payload.bindings.pinned_base_revision
        && stored.requester_resident_id == payload.requester.resident_id
        && stored.requester_configuration_id == payload.requester.resident_configuration_id
        && stored.bootstrap_reason == payload.requester.bootstrap.reason()
}

// ---------------------------------------------------------------------------------------------
// Startup reconciliation
// ---------------------------------------------------------------------------------------------

/// Recomputes every Proposal's digest from its stored bytes, strictly decodes and re-encodes them,
/// compares every indexed column to the parsed value, verifies all structural references, and
/// requires exactly one Event per Proposal with no orphan. It also verifies that the Owner account
/// still exists for every v1 bootstrap record.
///
/// Current readiness is derived afterwards and never fails startup: a historical Proposal does not
/// become corrupt merely because a referenced current revision moved.
pub(crate) fn verify_proposal_state(connection: &Connection) -> Result<(), ProposalError> {
    let mut statement = connection.prepare(
        "SELECT workspace_id, digest_value FROM proposals ORDER BY workspace_id, digest_value",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    if !rows.is_empty() && !owner_account_exists(connection)? {
        return Err(ProposalError::InvalidStore);
    }

    for (workspace_id, digest_value) in &rows {
        let workspace_id = parse_uuid(workspace_id)?;
        let stored = load_stored(connection, workspace_id, digest_value)?
            .ok_or(ProposalError::InvalidStore)?;
        let proposal = verified_proposal(connection, stored)?;
        validate_references(connection, workspace_id, proposal.payload())?;
        let event_count: i64 = connection.query_row(
            "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = 'proposal.submitted'
               AND json_extract(payload_json, '$.proposal.digest.value') = ?2",
            params![workspace_id.to_string(), digest_value.as_str()],
            |row| row.get(0),
        )?;
        if event_count != 1 {
            return Err(ProposalError::InvalidStore);
        }
    }

    let total_events: i64 = connection.query_row(
        "SELECT count(*) FROM events WHERE kind = 'proposal.submitted'",
        [],
        |row| row.get(0),
    )?;
    if usize::try_from(total_events) != Ok(rows.len()) {
        return Err(ProposalError::InvalidStore);
    }
    Ok(())
}

/// Requires one Event per Proposal whose decoded byte field exactly equals the stored Proposal
/// bytes, and whose encoding is the canonical padded base64 of them — a differently encoded
/// payload carrying the same bytes is different evidence, not the same evidence reformatted.
pub(crate) fn verify_proposal_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), ProposalError> {
    let payload: ProposalSubmittedPayload = serde_json::from_str(payload_json)?;
    let submitted = payload.proposal;
    if submitted.digest.algorithm != DIGEST_ALGORITHM
        || submitted.digest.canonicalizer != CANONICALIZER
        || submitted.submitted_at_ms != occurred_at_ms
    {
        return Err(ProposalError::InvalidStore);
    }
    let canonical_bytes = BASE64
        .decode(submitted.canonical_bytes_base64.as_bytes())
        .map_err(|_| ProposalError::InvalidStore)?;
    if BASE64.encode(&canonical_bytes) != submitted.canonical_bytes_base64 {
        return Err(ProposalError::InvalidStore);
    }
    if HEXLOWER.encode(&sha256(&canonical_bytes)) != submitted.digest.value {
        return Err(ProposalError::InvalidStore);
    }
    let stored = load_stored(connection, workspace_id, &submitted.digest.value)?
        .ok_or(ProposalError::InvalidStore)?;
    if stored.canonical_bytes != canonical_bytes || stored.submitted_at_ms != occurred_at_ms {
        return Err(ProposalError::InvalidStore);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------------------------

fn parse_uuid(value: &str) -> Result<Uuid, ProposalError> {
    Uuid::parse_str(value).map_err(|_| ProposalError::InvalidStore)
}

fn positive_u32(value: i64) -> Result<u32, ProposalError> {
    let value = u32::try_from(value).map_err(|_| ProposalError::InvalidStore)?;
    if value == 0 {
        return Err(ProposalError::InvalidStore);
    }
    Ok(value)
}

fn map_project_error(error: ProjectError) -> ProposalError {
    match error {
        ProjectError::Store(error) => ProposalError::Store(error),
        ProjectError::Json(error) => ProposalError::Json(error),
        _ => ProposalError::InvalidStore,
    }
}

fn map_mission_error(error: MissionError) -> ProposalError {
    match error {
        MissionError::Store(error) => ProposalError::Store(error),
        MissionError::Json(error) => ProposalError::Json(error),
        _ => ProposalError::InvalidStore,
    }
}

fn map_policy_error(error: ProjectPolicyError) -> ProposalError {
    match error {
        ProjectPolicyError::Store(error) => ProposalError::Store(error),
        ProjectPolicyError::Json(error) => ProposalError::Json(error),
        _ => ProposalError::InvalidStore,
    }
}

fn map_repository_error(error: ProjectRepositoryError) -> ProposalError {
    match error {
        ProjectRepositoryError::Store(error) => ProposalError::Store(error),
        ProjectRepositoryError::Json(error) => ProposalError::Json(error),
        _ => ProposalError::InvalidStore,
    }
}

fn map_lifecycle_error(error: ResidentLifecycleError) -> ProposalError {
    match error {
        ResidentLifecycleError::Store(error) => ProposalError::Store(error),
        ResidentLifecycleError::Json(error) => ProposalError::Json(error),
        _ => ProposalError::InvalidStore,
    }
}

fn map_configuration_error(error: ResidentConfigurationError) -> ProposalError {
    match error {
        ResidentConfigurationError::Store(error) => ProposalError::Store(error),
        ResidentConfigurationError::Json(error) => ProposalError::Json(error),
        _ => ProposalError::InvalidStore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fabricated Proposal. Every identity in it is patterned rather than random so that no
    /// reader mistakes it for something that exists: these UUIDs name no Workspace, the commit id
    /// names no commit, and the context digests are not digests of any file.
    const WORKSPACE_ID: Uuid = Uuid::from_bytes([0x11; 16]);
    const PROJECT_ID: Uuid = Uuid::from_bytes([0x22; 16]);
    const MISSION_ID: Uuid = Uuid::from_bytes([0x33; 16]);
    const RESIDENT_ID: Uuid = Uuid::from_bytes([0x44; 16]);
    const CONFIGURATION_ID: Uuid = Uuid::from_bytes([0x55; 16]);
    const BASE_REVISION: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

    /// The exact canonical bytes of [`fixture_payload`], recorded once and pinned here.
    ///
    /// Independence matters more than convenience: this constant and [`FIXTURE_DIGEST`] were
    /// produced outside the code under test, so a canonicalizer that silently changed its layout
    /// would fail against them rather than agree with itself. The same two values, and the
    /// `sha256sum` command that reproduces the digest from the bytes with no Beacon code at all,
    /// are recorded in `docs/implementation/phase-2-proposal-v1.md`.
    const FIXTURE_RECORD_HEX: &str = concat!(
        "626561636f6e2e70726f706f73616c2e76310011111111111111111111111111",
        "1111112222222222222222222222222222222233333333333333333333333333",
        "3333334444444444444444444444444444444455555555555555555555555555",
        "55555501000000000000003e546865204f776e65722073656c65637465642074",
        "686973205265736964656e7420746f20726571756573742074686973206f6e65",
        "2050726f706f73616c2e00000001000000010000000100000000000000286131",
        "6232633364346535663661316232633364346535663661316232633364346535",
        "663661316232000000000000003041646420612072656772657373696f6e2074",
        "65737420666f722074686520696e76617269616e7420636865636b65722e0200",
        "000002000000000000000a7372632f6c69622e72736666666666666666666666",
        "6666666666666666666666666666666666666666660000000000000013746573",
        "74732f696e76617269616e74732e727377777777777777777777777777777777",
        "7777777777777777777777777777777772e7780cda9baa6a2c6ab87af538f7b6",
        "29937ba6f4c31cb94c3f2aad8cd67a1000000002000000000000000a7372632f",
        "6c69622e7273000000000000001374657374732f696e76617269616e74732e72",
        "730000000202000000000000000a7372632f6c69622e72730100000000000000",
        "1374657374732f696e76617269616e74732e727300000001000000000000000a",
        "636172676f2d7465737400000001000000000000000a636172676f2d74657374",
        "000000030000000000000005636172676f000000000000000474657374000000",
        "000000000b2d2d776f726b737061636500000000000000012e01010101010101",
        "00000000000493e000000000000000f000000000800000000000000010000000",
        "000000000010000000000000000000000101",
    );
    const FIXTURE_DIGEST: &str = "a40cb728defbb93cdad8c9c81286f31a8bca9287b92ec1baa5f260016f7ae404";

    fn context_entries() -> Vec<ContextEntry> {
        vec![
            ContextEntry {
                path: "src/lib.rs".to_owned(),
                content_sha256: [0x66; 32],
            },
            ContextEntry {
                path: "tests/invariants.rs".to_owned(),
                content_sha256: [0x77; 32],
            },
        ]
    }

    fn fixture_payload() -> ProposalV1 {
        let entries = context_entries();
        let manifest_sha256 = sha256(&encoded_context_entries(&entries).unwrap());
        ProposalV1 {
            scope: ProposalScope {
                workspace_id: WORKSPACE_ID,
                project_id: PROJECT_ID,
                mission_id: MISSION_ID,
            },
            requester: ProposalRequester {
                resident_id: RESIDENT_ID,
                resident_configuration_id: CONFIGURATION_ID,
                bootstrap: OwnerBootstrap::Owner {
                    reason: "The Owner selected this Resident to request this one Proposal."
                        .to_owned(),
                },
            },
            bindings: ProposalBindings {
                charter_revision: 1,
                project_policy_revision: 1,
                repository_revision: 1,
                pinned_base_revision: BASE_REVISION.to_owned(),
            },
            intent: ProposalIntent {
                summary: "Add a regression test for the invariant checker.".to_owned(),
                readable_context: ReadableContext::Declared {
                    entries,
                    manifest_sha256,
                },
                target_paths: vec!["src/lib.rs".to_owned(), "tests/invariants.rs".to_owned()],
                declared_mutations: vec![
                    DeclaredMutation {
                        kind: MutationKind::ModifyFile,
                        target: "src/lib.rs".to_owned(),
                    },
                    DeclaredMutation {
                        kind: MutationKind::CreateFile,
                        target: "tests/invariants.rs".to_owned(),
                    },
                ],
                declared_check_runs: vec!["cargo-test".to_owned()],
                declared_checks: vec![DeclaredCheck {
                    name: "cargo-test".to_owned(),
                    argv: vec![
                        "cargo".to_owned(),
                        "test".to_owned(),
                        "--workspace".to_owned(),
                    ],
                    working_directory: ".".to_owned(),
                }],
            },
            requirements: ProposalRequirements {
                execution_policy: ConstraintBinding::NotDeclared,
                sandbox: ConstraintBinding::NotDeclared,
                network: ConstraintBinding::NotDeclared,
                model_provider: ConstraintBinding::NotDeclared,
                command_allowlist: ConstraintBinding::NotDeclared,
                tool_allowlist: ConstraintBinding::NotDeclared,
                evaluations: ConstraintBinding::NotDeclared,
            },
            bounds: ProposalBounds {
                wall_clock_ms: 300_000,
                cpu_seconds: 240,
                address_space_bytes: 2 * 1024 * 1024 * 1024,
                file_size_bytes: 256 * 1024 * 1024,
                output_bytes: 1024 * 1024,
                // Zero is a legal bound: the grammar assigns no positivity rule, and inventing one
                // would refuse bytes ADR 0034 accepts.
                token_ceiling: 0,
            },
            budget: BudgetEnvelope::NoSpend,
            rollback: RollbackStrategy::DiscardDisposableWorktree,
        }
    }

    fn fixture_bytes() -> Vec<u8> {
        encode(&fixture_payload()).unwrap()
    }

    #[test]
    fn every_object_store_failure_maps_to_its_own_reported_state() {
        // Exhaustive over the lower layer's error vocabulary, so an arm added there cannot be
        // folded into an existing reason by a wildcard. The wrong answer is any mapping that
        // sends an error to PinnedRevisionAbsent -- a negative answer Beacon never established.
        assert_eq!(
            inspection_error(ObjectStoreError::Elapsed),
            ContextComparison::InspectionTimedOut
        );
        for broken in [
            ObjectStoreError::NotARepository,
            ObjectStoreError::IndirectionTooDeep,
            ObjectStoreError::UnreadableIndex,
        ] {
            assert_eq!(
                inspection_error(broken),
                ContextComparison::InspectionFailed
            );
        }
    }

    #[test]
    fn an_elapsed_deadline_reaches_the_object_store_and_comes_back_as_elapsed() {
        // Proves the budget is threaded into the inspection rather than only checked around it:
        // the deadline this function passes down is the one `contains` consults, so an elapsed
        // budget surfaces as Elapsed from the store itself -- which `inspection_error` above then
        // reports as not-evaluated. Without the threading it would report a negative answer.
        let root =
            std::env::temp_dir().join(format!("beacond-proposal-deadline-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".git/objects/pack")).unwrap();
        let store = ObjectStore::open(&root).expect("a directory with .git/objects is a store");

        let mut elapsed = || true;
        let outcome = store.contains(&"a".repeat(40), &mut elapsed);

        assert_eq!(outcome, Err(ObjectStoreError::Elapsed));
        assert_eq!(
            inspection_error(outcome.unwrap_err()),
            ContextComparison::InspectionTimedOut
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_elapsed_budget_stops_the_comparison_before_it_touches_the_store() {
        // "Not evaluated" is its own state (AGENTS.md:152). The wrong answer is reporting an
        // elapsed inspection as RepositoryUnreadable -- that says the Operator's repository is
        // broken when nothing was ever looked at.
        //
        // The deadline is consulted on entry, so an already-elapsed budget returns before the
        // connection is read at all. That is what makes the outcome reachable without a slow test:
        // an in-memory store with no schema would fail any query, and none is made.
        let connection = Connection::open_in_memory().unwrap();
        let mut elapsed = || true;
        let outcome = compare_readable_context_within(
            &connection,
            WORKSPACE_ID,
            &fixture_payload(),
            &mut elapsed,
        )
        .unwrap();

        assert_eq!(outcome, Some(ContextComparison::InspectionTimedOut));
    }

    #[test]
    fn a_budget_with_time_left_gets_past_the_entry_check() {
        // The control: a deadline that has NOT elapsed must not short-circuit, or the test above
        // would pass against a function that always reports a timeout.
        let connection = Connection::open_in_memory().unwrap();
        let mut plenty = || false;
        // No `proposals`/`projects` schema exists here, so the first real store read errors. That
        // it errors rather than returning InspectionTimedOut is the whole point.
        let outcome = compare_readable_context_within(
            &connection,
            WORKSPACE_ID,
            &fixture_payload(),
            &mut plenty,
        );

        assert!(
            !matches!(outcome, Ok(Some(ContextComparison::InspectionTimedOut))),
            "a live budget short-circuited as a timeout: {outcome:?}"
        );
    }

    #[test]
    fn a_known_fixture_reproduces_its_digest_from_the_bytes_alone() {
        // The encoder must produce exactly the recorded bytes...
        assert_eq!(HEXLOWER.encode(&fixture_bytes()), FIXTURE_RECORD_HEX);
        // ...and the recorded digest must be the digest of the recorded bytes, computed without
        // going through the encoder at all. Two separate claims: an encoder that drifted would
        // fail the first, and a digest taken over the wrong preimage would fail the second.
        let recorded = HEXLOWER.decode(FIXTURE_RECORD_HEX.as_bytes()).unwrap();
        assert_eq!(HEXLOWER.encode(&sha256(&recorded)), FIXTURE_DIGEST);
    }

    #[test]
    fn the_record_starts_with_the_canonicalizer_magic() {
        let bytes = fixture_bytes();
        assert_eq!(&bytes[..MAGIC.len()], MAGIC);
        assert_eq!(MAGIC, b"beacon.proposal.v1\0");

        let mut wrong = bytes.clone();
        wrong[0] = b'B';
        assert_eq!(decode(&wrong), Err(MalformedProposal::Magic));
        assert_eq!(decode(b"short"), Err(MalformedProposal::Magic));
    }

    #[test]
    fn every_single_byte_of_the_fixture_is_load_bearing() {
        let bytes = fixture_bytes();
        let payload = fixture_payload();
        for offset in 0..bytes.len() {
            let mut mutated = bytes.clone();
            mutated[offset] ^= 0x01;
            match decode(&mutated) {
                // Refused: the byte cannot be changed at all.
                Err(_) => {}
                Ok(decoded) => {
                    // Accepted: it must name a different Proposal, and re-encode to exactly the
                    // bytes supplied. A decoder that normalized would re-encode to the original.
                    assert_ne!(
                        decoded, payload,
                        "byte {offset} decodes to the same payload, so it carries no identity"
                    );
                    assert_ne!(sha256(&mutated), sha256(&bytes));
                    assert_eq!(
                        encode(&decoded).unwrap(),
                        mutated,
                        "byte {offset} was normalized away rather than kept"
                    );
                }
            }
        }
    }

    #[test]
    fn a_trailing_byte_is_refused() {
        let mut bytes = fixture_bytes();
        bytes.push(0x00);
        assert_eq!(decode(&bytes), Err(MalformedProposal::TrailingBytes));
    }

    #[test]
    fn a_truncated_record_is_refused() {
        let bytes = fixture_bytes();
        for length in MAGIC.len()..bytes.len() {
            assert!(decode(&bytes[..length]).is_err(), "prefix {length} decoded");
        }
    }

    #[test]
    fn decode_then_encode_is_the_identity() {
        let bytes = fixture_bytes();
        let payload = decode(&bytes).unwrap();
        assert_eq!(payload, fixture_payload());
        assert_eq!(encode(&payload).unwrap(), bytes);
    }

    #[test]
    fn a_declared_count_that_cannot_fit_is_refused_before_any_entry_is_read() {
        // The decoder proves a declared count against the remaining input and the record's own
        // 64 KiB envelope before decoding a single entry, and never uses the count to reserve
        // capacity (S4: there is no separate, tighter per-field entry-count limit to invent --
        // the envelope is the field's own limit). Two distinct refusals prove both halves of that
        // proof independently: a count that could not exist in ANY legal record, and a count that
        // is individually plausible but does not fit what is actually left in THIS one.
        let bytes = fixture_bytes();
        let marker = 2_u32.to_be_bytes();
        let targets_at = find_target_count_offset(&bytes);
        assert_eq!(&bytes[targets_at..targets_at + 4], &marker);

        // A four-billion-entry target list: bigger than the entire 64 KiB envelope could ever
        // hold no matter what the rest of the record contains.
        let mut over_envelope = bytes.clone();
        over_envelope[targets_at..targets_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            decode(&over_envelope),
            Err(MalformedProposal::OverLimitLength)
        );

        // 10,000 entries: comfortably inside the 64 KiB envelope in the abstract, but this
        // record's remaining bytes cannot possibly hold that many minimum-sized entries.
        let mut over_remaining = bytes.clone();
        over_remaining[targets_at..targets_at + 4].copy_from_slice(&10_000_u32.to_be_bytes());
        assert_eq!(decode(&over_remaining), Err(MalformedProposal::Truncated));
    }

    /// The offset of the target-path list count: immediately after the readable context, which
    /// ends with its 32-byte manifest digest.
    fn find_target_count_offset(bytes: &[u8]) -> usize {
        let entries = context_entries();
        let manifest = sha256(&encoded_context_entries(&entries).unwrap());
        let position = bytes
            .windows(32)
            .position(|window| window == manifest)
            .unwrap();
        position + 32
    }

    #[test]
    fn unsorted_or_duplicate_targets_are_refused_rather_than_sorted() {
        let mut payload = fixture_payload();
        payload.intent.target_paths =
            vec!["tests/invariants.rs".to_owned(), "src/lib.rs".to_owned()];
        assert_eq!(
            decode(&encode(&payload).unwrap()),
            Err(MalformedProposal::NonCanonicalOrder)
        );

        let mut duplicated = fixture_payload();
        duplicated.intent.target_paths = vec!["src/lib.rs".to_owned(), "src/lib.rs".to_owned()];
        assert_eq!(
            decode(&encode(&duplicated).unwrap()),
            Err(MalformedProposal::DuplicateEntry)
        );
    }

    #[test]
    fn unsorted_check_runs_and_declared_checks_are_refused() {
        let mut payload = fixture_payload();
        payload.intent.declared_checks.push(DeclaredCheck {
            name: "audit".to_owned(),
            argv: vec!["cargo".to_owned(), "audit".to_owned()],
            working_directory: ".".to_owned(),
        });
        payload.intent.declared_check_runs = vec!["cargo-test".to_owned(), "audit".to_owned()];
        // "cargo-test" then "audit" is descending; the checks list is out of order for the same
        // reason. Beacon refuses both rather than applying the Repository's canonical name order
        // on the caller's behalf.
        assert_eq!(
            decode(&encode(&payload).unwrap()),
            Err(MalformedProposal::NonCanonicalOrder)
        );
    }

    #[test]
    fn a_check_run_that_names_no_declared_check_is_refused() {
        let mut payload = fixture_payload();
        payload.intent.declared_check_runs = vec!["clippy".to_owned()];
        assert_eq!(
            decode(&encode(&payload).unwrap()),
            Err(MalformedProposal::CheckRunUnresolved)
        );
    }

    #[test]
    fn a_declared_check_without_a_run_is_accepted() {
        // The accepted text requires every run name to resolve in the checks, and says nothing
        // about the converse. Refusing this would be an invented constraint.
        let mut payload = fixture_payload();
        payload.intent.declared_check_runs.clear();
        assert!(decode(&encode(&payload).unwrap()).is_ok());
    }

    #[test]
    fn empty_targets_mutations_runs_and_checks_are_accepted() {
        let mut payload = fixture_payload();
        payload.intent.target_paths.clear();
        payload.intent.declared_mutations.clear();
        payload.intent.declared_check_runs.clear();
        payload.intent.declared_checks.clear();
        let bytes = encode(&payload).unwrap();
        assert_eq!(decode(&bytes).unwrap(), payload);
        assert_ne!(sha256(&bytes), sha256(&fixture_bytes()));
    }

    #[test]
    fn every_mutation_names_a_target_and_every_target_is_mutated() {
        let mut unnamed = fixture_payload();
        unnamed.intent.declared_mutations.pop();
        assert_eq!(
            decode(&encode(&unnamed).unwrap()),
            Err(MalformedProposal::MutationTarget)
        );

        let mut stray = fixture_payload();
        stray.intent.declared_mutations.push(DeclaredMutation {
            kind: MutationKind::DeleteFile,
            target: "src/other.rs".to_owned(),
        });
        assert_eq!(
            decode(&encode(&stray).unwrap()),
            Err(MalformedProposal::MutationTarget)
        );

        let mut duplicated = fixture_payload();
        duplicated.intent.declared_mutations.push(DeclaredMutation {
            kind: MutationKind::ModifyFile,
            target: "src/lib.rs".to_owned(),
        });
        assert_eq!(
            decode(&encode(&duplicated).unwrap()),
            Err(MalformedProposal::DuplicateEntry)
        );
    }

    #[test]
    fn mutation_order_is_the_callers_and_carries_identity() {
        // Ordered, duplicate-free, and an intended sequence: two orders of the same effects are
        // two different Proposals rather than one Proposal written two ways.
        let mut reordered = fixture_payload();
        reordered.intent.declared_mutations.reverse();
        let bytes = encode(&reordered).unwrap();
        assert_eq!(decode(&bytes).unwrap(), reordered);
        assert_ne!(sha256(&bytes), sha256(&fixture_bytes()));

        let mut retyped = fixture_payload();
        retyped.intent.declared_mutations[0].kind = MutationKind::DeleteFile;
        assert_ne!(sha256(&encode(&retyped).unwrap()), sha256(&fixture_bytes()));
    }

    #[test]
    fn not_declared_and_declared_empty_have_different_bytes_and_digests() {
        let mut absent = fixture_payload();
        absent.intent.readable_context = ReadableContext::NotDeclared;
        let mut empty = fixture_payload();
        empty.intent.readable_context = ReadableContext::Declared {
            entries: Vec::new(),
            manifest_sha256: sha256(&encoded_context_entries(&[]).unwrap()),
        };

        let absent_bytes = encode(&absent).unwrap();
        let empty_bytes = encode(&empty).unwrap();
        assert_ne!(absent_bytes, empty_bytes);
        assert_ne!(sha256(&absent_bytes), sha256(&empty_bytes));
        assert_eq!(decode(&absent_bytes).unwrap(), absent);
        assert_eq!(decode(&empty_bytes).unwrap(), empty);
        // Neither is an encoding of the other: absence is one byte, a positive empty declaration
        // is a tag, a zero count, and a digest over that empty list.
        assert_eq!(empty_bytes.len(), absent_bytes.len() + 4 + 32);
    }

    #[test]
    fn a_mismatched_context_manifest_is_refused() {
        let mut payload = fixture_payload();
        payload.intent.readable_context = ReadableContext::Declared {
            entries: context_entries(),
            manifest_sha256: [0x00; 32],
        };
        assert_eq!(
            decode(&encode(&payload).unwrap()),
            Err(MalformedProposal::ManifestMismatch)
        );

        // The entries stay in the Proposal; the manifest is a comparison key, not a substitute for
        // them. Dropping an entry without restating the manifest is caught for the same reason.
        let mut dropped = fixture_payload();
        let ReadableContext::Declared {
            manifest_sha256, ..
        } = fixture_payload().intent.readable_context
        else {
            unreachable!("the fixture declares its readable context")
        };
        dropped.intent.readable_context = ReadableContext::Declared {
            entries: vec![context_entries().remove(0)],
            manifest_sha256,
        };
        assert_eq!(
            decode(&encode(&dropped).unwrap()),
            Err(MalformedProposal::ManifestMismatch)
        );
    }

    #[test]
    fn an_unsorted_readable_context_entry_list_is_refused() {
        let mut entries = context_entries();
        entries.reverse();
        let mut payload = fixture_payload();
        payload.intent.readable_context = ReadableContext::Declared {
            manifest_sha256: sha256(&encoded_context_entries(&entries).unwrap()),
            entries,
        };
        assert_eq!(
            decode(&encode(&payload).unwrap()),
            Err(MalformedProposal::NonCanonicalOrder)
        );
    }

    #[test]
    fn paths_are_refused_rather_than_normalized() {
        for path in [
            "/etc/passwd",
            "../outside.rs",
            "src/../../outside.rs",
            "src/./lib.rs",
            "src//lib.rs",
            "src\\lib.rs",
            "src/lib\0.rs",
            "",
        ] {
            let mut payload = fixture_payload();
            payload.intent.target_paths = vec![path.to_owned()];
            payload.intent.declared_mutations = vec![DeclaredMutation {
                kind: MutationKind::ModifyFile,
                target: path.to_owned(),
            }];
            assert_eq!(
                decode(&encode(&payload).unwrap()),
                Err(MalformedProposal::TargetPath),
                "path {path:?} was not refused"
            );
        }
    }

    #[test]
    fn paths_are_not_unicode_normalized_or_case_folded() {
        // Composed and decomposed forms of the same glyph, and two cases of one word, can name
        // different filesystem objects. Each pair is two distinct accepted paths with two
        // identities, never one path Beacon chose on the caller's behalf.
        for (left, right) in [
            ("caf\u{e9}.rs", "cafe\u{301}.rs"),
            ("Readme.md", "readme.md"),
        ] {
            let mut ordered = [left.to_owned(), right.to_owned()];
            ordered.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            let mut payload = fixture_payload();
            payload.intent.target_paths = ordered.to_vec();
            payload.intent.declared_mutations = ordered
                .iter()
                .map(|target| DeclaredMutation {
                    kind: MutationKind::ModifyFile,
                    target: target.clone(),
                })
                .collect();
            let decoded = decode(&encode(&payload).unwrap()).unwrap();
            assert_eq!(decoded.intent.target_paths, ordered.to_vec());
            assert_ne!(
                decoded.intent.target_paths[0],
                decoded.intent.target_paths[1]
            );
        }
    }

    #[test]
    fn human_text_must_be_trimmed_bounded_and_free_of_control_characters() {
        for summary in [
            "  leading space",
            "trailing space  ",
            "",
            "a bell\u{7}rings",
            "a null\0byte",
        ] {
            let mut payload = fixture_payload();
            payload.intent.summary = summary.to_owned();
            assert_eq!(
                decode(&encode(&payload).unwrap()),
                Err(MalformedProposal::Summary),
                "summary {summary:?} was not refused"
            );
        }

        let mut long = fixture_payload();
        long.intent.summary = "\u{e9}".repeat(MAX_SUMMARY_SCALARS + 1);
        assert_eq!(
            decode(&encode(&long).unwrap()),
            Err(MalformedProposal::Summary)
        );

        let mut multiline = fixture_payload();
        multiline.intent.summary = "Two lines.\nStill one summary.".to_owned();
        assert!(decode(&encode(&multiline).unwrap()).is_ok());

        let mut reason = fixture_payload();
        reason.requester.bootstrap = OwnerBootstrap::Owner {
            reason: " untrimmed ".to_owned(),
        };
        assert_eq!(
            decode(&encode(&reason).unwrap()),
            Err(MalformedProposal::BootstrapReason)
        );
    }

    #[test]
    fn the_pinned_base_revision_is_lowercase_hexadecimal_text() {
        for revision in [
            "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2",
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b",
            "g1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        ] {
            let mut payload = fixture_payload();
            payload.bindings.pinned_base_revision = revision.to_owned();
            assert!(
                matches!(
                    decode(&encode(&payload).unwrap()),
                    Err(MalformedProposal::PinnedBaseRevision | MalformedProposal::OverLimitLength)
                ),
                "revision {revision:?} was not refused"
            );
        }
    }

    #[test]
    fn a_bound_descriptor_decodes_but_is_never_accepted_by_submission() {
        // The tag is defined now so a later reviewed decision needs no new tag, and existing
        // identities are never reinterpreted. Submission still refuses it: no owning module can
        // resolve one, and treating an unknown kind as supported would assert a check Beacon
        // cannot perform. `submit` enforces that; the grammar records the shape.
        let mut payload = fixture_payload();
        payload.requirements.sandbox = ConstraintBinding::Bound {
            kind: "landlock".to_owned(),
            version: 1,
            canonicalizer: "beacon.sandbox.descriptor.v1".to_owned(),
            sha256: [0x88; 32],
        };
        let bytes = encode(&payload).unwrap();
        assert_eq!(decode(&bytes).unwrap(), payload);
        assert_ne!(sha256(&bytes), sha256(&fixture_bytes()));

        for broken in [
            ConstraintBinding::Bound {
                kind: String::new(),
                version: 1,
                canonicalizer: "beacon.sandbox.v1".to_owned(),
                sha256: [0x88; 32],
            },
            ConstraintBinding::Bound {
                kind: "Landlock".to_owned(),
                version: 1,
                canonicalizer: "beacon.sandbox.v1".to_owned(),
                sha256: [0x88; 32],
            },
            ConstraintBinding::Bound {
                kind: "landlock".to_owned(),
                version: 0,
                canonicalizer: "beacon.sandbox.v1".to_owned(),
                sha256: [0x88; 32],
            },
        ] {
            let mut invalid = fixture_payload();
            invalid.requirements.sandbox = broken;
            assert_eq!(
                decode(&encode(&invalid).unwrap()),
                Err(MalformedProposal::DescriptorIdentity)
            );
        }
    }

    #[test]
    fn an_unassigned_tag_is_refused_in_every_closed_position() {
        let bytes = fixture_bytes();
        let payload = fixture_payload();
        let mut unassigned = 0;
        for offset in 0..bytes.len() {
            let mut mutated = bytes.clone();
            mutated[offset] = 0xFE;
            if decode(&mutated) == Err(MalformedProposal::UnknownTag) {
                unassigned += 1;
            }
        }
        // Bootstrap basis, readable context, two mutation kinds, seven constraints, budget, and
        // rollback: every closed variant position refuses a tag this grammar never assigned.
        assert!(unassigned >= 13, "only {unassigned} tag positions refused");
        assert_eq!(decode(&bytes).unwrap(), payload);
    }

    #[test]
    fn each_identity_bearing_field_changes_the_digest() {
        let base = sha256(&fixture_bytes());
        let mut variants: Vec<(&str, ProposalV1)> = Vec::new();

        let mut requester = fixture_payload();
        requester.requester.resident_id = Uuid::from_bytes([0x99; 16]);
        variants.push(("requester", requester));

        let mut configuration = fixture_payload();
        configuration.requester.resident_configuration_id = Uuid::from_bytes([0x9a; 16]);
        variants.push(("configuration", configuration));

        let mut reason = fixture_payload();
        reason.requester.bootstrap = OwnerBootstrap::Owner {
            reason: "A different recorded reason.".to_owned(),
        };
        variants.push(("bootstrap reason", reason));

        let mut policy = fixture_payload();
        policy.bindings.project_policy_revision = 2;
        variants.push(("project policy revision", policy));

        let mut repository = fixture_payload();
        repository.bindings.repository_revision = 2;
        variants.push(("repository revision", repository));

        let mut base_revision = fixture_payload();
        base_revision.bindings.pinned_base_revision =
            "b1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_owned();
        variants.push(("pinned base revision", base_revision));

        let mut targets = fixture_payload();
        targets.intent.target_paths = vec!["src/lib.rs".to_owned()];
        targets.intent.declared_mutations = vec![DeclaredMutation {
            kind: MutationKind::ModifyFile,
            target: "src/lib.rs".to_owned(),
        }];
        variants.push(("targets", targets));

        let mut mutations = fixture_payload();
        mutations.intent.declared_mutations[0].kind = MutationKind::CreateFile;
        variants.push(("mutations", mutations));

        let mut runs = fixture_payload();
        runs.intent.declared_check_runs.clear();
        variants.push(("check runs", runs));

        let mut checks = fixture_payload();
        checks.intent.declared_checks[0].argv.pop();
        variants.push(("checks", checks));

        let mut bounds = fixture_payload();
        bounds.bounds.token_ceiling = 1;
        variants.push(("bounds", bounds));

        for (name, variant) in variants {
            let bytes = encode(&variant).unwrap();
            assert_ne!(sha256(&bytes), base, "{name} did not change the digest");
            assert_eq!(
                decode(&bytes).unwrap(),
                variant,
                "{name} did not round-trip"
            );
        }

        // Budget and rollback are single-arm in v1, so no two accepted Proposals can differ in
        // them. Their bytes are proved load-bearing instead: change the tag and the identity
        // changes, and the record is refused rather than read as the value it used to hold.
        let bytes = fixture_bytes();
        for position in [bytes.len() - 2, bytes.len() - 1] {
            let mut mutated = bytes.clone();
            mutated[position] = 0x02;
            assert_ne!(sha256(&mutated), base);
            assert_eq!(decode(&mutated), Err(MalformedProposal::UnknownTag));
        }
    }

    #[test]
    fn every_bound_is_encoded_and_zero_is_a_legal_value() {
        let base = sha256(&fixture_bytes());
        let mut fields: Vec<(&str, ProposalBounds)> = Vec::new();
        let bounds = fixture_payload().bounds;
        fields.push((
            "wall_clock_ms",
            ProposalBounds {
                wall_clock_ms: 1,
                ..bounds
            },
        ));
        fields.push((
            "cpu_seconds",
            ProposalBounds {
                cpu_seconds: 1,
                ..bounds
            },
        ));
        fields.push((
            "address_space_bytes",
            ProposalBounds {
                address_space_bytes: 1,
                ..bounds
            },
        ));
        fields.push((
            "file_size_bytes",
            ProposalBounds {
                file_size_bytes: 1,
                ..bounds
            },
        ));
        fields.push((
            "output_bytes",
            ProposalBounds {
                output_bytes: 1,
                ..bounds
            },
        ));
        fields.push((
            "token_ceiling",
            ProposalBounds {
                token_ceiling: 1,
                ..bounds
            },
        ));
        for (name, replacement) in fields {
            let mut payload = fixture_payload();
            payload.bounds = replacement;
            assert_ne!(
                sha256(&encode(&payload).unwrap()),
                base,
                "{name} is not in the preimage"
            );
        }

        let mut zeroed = fixture_payload();
        zeroed.bounds = ProposalBounds {
            wall_clock_ms: 0,
            cpu_seconds: 0,
            address_space_bytes: 0,
            file_size_bytes: 0,
            output_bytes: 0,
            token_ceiling: 0,
        };
        assert_eq!(decode(&encode(&zeroed).unwrap()).unwrap(), zeroed);
    }
}
