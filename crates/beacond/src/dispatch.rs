//! Dispatch-on-match: the gates (ADR 0039).
//!
//! Dispatch is the only act in Beacon that may mutate a Git repository on the strength of a record.
//! **The repository it may mutate is a Beacon-made disposable copy and nothing else**, enforced by a
//! provenance gate against durable Beacon-recorded identity rather than by a pathname convention.
//! Landing into a repository the Operator owns stays disabled (ADR 0039, "Conditions on the landing
//! slices").
//!
//! The gates were built and proved before the landing, deliberately: BLUEPRINT `:907` says "a denied
//! or stale Proposal causes zero repository mutation", and a negative criterion is proved most
//! honestly by a version that cannot mutate anything at all, before the version that can. Those
//! measurements are still the row-34 tests, now run against a build that has a landing path.
//!
//! # What the gates are, and why they run in this order
//!
//! Every gate is re-derived here, at dispatch time, from bytes. None may be read from a cached
//! readiness value, an earlier Receipt, or a caller's claim: a decision that trusts a value computed
//! a moment ago is trusting a summary of a world that has since moved. The record gates run before
//! the live ones, and both run before anything would be written, so a refusal costs the repository
//! nothing.
//!
//! # What this module is careful about
//!
//! **A Candidate selects bytes; a Ruling authorizes them.** A `beacon.ruling.v1` record is a
//! complete decision that names no Candidate, so it is not dispatchable — not because it is old,
//! but because nothing in it says which bytes the Operator saw.
//!
//! **The repository is pinned by identity, not by pathname.** A kernel review redirected a mutation
//! by renaming the source directory and installing a decoy at the same path between the gate and the
//! act. Every read here goes through [`PinnedRepository`], and the pin is re-checked after each one.
//!
//! **Gate reads may not write.** `git status --porcelain` rewrites `.git/index` unless
//! `GIT_OPTIONAL_LOCKS=0` is set, so the head-and-cleanliness gate would otherwise mutate the
//! repository it is measuring. Reads use `worktree::read_only_git_command`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::candidate::{candidate_event_sequence, verified_candidate_for_proposal};
use crate::operator_account::{OperatorAccountError, OperatorAccountService};
use crate::project::current_autonomy_profile;
use crate::project_repository::{PreflightBinding, repository_for_preflight_at};
use crate::proposal::{
    MutationKind, ProposalDigest, UnmetPrecondition, proposal_row_scope, verified_proposal_within,
};
use crate::ruling::{
    RulingDecision, RulingDigest, ruling_event_sequence, ruling_row_scope, verified_ruling_within,
};
use crate::worktree::{
    GitIdentity, PinnedRepository, READ_BUDGET, WRITE_BUDGET, read_pinned, write_pinned,
};
use crate::{AutonomyProfile, CandidateDigest, RepositorySourceKind};

/// A dispatch Run's lease.
const LEASE_MS: u64 = 60_000;

/// Who Beacon says wrote a dispatched commit. Permanent, public-facing bytes in someone else's
/// repository: ADR 0039 asks the Product Steward to confirm this identity before any dispatch into
/// a repository that is not a Beacon-made copy.
const COMMIT_AUTHOR_NAME: &str = "Beacon";
const COMMIT_AUTHOR_EMAIL: &str = "beacon@localhost";

/// How a dispatch ended.
///
/// A refusal names the gate that refused it, because an Operator reads a correction rather than a
/// category. The three families never collapse: a gate said no, the act applied a change, or the
/// act's result could not be established.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchOutcome {
    /// The Ruling's bytes, columns, relations or Event do not agree with each other.
    RulingUnreadable,
    /// The Ruling refused the Proposal. **Row 34's "denied".**
    RulingRefused,
    /// A later Ruling reverses this one.
    RulingSuperseded,
    /// A revision the Ruling copied has moved. **Row 34's "stale", as the store sees it.**
    RulingDrifted,
    /// The stored Proposal's bytes do not hash to the digest the Ruling names.
    ProposalDigestMismatch,
    /// A live precondition of the Proposal does not hold.
    ProposalNotReady,
    /// The Proposal has no Candidate, or the Ruling names none, so no bytes are authorized.
    CandidateAbsent,
    /// The Candidate the Ruling names is not the Candidate the Proposal has, or was recorded after
    /// the decision that claims to have seen it.
    CandidateMismatch,
    /// Supplied content does not hash to what the Candidate declares.
    ContentDigestMismatch,
    /// Supplied content exceeds a bound the Proposal itself declared.
    ContentOverBounds,
    /// The Repository Binding names something other than a local path.
    SourceNotLocal,
    /// The bound repository is not a Beacon-made disposable copy. v1 lands nowhere else.
    SourceNotBeaconMade,
    /// The repository's head is not the pinned base revision. **Row 34's "stale", live.**
    RepositoryHeadMoved,
    /// The repository's head is not on a branch, so there is no ref to move.
    SourceDetached,
    /// The repository has uncommitted or untracked content.
    SourceDirty,
    /// The Project's autonomy profile does not admit dispatch.
    AutonomyProfileRefuses,
    /// The Installation is paused.
    InstallationPaused,
    /// A declared pre-image is not what the pinned base revision holds: the Candidate describes a
    /// tree that is not there.
    PreImageMismatch,
    /// The attempt exceeded the wall-clock bound the Proposal itself declared.
    WallClockExceeded,
    /// This Ruling has already been dispatched. One decision authorizes one application: an attempt
    /// whose result was never established stays spent until the Operator resolves it, rather than
    /// being retried into a repeat effect.
    AlreadyDispatched,
    /// Every gate held and the act was not attempted.
    ///
    /// Emitted by the build that had no landing path. Retained so recorded Receipts remain
    /// decodable; new dispatches do not emit it.
    Admissible,
    /// An object, tree or commit could not be written. **After preparation**: the repository is not
    /// byte-identical, because objects already written are reachable from no ref.
    ObjectWriteFailed,
    /// The prepared commit does not touch exactly the declared set, or a blob in it does not hash
    /// to what the Candidate declared. Refused rather than published, with the same residue.
    EffectDoesNotMatchDeclaration,
    /// The compare-and-swap refused: the branch no longer held the pinned base at the moment of the
    /// swap. Nothing was published; the prepared objects remain unreferenced.
    LandingRefused,
    /// The swap's result could not be established. The Receipt names the prepared commit, and the
    /// preparation record persisted it before the attempt so this is answerable afterwards.
    LandingUnobserved,
    /// The branch now names the prepared commit, verified after the swap.
    Applied,
}

impl DispatchOutcome {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::RulingUnreadable => "ruling_unreadable",
            Self::RulingRefused => "ruling_refused",
            Self::RulingSuperseded => "ruling_superseded",
            Self::RulingDrifted => "ruling_drifted",
            Self::ProposalDigestMismatch => "proposal_digest_mismatch",
            Self::ProposalNotReady => "proposal_not_ready",
            Self::CandidateAbsent => "candidate_absent",
            Self::CandidateMismatch => "candidate_mismatch",
            Self::ContentDigestMismatch => "content_digest_mismatch",
            Self::ContentOverBounds => "content_over_bounds",
            Self::SourceNotLocal => "source_not_local",
            Self::SourceNotBeaconMade => "source_not_beacon_made",
            Self::RepositoryHeadMoved => "repository_head_moved",
            Self::SourceDetached => "source_detached",
            Self::SourceDirty => "source_dirty",
            Self::AutonomyProfileRefuses => "autonomy_profile_refuses",
            Self::InstallationPaused => "installation_paused",
            Self::PreImageMismatch => "pre_image_mismatch",
            Self::WallClockExceeded => "wall_clock_exceeded",
            Self::AlreadyDispatched => "already_dispatched",
            Self::Admissible => "admissible",
            Self::ObjectWriteFailed => "object_write_failed",
            Self::EffectDoesNotMatchDeclaration => "effect_does_not_match_declaration",
            Self::LandingRefused => "landing_refused",
            Self::LandingUnobserved => "landing_unobserved",
            Self::Applied => "applied",
        }
    }

    /// The Run state this outcome terminates in. One owner for the mapping, as
    /// `PreflightOutcome::run_state` and `ExecutionSummary::run_state` are.
    ///
    /// Everything this build can produce is a refusal in Run terms: `Admissible` says the act would
    /// have been permitted and that nothing was done, which is not `applied`.
    pub(crate) fn run_state(&self) -> &'static str {
        match self {
            Self::Applied => "applied",
            // Everything else is a Run that did not apply a change. `LandingUnobserved` is a
            // terminal Receipt whose Git result is unknown -- which is not `unconfirmed`: ADR 0031
            // reserves that for a lease that expired with NO Receipt, and reusing the word would
            // make two different facts one.
            _ => "refused",
        }
    }
}

/// One declared mutation as dispatch resolved it against the supplied content.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MutationRecord {
    pub kind: MutationKind,
    pub path: String,
    /// What the Candidate declares the file hashes to before the change, as lowercase hex.
    pub pre_image: Option<String>,
    /// What it declares afterwards.
    pub post_image: Option<String>,
    /// What this dispatch did with it. Recorded per mutation rather than once, so an attempt that
    /// wrote some and not others has a place to say so.
    pub effect: MutationEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationEffect {
    /// Verified against the supplied content and not written.
    NotAttempted,
    /// Written into the prepared commit, and the blob in that commit's tree hashes to what the
    /// Candidate declared. A fact about objects, never about a checkout.
    Written,
    /// Removed in the prepared commit: the path resolves to nothing there.
    Removed,
}

/// The record of one dispatch.
///
/// It fills, with measurements, the slots a preflight Receipt records as absent — the Proposal and
/// Ruling digests above all — and names in `not_verified` everything it did not establish.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DispatchReceipt {
    pub version: u32,
    pub run_id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    /// The authenticated actor. Derived here, never accepted from a caller.
    pub actor: DispatchActor,
    pub ruling_digest: RulingDigest,
    /// Present once the Ruling could be read; absent when the refusal was the Ruling itself.
    pub proposal_digest: Option<ProposalDigest>,
    pub candidate_digest: Option<CandidateDigest>,
    pub outcome: DispatchOutcome,
    /// The autonomy profile this dispatch ran under, recorded because it is a gate.
    pub autonomy_profile: Option<AutonomyProfile>,
    pub charter_revision: u32,
    pub project_policy_revision: u32,
    pub repository_revision: u32,
    pub source_kind: Option<RepositorySourceKind>,
    pub source: Option<String>,
    /// The device and inode of the directory the gates measured and the swap entered. A pathname is
    /// what a rename moves; this is what "the same repository" means to the kernel.
    pub source_identity: Option<SourceIdentity>,
    /// The ref the swap targeted.
    pub branch: Option<String>,
    /// The base revision the Ruling pinned, and the head observed live. They are different facts
    /// and are recorded separately.
    pub pinned_base_revision: Option<String>,
    pub observed_head: Option<String>,
    /// The commit this dispatch prepared, present from the moment it exists -- including when the
    /// landing's result could not be established, which is the case it exists for.
    pub prepared_commit: Option<String>,
    /// The revision the branch names afterwards, when that was verified.
    pub resulting_revision: Option<String>,
    /// SHA-256 over the prepared commit's own name-status output -- a measured diff digest, which
    /// a Candidate digest is not.
    pub diff_digest: Option<String>,
    /// Whether Beacon updated the copy's working tree after the swap. A separately observable step,
    /// recorded separately.
    pub checkout_updated: bool,
    /// Every Git invocation this dispatch attempted, in order, with a bounded result.
    pub git_steps: Vec<GitStep>,
    /// Measured zeroes: a dispatch runs no Declared Check and invokes no model, by construction.
    pub model_invocations: u32,
    pub cost_usd_micros: u64,
    /// One row per declared mutation, in the Proposal's order. Empty when the refusal came before
    /// the Candidate could be read.
    pub per_mutation: Vec<MutationRecord>,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    /// Everything this dispatch did not establish, as a **closed vocabulary in a fixed order**. A
    /// Receipt that cannot say what it failed to check is not evidence (ADR 0027), and free-form
    /// strings cannot be checked by a reader or by reconciliation (gpt1).
    pub not_verified: Vec<NotVerified>,
}

