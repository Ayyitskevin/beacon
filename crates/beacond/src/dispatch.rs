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
use crate::project_repository::{PreflightBinding, repository_for_preflight_at};
use crate::proposal::{MutationKind, ProposalDigest, UnmetPrecondition, verified_proposal_within};
use crate::ruling::{RulingDecision, RulingDigest, ruling_event_sequence, verified_ruling_within};
use crate::worktree::{PinnedRepository, READ_BUDGET, read_pinned};
use crate::{AutonomyProfile, CandidateDigest, RepositorySourceKind};

/// A dispatch Run's lease. It performs no execution and no landing in this build, so it is short.
const LEASE_MS: u64 = 60_000;

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
    /// Every gate held. Nothing was applied, because this build has no landing path.
    ///
    /// The same shape as `PreflightOutcome::Verified`: it reports that the act would be admissible,
    /// never that it happened. When landing ships, this outcome stops being emitted and stays in
    /// the vocabulary so recorded Receipts remain decodable.
    Admissible,
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
            Self::Admissible => "admissible",
        }
    }

    /// The Run state this outcome terminates in. One owner for the mapping, as
    /// `PreflightOutcome::run_state` and `ExecutionSummary::run_state` are.
    ///
    /// Everything this build can produce is a refusal in Run terms: `Admissible` says the act would
    /// have been permitted and that nothing was done, which is not `applied`.
    pub(crate) fn run_state(&self) -> &'static str {
        "refused"
    }

    /// Did every gate hold?
    fn admissible(&self) -> bool {
        matches!(self, Self::Admissible)
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
    /// Verified against the supplied content and not written: this build has no landing path.
    NotAttempted,
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
    /// One row per declared mutation, in the Proposal's order. Empty when the refusal came before
    /// the Candidate could be read.
    pub per_mutation: Vec<MutationRecord>,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    /// Everything this dispatch did not establish. A Receipt that cannot say what it failed to
    /// check is not evidence (ADR 0027).
    pub not_verified: Vec<String>,
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
    /// **Applies nothing.** The act this build gates does not exist yet, so the best outcome
    /// reachable is `Admissible`: every gate held, and the repository is untouched either way.
    pub fn dispatch(
        &self,
        ruling_digest: &str,
        content: &[SuppliedContent],
    ) -> Result<DispatchReceipt, DispatchError> {
        let opened = self.open_run(ruling_digest)?;
        let decided = self.decide(&opened, content)?;
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
            let Some(ruling) =
                verified_ruling_within(&transaction, self.workspace_id, ruling_digest)
                    .map_err(|_| DispatchError::InvalidStore)?
            else {
                return Err(DispatchError::RulingNotFound);
            };
            let bindings = ruling.payload().bindings.clone();
            let Some(proposal) = verified_proposal_within(
                &transaction,
                self.workspace_id,
                &ruling.payload().proposal.value,
            )
            .map_err(|_| DispatchError::InvalidStore)?
            else {
                return Err(DispatchError::InvalidStore);
            };
            let project_id = proposal.payload().scope.project_id;
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
        let Some(ruling) =
            verified_ruling_within(connection, self.workspace_id, &opened.ruling_digest)
                .map_err(|_| DispatchError::InvalidStore)?
        else {
            return Ok(decided.refused(DispatchOutcome::RulingUnreadable));
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
        let Some(proposal) =
            verified_proposal_within(connection, self.workspace_id, &proposal_digest)
                .map_err(|_| DispatchError::InvalidStore)?
        else {
            return Ok(decided.refused(DispatchOutcome::ProposalDigestMismatch));
        };
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
        let Some(candidate) =
            verified_candidate_for_proposal(connection, self.workspace_id, &proposal_digest)
                .map_err(|_| DispatchError::InvalidStore)?
        else {
            return Ok(decided.refused(DispatchOutcome::CandidateAbsent));
        };
        if candidate.digest() != &named {
            return Ok(decided.refused(DispatchOutcome::CandidateMismatch));
        }
        let candidate_sequence =
            candidate_event_sequence(connection, self.workspace_id, &named.value)
                .map_err(|_| DispatchError::InvalidStore)?
                .ok_or(DispatchError::InvalidStore)?;
        let ruling_sequence =
            ruling_event_sequence(connection, self.workspace_id, &opened.ruling_digest)
                .map_err(|_| DispatchError::InvalidStore)?
                .ok_or(DispatchError::InvalidStore)?;
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
        let profile = current_autonomy_profile(connection, self.workspace_id, opened.project_id)?
            .ok_or(DispatchError::InvalidStore)?;
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
        let file_size_bytes = proposal.payload().bounds.file_size_bytes;
        if content
            .iter()
            .filter_map(|supplied| supplied.bytes.as_ref())
            .any(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX) > file_size_bytes)
        {
            return Ok(decided.refused(DispatchOutcome::ContentOverBounds));
        }
        if !content_matches(candidate.payload().entries.as_slice(), content) {
            return Ok(decided.refused(DispatchOutcome::ContentDigestMismatch));
        }

        // G10: the Binding, at the exact revision the Ruling copied.
        let Some(binding) = repository_for_preflight_at(
            connection,
            self.workspace_id,
            opened.project_id,
            ruling.payload().bindings.repository_revision,
            ruling.payload().bindings.charter_revision,
        )
        .map_err(|_| DispatchError::InvalidStore)?
        else {
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

        Ok(decided.held())
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
fn not_verified(outcome: &DispatchOutcome, decided: &Decided) -> Vec<String> {
    let mut names = vec![
        "execution policy constraint family (not declarable in Proposal v1)".to_owned(),
        "sandbox constraint family (not declarable in Proposal v1)".to_owned(),
        "network constraint family (not declarable in Proposal v1)".to_owned(),
        "model provider constraint family (not declarable in Proposal v1)".to_owned(),
        "command allowlist constraint family (not declarable in Proposal v1)".to_owned(),
        "tool allowlist constraint family (not declarable in Proposal v1)".to_owned(),
        "evaluations constraint family (not declarable in Proposal v1)".to_owned(),
        "non-author Evaluation of the Candidate (no Evaluation citation in Ruling v2)".to_owned(),
        "Safety Snapshot before a high-impact dispatch (no snapshot mechanism exists)".to_owned(),
        "base revision reachability from a ref (object presence verified only)".to_owned(),
        "the change was not applied: this build has no landing path".to_owned(),
    ];
    if !decided.drift.is_empty() {
        names.push(format!(
            "the Ruling's copied revisions have moved: {:?}",
            decided.drift
        ));
    }
    if !decided.unmet.is_empty() {
        names.push(format!(
            "live Proposal preconditions do not hold: {:?}",
            decided.unmet
        ));
    }
    if !outcome.admissible() {
        names.push(format!(
            "every gate after {} was not reached",
            outcome.as_str()
        ));
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

/// The Project's autonomy profile at its current Charter revision.
fn current_autonomy_profile(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<AutonomyProfile>, DispatchError> {
    let value: Option<String> = connection
        .query_row(
            "SELECT revisions.autonomy_profile
             FROM project_charter_revisions AS revisions
             JOIN projects ON projects.workspace_id = revisions.workspace_id
                          AND projects.id = revisions.project_id
                          AND projects.current_charter_revision = revisions.revision
             WHERE revisions.workspace_id = ?1 AND revisions.project_id = ?2",
            params![workspace_id.to_string(), project_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(match value.as_deref() {
        Some("observe") => Some(AutonomyProfile::Observe),
        Some("supervised") => Some(AutonomyProfile::Supervised),
        Some(_) => return Err(DispatchError::InvalidStore),
        None => None,
    })
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
/// **What this is not.** Pause in the Blueprint also sends bounded interruption requests to active
/// Runs, refuses Worker lease renewals, and is reachable from the tray and a global shortcut. None
/// of that is built, and none of it is claimed here: this is one Owner-set flag and the one gate
/// that consumes it.
pub struct PauseService {
    accounts: Arc<OperatorAccountService>,
}

impl PauseService {
    pub fn new(accounts: Arc<OperatorAccountService>) -> Self {
        Self { accounts }
    }

    pub fn is_paused(&self) -> Result<bool, DispatchError> {
        self.accounts
            .with_authenticated_store(|connection, _now| installation_is_paused(connection))
    }

    /// Pause or resume the Installation.
    pub fn set_paused(&self, paused: bool) -> Result<(), DispatchError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let changed_at_ms = now;
            connection.execute(
                "UPDATE installation_pause SET paused = ?1, changed_at_ms = ?2 WHERE singleton = 1",
                params![i64::from(paused), changed_at_ms],
            )?;
            Ok(())
        })
    }
}
