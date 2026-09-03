//! Dispatch-on-match: the gates (ADR 0039).
//!
//! Dispatch is the only act in Beacon that may mutate a Git repository on the strength of a record.
//! **This build has no landing path**: every gate below runs, and the act that would apply a change
//! does not exist here. That ordering is deliberate — BLUEPRINT `:907` says "a denied or stale
//! Proposal causes zero repository mutation", and a negative criterion is proved most honestly by a
//! version that cannot mutate anything at all, before the version that can.
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
/// category. The three families never collapse: a gate said no, the act would have been admissible,
/// or the act's result could not be established — and only the first two are reachable here.
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
    /// What this build did with it: nothing, because there is no landing path. Recorded per
    /// mutation rather than once, so a later build that writes some and not others has a place to
    /// say so.
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
    /// The base revision the Ruling pinned, and the head observed live. They are different facts
    /// and are recorded separately.
    pub pinned_base_revision: Option<String>,
    pub observed_head: Option<String>,
    /// The commit this dispatch prepared, present from the moment it exists -- including when the
    /// landing's result could not be established, which is the case it exists for.
    pub prepared_commit: Option<String>,
    /// The revision the branch names afterwards, when that was verified.
    pub resulting_revision: Option<String>,
    /// Whether Beacon updated the copy's working tree after the swap. A separately observable step,
    /// recorded separately.
    pub checkout_updated: bool,
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
/// Constructed with the directory Beacon's own repository copies live under, because v1 dispatch
/// reaches nothing else: the provenance gate is a containment check against that root, not a flag a
/// row could carry.
pub struct DispatchService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
    copies_root: PathBuf,
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
    pub fn new(
        accounts: Arc<OperatorAccountService>,
        workspace_id: Uuid,
        copies_root: impl AsRef<Path>,
    ) -> Self {
        Self {
            accounts,
            workspace_id,
            copies_root: copies_root.as_ref().to_owned(),
        }
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
        if !self.is_beacon_made_copy(repository.path()) {
            return Ok(decided.refused(DispatchOutcome::SourceNotBeaconMade));
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
        let repository = decided
            .repository
            .as_ref()
            .ok_or(DispatchError::InvalidStore)?;
        let base = decided
            .bindings
            .as_ref()
            .ok_or(DispatchError::InvalidStore)?
            .pinned_base_revision
            .clone();
        let branch = decided.branch.clone().ok_or(DispatchError::InvalidStore)?;
        let scratch = ScratchIndex::new();

        // W2: the base tree into a scratch index, then one entry per declared mutation. The
        // repository's own index is never opened.
        if write_pinned(
            repository,
            Some(scratch.path()),
            None,
            &["read-tree", &base],
            None,
            WRITE_BUDGET,
        )
        .is_none()
        {
            return Ok(DispatchOutcome::ObjectWriteFailed);
        }
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
                        return Ok(DispatchOutcome::ObjectWriteFailed);
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
                return Ok(DispatchOutcome::ObjectWriteFailed);
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
            return Ok(DispatchOutcome::ObjectWriteFailed);
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
            &["commit-tree", tree.trim(), "-p", &base, "-m", &message],
            None,
            WRITE_BUDGET,
        ) else {
            return Ok(DispatchOutcome::ObjectWriteFailed);
        };
        let commit = commit.trim().to_owned();
        decided.prepared_commit = Some(commit.clone());

        // W4: the prepared commit touches exactly the declared set, and every blob in it hashes to
        // what the Candidate declared. NUL-delimited, because a declared path may contain a
        // newline.
        match self.effect_matches(repository, &base, &commit, decided) {
            Ok(true) => {}
            Ok(false) => return Ok(DispatchOutcome::EffectDoesNotMatchDeclaration),
            Err(error) => return Err(error),
        }

        // The reservation, before the irreversible step: one preparation per Ruling, ever, and a
        // durable record of the exact commit an unobserved attempt was about to publish. A
        // uniqueness violation here is the race the gate above cannot close, and it is a refusal
        // rather than an error -- the Run still ends with a Receipt (AGENTS.md:154).
        match self.reserve(opened, decided, &branch, &base, &commit) {
            Ok(()) => {}
            Err(DispatchError::Store(rusqlite::Error::SqliteFailure(failure, _)))
                if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                    || failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY =>
            {
                return Ok(DispatchOutcome::AlreadyDispatched);
            }
            Err(error) => return Err(error),
        }

        // L1: the compare-and-swap. `update-ref <ref> <new> <old>` fails unless the ref still holds
        // exactly the value the gates observed -- which a fast-forward merge does not check: a
        // branch moved BACK to an ancestor still fast-forwards, and two dispatchers that both
        // observed the base both succeed (gpt1's probes, R9).
        let landed = swap_ref(repository, &branch, &commit, &base);
        if landed.is_none() {
            // Refused or unestablished. Which one is answerable by reading the ref, and the answer
            // is a fact rather than an assumption either way.
            return Ok(
                match read_pinned(repository, &["rev-parse", "--verify", &branch], READ_BUDGET) {
                    Some(head) if head.trim() == commit => DispatchOutcome::Applied,
                    Some(head) if head.trim() == base => DispatchOutcome::LandingRefused,
                    Some(_) => DispatchOutcome::LandingRefused,
                    None => DispatchOutcome::LandingUnobserved,
                },
            );
        }
        let Some(head) = read_pinned(repository, &["rev-parse", "--verify", &branch], READ_BUDGET)
        else {
            return Ok(DispatchOutcome::LandingUnobserved);
        };
        if head.trim() != commit {
            return Ok(DispatchOutcome::LandingUnobserved);
        }
        decided.observed_head = Some(commit.clone());

        // The copy's working tree, updated after the swap. Beacon owns this checkout -- v1 lands
        // nowhere else -- so leaving it showing the previous revision would be Beacon knowingly
        // leaving its own tree stale. It is a separately observable step and is not described as
        // atomic with the ref.
        decided.checkout_updated = write_pinned(
            repository,
            None,
            None,
            &["reset", "--hard", "--quiet", &commit],
            None,
            WRITE_BUDGET,
        )
        .is_some();

        for record in &mut decided.per_mutation {
            record.effect = match record.kind {
                MutationKind::DeleteFile => MutationEffect::Removed,
                _ => MutationEffect::Written,
            };
        }
        Ok(DispatchOutcome::Applied)
    }

    /// Does `base..commit` touch exactly the declared set, and do its blobs hold the declared bytes?
    fn effect_matches(
        &self,
        repository: &PinnedRepository,
        base: &str,
        commit: &str,
        decided: &Decided,
    ) -> Result<bool, DispatchError> {
        let Some(diff) = read_pinned(
            repository,
            &["diff", "--name-status", "-z", base, commit],
            READ_BUDGET,
        ) else {
            return Ok(false);
        };
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

    /// Spend the authority and persist the prepared commit, before anything irreversible.
    fn reserve(
        &self,
        opened: &OpenRun,
        decided: &Decided,
        branch: &str,
        base: &str,
        commit: &str,
    ) -> Result<(), DispatchError> {
        let repository = decided
            .repository
            .as_ref()
            .ok_or(DispatchError::InvalidStore)?;
        let (device, inode) = repository.identity();
        self.accounts.with_authenticated_store(|connection, now| {
            connection.execute(
                "INSERT INTO dispatch_preparations (
                     workspace_id, run_id, ruling_digest, source, source_device, source_inode,
                     branch, pinned_base_revision, prepared_commit, prepared_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    self.workspace_id.to_string(),
                    opened.run_id.to_string(),
                    opened.ruling_digest.as_str(),
                    repository.path().to_string_lossy(),
                    i64::try_from(device).unwrap_or(i64::MAX),
                    i64::try_from(inode).unwrap_or(i64::MAX),
                    branch,
                    base,
                    commit,
                    now,
                ],
            )?;
            Ok(())
        })
    }

    /// Is this directory one of Beacon's own repository copies?
    ///
    /// Containment under the root Beacon creates copies in, compared after canonicalization, so a
    /// symlink pointing into the root does not qualify a repository outside it. No stored flag: a
    /// row saying "Beacon made this" would be a claim, and this is a fact about where the directory
    /// is.
    fn is_beacon_made_copy(&self, repository: &Path) -> bool {
        let Ok(root) = std::fs::canonicalize(&self.copies_root) else {
            return false;
        };
        repository.starts_with(&root) && repository != root
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
    fn close(&self, opened: OpenRun, decided: Decided) -> Result<DispatchReceipt, DispatchError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let ended_at_ms = u64::try_from(now)
                .map_err(|_| DispatchError::InvalidStore)?
                .max(opened.started_at_ms);
            let outcome = decided.outcome.clone().ok_or(DispatchError::InvalidStore)?;
            let bindings = decided.bindings.clone();
            let receipt = DispatchReceipt {
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
                pinned_base_revision: bindings.map(|b| b.pinned_base_revision),
                observed_head: decided.observed_head.clone(),
                prepared_commit: decided.prepared_commit.clone(),
                resulting_revision: match outcome {
                    DispatchOutcome::Applied => decided.prepared_commit.clone(),
                    _ => None,
                },
                checkout_updated: decided.checkout_updated,
                per_mutation: decided.per_mutation.clone(),
                started_at_ms: opened.started_at_ms,
                ended_at_ms,
                not_verified: not_verified(&outcome, &decided),
            };
            let payload = serde_json::to_string(&receipt)?;
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE runs SET state = ?3, ended_at_ms = ?4 WHERE workspace_id = ?1 AND id = ?2",
                params![
                    self.workspace_id.to_string(),
                    opened.run_id.to_string(),
                    outcome.run_state(),
                    i64::try_from(ended_at_ms).map_err(|_| DispatchError::InvalidStore)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO receipts (
                     workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
                 ) VALUES (?1, ?2, 3, ?3, ?4, ?5)",
                params![
                    self.workspace_id.to_string(),
                    opened.run_id.to_string(),
                    outcome.as_str(),
                    payload.as_str(),
                    i64::try_from(ended_at_ms).map_err(|_| DispatchError::InvalidStore)?,
                ],
            )?;
            transaction.execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'proposal.dispatched', ?2, ?3)",
                params![
                    self.workspace_id.to_string(),
                    payload.as_str(),
                    i64::try_from(ended_at_ms).map_err(|_| DispatchError::InvalidStore)?,
                ],
            )?;
            transaction.commit()?;
            Ok(receipt)
        })
    }
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