impl DispatchReceipt {
    /// Was the repository byte-identical afterwards?
    ///
    /// Read off the evidence rather than inferred from the outcome: objects are written exactly
    /// when a commit was prepared, so a Receipt with no prepared commit is one that refused before
    /// anything reached the object database. A Receipt WITH one may have left objects no ref names,
    /// which is the weaker claim and the one gpt1 required be stated separately.
    pub fn left_repository_byte_identical(&self) -> bool {
        self.prepared_commit.is_none()
    }
}

/// What a dispatch could not establish. Closed, ordered, and the same set on every Receipt except
/// for the arms that describe one attempt's own outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotVerified {
    /// The seven families ADR 0034 leaves undeclarable in Proposal v1. One arm each, because an
    /// Operator reading a Receipt should see which contract is missing, not a count.
    ExecutionPolicyConstraint,
    SandboxConstraint,
    NetworkConstraint,
    ModelProviderConstraint,
    CommandAllowlistConstraint,
    ToolAllowlistConstraint,
    EvaluationsConstraint,
    /// No Ruling grammar cites an Evaluation, so no dispatch can claim a non-author one.
    NonAuthorEvaluationOfCandidate,
    /// No snapshot mechanism exists (`BLUEPRINT.md:450-452`).
    SafetySnapshot,
    /// The object store answers presence, not reachability from a ref.
    BaseRevisionReachability,
    /// The Declared Checks were not run against the applied change: that is its own Run.
    DeclaredChecksAgainstResult,
    /// A revision the Ruling copied has moved; the Receipt's drift list says which.
    RulingBindingsMoved,
    /// A live precondition of the Proposal does not hold.
    ProposalPreconditions,
    /// The change was not applied.
    ChangeNotApplied,
    /// Whether the branch moved could not be established; `prepared_commit` names what to inspect.
    LandingResultUnknown,
    /// Objects were written that no ref names.
    UnreferencedObjectResidue,
    /// This Receipt was reconstructed at startup after an interruption. It establishes what the
    /// repository says now, not what the attempt observed.
    ReconstructedAfterInterruption,
    /// The interruption fell between writing objects and persisting what they were, so unreferenced
    /// objects exist and nothing in the store can name them.
    ResidueCannotBeNamed,
}

/// The device and inode of a pinned repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceIdentity {
    pub device: u64,
    pub inode: u64,
}

/// One Git invocation a dispatch attempted, and how it ended.
///
/// The Blueprint's Receipt contract requires the commands run and their results. Recorded as a
/// closed vocabulary rather than as argv strings and stderr: a bounded category is what a later
/// reader can act on, and dynamic error text is neither stable nor safe to replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitStep {
    pub step: GitStepName,
    pub result: GitStepResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitStepName {
    /// `read-tree` of the pinned base into a scratch index.
    ReadTree,
    /// `hash-object --stdin`, one per written mutation.
    HashObject,
    /// `update-index`, one per declared mutation.
    UpdateIndex,
    /// `write-tree`.
    WriteTree,
    /// `commit-tree`.
    CommitTree,
    /// `diff --name-status -z`, the effect post-condition.
    DiffNameStatus,
    /// `update-ref <ref> <new> <old>`: the compare-and-swap.
    UpdateRef,
    /// `read-tree -u --reset` on Beacon's own copy, after the swap: index and working tree only,
    /// never the ref.
    ReadTreeCheckout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitStepResult {
    Succeeded,
    /// Git ran and did not do it. For the swap this is the ref no longer holding the expected
    /// value; it is never "we could not tell".
    Refused,
}

/// The authenticated command actor, which for a dispatch is always the Owner.
///
/// One closed arm, derived at the authenticated boundary and never taken from a caller: "no Agent
/// may dispatch" is a fact about the vocabulary and the call path, not a sentence in a doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchActor {
    Owner,
}

/// Why a dispatch could not be attempted at all. These produce no Run and no Receipt: there was
/// nothing to dispatch.
#[derive(Debug)]
pub enum DispatchError {
    Account(OperatorAccountError),
    /// Resume was refused: these Runs have not been resolved or explicitly accepted.
    UnresolvedRuns(UnresolvedRuns),
    /// No Ruling in this Workspace has that digest.
    RulingNotFound,
    /// The Ruling names a Proposal this Workspace does not have, which is a broken store rather
    /// than a decision about a repository.
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::UnresolvedRuns(unresolved) => write!(
                formatter,
                "{} Run(s) are unresolved; resolve them or accept their uncertainty",
                unresolved.run_ids.len()
            ),
            Self::RulingNotFound => write!(formatter, "no Ruling has that digest"),
            Self::InvalidStore => write!(formatter, "dispatch state is invalid"),
            Self::Store(error) => write!(formatter, "dispatch store failed: {error}"),
            Self::Json(error) => write!(formatter, "dispatch Receipt is invalid: {error}"),
        }
    }
}

impl std::error::Error for DispatchError {}

impl From<OperatorAccountError> for DispatchError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for DispatchError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for DispatchError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

// ---------------------------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------------------------

/// Owns the dispatch gates behind one deep interface.
///
/// Takes no root and no path: which repositories a dispatch may reach is a fact the store holds,
/// written by whatever act created them, and not a directory a caller names.
pub struct DispatchService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
    #[cfg(debug_assertions)]
    failpoints: DispatchFailpoints,
}

/// Places a test can stop this act dead, to reproduce a crash image rather than synthesize one.
///
/// Two reviews rejected the recovery evidence for the same reason: the tests deleted committed
/// records to arrange a state, which proves what reconciliation does with a state nobody showed was
/// reachable. A hook that panics inside the act leaves exactly what a killed process leaves -- the
/// external Git effects that already happened, the store transactions that had already committed,
/// and nothing of the one that was open.
///
/// Compiled out of release builds entirely: the field, the calls and this type do not exist there.
#[cfg(debug_assertions)]
#[derive(Default, Clone)]
#[doc(hidden)]
pub struct DispatchFailpoints {
    /// After the objects exist, before the Ruling is claimed.
    pub before_reserve: Option<Arc<dyn Fn() + Send + Sync>>,
    /// After the claim is committed, before the landing transaction opens.
    pub before_landing: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Inside the landing transaction, immediately after the swap and before anything about it is
    /// committed.
    pub after_swap: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Inside the landing transaction, after the swap was observed and before Beacon updates the
    /// copy's working tree -- the window a checkout that moves the ref would overwrite.
    pub before_checkout: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// One declared mutation's content, supplied by the caller at dispatch time.
///
/// Content is supplied rather than stored: a Candidate carries digests only, and this is what those
/// digests are checked against. `None` is a deletion, which has no content by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuppliedContent {
    pub path: String,
    pub bytes: Option<Vec<u8>>,
}

impl DispatchService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
            #[cfg(debug_assertions)]
            failpoints: DispatchFailpoints::default(),
        }
    }

    /// The same service, interruptible at the points above. Debug builds only.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    #[must_use]
    pub fn with_failpoints(mut self, failpoints: DispatchFailpoints) -> Self {
        self.failpoints = failpoints;
        self
    }

    /// Run every gate for one Ruling and record what they decided.
    ///
    /// The gates, then the act. A refusal at any gate leaves the repository byte-identical; a
    /// refusal after the commit step leaves the refs and working tree unchanged and may leave
    /// objects no ref names, which the Receipt says.
    pub fn dispatch(
        &self,
        ruling_digest: &str,
        content: &[SuppliedContent],
    ) -> Result<DispatchReceipt, DispatchError> {
        let opened = self.open_run(ruling_digest)?;
        let mut decided = self.decide(&opened, content)?;
        // Only a decision that held reaches the act. Everything above this line left the repository
        // byte-identical; everything below may not, and says which.
        if decided.outcome == Some(DispatchOutcome::Admissible) {
            decided.outcome = Some(self.land(&opened, &mut decided, content)?);
        }
        self.close(opened, decided)
    }

    /// Open the Run, pinning the revisions this attempt is about.
    ///
    /// The Ruling is read once here to find the Project and the revisions the Run row must carry;
    /// every gate below re-reads it inside the deciding transaction rather than trusting this.
    fn open_run(&self, ruling_digest: &str) -> Result<OpenRun, DispatchError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let started_at_ms = u64::try_from(now).map_err(|_| DispatchError::InvalidStore)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Same reasoning for the Ruling: `ruling_row_scope` reads the columns the Run row
            // needs. Whether those bytes prove is the first gate's question, and it must be
            // answerable with a Receipt.
            let Some((proposal_digest, bindings)) =
                ruling_row_scope(&transaction, self.workspace_id, ruling_digest)
                    .map_err(|_| DispatchError::InvalidStore)?
            else {
                return Err(DispatchError::RulingNotFound);
            };
            // The Run row's own columns come from the Proposal's indexed row, NOT from a proof: a
            // record too corrupt to prove must still reach the gate that refuses it, and a Run that
            // never opened cannot carry a Receipt saying why (gpt1's P0).
            let Some((project_id, _)) =
                proposal_row_scope(&transaction, self.workspace_id, &proposal_digest)
                    .map_err(|_| DispatchError::InvalidStore)?
            else {
                return Err(DispatchError::InvalidStore);
            };
            // The declared wall-clock ceiling, read from the Proposal's own bytes. A record too
            // corrupt to prove yields the smallest ceiling rather than none: the gates refuse it in
            // a moment either way, and an unbounded attempt is never the safe default.
            let wall_clock_ms =
                verified_proposal_within(&transaction, self.workspace_id, &proposal_digest)
                    .ok()
                    .flatten()
                    .map_or(0, |proposal| proposal.payload().bounds.wall_clock_ms);
            let run_id = Uuid::new_v4();
            transaction.execute(
                "INSERT INTO runs (
                     workspace_id, id, project_id, kind, repository_revision, charter_revision,
                     state, lease_expires_at_ms, started_at_ms, ended_at_ms
                 ) VALUES (?1, ?2, ?3, 'proposal_dispatch', ?4, ?5, 'dispatching', ?6, ?7, NULL)",
                params![
                    self.workspace_id.to_string(),
                    run_id.to_string(),
                    project_id.to_string(),
                    i64::from(bindings.repository_revision),
                    i64::from(bindings.charter_revision),
                    i64::try_from(started_at_ms.saturating_add(LEASE_MS))
                        .map_err(|_| DispatchError::InvalidStore)?,
                    i64::try_from(started_at_ms).map_err(|_| DispatchError::InvalidStore)?,
                ],
            )?;
            transaction.commit()?;
            Ok(OpenRun {
                run_id,
                project_id,
                ruling_digest: ruling_digest.to_owned(),
                started_at_ms,
                wall_clock_ms,
            })
        })
    }

    /// Every gate, inside one immediate transaction.
    ///
    /// One transaction for the record gates is what makes the answer a fact about one moment rather
    /// than a sequence of separate observations. The live repository reads happen inside it too:
    /// they cannot be transactional, and the transaction is what stops the store's own answers from
    /// moving underneath them.
    fn decide(
        &self,
        opened: &OpenRun,
        content: &[SuppliedContent],
    ) -> Result<Decided, DispatchError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let decided = self.gates(&transaction, opened, content)?;
            transaction.commit()?;
            Ok(decided)
        })
    }

    #[allow(clippy::too_many_lines)]
    fn gates(
        &self,
        connection: &Connection,
        opened: &OpenRun,
        content: &[SuppliedContent],
    ) -> Result<Decided, DispatchError> {
        let mut decided = Decided::default();

        // G2: the Ruling, proved from its own bytes, with its force derived now and its Event
        // required. A snapshot taken before this transaction would be a fact about another moment.
        // A Ruling that cannot be proved is a REFUSAL, not an error: the Run is already open, and
        // an error path would leave it nonterminal with no Receipt -- which AGENTS.md:154 forbids
        // and which would make "every gate refuses with evidence" false for exactly the states the
        // gate is named for (gpt1's P0).
        let ruling =
            match verified_ruling_within(connection, self.workspace_id, &opened.ruling_digest) {
                Ok(Some(ruling)) => ruling,
                Ok(None) | Err(_) => {
                    return Ok(decided.refused(DispatchOutcome::RulingUnreadable));
                }
            };
        decided.ruling_digest = Some(ruling.digest().clone());
        decided.proposal_digest = Some(ruling.payload().proposal.clone());
        decided.bindings = Some(ruling.payload().bindings.clone());

        // G3-G5: what the Ruling decided, and whether it still applies. Reported separately
        // because a Ruling can be both superseded and drifted, and an Operator corrects each
        // differently.
        if ruling.payload().decision != RulingDecision::Approve {
            return Ok(decided.refused(DispatchOutcome::RulingRefused));
        }
        if ruling.force().superseded_by().is_some() {
            return Ok(decided.refused(DispatchOutcome::RulingSuperseded));
        }
        if !ruling.force().drift().is_empty() {
            decided.drift = ruling.force().drift().to_vec();
            return Ok(decided.refused(DispatchOutcome::RulingDrifted));
        }

        // G6-G7: the Proposal, through its owning module's full contract -- which includes
        // readiness. A digest comparison alone would be a weaker rule than `ProposalService::get`
        // applies, and readiness is the part a dispatch must not skip.
        let proposal_digest = ruling.payload().proposal.value.clone();
        let proposal =
            match verified_proposal_within(connection, self.workspace_id, &proposal_digest) {
                Ok(Some(proposal)) => proposal,
                Ok(None) | Err(_) => {
                    return Ok(decided.refused(DispatchOutcome::ProposalDigestMismatch));
                }
            };
        decided.summary = proposal.payload().intent.summary.clone();
        let unmet = live_unmet(proposal.readiness().unmet());
        if !unmet.is_empty() {
            decided.unmet = unmet;
            return Ok(decided.refused(DispatchOutcome::ProposalNotReady));
        }

        // G8-G9: the Candidate. `verified_candidate_for_proposal` proves it from bytes and
        // re-derives the copy rule; the Ruling must name that same record, and it must have existed
        // BEFORE the decision that claims to have seen it.
        let Some(named) = ruling.candidate().cloned() else {
            // A v1 Ruling. A complete decision that names no bytes, so there is nothing to apply.
            return Ok(decided.refused(DispatchOutcome::CandidateAbsent));
        };
        decided.candidate_digest = Some(named.clone());
        let candidate = match verified_candidate_for_proposal(
            connection,
            self.workspace_id,
            &proposal_digest,
        ) {
            Ok(Some(candidate)) => candidate,
            Ok(None) => return Ok(decided.refused(DispatchOutcome::CandidateAbsent)),
            Err(_) => return Ok(decided.refused(DispatchOutcome::CandidateMismatch)),
        };
        if candidate.digest() != &named {
            return Ok(decided.refused(DispatchOutcome::CandidateMismatch));
        }
        let (Ok(Some(candidate_sequence)), Ok(Some(ruling_sequence))) = (
            candidate_event_sequence(connection, self.workspace_id, &named.value),
            ruling_event_sequence(connection, self.workspace_id, &opened.ruling_digest),
        ) else {
            return Ok(decided.refused(DispatchOutcome::CandidateMismatch));
        };
        if candidate_sequence >= ruling_sequence {
            // Recorded at or after the decision. The Ruling cannot have been shown bytes that did
            // not exist when it was made, whatever its own digest says about them.
            return Ok(decided.refused(DispatchOutcome::CandidateMismatch));
        }
        decided.per_mutation = candidate
            .payload()
            .entries
            .iter()
            .map(|entry| MutationRecord {
                kind: entry.kind,
                path: entry.path.clone(),
                pre_image: entry.pre_image_sha256.as_ref().map(hex),
                post_image: entry.post_image_sha256.as_ref().map(hex),
                effect: MutationEffect::NotAttempted,
            })
            .collect();

        // The autonomy gate. ADR 0004 gives Observe, Supervised and Bounded; BLUEPRINT:405-406 says
        // Observe permits no external mutation, so Observe refuses here rather than being recorded
        // as a limitation.
        // Through the Project module's own reader: "current Charter revision" is its invariant, and
        // a gate that authorizes a write must not carry a second definition of it (gpt1).
        let Ok(Some(profile)) =
            current_autonomy_profile(connection, self.workspace_id, opened.project_id)
        else {
            return Ok(decided.refused(DispatchOutcome::AutonomyProfileRefuses));
        };
        decided.autonomy_profile = Some(profile);
        if profile != AutonomyProfile::Supervised {
            return Ok(decided.refused(DispatchOutcome::AutonomyProfileRefuses));
        }

        // The pause gate. A paused Installation refuses new dispatches (BLUEPRINT:377-379,
        // AGENTS.md:128) -- as a gate, never as a line in `not_verified`.
        if installation_is_paused(connection)? {
            return Ok(decided.refused(DispatchOutcome::InstallationPaused));
        }

        // G12: the supplied content, against what the Candidate declares -- and against the
        // Proposal's own bounds first, so a caller cannot make Beacon read more than the Proposal
        // said it would write.
        // The Proposal's own bounds, applied BEFORE its content is read for anything else: a
        // caller may not make Beacon carry more than the Proposal said it would write. Three
        // separate limits, so a refusal says which one was crossed rather than "too big".
        let bounds = proposal.payload().bounds;
        let over_file = content
            .iter()
            .filter_map(|supplied| supplied.bytes.as_ref())
            .any(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX) > bounds.file_size_bytes);
        let total: u64 = content
            .iter()
            .filter_map(|supplied| supplied.bytes.as_ref())
            .map(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .fold(0, u64::saturating_add);
        // The aggregate is the Proposal's own output ceiling: everything this dispatch writes,
        // together, is an effect of the Run and is bounded by what the Run declared.
        let over_total = total > bounds.output_bytes;
        // Deliberately no count bound here: a supplied list longer than the declaration is a
        // MISMATCH with what was authorized, not a resource ceiling, and `content_matches` below
        // refuses it as one. Each refusal means one thing.
        if over_file || over_total {
            return Ok(decided.refused(DispatchOutcome::ContentOverBounds));
        }
        if !content_matches(candidate.payload().entries.as_slice(), content) {
            return Ok(decided.refused(DispatchOutcome::ContentDigestMismatch));
        }

        // G10: the Binding, at the exact revision the Ruling copied.
        let Ok(Some(binding)) = repository_for_preflight_at(
            connection,
            self.workspace_id,
            opened.project_id,
            ruling.payload().bindings.repository_revision,
            ruling.payload().bindings.charter_revision,
        ) else {
            return Ok(decided.refused(DispatchOutcome::RulingDrifted));
        };
        decided.binding = Some(binding.clone());
        if binding.source_kind != RepositorySourceKind::LocalPath {
            return Ok(decided.refused(DispatchOutcome::SourceNotLocal));
        }

        // The provenance gate. v1 reaches only repositories Beacon made: a Candidate that
        // authorizes an effect is a Material Artifact and v1 has no non-author Evaluation of one,
        // so the blast radius is bounded by construction rather than by intent (R8).
        let Ok(repository) = PinnedRepository::open(Path::new(&binding.source)) else {
            return Ok(decided.refused(DispatchOutcome::SourceNotBeaconMade));
        };
        if !is_beacon_made(connection, self.workspace_id, &repository)? {
            return Ok(decided.refused(DispatchOutcome::SourceNotBeaconMade));
        }

        // The declared pre-images, against the pinned base tree itself. Nothing before this read
        // the repository's contents, so a Candidate claiming a file held bytes it never held --
        // including a deletion of a path that is not there -- would otherwise reach the object
        // writes and be caught only by the effect post-condition, after residue exists (gpt1).
        //
        // Checked before the head gate so it costs one read of a tree that is about to be read
        // anyway, and refused as its own reason rather than as a content mismatch: the supplied
        // content is not what disagrees.
        if !pre_images_hold(
            &repository,
            ruling.payload().bindings.pinned_base_revision.as_str(),
            &decided.per_mutation,
        ) {
            return Ok(decided.refused(DispatchOutcome::PreImageMismatch));
        }

        // G11: the live repository, read through the pinned directory with a read-only Git shape.
        // Nothing here may write: `git status` rewrites `.git/index` without GIT_OPTIONAL_LOCKS=0,
        // and a gate that mutates what it measures cannot support a zero-mutation claim.
        let Some(branch) = read_pinned(
            &repository,
            &["symbolic-ref", "--quiet", "HEAD"],
            READ_BUDGET,
        ) else {
            return Ok(decided.refused(DispatchOutcome::SourceDetached));
        };
        if branch.trim().is_empty() {
            return Ok(decided.refused(DispatchOutcome::SourceDetached));
        }
        let Some(head) = read_pinned(&repository, &["rev-parse", "--verify", "HEAD"], READ_BUDGET)
        else {
            return Ok(decided.refused(DispatchOutcome::RepositoryHeadMoved));
        };
        let head = head.trim().to_owned();
        decided.observed_head = Some(head.clone());
        if head != ruling.payload().bindings.pinned_base_revision {
            // The store's own drift check cannot see this: it compares the Ruling to the Binding
            // rows, and neither of those moves when someone commits. This is row 34's "stale" as
            // the repository knows it.
            return Ok(decided.refused(DispatchOutcome::RepositoryHeadMoved));
        }
        let Some(status) = read_pinned(
            &repository,
            &["status", "--porcelain", "--untracked-files=all"],
            READ_BUDGET,
        ) else {
            return Ok(decided.refused(DispatchOutcome::SourceDirty));
        };
        if !status.trim().is_empty() {
            return Ok(decided.refused(DispatchOutcome::SourceDirty));
        }

        // One decision, one application. Checked here so the ordinary repeat costs the repository
        // nothing; the unique index below is what closes the race, because a check and an insert
        // are two moments.
        if already_prepared(connection, self.workspace_id, &opened.ruling_digest)? {
            return Ok(decided.refused(DispatchOutcome::AlreadyDispatched));
        }

        decided.source_identity = {
            let (device, inode) = repository.identity();
            Some(SourceIdentity { device, inode })
        };
        decided.repository = Some(repository);
        decided.branch = Some(branch.trim().to_owned());
        Ok(decided.held())
    }

    /// Build the change as OBJECTS, publish it with a compare-and-swap, and update the copy.
    ///
    /// Nothing here passes through a checkout or an index the repository owns. Blobs are written
    /// with `hash-object --no-filters`, the tree is built in a scratch index outside the
    /// repository, and the commit is `commit-tree` -- so Git's built-in `text eol=lf` conversion,
    /// which a kernel review measured changing CRLF Candidate bytes into an LF blob while
    /// `diff --name-status` still reported the declared modification, has no path into the store.
    /// The declared bytes ARE the object bytes, and the post-condition is a fact about the commit.
    fn land(
        &self,
        opened: &OpenRun,
        decided: &mut Decided,
        content: &[SuppliedContent],
    ) -> Result<DispatchOutcome, DispatchError> {
        // Taken for the preparation, put back before the landing transaction: the same value
        // cannot be borrowed while `decided` is also written to, and every step here records what
        // it did.
        let repository = decided
            .repository
            .take()
            .ok_or(DispatchError::InvalidStore)?;
        let base = decided
            .bindings
            .as_ref()
            .ok_or(DispatchError::InvalidStore)?
            .pinned_base_revision
            .clone();
        let branch = decided.branch.clone().ok_or(DispatchError::InvalidStore)?;
        let scratch = ScratchIndex::new();
        let prepared = self.prepare(&repository, opened, decided, content, &base, &scratch);
        decided.repository = Some(repository);
        let commit = match prepared? {
            Ok(commit) => commit,
            Err(outcome) => return Ok(outcome),
        };

        // The claim, and the durable record of what is about to be published -- committed BEFORE
        // the swap, in its own transaction.
        //
        // The swap is an EXTERNAL effect, and a store transaction that spans it does not survive a
        // crash the way the ref does: both reviews found that a process killed after `update-ref`
        // rolls the preparation back and leaves a moved ref that nothing in the store can explain.
        // So the intent is committed first and the linearized transaction follows (Fable's ruling
        // of 2026-09-02). The cost is stated rather than hidden: a Ruling claimed here stays
        // claimed even if the landing transaction then refuses it, because a claim that is returned
        // on failure is not a claim.
        #[cfg(debug_assertions)]
        if let Some(hook) = self.failpoints.before_reserve.clone() {
            hook();
        }
        if let Some(outcome) = self.reserve(opened, decided, &branch, &base, &commit)? {
            return Ok(outcome);
        }
        #[cfg(debug_assertions)]
        if let Some(hook) = self.failpoints.before_landing.clone() {
            hook();
        }

        // Everything from here is ONE write transaction, and the Git swap happens inside it.
        //
        // A cross-lab review found the window this closes: the gates committed, then objects were
        // prepared, then the reservation opened its own transaction, then the swap ran, then the
        // Receipt opened a third. A Ruling superseded anywhere in that sequence still landed,
        // because the compare-and-swap protects the Git ref and knows nothing about authority.
        //
        // Holding the store's write lock across the swap is what serializes them: a competing
        // Ruling, Policy publication, Charter revision or Pause is a write, and cannot commit while
        // this transaction is open. So the authority re-read inside it is not a narrower window --
        // it is the last word, and nothing can change underneath it before the ref moves.
        self.land_within(opened, decided, &branch, &base, &commit)
    }

    /// Write the change as objects and prove it, without publishing anything.
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &self,
        repository: &PinnedRepository,
        opened: &OpenRun,
        decided: &mut Decided,
        content: &[SuppliedContent],
        base: &str,
        scratch: &ScratchIndex,
    ) -> Result<Result<String, DispatchOutcome>, DispatchError> {
        // W2: the base tree into a scratch index, then one entry per declared mutation. The
        // repository's own index is never opened.
        if write_pinned(
            repository,
            Some(scratch.path()),
            None,
            &["read-tree", base],
            None,
            WRITE_BUDGET,
        )
        .is_none()
        {
            return Ok(Err(DispatchOutcome::ObjectWriteFailed));
        }
        decided.git_steps.push(GitStep {
            step: GitStepName::ReadTree,
            result: GitStepResult::Succeeded,
        });
        for (entry, supplied) in decided.per_mutation.iter().zip(content) {
            let wrote = match (&entry.kind, &supplied.bytes) {
                (MutationKind::DeleteFile, _) => write_pinned(
                    repository,
                    Some(scratch.path()),
                    None,
                    &["update-index", "--force-remove", "--", &entry.path],
                    None,
                    WRITE_BUDGET,
                )
                .is_some(),
                (_, Some(bytes)) => {
                    // `--no-filters` is the whole point: the blob is the supplied bytes, whatever
                    // `.gitattributes` would have done to them on the way in.
                    let Some(blob) = write_pinned(
                        repository,
                        None,
                        None,
                        &blob_arguments(),
                        Some(bytes),
                        WRITE_BUDGET,
                    ) else {
                        return Ok(Err(DispatchOutcome::ObjectWriteFailed));
                    };
                    let cacheinfo = format!("100644,{},{}", blob.trim(), entry.path);
                    write_pinned(
                        repository,
                        Some(scratch.path()),
                        None,
                        &["update-index", "--add", "--cacheinfo", &cacheinfo],
                        None,
                        WRITE_BUDGET,
                    )
                    .is_some()
                }
                (_, None) => false,
            };
            if !wrote {
                return Ok(Err(DispatchOutcome::ObjectWriteFailed));
            }
        }

        // W3: the tree and the commit. The identity and the time are fixed by the caller, so the
        // resulting revision is a function of its inputs rather than of the clock.
        let Some(tree) = write_pinned(
            repository,
            Some(scratch.path()),
            None,
            &["write-tree"],
            None,
            WRITE_BUDGET,
        ) else {
            return Ok(Err(DispatchOutcome::ObjectWriteFailed));
        };
        let identity = GitIdentity {
            name: COMMIT_AUTHOR_NAME.to_owned(),
            email: COMMIT_AUTHOR_EMAIL.to_owned(),
            date: format!("{} +0000", opened.started_at_ms / 1_000),
        };
        let message = commit_message(decided);
        let Some(commit) = write_pinned(
            repository,
            None,
            Some(&identity),
            &["commit-tree", tree.trim(), "-p", base, "-m", &message],
            None,
            WRITE_BUDGET,
        ) else {
            return Ok(Err(DispatchOutcome::ObjectWriteFailed));
        };
        let commit = commit.trim().to_owned();
        decided.prepared_commit = Some(commit.clone());
        for step in [
            GitStepName::HashObject,
            GitStepName::UpdateIndex,
            GitStepName::WriteTree,
            GitStepName::CommitTree,
        ] {
            decided.git_steps.push(GitStep {
                step,
                result: GitStepResult::Succeeded,
            });
        }

        // W4: the prepared commit touches exactly the declared set, and every blob in it hashes to
        // what the Candidate declared. NUL-delimited, because a declared path may contain a
        // newline.
        //
        // With pre-images verified against the base tree before any object was written, a
        // declaration that disagrees with the repository is refused earlier and this check has no
        // reachable arrangement left. It stays because it is the only thing that would notice Git
        // itself producing something other than what was asked for -- and because ADR 0039 makes
        // the post-condition a fact about objects, which is a claim that needs a check behind it.
        match self.effect_matches(repository, base, &commit, decided) {
            Ok(true) => {}
            Ok(false) => return Ok(Err(DispatchOutcome::EffectDoesNotMatchDeclaration)),
            Err(error) => return Err(error),
        }
        Ok(Ok(commit))
    }

    /// Claim the Ruling and record what is about to be published, in its own committed transaction.
    ///
    /// `Some(outcome)` means the claim was already taken, which is the refusal the caller returns.
    fn reserve(
        &self,
        opened: &OpenRun,
        decided: &Decided,
        branch: &str,
        base: &str,
        commit: &str,
    ) -> Result<Option<DispatchOutcome>, DispatchError> {
        let repository = decided
            .repository
            .as_ref()
            .ok_or(DispatchError::InvalidStore)?;
        let (device, inode) = repository.identity();
        let source = repository.path().to_string_lossy().into_owned();
        self.accounts.with_authenticated_store(|connection, now| {
            let claimed = connection.execute(
                "INSERT INTO dispatch_preparations (
                     workspace_id, run_id, ruling_digest, source, source_device, source_inode,
                     branch, pinned_base_revision, prepared_commit, prepared_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    self.workspace_id.to_string(),
                    opened.run_id.to_string(),
                    opened.ruling_digest.as_str(),
                    source.as_str(),
                    i64::try_from(device).unwrap_or(i64::MAX),
                    i64::try_from(inode).unwrap_or(i64::MAX),
                    branch,
                    base,
                    commit,
                    now,
                ],
            );
            match claimed {
                Ok(_) => Ok(None),
                Err(rusqlite::Error::SqliteFailure(failure, _))
                    if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                        || failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY =>
                {
                    Ok(Some(DispatchOutcome::AlreadyDispatched))
                }
                Err(error) => Err(DispatchError::Store(error)),
            }
        })
    }

    /// The irreversible step and its record, under one write lock.
    #[allow(clippy::too_many_lines)]
    fn land_within(
        &self,
        opened: &OpenRun,
        decided: &mut Decided,
        branch: &str,
        base: &str,
        commit: &str,
    ) -> Result<DispatchOutcome, DispatchError> {
        // The pinned repository is taken out of `decided` for the duration: the closure below
        // needs it and needs to write to `decided`, and the same value cannot be both.
        let repository = decided
            .repository
            .take()
            .ok_or(DispatchError::InvalidStore)?;
        let outcome = self.accounts.with_authenticated_store(|connection, now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

            // The Proposal's own wall-clock bound, applied to the attempt that is about to become
            // irreversible: a Run may not exceed the ceiling its Proposal declared, and preparation
            // is the part that can take real time.
            let elapsed = u64::try_from(now)
                .unwrap_or(u64::MAX)
                .saturating_sub(opened.started_at_ms);
            if elapsed > opened.wall_clock_ms {
                transaction.commit()?;
                return Ok(DispatchOutcome::WallClockExceeded);
            }

            // The authority, re-read at the last moment it can be read: the same owning-module
            // proof the gates ran, plus the two live gates that are not about the record.
            if let Some(refusal) = reverify_authority(&transaction, self.workspace_id, opened)? {
                transaction.commit()?;
                return Ok(refusal);
            }

            // L1, inside the lock. `update-ref <ref> <new> <old>` fails unless the ref still holds
            // exactly the value the gates observed -- which a fast-forward merge does not check.
            let swapped = swap_ref(&repository, branch, commit, base);
            #[cfg(debug_assertions)]
            if let Some(hook) = self.failpoints.after_swap.clone() {
                // The ref has moved and nothing about it is committed. An interruption here loses
                // exactly this transaction, which is why the claim above is not in it.
                hook();
            }
            decided.git_steps.push(GitStep {
                step: GitStepName::UpdateRef,
                result: if swapped.is_some() {
                    GitStepResult::Succeeded
                } else {
                    GitStepResult::Refused
                },
            });
            let observed =
                read_pinned(&repository, &["rev-parse", "--verify", branch], READ_BUDGET);
            let outcome = match (swapped.is_some(), observed.as_deref().map(str::trim)) {
                (_, Some(head)) if head == commit => DispatchOutcome::Applied,
                (_, Some(_)) => DispatchOutcome::LandingRefused,
                (_, None) => DispatchOutcome::LandingUnobserved,
            };
            if outcome == DispatchOutcome::Applied {
                decided.observed_head = Some(commit.to_owned());
                // Beacon owns this checkout -- v1 lands nowhere else -- so leaving it showing the
                // previous revision would be Beacon knowingly leaving its own tree stale.
                //
                // `read-tree -u --reset`, NOT `reset --hard`: a kernel review probed the latter and
                // found it moves the current branch as well as the index and worktree, so a branch
                // that moved after the swap was silently pulled back. That made the checkout a
                // SECOND unguarded ref write after the one irreversible step this act is supposed
                // to have. This primitive writes the index and the working tree and leaves the ref
                // exactly where the compare-and-swap put it.
                #[cfg(debug_assertions)]
                if let Some(hook) = self.failpoints.before_checkout.clone() {
                    hook();
                }
                decided.checkout_updated = write_pinned(
                    &repository,
                    None,
                    None,
                    &["read-tree", "-u", "--reset", commit],
                    None,
                    WRITE_BUDGET,
                )
                .is_some();
                decided.git_steps.push(GitStep {
                    step: GitStepName::ReadTreeCheckout,
                    result: if decided.checkout_updated {
                        GitStepResult::Succeeded
                    } else {
                        GitStepResult::Refused
                    },
                });
                for record in &mut decided.per_mutation {
                    record.effect = match record.kind {
                        MutationKind::DeleteFile => MutationEffect::Removed,
                        _ => MutationEffect::Written,
                    };
                }
            }

            // The Receipt, the Event and the Run's terminal state -- in this same transaction, so
            // the record of what happened commits with the decision that let it happen.
            let receipt = self.build_receipt(opened, decided, &outcome, now)?;
            write_receipt(&transaction, self.workspace_id, opened.run_id, &receipt)?;
            transaction.commit()?;
            decided.receipt = Some(receipt);
            Ok(outcome)
        });
        decided.repository = Some(repository);
        outcome
    }

    /// Does `base..commit` touch exactly the declared set, and do its blobs hold the declared bytes?
    fn effect_matches(
        &self,
        repository: &PinnedRepository,
        base: &str,
        commit: &str,
        decided: &mut Decided,
    ) -> Result<bool, DispatchError> {
        let Some(diff) = read_pinned(
            repository,
            &["diff", "--name-status", "-z", base, commit],
            READ_BUDGET,
        ) else {
            return Ok(false);
        };
        // The measured diff digest: SHA-256 over what the commit actually changed, which is a
        // different fact from the Candidate's digest and is recorded as one.
        let measured = hex(&crate::proposal::sha256(diff.as_bytes()));
        // `-z` output is NUL-delimited status/path pairs, so a path containing a newline is read
        // correctly rather than splitting a record in half.
        let fields: Vec<&str> = diff.split('\0').filter(|part| !part.is_empty()).collect();
        if fields.len() != decided.per_mutation.len() * 2 {
            return Ok(false);
        }
        for (record, pair) in decided.per_mutation.iter().zip(fields.chunks(2)) {
            let expected_status = match record.kind {
                MutationKind::CreateFile => "A",
                MutationKind::ModifyFile => "M",
                MutationKind::DeleteFile => "D",
            };
            if pair[0] != expected_status || pair[1] != record.path {
                return Ok(false);
            }
        }
        decided.diff_digest = Some(measured);
        decided.git_steps.push(GitStep {
            step: GitStepName::DiffNameStatus,
            result: GitStepResult::Succeeded,
        });

        // And the bytes in the prepared commit's tree, which is what gets published -- not the
        // bytes on disk, which no longer matter and never entered the objects.
        for record in &decided.per_mutation {
            let Some(post) = &record.post_image else {
                continue;
            };
            let path = format!("{commit}:{}", record.path);
            let Some(blob) = read_pinned(repository, &["cat-file", "blob", &path], READ_BUDGET)
            else {
                return Ok(false);
            };
            if hex(&crate::proposal::sha256(blob.as_bytes())) != *post {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

// ---------------------------------------------------------------------------------------------
// Deciding and recording
// ---------------------------------------------------------------------------------------------

/// A Git index outside the repository, removed when the landing ends.
///
/// The tree is built here rather than in `.git/index`, so a dispatch that fails leaves the
/// repository's own index exactly as it found it -- and a gate read, which does not write, cannot
/// be confused with this, which does.
struct ScratchIndex {
    path: PathBuf,
}

impl ScratchIndex {
    fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!("beacon-dispatch-index-{}", Uuid::new_v4())),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The commit message, built rather than concatenated.
///
/// A Proposal summary may contain newlines and tabs (ADR 0034 permits both), so pasting it above
/// the trailers would let a summary forge a `Beacon-Ruling:` line. The subject is the summary's
/// first line with control characters removed and a bounded length; the trailers follow one blank
/// line and nothing else does.
fn commit_message(decided: &Decided) -> String {
    let subject: String = decided
        .summary
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_SUBJECT_SCALARS)
        .collect();
    let subject = subject.trim();
    let subject = if subject.is_empty() {
        "Beacon dispatched a Proposal"
    } else {
        subject
    };
    let mut message = format!("{subject}\n\n");
    if let Some(proposal) = &decided.proposal_digest {
        message.push_str(&format!("Beacon-Proposal: {}\n", proposal.value));
    }
    if let Some(ruling) = &decided.ruling_digest {
        message.push_str(&format!("Beacon-Ruling: {}\n", ruling.value));
    }
    if let Some(candidate) = &decided.candidate_digest {
        message.push_str(&format!("Beacon-Candidate: {}\n", candidate.value));
    }
    message
}

/// The subject line's ceiling, in characters.
const MAX_SUBJECT_SCALARS: usize = 120;

struct OpenRun {
    run_id: Uuid,
    project_id: Uuid,
    ruling_digest: String,
    started_at_ms: u64,
    /// The Proposal's declared wall-clock ceiling, carried from the Run's opening so the landing
    /// can hold the attempt to it.
    wall_clock_ms: u64,
}

/// What the gates established, accumulated as they run so a Receipt can name the last thing that
/// was true rather than only the refusal.
#[derive(Default)]
struct Decided {
    outcome: Option<DispatchOutcome>,
    ruling_digest: Option<RulingDigest>,
    proposal_digest: Option<ProposalDigest>,
    candidate_digest: Option<CandidateDigest>,
    bindings: Option<crate::proposal::ProposalBindings>,
    binding: Option<PreflightBinding>,
    autonomy_profile: Option<AutonomyProfile>,
    observed_head: Option<String>,
    per_mutation: Vec<MutationRecord>,
    drift: Vec<UnmetPrecondition>,
    unmet: Vec<UnmetPrecondition>,
    /// The repository the gates measured, pinned by identity for the whole attempt.
    repository: Option<PinnedRepository>,
    branch: Option<String>,
    summary: String,
    prepared_commit: Option<String>,
    checkout_updated: bool,
    git_steps: Vec<GitStep>,
    source_identity: Option<SourceIdentity>,
    /// SHA-256 over the commit's own NUL-delimited name-status output: a measured diff digest,
    /// which a Candidate digest is not.
    diff_digest: Option<String>,
    receipt: Option<DispatchReceipt>,
}

impl Decided {
    fn refused(mut self, outcome: DispatchOutcome) -> Self {
        self.outcome = Some(outcome);
        self
    }

    fn held(mut self) -> Self {
        self.outcome = Some(DispatchOutcome::Admissible);
        self
    }
}

impl DispatchService {
    /// Build the Receipt for one decided attempt. Pure: it writes nothing, so the landing
    /// transaction and the refusal path produce the same record from the same facts.
    fn build_receipt(
        &self,
        opened: &OpenRun,
        decided: &Decided,
        outcome: &DispatchOutcome,
        now: i64,
    ) -> Result<DispatchReceipt, DispatchError> {
        let ended_at_ms = u64::try_from(now)
            .map_err(|_| DispatchError::InvalidStore)?
            .max(opened.started_at_ms);
        let bindings = decided.bindings.clone();
        Ok(DispatchReceipt {
            version: 3,
            run_id: opened.run_id,
            workspace_id: self.workspace_id,
            project_id: opened.project_id,
            actor: DispatchActor::Owner,
            ruling_digest: decided
                .ruling_digest
                .clone()
                .unwrap_or_else(|| RulingDigest {
                    algorithm: "sha256".to_owned(),
                    canonicalizer: "beacon.ruling.v1".to_owned(),
                    value: opened.ruling_digest.clone(),
                }),
            proposal_digest: decided.proposal_digest.clone(),
            candidate_digest: decided.candidate_digest.clone(),
            outcome: outcome.clone(),
            autonomy_profile: decided.autonomy_profile,
            charter_revision: bindings.as_ref().map_or(0, |b| b.charter_revision),
            project_policy_revision: bindings.as_ref().map_or(0, |b| b.project_policy_revision),
            repository_revision: bindings.as_ref().map_or(0, |b| b.repository_revision),
            source_kind: decided.binding.as_ref().map(|binding| binding.source_kind),
            source: decided
                .binding
                .as_ref()
                .map(|binding| binding.source.clone()),
            source_identity: decided.source_identity,
            branch: decided.branch.clone(),
            pinned_base_revision: bindings.map(|b| b.pinned_base_revision),
            observed_head: decided.observed_head.clone(),
            prepared_commit: decided.prepared_commit.clone(),
            resulting_revision: match outcome {
                DispatchOutcome::Applied => decided.prepared_commit.clone(),
                _ => None,
            },
            diff_digest: decided.diff_digest.clone(),
            checkout_updated: decided.checkout_updated,
            git_steps: decided.git_steps.clone(),
            // No Declared Check ran and no model was invoked, so these are measured zeroes rather
            // than absences: dispatch performs neither by construction.
            model_invocations: 0,
            cost_usd_micros: 0,
            per_mutation: decided.per_mutation.clone(),
            started_at_ms: opened.started_at_ms,
            ended_at_ms,
            not_verified: not_verified(outcome, decided),
        })
    }

    /// Record a refusal that never reached the landing transaction.
    fn close(&self, opened: OpenRun, decided: Decided) -> Result<DispatchReceipt, DispatchError> {
        // A landing already wrote its own Receipt inside its transaction; returning that one is
        // what keeps a dispatch to exactly one Receipt.
        if let Some(receipt) = decided.receipt.clone() {
            return Ok(receipt);
        }
        self.accounts.with_authenticated_store(|connection, now| {
            let outcome = decided.outcome.clone().ok_or(DispatchError::InvalidStore)?;
            let receipt = self.build_receipt(&opened, &decided, &outcome, now)?;
            let transaction = connection.transaction()?;
            write_receipt(&transaction, self.workspace_id, opened.run_id, &receipt)?;
            transaction.commit()?;
            Ok(receipt)
        })
    }
}

/// The Run's terminal state, its Receipt and its audit Event, together.
///
/// One function, so a landing and a refusal cannot come to disagree about what recording an
/// attempt means -- and so the whole record is one statement in whichever transaction the caller
/// is holding.
fn write_receipt(
    connection: &Connection,
    workspace_id: Uuid,
    run_id: Uuid,
    receipt: &DispatchReceipt,
) -> Result<(), DispatchError> {
    let payload = serde_json::to_string(receipt)?;
    let ended_at_ms =
        i64::try_from(receipt.ended_at_ms).map_err(|_| DispatchError::InvalidStore)?;
    connection.execute(
        "UPDATE runs SET state = ?3, ended_at_ms = ?4 WHERE workspace_id = ?1 AND id = ?2",
        params![
            workspace_id.to_string(),
            run_id.to_string(),
            receipt.outcome.run_state(),
            ended_at_ms,
        ],
    )?;
    connection.execute(
        "INSERT INTO receipts (
             workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
         ) VALUES (?1, ?2, 3, ?3, ?4, ?5)",
        params![
            workspace_id.to_string(),
            run_id.to_string(),
            receipt.outcome.as_str(),
            payload.as_str(),
            ended_at_ms,
        ],
    )?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'proposal.dispatched', ?2, ?3)",
        params![workspace_id.to_string(), payload.as_str(), ended_at_ms],
    )?;
    Ok(())
}

/// Everything a dispatch in this build did not establish.
///
/// The seven undeclared constraint families are here on every Receipt, because ADR 0034 says they
/// must resolve before dispatch is conceivable and ADR 0039 proceeds anyway under stated
/// conditions -- so no record of a dispatch can be read as having checked them. The rest names what
/// this build has not built.
fn not_verified(outcome: &DispatchOutcome, decided: &Decided) -> Vec<NotVerified> {
    let mut names = vec![
        NotVerified::ExecutionPolicyConstraint,
        NotVerified::SandboxConstraint,
        NotVerified::NetworkConstraint,
        NotVerified::ModelProviderConstraint,
        NotVerified::CommandAllowlistConstraint,
        NotVerified::ToolAllowlistConstraint,
        NotVerified::EvaluationsConstraint,
        NotVerified::NonAuthorEvaluationOfCandidate,
        NotVerified::SafetySnapshot,
        NotVerified::BaseRevisionReachability,
        NotVerified::DeclaredChecksAgainstResult,
    ];
    if !decided.drift.is_empty() {
        names.push(NotVerified::RulingBindingsMoved);
    }
    if !decided.unmet.is_empty() {
        names.push(NotVerified::ProposalPreconditions);
    }
    match outcome {
        DispatchOutcome::Applied => {}
        DispatchOutcome::LandingUnobserved => names.push(NotVerified::LandingResultUnknown),
        _ => names.push(NotVerified::ChangeNotApplied),
    }
    if decided.prepared_commit.is_some() && *outcome != DispatchOutcome::Applied {
        names.push(NotVerified::UnreferencedObjectResidue);
    }
    names
}

/// The unmet preconditions a dispatch is entitled to refuse for.
///
/// Nine of the sixteen. The other seven are the constraint families ADR 0034 leaves undeclarable in
/// v1: every Proposal carries all seven, permanently, so requiring their absence would refuse every
/// Proposal that exists. They are named in every Receipt's `not_verified` instead.
fn live_unmet(unmet: &[UnmetPrecondition]) -> Vec<UnmetPrecondition> {
    unmet
        .iter()
        .filter(|precondition| {
            !matches!(
                precondition,
                UnmetPrecondition::ExecutionPolicyNotDeclared
                    | UnmetPrecondition::SandboxNotDeclared
                    | UnmetPrecondition::NetworkNotDeclared
                    | UnmetPrecondition::ModelProviderNotDeclared
                    | UnmetPrecondition::CommandAllowlistNotDeclared
                    | UnmetPrecondition::ToolAllowlistNotDeclared
                    | UnmetPrecondition::EvaluationsNotDeclared
            )
        })
        .cloned()
        .collect()
}

/// Does the supplied content hash to exactly what the Candidate declares, one entry at a time and
/// in the Candidate's own order?
///
/// A creation or a modification must be supplied and must hash to the post-image; a deletion must be
/// supplied with no content, because "the file is gone" and "nobody said" are different statements.
/// Extra supplied entries are a mismatch: dispatch writes what was declared and nothing beside it.
fn content_matches(
    entries: &[crate::candidate::CandidateEntry],
    content: &[SuppliedContent],
) -> bool {
    if entries.len() != content.len() {
        return false;
    }
    entries.iter().zip(content).all(|(entry, supplied)| {
        entry.path == supplied.path
            && match (&entry.post_image_sha256, &supplied.bytes) {
                (Some(expected), Some(bytes)) => &crate::proposal::sha256(bytes) == expected,
                (None, None) => true,
                _ => false,
            }
    })
}

fn hex(value: &[u8; 32]) -> String {
    data_encoding::HEXLOWER.encode(value)
}

/// How a Candidate's bytes become a blob.
///
/// **The protection is the absence of `--path`.** Git looks up `.gitattributes` by pathname, so a
/// blob hashed from standard input with no path has nothing to match and cannot be converted --
/// which is why `git hash-object` documents `--no-filters` as implied for stdin unless `--path` is
/// given, and refuses the two flags together outright. `--no-filters` stays as the explicit
/// statement of intent; adding `--path` here would silently restore the conversion a kernel review
/// measured turning CRLF Candidate bytes into an LF blob, and `the_blob_invocation_names_no_path`
/// is what makes that regression fail rather than pass quietly.
fn blob_arguments() -> [&'static str; 6] {
    ["hash-object", "-w", "-t", "blob", "--no-filters", "--stdin"]
}

/// The authority, re-read at the last moment before the irreversible step.
///
/// Returns the refusal if anything that permitted this dispatch has stopped permitting it. Called
/// inside the landing transaction, so nothing it reads can change before the ref moves: a competing
/// Ruling, Policy publication, Charter revision or Pause is a write, and the write lock is held.
fn reverify_authority(
    connection: &Connection,
    workspace_id: Uuid,
    opened: &OpenRun,
) -> Result<Option<DispatchOutcome>, DispatchError> {
    let ruling = match verified_ruling_within(connection, workspace_id, &opened.ruling_digest) {
        Ok(Some(ruling)) => ruling,
        Ok(None) | Err(_) => return Ok(Some(DispatchOutcome::RulingUnreadable)),
    };
    if ruling.payload().decision != RulingDecision::Approve {
        return Ok(Some(DispatchOutcome::RulingRefused));
    }
    if ruling.force().superseded_by().is_some() {
        return Ok(Some(DispatchOutcome::RulingSuperseded));
    }
    if !ruling.force().drift().is_empty() {
        return Ok(Some(DispatchOutcome::RulingDrifted));
    }
    // The Proposal's live readiness, through its owning module, exactly as G6-G7 read it. A kernel
    // review pointed out that leaving it out made "the authority, re-read" an overstatement: a
    // precondition that becomes unmet in the window -- a Resident deactivated, a Configuration
    // changed -- is as much a reason to refuse as a supersession.
    match verified_proposal_within(connection, workspace_id, &ruling.payload().proposal.value) {
        Ok(Some(proposal)) => {
            if !live_unmet(proposal.readiness().unmet()).is_empty() {
                return Ok(Some(DispatchOutcome::ProposalNotReady));
            }
        }
        Ok(None) | Err(_) => return Ok(Some(DispatchOutcome::ProposalDigestMismatch)),
    }
    match current_autonomy_profile(connection, workspace_id, opened.project_id) {
        Ok(Some(AutonomyProfile::Supervised)) => {}
        _ => return Ok(Some(DispatchOutcome::AutonomyProfileRefuses)),
    }
    if installation_is_paused(connection)? {
        return Ok(Some(DispatchOutcome::InstallationPaused));
    }
    Ok(None)
}

/// Move `branch` from exactly `expected` to `commit`, or fail.
///
/// `update-ref <ref> <new> <old>` is a compare-and-swap: Git refuses unless the ref currently holds
/// `old`. That is the property a fast-forward merge does NOT have -- a branch moved back to an
/// ancestor still fast-forwards, and two dispatchers that both observed the base both succeed -- so
/// the old value is not optional here, and
/// `a_swap_refuses_unless_the_ref_still_holds_the_expected_value` is what keeps it from being
/// dropped.
fn swap_ref(
    repository: &PinnedRepository,
    branch: &str,
    commit: &str,
    expected: &str,
) -> Option<String> {
    write_pinned(
        repository,
        None,
        None,
        &["update-ref", branch, commit, expected],
        None,
        WRITE_BUDGET,
    )
}

/// Does every declared pre-image match what the pinned base revision actually holds?
///
/// A create must find nothing at its path; a modify or a delete must find a blob whose bytes hash
/// to the declared pre-image. Read from the base TREE, not from the working tree: the tree is what
/// the change will be applied to.
fn pre_images_hold(
    repository: &PinnedRepository,
    base: &str,
    per_mutation: &[MutationRecord],
) -> bool {
    per_mutation.iter().all(|record| {
        let path = format!("{base}:{}", record.path);
        let blob = read_pinned(repository, &["cat-file", "blob", &path], READ_BUDGET);
        match (&record.pre_image, blob) {
            // Declared absent, and absent: `cat-file` fails for a path the tree does not hold.
            (None, None) => true,
            (None, Some(_)) => false,
            (Some(_), None) => false,
            (Some(declared), Some(bytes)) => {
                hex(&crate::proposal::sha256(bytes.as_bytes())) == *declared
            }
        }
    })
}

/// Did Beacon make this repository?
///
/// Read from the provenance record Beacon writes when it creates a copy, and matched on **device
/// and inode**, not on the pathname it was created at: a path is what a rename moves, and a
/// containment check against a caller-chosen directory proves only where a repository sits, never
/// where it came from (gpt1). v1 dispatch reaches nothing Beacon did not make.
fn is_beacon_made(
    connection: &Connection,
    workspace_id: Uuid,
    repository: &PinnedRepository,
) -> Result<bool, DispatchError> {
    let (device, inode) = repository.identity();
    let found: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM beacon_repositories
             WHERE workspace_id = ?1 AND device = ?2 AND inode = ?3",
            params![
                workspace_id.to_string(),
                i64::try_from(device).unwrap_or(i64::MAX),
                i64::try_from(inode).unwrap_or(i64::MAX),
            ],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Record that Beacon created this repository, so a dispatch may reach it.
pub(crate) fn record_beacon_repository(
    connection: &Connection,
    workspace_id: Uuid,
    path: &Path,
    now: i64,
) -> Result<(), DispatchError> {
    let repository = PinnedRepository::open(path).map_err(|_| DispatchError::InvalidStore)?;
    let (device, inode) = repository.identity();
    connection.execute(
        "INSERT INTO beacon_repositories (
             workspace_id, path, device, inode, origin, created_at_ms
         ) VALUES (?1, ?2, ?3, ?4, 'sample_project_copy', ?5)",
        params![
            workspace_id.to_string(),
            repository.path().to_string_lossy(),
            i64::try_from(device).unwrap_or(i64::MAX),
            i64::try_from(inode).unwrap_or(i64::MAX),
            now,
        ],
    )?;
    Ok(())
}

/// Has this Ruling already been prepared for dispatch?
fn already_prepared(
    connection: &Connection,
    workspace_id: Uuid,
    ruling_digest: &str,
) -> Result<bool, DispatchError> {
    let existing: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM dispatch_preparations WHERE workspace_id = ?1 AND ruling_digest = ?2",
            params![workspace_id.to_string(), ruling_digest],
            |row| row.get(0),
        )
        .optional()?;
    Ok(existing.is_some())
}

/// Is the Installation paused?
pub(crate) fn installation_is_paused(connection: &Connection) -> Result<bool, DispatchError> {
    let paused: i64 = connection.query_row(
        "SELECT paused FROM installation_pause WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(paused == 1)
}

// ---------------------------------------------------------------------------------------------
// Startup reconciliation
// ---------------------------------------------------------------------------------------------

/// Finish every dispatch attempt that never wrote its own Receipt.
///
/// A cross-lab review found the hole this closes: the preparation record was written durably before
/// the swap and then never read, so a process that died between the swap and the Receipt could
/// leave a branch permanently moved with Beacon recording nothing terminal. `AGENTS.md:154` admits
/// no exception, and a Run that changed a repository and cannot say so is the worst shape a Receipt
/// can be missing from.
///
/// This is the one place startup reads a repository, and the distinction matters: it does not
/// re-judge a finished Receipt -- later history is ordinary work, not corruption -- it resolves an
/// attempt that is still open, by asking the ref what became of the commit the preparation named.
///
/// Runs the reconciliation cannot resolve stay open with the truth recorded: an unreadable
/// repository, or one whose identity no longer matches what was pinned, yields `LandingUnobserved`
/// naming the prepared commit, never a guess in either direction.
pub(crate) fn finish_prepared_dispatches(connection: &Connection) -> Result<(), DispatchError> {
    let mut statement = connection.prepare(
        "SELECT preparations.workspace_id, preparations.run_id, preparations.source,
                preparations.source_device, preparations.source_inode, preparations.branch,
                preparations.pinned_base_revision, preparations.prepared_commit,
                runs.project_id, runs.started_at_ms, runs.charter_revision,
                runs.repository_revision
         FROM dispatch_preparations AS preparations
         JOIN runs ON runs.workspace_id = preparations.workspace_id
                  AND runs.id = preparations.run_id
         WHERE runs.kind = 'proposal_dispatch'
           AND NOT EXISTS (
               SELECT 1 FROM receipts
               WHERE receipts.workspace_id = preparations.workspace_id
                 AND receipts.run_id = preparations.run_id
           )
         ORDER BY preparations.prepared_at_ms, preparations.run_id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(PreparedDispatch {
                workspace_id: row.get::<_, String>(0)?,
                run_id: row.get::<_, String>(1)?,
                source: row.get::<_, String>(2)?,
                device: row.get::<_, i64>(3)?,
                inode: row.get::<_, i64>(4)?,
                branch: row.get::<_, String>(5)?,
                pinned_base_revision: row.get::<_, String>(6)?,
                prepared_commit: row.get::<_, String>(7)?,
                project_id: row.get::<_, String>(8)?,
                started_at_ms: row.get::<_, i64>(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    // The other crash boundary, which has no preparation to read: a process that died between
    // `commit-tree` and the landing transaction left objects in the store whose identity was never
    // persisted. Nothing can name them, and saying so is the only honest terminal record -- a
    // guess in either direction would be worse than the uncertainty.
    let mut statement = connection.prepare(
        "SELECT workspace_id, id, project_id, started_at_ms FROM runs
         WHERE kind = 'proposal_dispatch' AND state = 'dispatching'
           AND NOT EXISTS (
               SELECT 1 FROM receipts
               WHERE receipts.workspace_id = runs.workspace_id AND receipts.run_id = runs.id
           )
           AND NOT EXISTS (
               SELECT 1 FROM dispatch_preparations AS preparations
               WHERE preparations.workspace_id = runs.workspace_id
                 AND preparations.run_id = runs.id
           )
         ORDER BY started_at_ms, id",
    )?;
    let unprepared = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (workspace_id, run_id, project_id, started_at_ms) in unprepared {
        let workspace_id =
            Uuid::parse_str(&workspace_id).map_err(|_| DispatchError::InvalidStore)?;
        let run_id = Uuid::parse_str(&run_id).map_err(|_| DispatchError::InvalidStore)?;
        let project_id = Uuid::parse_str(&project_id).map_err(|_| DispatchError::InvalidStore)?;
        let started_at_ms =
            u64::try_from(started_at_ms).map_err(|_| DispatchError::InvalidStore)?;
        let receipt = interrupted_receipt(
            run_id,
            workspace_id,
            project_id,
            started_at_ms,
            DispatchOutcome::LandingUnobserved,
            None,
            true,
        );
        write_receipt(connection, workspace_id, run_id, &receipt)?;
    }

    for prepared in rows {
        let workspace_id =
            Uuid::parse_str(&prepared.workspace_id).map_err(|_| DispatchError::InvalidStore)?;
        let run_id = Uuid::parse_str(&prepared.run_id).map_err(|_| DispatchError::InvalidStore)?;
        let project_id =
            Uuid::parse_str(&prepared.project_id).map_err(|_| DispatchError::InvalidStore)?;
        let outcome = observe_landing(&prepared);
        let started_at_ms =
            u64::try_from(prepared.started_at_ms).map_err(|_| DispatchError::InvalidStore)?;
        let mut receipt = interrupted_receipt(
            run_id,
            workspace_id,
            project_id,
            started_at_ms,
            outcome.clone(),
            Some(&prepared),
            false,
        );
        receipt.ruling_digest.value = ruling_of_preparation(connection, &prepared)?;
        write_receipt(connection, workspace_id, run_id, &receipt)?;
    }
    Ok(())
}

/// A terminal Receipt reconstructed at startup from what survived an interruption.
///
/// It establishes strictly less than a Receipt the attempt would have written for itself, and says
/// so in `not_verified` rather than looking like one.
fn interrupted_receipt(
    run_id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    started_at_ms: u64,
    outcome: DispatchOutcome,
    prepared: Option<&PreparedDispatch>,
    residue_unnamed: bool,
) -> DispatchReceipt {
    let mut not_verified = vec![
        NotVerified::ExecutionPolicyConstraint,
        NotVerified::SandboxConstraint,
        NotVerified::NetworkConstraint,
        NotVerified::ModelProviderConstraint,
        NotVerified::CommandAllowlistConstraint,
        NotVerified::ToolAllowlistConstraint,
        NotVerified::EvaluationsConstraint,
        NotVerified::NonAuthorEvaluationOfCandidate,
        NotVerified::SafetySnapshot,
        NotVerified::BaseRevisionReachability,
        NotVerified::DeclaredChecksAgainstResult,
    ];
    // The same outcome tokens a live Receipt carries, in the same order: a reconstructed refusal
    // that did not say the change was not applied would be the one refusal in the store a reader
    // could not check.
    match outcome {
        DispatchOutcome::Applied => {}
        DispatchOutcome::LandingUnobserved => {
            not_verified.push(NotVerified::LandingResultUnknown);
        }
        _ => not_verified.push(NotVerified::ChangeNotApplied),
    }
    if outcome != DispatchOutcome::Applied && prepared.is_some() {
        not_verified.push(NotVerified::UnreferencedObjectResidue);
    }
    not_verified.push(NotVerified::ReconstructedAfterInterruption);
    if residue_unnamed {
        // The interruption landed between writing objects and persisting what they were, so the
        // residue exists and nothing can name it.
        not_verified.push(NotVerified::ResidueCannotBeNamed);
    }
    DispatchReceipt {
        version: 3,
        run_id,
        workspace_id,
        project_id,
        actor: DispatchActor::Owner,
        ruling_digest: RulingDigest {
            algorithm: "sha256".to_owned(),
            canonicalizer: "beacon.ruling.v2".to_owned(),
            value: "0".repeat(64),
        },
        proposal_digest: None,
        candidate_digest: None,
        outcome: outcome.clone(),
        autonomy_profile: None,
        charter_revision: 0,
        project_policy_revision: 0,
        repository_revision: 0,
        source_kind: prepared.map(|_| RepositorySourceKind::LocalPath),
        source: prepared.map(|prepared| prepared.source.clone()),
        source_identity: prepared.map(|prepared| SourceIdentity {
            device: u64::try_from(prepared.device).unwrap_or(u64::MAX),
            inode: u64::try_from(prepared.inode).unwrap_or(u64::MAX),
        }),
        branch: prepared.map(|prepared| prepared.branch.clone()),
        pinned_base_revision: prepared.map(|prepared| prepared.pinned_base_revision.clone()),
        observed_head: None,
        prepared_commit: prepared.map(|prepared| prepared.prepared_commit.clone()),
        resulting_revision: match (&outcome, prepared) {
            (DispatchOutcome::Applied, Some(prepared)) => Some(prepared.prepared_commit.clone()),
            _ => None,
        },
        diff_digest: None,
        checkout_updated: false,
        git_steps: Vec::new(),
        model_invocations: 0,
        cost_usd_micros: 0,
        per_mutation: Vec::new(),
        started_at_ms,
        ended_at_ms: started_at_ms,
        not_verified,
    }
}

struct PreparedDispatch {
    workspace_id: String,
    run_id: String,
    source: String,
    device: i64,
    inode: i64,
    branch: String,
    pinned_base_revision: String,
    prepared_commit: String,
    project_id: String,
    started_at_ms: i64,
}

/// What became of a prepared commit, asked of the repository itself.
fn observe_landing(prepared: &PreparedDispatch) -> DispatchOutcome {
    let Ok(repository) = PinnedRepository::open(Path::new(&prepared.source)) else {
        return DispatchOutcome::LandingUnobserved;
    };
    let (device, inode) = repository.identity();
    // The identity the attempt pinned, or nothing: a directory that now has a different identity is
    // a different repository, and reading its ref would answer a question about the wrong tree.
    if i64::try_from(device).unwrap_or(i64::MAX) != prepared.device
        || i64::try_from(inode).unwrap_or(i64::MAX) != prepared.inode
    {
        return DispatchOutcome::LandingUnobserved;
    }
    match read_pinned(
        &repository,
        &["rev-parse", "--verify", &prepared.branch],
        READ_BUDGET,
    ) {
        Some(head) if head.trim() == prepared.prepared_commit => DispatchOutcome::Applied,
        Some(head) if head.trim() == prepared.pinned_base_revision => {
            DispatchOutcome::LandingRefused
        }
        // The branch holds something else entirely: the attempt may have landed and been moved
        // afterwards, or never landed. That is exactly what "unobserved" means.
        Some(_) | None => DispatchOutcome::LandingUnobserved,
    }
}

fn ruling_of_preparation(
    connection: &Connection,
    prepared: &PreparedDispatch,
) -> Result<String, DispatchError> {
    connection
        .query_row(
            "SELECT ruling_digest FROM dispatch_preparations
             WHERE workspace_id = ?1 AND run_id = ?2",
            params![prepared.workspace_id, prepared.run_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(DispatchError::Store)
}

/// Replay one `proposal.dispatched` Event against its stored Receipt.
///
/// The same shape as the preflight replay: the Event payload and the stored `receipt_json` must be
/// byte-identical AND equal as parsed values, the Run must exist with the state this outcome maps
/// to, and the Receipt's own times must be consistent. Deliberately re-derives nothing about the
/// repository -- a Receipt records what was true then, and later history is not corruption.
pub(crate) fn verify_dispatch_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), DispatchError> {
    let receipt: DispatchReceipt = serde_json::from_str(payload_json)?;
    if receipt.version != 3
        || receipt.workspace_id != workspace_id
        || receipt.started_at_ms > receipt.ended_at_ms
        || receipt.ended_at_ms != occurred_at_ms
        || receipt.actor != DispatchActor::Owner
    {
        return Err(DispatchError::InvalidStore);
    }
    let stored: Option<(i64, String, String, String, String)> = connection
        .query_row(
            "SELECT receipts.version, receipts.outcome, receipts.receipt_json, runs.kind, runs.state
             FROM receipts
             JOIN runs ON runs.workspace_id = receipts.workspace_id
                      AND runs.id = receipts.run_id
             WHERE receipts.workspace_id = ?1 AND receipts.run_id = ?2",
            params![workspace_id.to_string(), receipt.run_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((version, outcome, receipt_json, kind, state)) = stored else {
        return Err(DispatchError::InvalidStore);
    };
    let stored_receipt: DispatchReceipt = serde_json::from_str(&receipt_json)?;
    if version != 3
        || kind != "proposal_dispatch"
        || outcome != receipt.outcome.as_str()
        || state != receipt.outcome.run_state()
        || receipt_json.as_bytes() != payload_json.as_bytes()
        || stored_receipt != receipt
    {
        return Err(DispatchError::InvalidStore);
    }
    verify_outcome_invariants(&receipt)?;
    verify_against_preparation(connection, workspace_id, &receipt)
}

/// The invariants one outcome arm owes its own record.
///
/// Byte equality between a Receipt and its Event proves the two copies agree; it does not prove
/// either says something a dispatch could have produced. An `Applied` naming no commit, or a
/// refusal naming a resulting revision, is incoherent whatever its bytes (gpt2).
fn verify_outcome_invariants(receipt: &DispatchReceipt) -> Result<(), DispatchError> {
    let names = |token: NotVerified| receipt.not_verified.contains(&token);
    let reconstructed = names(NotVerified::ReconstructedAfterInterruption);
    let coherent = match receipt.outcome {
        DispatchOutcome::Applied => {
            receipt.prepared_commit.is_some()
                && receipt.resulting_revision == receipt.prepared_commit
                && receipt.branch.is_some()
                && receipt.source_identity.is_some()
                && !names(NotVerified::ChangeNotApplied)
                // A reconstructed Applied read the ref rather than measuring a landing, so it
                // carries neither the diff it did not take nor the profile it did not read.
                && (reconstructed
                    || (receipt.diff_digest.is_some()
                        && receipt.observed_head.is_some()
                        && receipt.autonomy_profile == Some(AutonomyProfile::Supervised)))
        }
        DispatchOutcome::LandingUnobserved => {
            receipt.resulting_revision.is_none()
                && receipt.diff_digest.is_none()
                && !receipt.checkout_updated
                && names(NotVerified::LandingResultUnknown)
        }
        // Every other arm refused. Objects it wrote may survive -- that is what the residue token
        // is for -- but nothing it prepared may be presented as a landing.
        _ => {
            receipt.resulting_revision.is_none()
                && receipt.diff_digest.is_none()
                && !receipt.checkout_updated
                && names(NotVerified::ChangeNotApplied)
                && (receipt.prepared_commit.is_none()
                    || names(NotVerified::UnreferencedObjectResidue))
        }
    };
    if coherent {
        Ok(())
    } else {
        Err(DispatchError::InvalidStore)
    }
}

/// Check a Receipt against the durable preparation its own landing transaction wrote.
///
/// The preparation is the independent record: it is inserted inside the transaction that performs
/// the swap, before the swap, and is never rewritten. So the arms that claim the swap ran must have
/// one, and it must name the same commit, branch, base and repository identity. This re-derives
/// nothing about the repository -- later history is ordinary work, and `resulting_revision` is a
/// landing-at-time fact rather than a claim about the ref now.
fn verify_against_preparation(
    connection: &Connection,
    workspace_id: Uuid,
    receipt: &DispatchReceipt,
) -> Result<(), DispatchError> {
    let claims_a_swap = matches!(
        receipt.outcome,
        DispatchOutcome::Applied | DispatchOutcome::LandingRefused
    ) || (receipt.outcome == DispatchOutcome::LandingUnobserved
        && receipt.prepared_commit.is_some());
    let prepared: Option<(String, String, String, i64, i64)> = connection
        .query_row(
            "SELECT prepared_commit, branch, pinned_base_revision, source_device, source_inode
             FROM dispatch_preparations
             WHERE workspace_id = ?1 AND run_id = ?2",
            params![workspace_id.to_string(), receipt.run_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((commit, branch, base, device, inode)) = prepared else {
        return if claims_a_swap {
            Err(DispatchError::InvalidStore)
        } else {
            Ok(())
        };
    };
    let identity = SourceIdentity {
        device: u64::try_from(device).unwrap_or(u64::MAX),
        inode: u64::try_from(inode).unwrap_or(u64::MAX),
    };
    if receipt.prepared_commit.as_deref() != Some(commit.as_str())
        || receipt.branch.as_deref() != Some(branch.as_str())
        || receipt.pinned_base_revision.as_deref() != Some(base.as_str())
        || receipt.source_identity != Some(identity)
    {
        return Err(DispatchError::InvalidStore);
    }
    Ok(())
}

/// Dispatch Run, Receipt and Event cardinality is NOT checked here.
///
/// `check_preflight::verify_run_state` owns it for every Run kind, and its own comment says why: one
/// reconciliation owning kind, state, Receipt and terminal-event cardinality is what stops a
/// relabelled Run from falling between two validators and being checked by neither. Dispatch adds
/// its arms there rather than a second walk here.
const _: () = ();

// ---------------------------------------------------------------------------------------------
// Pause Work
// ---------------------------------------------------------------------------------------------

/// The Operator's Installation-wide Pause, as much of it as dispatch needs to be gated by.
///
/// `BLUEPRINT.md:377-379` and `AGENTS.md:128` make a paused Installation refuse new dispatches.
/// Recording "pause not verified" in a Receipt would not make a forbidden dispatch permissible, so
/// the gate is real and this is the state it reads.
///
/// **Pause and Resume are separate Commands, not one boolean.** They are not symmetric:
/// `AGENTS.md:128-137` makes Resume conditional on every stopping or unconfirmed Run having been
/// reviewed and either resolved or explicitly accepted, and a `set_paused(false)` made that
/// forbidden transition representable in one call with no Event (gpt1's P0). Each records its own
/// audit Event in the same transaction as the state change.
///
/// **What this is not.** Pause in the Blueprint also sends bounded interruption requests to active
/// Runs, refuses Worker lease renewals, and is reachable from the tray and a global shortcut. None
/// of that is built, and none of it is claimed here.
pub struct PauseService {
    accounts: Arc<OperatorAccountService>,
}

/// Why a Resume was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedRuns {
    /// The Runs that have not reached a terminal state, or reached `unconfirmed`.
    pub run_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PauseChangedPayload {
    paused: bool,
    /// Present only on a Resume that accepted uncertainty, carrying the Operator's stated reason
    /// and the Runs it covered -- so "explicitly accepts its uncertainty" is a record rather than a
    /// silence.
    accepted_unresolved: Option<AcceptedUnresolved>,
    changed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptedUnresolved {
    reason: String,
    run_ids: Vec<Uuid>,
}

impl PauseService {
    pub fn new(accounts: Arc<OperatorAccountService>) -> Self {
        Self { accounts }
    }

    pub fn is_paused(&self) -> Result<bool, DispatchError> {
        self.accounts
            .with_authenticated_store(|connection, _now| installation_is_paused(connection))
    }

    /// Pause the Installation. Always permitted: stopping is the safe direction.
    pub fn pause(&self) -> Result<(), DispatchError> {
        self.accounts
            .with_authenticated_store(|connection, now| self.set(connection, now, true, None))
    }

    /// Resume, if every Run has been resolved.
    ///
    /// Refuses while any Run is still open or `unconfirmed`, and names them: an Operator who has
    /// not looked at an uncertain Run has not resolved it, and Resume is where that is enforced.
    pub fn resume(&self) -> Result<(), DispatchError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let unresolved = unresolved_runs(connection)?;
            if !unresolved.is_empty() {
                return Err(DispatchError::UnresolvedRuns(UnresolvedRuns {
                    run_ids: unresolved,
                }));
            }
            self.set(connection, now, false, None)
        })
    }

    /// Resume, accepting the uncertainty of Runs that could not be resolved.
    ///
    /// The other half of `AGENTS.md:137`: an Operator may proceed, and what they accepted is
    /// recorded with their reason rather than inferred from the fact that they proceeded.
    pub fn resume_accepting_unresolved(&self, reason: &str) -> Result<(), DispatchError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let run_ids = unresolved_runs(connection)?;
            self.set(
                connection,
                now,
                false,
                Some(AcceptedUnresolved {
                    reason: reason.to_owned(),
                    run_ids,
                }),
            )
        })
    }

    /// The state change and its Event, in one transaction.
    fn set(
        &self,
        connection: &Connection,
        now: i64,
        paused: bool,
        accepted: Option<AcceptedUnresolved>,
    ) -> Result<(), DispatchError> {
        let changed_at_ms = u64::try_from(now).map_err(|_| DispatchError::InvalidStore)?;
        let payload = serde_json::to_string(&PauseChangedPayload {
            paused,
            accepted_unresolved: accepted,
            changed_at_ms,
        })?;
        let workspace: String = connection.query_row(
            "SELECT id FROM workspaces ORDER BY created_at_ms, id LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE installation_pause SET paused = ?1, changed_at_ms = ?2 WHERE singleton = 1",
            params![i64::from(paused), now],
        )?;
        transaction.execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                workspace,
                if paused {
                    "installation.paused"
                } else {
                    "installation.resumed"
                },
                payload,
                now
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

/// Replay one Pause or Resume Event: it must decode, and its recorded state and time must agree
/// with the kind it was stored under and when it happened.
///
/// The store holds one current flag rather than a history, so this proves the Event is a
/// well-formed record of a transition -- not that the transition is still in effect, which is a
/// different question with a different answer as soon as the next one happens.
pub(crate) fn verify_pause_event(
    paused: bool,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), DispatchError> {
    let payload: PauseChangedPayload = serde_json::from_str(payload_json)?;
    if payload.paused != paused || payload.changed_at_ms != occurred_at_ms {
        return Err(DispatchError::InvalidStore);
    }
    // Only a Resume may carry an acceptance, because only a Resume has uncertainty to accept.
    if payload.paused && payload.accepted_unresolved.is_some() {
        return Err(DispatchError::InvalidStore);
    }
    Ok(())
}

/// Every Run that has not reached a terminal state, or reached one that says its effects are
/// unknown.
fn unresolved_runs(connection: &Connection) -> Result<Vec<Uuid>, DispatchError> {
    let mut statement = connection.prepare(
        "SELECT id FROM runs
         WHERE state IN ('preflighting', 'dispatching', 'unconfirmed')
         ORDER BY started_at_ms, id",
    )?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    ids.into_iter()
        .map(|id| Uuid::parse_str(&id).map_err(|_| DispatchError::InvalidStore))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(path: &Path, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Beacon Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Beacon Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .expect("git is available in the test environment");
        assert!(output.status.success(), "git {arguments:?} failed");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn the_blob_invocation_names_no_path() {
        // The whole of the attribute-conversion protection, asserted where it can regress. Git
        // matches `.gitattributes` by pathname, so a stdin blob with no path cannot be converted;
        // adding `--path` would restore the conversion AND make Git refuse `--no-filters`, so this
        // is the shape that must not drift.
        let arguments = blob_arguments();
        assert!(arguments.contains(&"--stdin"));
        assert!(arguments.contains(&"--no-filters"));
        assert!(
            !arguments.contains(&"--path"),
            "naming a path would let .gitattributes rewrite Candidate bytes"
        );
    }

    #[test]
    fn the_checkout_step_updates_the_tree_without_moving_the_ref() {
        // A kernel review probed `reset --hard` after a successful swap and found it moves the
        // current branch too: a branch that moved after the compare-and-swap was pulled back, which
        // made the checkout a SECOND unguarded ref write. This pins the primitive that does not.
        let root = std::env::temp_dir().join(format!("beacon-checkout-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "--quiet", "-m", "one"]);
        let first = git(&root, &["rev-parse", "HEAD"]);
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        git(&root, &["commit", "--quiet", "-am", "two"]);
        let second = git(&root, &["rev-parse", "HEAD"]);
        git(&root, &["update-ref", "refs/heads/main", &first]);

        let repository = PinnedRepository::open(&root).unwrap();
        assert!(
            write_pinned(
                &repository,
                None,
                None,
                &["read-tree", "-u", "--reset", &second],
                None,
                WRITE_BUDGET,
            )
            .is_some()
        );

        // The tree holds the later revision's content...
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "two\n"
        );
        // ...and the branch is exactly where it was left.
        assert_eq!(git(&root, &["rev-parse", "HEAD"]), first);
        assert_ne!(first, second);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_swap_refuses_unless_the_ref_still_holds_the_expected_value() {
        // The landing primitive's whole contract, and the one gpt1's probes disproved for
        // `merge --ff-only`: moving the branch BACK to an ancestor still fast-forwards, and two
        // dispatchers that both observed the base both succeed. A compare-and-swap does neither.
        let root = std::env::temp_dir().join(format!("beacon-swap-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "--quiet", "-m", "one"]);
        let first = git(&root, &["rev-parse", "HEAD"]);
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        git(&root, &["commit", "--quiet", "-am", "two"]);
        let second = git(&root, &["rev-parse", "HEAD"]);
        std::fs::write(root.join("a.txt"), "three\n").unwrap();
        git(&root, &["commit", "--quiet", "-am", "three"]);
        let third = git(&root, &["rev-parse", "HEAD"]);

        let repository = PinnedRepository::open(&root).unwrap();
        let branch = "refs/heads/main";

        // The ref holds `third`. A swap expecting `first` -- the ancestor case that a fast-forward
        // merge accepts -- must refuse, and must leave the ref exactly where it was.
        assert!(
            swap_ref(&repository, branch, &second, &first).is_none(),
            "a swap expecting a stale value succeeded"
        );
        assert_eq!(git(&root, &["rev-parse", "HEAD"]), third);

        // And the positive control: expecting what the ref actually holds moves it.
        assert!(swap_ref(&repository, branch, &second, &third).is_some());
        assert_eq!(git(&root, &["rev-parse", branch]), second);

        std::fs::remove_dir_all(&root).unwrap();
    }
}
