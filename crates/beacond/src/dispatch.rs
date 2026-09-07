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
    FilterInventoryFailure, GitIdentity, PinnedRead, PinnedReadFailure, PinnedRepository,
    READ_BUDGET, WRITE_BUDGET, read_pinned, read_pinned_bytes, read_pinned_exit, write_pinned,
};
use crate::{AutonomyProfile, CandidateDigest, RepositorySourceKind};

/// A dispatch Run's lease.
const LEASE_MS: u64 = 60_000;

/// The Receipt shape this build writes. Version 4 is the R46 era: a mismatch Receipt carries its
/// observed pre-image as a closed value. Version 3 Receipts predate the field and are read as
/// "unrecorded" -- accepted, never repaired, never invented (R46.7.1).
const RECEIPT_VERSION: u32 = 4;

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
    /// tree that is not there. COMPARED AND DIFFERED -- bytes were read, or the tree was read and
    /// reported no such object. Never a read that failed (R46).
    PreImageMismatch,
    /// A declared pre-image could not be READ, so no comparison happened. The Receipt names which
    /// read failed; it makes no claim about what the base revision holds.
    ///
    /// R46. Before it, this refused as `PreImageMismatch` -- a Receipt asserting a comparison that
    /// never took place, which is the absent/broken collapse the write side already refuses
    /// (R28.1: "a failed write, never a mismatch, because nothing here was compared").
    PreImageUnreadable,
    /// The filter-driver inventory could not be established, so no Git child could be run safely.
    ///
    /// The inventory is read from the repository's own configuration through the pinned
    /// descriptor, and it decides which filter drivers the hardened command must neutralize. A
    /// configuration Beacon cannot read leaves that undecided, and running anyway with an empty
    /// inventory is not a missing diagnostic -- it is the hardening switched off, in a repository
    /// the product has just failed to read (R47).
    ///
    /// R50 gives it its own outcome. R47 could not: it carried the fact as a `PreImageUnreadable`
    /// subcase, which named the pre-image read as the one that failed when the read that failed
    /// was a precondition of every gate. R48 removed the receipts CHECK, so the correct name now
    /// costs no migration.
    FilterInventoryUnreadable,
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
            Self::PreImageUnreadable => "pre_image_unreadable",
            Self::FilterInventoryUnreadable => "filter_inventory_unreadable",
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

    /// The fixed line an in-process caller is shown for this outcome (R42, audience 2).
    ///
    /// R42's three audiences, for a dispatch outcome, are not three surfaces here — measured,
    /// not assumed:
    ///
    /// * **Startup / the owning reader** is [`Self::as_str`]: the stored token IS the row key, and
    ///   R48's owners refuse a token they cannot decode.
    /// * **The in-process API** is this. `DispatchReceipt` is returned in process to the one named
    ///   Operator surface, and this is the wording that surface may show.
    /// * **The wire and the desktop do not exist for this type, deliberately.** ADR 0039 admits
    ///   dispatch from one named Operator surface only, `LocalCommand` has no arm that reaches it
    ///   (`no_transport_command_can_reach_dispatch_or_pause`), and `RecentReceiptOutcome` carries
    ///   two Receipt kinds, neither of them dispatch. The witness for that audience is therefore a
    ///   witness that it stays absent, not a mapping.
    ///
    /// Selected by the closed outcome NAME and never interpolated. Nothing a repository, a
    /// Candidate or git supplied reaches this line: git's own message for an unreadable inventory
    /// stays in `filter_inventory_failure`, where it is bounded evidence a reader can inspect
    /// rather than a sentence assembled from someone else's text (R28.4, R42).
    pub fn caller_line(&self) -> &'static str {
        match self {
            Self::RulingUnreadable => "the Ruling record does not agree with itself",
            Self::RulingRefused => "the Ruling refused this Proposal",
            Self::RulingSuperseded => "a later Ruling reverses this one",
            Self::RulingDrifted => "a revision the Ruling recorded has moved",
            Self::ProposalDigestMismatch => {
                "the stored Proposal does not hash to the digest the Ruling names"
            }
            Self::ProposalNotReady => "a live precondition of the Proposal does not hold",
            Self::CandidateAbsent => "no Candidate authorizes any bytes for this Proposal",
            Self::CandidateMismatch => {
                "the Candidate the Ruling names is not the Candidate the Proposal has"
            }
            Self::ContentDigestMismatch => {
                "the supplied content does not hash to what the Candidate declares"
            }
            Self::ContentOverBounds => "the supplied content exceeds a bound the Proposal declared",
            Self::SourceNotLocal => "the Repository Binding does not name a local path",
            Self::SourceNotBeaconMade => {
                "the bound repository is not a disposable copy Beacon made"
            }
            Self::RepositoryHeadMoved => {
                "the repository's head is not the base revision this Ruling pinned"
            }
            Self::SourceDetached => {
                "the repository's head is not on a branch, so there is no ref to move"
            }
            Self::SourceDirty => "the repository holds uncommitted or untracked content",
            Self::AutonomyProfileRefuses => {
                "the Project's autonomy profile does not admit dispatch"
            }
            Self::InstallationPaused => "the Installation is paused",
            Self::PreImageMismatch => "a declared pre-image is not what the base revision holds",
            Self::PreImageUnreadable => {
                "a declared pre-image could not be read, so nothing was compared"
            }
            // R50. The line says what could not be read and what that prevented, because an
            // Operator's next step is to look at the repository's own configuration -- and it
            // names neither the file nor what git said about it.
            Self::FilterInventoryUnreadable => {
                "the repository's filter configuration could not be read, so no Git command could be run safely"
            }
            Self::WallClockExceeded => "the attempt exceeded the time bound the Proposal declared",
            Self::AlreadyDispatched => "this Ruling has already been dispatched",
            Self::Admissible => "every gate held and the act was not attempted",
            Self::ObjectWriteFailed => "an object, tree or commit could not be written",
            Self::EffectDoesNotMatchDeclaration => {
                "the prepared commit does not match what the Candidate declares"
            }
            Self::LandingRefused => {
                "the branch no longer held the pinned base at the moment of the swap"
            }
            Self::LandingUnobserved => "the result of the swap could not be established",
            Self::Applied => "the branch names the prepared commit",
        }
    }

    /// The Run state this outcome terminates in. One owner for the mapping, as
    /// `PreflightOutcome::run_state` and `ExecutionSummary::run_state` are.
    ///
    /// `Applied` is the one state that says a change reached a repository. `Admissible` does not:
    /// it says the act would have been permitted and that nothing was done.
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
    /// What the pinned base revision ACTUALLY held at this path, as lowercase hex, in the same
    /// encoding as `pre_image`.
    ///
    /// R46.5 (grok's HOLD): a `PreImageMismatch` Receipt carried only the declared digests, so it
    /// asserted a comparison and recorded nothing that was read -- the same shape of claim R46
    /// exists to remove one level up. This is the evidence that a comparison happened at all.
    ///
    /// A CLOSED VALUE, never a missing field, on a version-4 mismatch (R46.7): a path that turns
    /// out absent is an OBSERVATION, and writing nothing for it made an honest refusal
    /// indistinguishable from one that recorded no evidence -- which item 6's owner rule then
    /// refused, bricking a store at its next start (blue's HOLD).
    ///
    /// Absent from the Receipt only where no observation belongs: an outcome that is not a
    /// mismatch, or a version-3 Receipt written before this lane, which the owner reads as
    /// "unrecorded" rather than repairing or inventing one (R46.7.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_pre_image: Option<ObservedPreImage>,
    /// What this dispatch did with it. Recorded per mutation rather than once, so an attempt that
    /// wrote some and not others has a place to say so.
    pub effect: MutationEffect,
}

/// What the pinned base revision held at a declared path, as the pre-image gate observed it.
///
/// Two arms because there are two observations, and "no bytes" is not the same fact as "no object"
/// -- the distinction this whole lane exists to keep (R46.7). A read that FAILED is neither: it
/// produces `PreImageUnreadable` and no observation at all.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedPreImage {
    /// The tree held bytes, and they hash to this, in the same encoding as `pre_image`.
    Bytes { digest: String },
    /// The tree held no such object. Git ran, under the hardening, and said so.
    Absent,
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
    /// WHICH pre-image read failed, present only with `PreImageUnreadable` (R46).
    ///
    /// The outcome says a comparison did not happen; this says why not, from the closed subcase
    /// vocabulary rather than a message. Defaulted so Receipts recorded before R46 stay decodable:
    /// none of them can carry the outcome this field belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_image_read_failure: Option<PinnedReadFailure>,
    /// WHY the filter-driver inventory could not be read, present only with
    /// `FilterInventoryUnreadable` (R50).
    ///
    /// Defaulted for the same reason its R46 neighbour is, and the reason is not "so old records
    /// still parse" in general -- it is that no Receipt recorded before this build can carry the
    /// outcome this field belongs to, so its absence in one is not missing evidence. The Receipt
    /// version does not move: version 4 is the R46 era's marker for the observed pre-image, and
    /// this field says nothing about a pre-image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_inventory_failure: Option<FilterInventoryFailure>,
}

/// What a Receipt can say about the repository it names, afterwards.
///
/// Four arms because there are four states, and the fourth is why this is not a boolean: a Receipt
/// reconstructed from an interruption that never reached its claim knows neither what was written
/// nor whether anything was. A boolean collapsed that into "byte-identical", so a record that
/// explicitly says the object residue is unknown reported proven byte identity at the same time
/// (gpt2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryEffect {
    /// Nothing was written: the refusal came before anything reached the object database.
    ByteIdentical,
    /// Objects exist that no ref names. The refs and working tree are unchanged.
    UnreferencedObjects,
    /// A commit was published: the branch names it.
    Published,
    /// The record cannot say. Nothing here is a claim about the repository.
    Unknown,
}

impl DispatchReceipt {
    /// What this Receipt can say about the repository afterwards.
    ///
    /// Read off the evidence rather than inferred from the outcome: objects are written exactly
    /// when a commit was prepared, so a Receipt with no prepared commit and nothing unknown is one
    /// that refused before anything reached the object database.
    pub fn repository_effect(&self) -> RepositoryEffect {
        // `ObjectResidueUnknown` is its own fact rather than an outcome: the interruption fell
        // before the claim, so whether anything was written is unrecorded even for outcomes that
        // would otherwise be definite.
        if self
            .not_verified
            .contains(&NotVerified::ObjectResidueUnknown)
        {
            return RepositoryEffect::Unknown;
        }
        // The repository could not say what became of the prepared commit. Every record that holds
        // this state names this token -- the coherence check requires it of a `LandingUnobserved`
        // Receipt (`receipt_is_coherent`), and records written before the write-side exclusion
        // above named it too, so this one arm covers old and new alike. An arm keyed on the outcome
        // as well was written and removed: it and this one masked each other, each surviving its
        // own mutant because the other answered first, and a check that cannot fail is a sentence
        // rather than a guard (C37).
        if self
            .not_verified
            .contains(&NotVerified::LandingResultUnknown)
        {
            return RepositoryEffect::Unknown;
        }
        match (&self.outcome, &self.prepared_commit) {
            (DispatchOutcome::Applied, _) => RepositoryEffect::Published,
            (_, None) => RepositoryEffect::ByteIdentical,
            // A commit was prepared and this attempt did not publish it. Whether its objects are
            // reachable is what the residue TOKEN says, and the token is derived from the branch
            // this attempt observed. Deriving it here from the shape of the record instead was the
            // same defect one level up: a cold review had a competing writer publish this exact
            // commit, so a ref demonstrably named the objects while this accessor said none did.
            (_, Some(_)) if self.names_unreferenced_residue() => {
                RepositoryEffect::UnreferencedObjects
            }
            // Objects were written and a ref was observed to name them -- by someone else's act,
            // not this one's. None of the four arms says that: it is not byte-identical, this
            // attempt published nothing, and the objects are not unreferenced. `Unknown` is the
            // arm that makes no false statement, and naming the state properly is a vocabulary
            // change this ADR does not make on its own.
            (_, Some(_)) => RepositoryEffect::Unknown,
        }
    }

    fn names_unreferenced_residue(&self) -> bool {
        self.not_verified
            .contains(&NotVerified::UnreferencedObjectResidue)
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
    /// The interruption fell before the claim, so **whether** this attempt left unreferenced
    /// objects is unknown, and if it did, nothing in the store can name them.
    ///
    /// Not "residue exists and cannot be named": the same durable state -- an open Run with no
    /// claim -- is left by a death after the Run opened and before a single object was written,
    /// and asserting residue there would be claiming a fact nobody measured (gpt2).
    ObjectResidueUnknown,
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitStep {
    pub step: GitStepName,
    /// The exact argument vector, as passed. Fixed text this build chooses plus the revisions and
    /// declared paths of the request -- never a caller's free text, and never a stderr message,
    /// which would make one implementation's Receipt undiffable against another's.
    pub argv: Vec<String>,
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
    /// `symbolic-ref --quiet HEAD`: the live head gate's first read.
    SymbolicRef,
    /// `rev-parse --verify`: the head the gates measured, and the head observed after the swap.
    RevParse,
    /// `status --porcelain --untracked-files=all`: the clean-tree gate.
    Status,
    /// `config --get`: the provenance gate reading the token the repository carries.
    Config,
    /// `cat-file blob`: the pre-image gate reading the base tree, and the effect post-condition
    /// reading the prepared commit's own blobs.
    CatFile,
    /// `update-ref <ref> <new> <old>`: the compare-and-swap.
    UpdateRef,
    /// `read-tree -u --reset` on Beacon's own copy, after the swap: index and working tree only,
    /// never the ref.
    ReadTreeCheckout,
    /// `merge-base --is-ancestor <prepared> <observed>`: the reachability read that earns the
    /// residue token. Runs only where the observed tip is not the prepared commit, so it never
    /// appears in an ordinary `Applied` Receipt's step list.
    MergeBase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitStepResult {
    Succeeded,
    /// The invocation did not succeed.
    ///
    /// Two arms and no more, deliberately and with the cost stated: the bounded runner reports a
    /// non-zero exit, a timeout, a failure to spawn and a repository that stopped being the pinned
    /// one as one absent answer, so a Receipt that named a cause here would be naming one it was
    /// never told (gpt1 asked for finer categories; giving them names without evidence would be
    /// worse than this). For the swap it means the ref no longer held the expected value.
    DidNotSucceed,
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
/// Two reviews rejected the recovery evidence twice, for two different reasons. The first version
/// deleted committed records to arrange a state, which proves what reconciliation does with a state
/// nobody showed was reachable. The second panicked in-process and caught the unwind, which runs
/// every destructor a kill does not: the scratch index removed, the open transaction dropped in an
/// orderly way, and the same process then opening the store it had just "crashed" with.
///
/// So the hooks carry no policy of their own. The tests run the act in a CHILD process and abort it
/// here -- no unwinding, no destructors, the database lock released the way a kill releases it --
/// and the parent recovers a store it never had open.
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
    /// Immediately before each pre-image byte read, so a test can break THAT read and no other
    /// (R46). Breaking the repository from outside trips an earlier gate -- an unreadable
    /// `.git/config` refuses as `SourceNotBeaconMade` long before the pre-image gate runs -- so
    /// the fault has to be constructed at the read it is about.
    pub before_pre_image_read: Option<Arc<dyn Fn() + Send + Sync>>,
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

    /// Fire the pre-image read failpoint, when this build has failpoints at all.
    ///
    /// `DispatchFailpoints` is `debug_assertions`-only, so the gate cannot take one as a
    /// parameter: a release build has no such type, and `cargo build` said so. The gate takes a
    /// `&dyn Fn()` and this decides what that is.
    #[cfg(debug_assertions)]
    fn before_pre_image_read(&self) {
        if let Some(failpoint) = &self.failpoints.before_pre_image_read {
            failpoint();
        }
    }

    #[cfg(not(debug_assertions))]
    #[allow(clippy::unused_self)]
    fn before_pre_image_read(&self) {}

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

        // G2-G7: the Ruling proved from its own bytes with its force derived now, what it
        // decided, whether it still applies, and the Proposal through its owning module's full
        // contract including readiness -- all through `verified_authority`, which the landing
        // transaction's re-verification also calls. A Ruling that cannot be proved is a REFUSAL,
        // not an error: the Run is already open, and an error path would leave it nonterminal with
        // no Receipt, which AGENTS.md:154 forbids (gpt1's P0).
        //
        // The digests are recorded before the refusal is returned, because a Receipt names the
        // last thing that was true rather than only the refusal.
        let facts = match verified_authority(connection, self.workspace_id, &opened.ruling_digest) {
            Ok(facts) => facts,
            Err(refused) => {
                if let Ok(Some(ruling)) =
                    verified_ruling_within(connection, self.workspace_id, &opened.ruling_digest)
                {
                    decided.ruling_digest = Some(ruling.digest().clone());
                    decided.proposal_digest = Some(ruling.payload().proposal.clone());
                    decided.bindings = Some(ruling.payload().bindings.clone());
                }
                decided.drift = refused.drift;
                decided.unmet = refused.unmet;
                return Ok(decided.refused(refused.outcome));
            }
        };
        let (ruling, proposal) = (facts.ruling, facts.proposal);
        decided.ruling_digest = Some(ruling.digest().clone());
        decided.proposal_digest = Some(ruling.payload().proposal.clone());
        decided.bindings = Some(ruling.payload().bindings.clone());
        decided.summary = proposal.payload().intent.summary.clone();
        let proposal_digest = ruling.payload().proposal.value.clone();

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
                // Filled by the pre-image gate, from what it read.
                observed_pre_image: None,
                effect: MutationEffect::NotAttempted,
            })
            .collect();

        // The autonomy and pause gates, through `live_permission` -- the same function the landing
        // transaction re-runs. Evaluated here, after the Candidate, because the order of refusals
        // is part of what a Receipt says and moving it would change which one a caller sees.
        let permission = live_permission(connection, self.workspace_id, opened.project_id)?;
        decided.autonomy_profile = permission.profile;
        if let Some(outcome) = permission.refusal {
            return Ok(decided.refused(outcome));
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
        if !is_beacon_made(connection, self.workspace_id, &repository, &mut decided)? {
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
        let pinned_base = ruling.payload().bindings.pinned_base_revision.clone();
        match pre_images_hold(&repository, &pinned_base, &mut decided, &|| {
            self.before_pre_image_read();
        }) {
            PreImageVerdict::Hold => {}
            PreImageVerdict::Mismatch => {
                return Ok(decided.refused(DispatchOutcome::PreImageMismatch));
            }
            // R46: no comparison happened, so the refusal names the READ, not a disagreement.
            PreImageVerdict::Unreadable(failure) => {
                decided.pre_image_read_failure = Some(failure);
                return Ok(decided.refused(DispatchOutcome::PreImageUnreadable));
            }
            // R50: no read happened either. The refusal names the inventory, which is the thing
            // that actually failed, and carries what git said about it.
            PreImageVerdict::InventoryUnestablished(failure) => {
                decided.filter_inventory_failure = Some(failure);
                return Ok(decided.refused(DispatchOutcome::FilterInventoryUnreadable));
            }
        }

        // G11: the live repository, read through the pinned directory with a read-only Git shape.
        // Nothing here may write: `git status` rewrites `.git/index` without GIT_OPTIONAL_LOCKS=0,
        // and a gate that mutates what it measures cannot support a zero-mutation claim.
        let symbolic_ref = ["symbolic-ref", "--quiet", "HEAD"];
        let branch = read_pinned(&repository, &symbolic_ref, READ_BUDGET);
        decided.record_step(GitStepName::SymbolicRef, &symbolic_ref, branch.is_some());
        let Some(branch) = branch else {
            return Ok(decided.refused(DispatchOutcome::SourceDetached));
        };
        if branch.trim().is_empty() {
            return Ok(decided.refused(DispatchOutcome::SourceDetached));
        }
        let rev_parse = ["rev-parse", "--verify", "HEAD"];
        let head = read_pinned(&repository, &rev_parse, READ_BUDGET);
        decided.record_step(GitStepName::RevParse, &rev_parse, head.is_some());
        let Some(head) = head else {
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
        let status_arguments = ["status", "--porcelain", "--untracked-files=all"];
        let status = read_pinned(&repository, &status_arguments, READ_BUDGET);
        decided.record_step(GitStepName::Status, &status_arguments, status.is_some());
        let Some(status) = status else {
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
        //
        // Every invocation below records itself as it is made, including the ones that fail: a
        // Receipt that lists steps a loop assumed rather than steps that ran is not an attempted-
        // command record (gpt1).
        let read_tree = ["read-tree", base];
        let read = write_pinned(
            repository,
            Some(scratch.path()),
            None,
            &read_tree,
            None,
            WRITE_BUDGET,
        );
        decided.record_step(GitStepName::ReadTree, &read_tree, read.is_some());
        if read.is_none() {
            return Ok(Err(DispatchOutcome::ObjectWriteFailed));
        }
        let entries: Vec<(MutationKind, String)> = decided
            .per_mutation
            .iter()
            .map(|record| (record.kind, record.path.clone()))
            .collect();
        for ((kind, path), supplied) in entries.iter().zip(content) {
            let wrote = match (kind, &supplied.bytes) {
                (MutationKind::DeleteFile, _) => {
                    let remove = ["update-index", "--force-remove", "--", path.as_str()];
                    let done = write_pinned(
                        repository,
                        Some(scratch.path()),
                        None,
                        &remove,
                        None,
                        WRITE_BUDGET,
                    );
                    decided.record_step(GitStepName::UpdateIndex, &remove, done.is_some());
                    done.is_some()
                }
                (_, Some(bytes)) => {
                    // `--no-filters` is the whole point: the blob is the supplied bytes, whatever
                    // `.gitattributes` would have done to them on the way in.
                    let hash = blob_arguments();
                    let blob =
                        write_pinned(repository, None, None, &hash, Some(bytes), WRITE_BUDGET);
                    decided.record_step(GitStepName::HashObject, &hash, blob.is_some());
                    let Some(blob) = blob else {
                        return Ok(Err(DispatchOutcome::ObjectWriteFailed));
                    };
                    let cacheinfo = format!("100644,{},{}", blob.trim(), path);
                    let add = ["update-index", "--add", "--cacheinfo", cacheinfo.as_str()];
                    let done = write_pinned(
                        repository,
                        Some(scratch.path()),
                        None,
                        &add,
                        None,
                        WRITE_BUDGET,
                    );
                    decided.record_step(GitStepName::UpdateIndex, &add, done.is_some());
                    done.is_some()
                }
                (_, None) => false,
            };
            if !wrote {
                return Ok(Err(DispatchOutcome::ObjectWriteFailed));
            }
        }

        // W3: the tree and the commit. The identity and the time are fixed by the caller, so the
        // resulting revision is a function of its inputs rather than of the clock.
        let write_tree = ["write-tree"];
        let tree = write_pinned(
            repository,
            Some(scratch.path()),
            None,
            &write_tree,
            None,
            WRITE_BUDGET,
        );
        decided.record_step(GitStepName::WriteTree, &write_tree, tree.is_some());
        let Some(tree) = tree else {
            return Ok(Err(DispatchOutcome::ObjectWriteFailed));
        };
        let identity = GitIdentity {
            name: COMMIT_AUTHOR_NAME.to_owned(),
            email: COMMIT_AUTHOR_EMAIL.to_owned(),
            date: format!("{} +0000", opened.started_at_ms / 1_000),
        };
        let message = commit_message(decided);
        let commit_tree = [
            "commit-tree",
            tree.trim(),
            "-p",
            base,
            "-m",
            message.as_str(),
        ];
        let commit = write_pinned(
            repository,
            None,
            Some(&identity),
            &commit_tree,
            None,
            WRITE_BUDGET,
        );
        decided.record_step(GitStepName::CommitTree, &commit_tree, commit.is_some());
        let Some(commit) = commit else {
            return Ok(Err(DispatchOutcome::ObjectWriteFailed));
        };
        let commit = commit.trim().to_owned();
        decided.prepared_commit = Some(commit.clone());

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
            let (swapped, swap) = swap_ref(&repository, branch, commit, base);
            #[cfg(debug_assertions)]
            if let Some(hook) = self.failpoints.after_swap.clone() {
                // The ref has moved and nothing about it is committed. A death here loses exactly
                // this transaction, which is why the claim above is not in it.
                hook();
            }
            decided.record_step(GitStepName::UpdateRef, &swap, swapped.is_some());
            let observe = ["rev-parse", "--verify", branch];
            let observed = read_pinned(&repository, &observe, READ_BUDGET);
            decided.record_step(GitStepName::RevParse, &observe, observed.is_some());
            // `Applied` is a claim about THIS attempt's swap, so the swap having succeeded is part
            // of it. The commit is a deterministic function of its inputs, so a branch that already
            // names it may have been moved there by something else -- and reporting that as this
            // attempt's application would be a Receipt claiming an act it did not perform (gpt1).
            let observed = observed.as_deref().map(str::trim);
            decided.observed_head = observed.map(str::to_owned);
            decided.landing_observation =
                observed_branch_reaches(&repository, commit, observed, decided);
            let outcome = match (swapped.is_some(), observed) {
                // This attempt performed the swap. That is an act, and a Receipt that called it a
                // refusal would hide a mutation Beacon made -- the one direction a record must
                // never take. `resultingRevision` is what the swap installed (R11); `observedHead`
                // records what the branch held afterwards, which is how a later writer having moved
                // it on is visible rather than erased (sol1).
                (true, Some(_)) => DispatchOutcome::Applied,
                (false, Some(_)) => DispatchOutcome::LandingRefused,
                // A swap that returned 0 and a branch that could not be read afterwards is still
                // an unestablished result: `landing_result_unknown` and the prepared commit are
                // what that state has to say, and it does not become an application by being
                // unreadable.
                (_, None) => DispatchOutcome::LandingUnobserved,
            };
            // Beacon updates its own checkout only for a landing THIS attempt performed whose
            // branch still names what it installed. A swap this attempt lost must not touch the
            // tree at all, and a branch a later writer moved on is a ref this attempt no longer
            // owns -- resetting the tree to its commit would be a second write against it.
            // Keyed on the TIP, not on reachability: a branch holding a descendant of this
            // commit reaches it and is still not at it, and resetting the tree to a commit the
            // branch has moved past would be a write against a ref this attempt no longer owns.
            // Two questions, two comparisons -- the reachability read answers the other one.
            if outcome == DispatchOutcome::Applied && observed == Some(commit) {
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
                let checkout = ["read-tree", "-u", "--reset", commit];
                decided.checkout_updated =
                    write_pinned(&repository, None, None, &checkout, None, WRITE_BUDGET).is_some();
                let updated = decided.checkout_updated;
                decided.record_step(GitStepName::ReadTreeCheckout, &checkout, updated);
            }
            if outcome == DispatchOutcome::Applied {
                // The effects are a fact about the swap this attempt performed, not about the
                // checkout that may have been skipped above.
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
        let diff_arguments = ["diff", "--name-status", "-z", base, commit];
        let diff = read_pinned(repository, &diff_arguments, READ_BUDGET);
        decided.record_step(GitStepName::DiffNameStatus, &diff_arguments, diff.is_some());
        let Some(diff) = diff else {
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

        // And the bytes in the prepared commit's tree, which is what gets published -- not the
        // bytes on disk, which no longer matter and never entered the objects.
        let posts: Vec<(String, String)> = decided
            .per_mutation
            .iter()
            .filter_map(|record| {
                record
                    .post_image
                    .clone()
                    .map(|post| (record.path.clone(), post))
            })
            .collect();
        for (declared_path, post) in posts {
            let path = format!("{commit}:{declared_path}");
            let cat_file = ["cat-file", "blob", path.as_str()];
            let read = read_pinned_bytes(repository, &cat_file, READ_BUDGET);
            let bytes = read.as_ref().ok().and_then(PinnedRead::bytes);
            decided.record_step(GitStepName::CatFile, &cat_file, bytes.is_some());
            // Unchanged behaviour: this post-condition answers "did the effect match", and every
            // answer that is not bytes means it did not -- an inventory that could not be read
            // included. It runs AFTER the objects exist, so its refusal is
            // `EffectDoesNotMatchDeclaration` with the residue that outcome already accounts for;
            // the pre-image gate is the one that must distinguish the reasons, because its refusal
            // names a comparison and nothing has been written yet.
            let Some(blob) = bytes else {
                return Ok(false);
            };
            if hex(&crate::proposal::sha256(blob)) != post {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

// ---------------------------------------------------------------------------------------------
// Deciding and recording
// ---------------------------------------------------------------------------------------------

impl Decided {
    /// Record one Git invocation, as it is made.
    fn record_step(&mut self, step: GitStepName, argv: &[&str], succeeded: bool) {
        self.git_steps.push(GitStep {
            step,
            argv: argv.iter().map(|argument| (*argument).to_owned()).collect(),
            result: if succeeded {
                GitStepResult::Succeeded
            } else {
                GitStepResult::DidNotSucceed
            },
        });
    }
}

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

/// Whether the branch this attempt observed **reaches** the prepared commit.
///
/// Residue is a fact about a ref, not about an outcome, and it is *earned* rather than asserted.
/// Two reviews walked this down one arm at a time. Deriving it from the outcome claimed that no ref
/// names the prepared commit in every prepared non-`Applied` state, including one where a competing
/// writer had published that exact commit (sol1). Deriving it from tip INEQUALITY claimed the same
/// thing whenever the branch held anything else -- but a branch holding a DESCENDANT of the prepared
/// commit reaches it, so its objects are named, and the Receipt said they were not (sol1 again, on
/// the fix). Tip equality is not reachability, and only one of them is the question.
///
/// So the affirmative arm is established by a bounded `merge-base --is-ancestor` read through the
/// pinned repository, and `unreferenced_object_residue` means exactly what that read gives:
/// **the branch this attempt observed does not reach the prepared commit** (FABLE-RULINGS R14).
/// Claims about refs this attempt did not observe are outside the token's definition, said here
/// rather than admitted later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum LandingObservation {
    /// No commit was prepared, so no object of this attempt's could be residue.
    #[default]
    NothingPrepared,
    /// The observed branch reaches the prepared commit -- it is that commit, or a descendant of it.
    /// Whoever put it there, a ref names these objects.
    BranchReachesPreparedCommit,
    /// The branch was read, the reachability read answered, and the answer is no.
    BranchDoesNotReachPreparedCommit,
    /// The branch could not be read, or the reachability read could not answer. Not a residue
    /// state in either direction: `landing_result_unknown` is what this has to say (C37).
    Unobserved,
}

/// Does the observed branch reach the prepared commit? Answered by a read, never by comparison.
///
/// `merge-base --is-ancestor` answers with its exit code -- 0 yes, 1 no, anything else an error --
/// which is why this uses `read_pinned_exit`: a reader that only sees success would collapse "no"
/// and "could not tell" into one value, and those are the two answers this whole finding is about.
/// Tip equality short-circuits it because a commit reaches itself and a Git invocation to establish
/// that would be a read with a known answer.
fn observed_branch_reaches(
    repository: &PinnedRepository,
    commit: &str,
    observed: Option<&str>,
    decided: &mut Decided,
) -> LandingObservation {
    let Some(observed) = observed else {
        return LandingObservation::Unobserved;
    };
    if observed == commit {
        return LandingObservation::BranchReachesPreparedCommit;
    }
    let is_ancestor = ["merge-base", "--is-ancestor", commit, observed];
    let exit = read_pinned_exit(repository, &is_ancestor, READ_BUDGET);
    // "Succeeded" here means the read ANSWERED. Exit 1 is an answer -- the interesting one -- and
    // recording it as a failed step would file the finding's own evidence as a failure.
    decided.record_step(
        GitStepName::MergeBase,
        &is_ancestor,
        matches!(exit, Some(0 | 1)),
    );
    match exit {
        Some(0) => LandingObservation::BranchReachesPreparedCommit,
        Some(1) => LandingObservation::BranchDoesNotReachPreparedCommit,
        _ => LandingObservation::Unobserved,
    }
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
    /// What the branch held when this attempt observed it. `NothingPrepared` until a landing runs.
    landing_observation: LandingObservation,
    git_steps: Vec<GitStep>,
    /// Which pre-image read failed, when the gate could not answer (R46).
    pre_image_read_failure: Option<PinnedReadFailure>,
    /// Why the filter-driver inventory could not be established (R50).
    filter_inventory_failure: Option<FilterInventoryFailure>,
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
            version: RECEIPT_VERSION,
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
            pre_image_read_failure: decided.pre_image_read_failure.clone(),
            filter_inventory_failure: decided.filter_inventory_failure.clone(),
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
         ) VALUES (?1, ?2, ?6, ?3, ?4, ?5)",
        params![
            workspace_id.to_string(),
            run_id.to_string(),
            receipt.outcome.as_str(),
            payload.as_str(),
            ended_at_ms,
            // The COLUMN and the JSON carry the same version, from one constant: the owner
            // compares them, and a literal here would drift the moment the shape moved (R46.7.1).
            receipt.version,
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
    // Residue is derived from the OBSERVATION, never from the outcome. The token asserts that
    // objects exist which no ref names, and only one of the four observations establishes that.
    // Keying it on the outcome asserted it in states where a ref demonstrably named the commit --
    // gpt2 found the unobserved case, sol1 the case where a competing writer published this exact
    // commit and this attempt's swap lost.
    if decided.prepared_commit.is_some()
        && decided.landing_observation == LandingObservation::BranchDoesNotReachPreparedCommit
    {
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

/// What one authority evaluation established, when it granted.
struct AuthorityFacts {
    ruling: crate::ruling::Ruling,
    proposal: crate::proposal::Proposal,
}

/// Why one authority evaluation refused, and what it had established when it did.
///
/// The lists are carried because a Receipt names the last thing that was true: a drifted Ruling
/// says which bindings moved, an unready Proposal which preconditions are unmet.
struct AuthorityRefused {
    outcome: DispatchOutcome,
    drift: Vec<UnmetPrecondition>,
    unmet: Vec<UnmetPrecondition>,
}

impl AuthorityRefused {
    fn outcome(outcome: DispatchOutcome) -> Self {
        Self {
            outcome,
            drift: Vec::new(),
            unmet: Vec::new(),
        }
    }
}

/// The recorded half of the authority decision: is this Ruling one that permits a dispatch?
///
/// **One evaluator, two call sites**, and that is the point. The gates and the re-verification
/// inside the landing transaction each encoded this sequence independently, with their own copy of
/// the refusal mapping; the second copy had already omitted the Proposal's readiness once and said
/// so in its own comment. A safety decision written twice drifts, and only one of the copies is the
/// one that runs at the moment it matters.
///
/// Every fact is read through its owning module's verified reader. Nothing here reads the world --
/// `live_permission` does that, separately, because the gates evaluate it AFTER the Candidate and
/// the order of refusals is part of what each Receipt says.
fn verified_authority(
    connection: &Connection,
    workspace_id: Uuid,
    ruling_digest: &str,
) -> Result<AuthorityFacts, AuthorityRefused> {
    let ruling = match verified_ruling_within(connection, workspace_id, ruling_digest) {
        Ok(Some(ruling)) => ruling,
        Ok(None) | Err(_) => {
            return Err(AuthorityRefused::outcome(DispatchOutcome::RulingUnreadable));
        }
    };
    if ruling.payload().decision != RulingDecision::Approve {
        return Err(AuthorityRefused::outcome(DispatchOutcome::RulingRefused));
    }
    if ruling.force().superseded_by().is_some() {
        return Err(AuthorityRefused::outcome(DispatchOutcome::RulingSuperseded));
    }
    if !ruling.force().drift().is_empty() {
        return Err(AuthorityRefused {
            outcome: DispatchOutcome::RulingDrifted,
            drift: ruling.force().drift().to_vec(),
            unmet: Vec::new(),
        });
    }
    // The Proposal through its owning module's FULL contract, which includes readiness. A digest
    // comparison alone would be a weaker rule than `ProposalService::get` applies, and a
    // precondition that becomes unmet in the landing window -- a Resident deactivated, a
    // Configuration changed -- is as much a reason to refuse as a supersession.
    let proposal = match verified_proposal_within(
        connection,
        workspace_id,
        &ruling.payload().proposal.value,
    ) {
        Ok(Some(proposal)) => proposal,
        Ok(None) | Err(_) => {
            return Err(AuthorityRefused::outcome(
                DispatchOutcome::ProposalDigestMismatch,
            ));
        }
    };
    let unmet = live_unmet(proposal.readiness().unmet());
    if !unmet.is_empty() {
        return Err(AuthorityRefused {
            outcome: DispatchOutcome::ProposalNotReady,
            drift: Vec::new(),
            unmet,
        });
    }
    Ok(AuthorityFacts { ruling, proposal })
}

/// The live permission, and the profile that decided it.
///
/// The profile is carried even when it refuses, because it IS the gate and a Receipt that refused
/// under Observe has to name Observe.
struct Permission {
    profile: Option<AutonomyProfile>,
    refusal: Option<DispatchOutcome>,
}

/// The live half: may this Installation dispatch at all, right now?
///
/// Observe permits no external mutation (`BLUEPRINT:405-406`), so it refuses here rather than being
/// recorded as a limitation; a paused Installation refuses as a gate, never as a line in
/// `not_verified` (`BLUEPRINT:377-379`, `AGENTS.md:128`). Read through the Project module's own
/// reader, because "current Charter revision" is its invariant and a gate that authorizes a write
/// must not carry a second definition of it.
fn live_permission(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Permission, DispatchError> {
    let Ok(Some(profile)) = current_autonomy_profile(connection, workspace_id, project_id) else {
        return Ok(Permission {
            profile: None,
            refusal: Some(DispatchOutcome::AutonomyProfileRefuses),
        });
    };
    let refusal = if profile != AutonomyProfile::Supervised {
        Some(DispatchOutcome::AutonomyProfileRefuses)
    } else if installation_is_paused(connection)? {
        Some(DispatchOutcome::InstallationPaused)
    } else {
        None
    };
    Ok(Permission {
        profile: Some(profile),
        refusal,
    })
}

/// The authority, re-read at the last moment before the irreversible step.
///
/// Returns the refusal if anything that permitted this dispatch has stopped permitting it. Called
/// inside the landing transaction, so nothing it reads can change before the ref moves: a competing
/// Ruling, Policy publication, Charter revision or Pause is a write, and the write lock is held.
/// It makes the SAME decision the gates made, by calling the same two functions -- which is why
/// there is no second copy of it to drift.
fn reverify_authority(
    connection: &Connection,
    workspace_id: Uuid,
    opened: &OpenRun,
) -> Result<Option<DispatchOutcome>, DispatchError> {
    if let Err(refused) = verified_authority(connection, workspace_id, &opened.ruling_digest) {
        return Ok(Some(refused.outcome));
    }
    Ok(live_permission(connection, workspace_id, opened.project_id)?.refusal)
}

/// Move `branch` from exactly `expected` to `commit`, or fail.
///
/// `update-ref <ref> <new> <old>` is a compare-and-swap: Git refuses unless the ref currently holds
/// `old`. That is the property a fast-forward merge does NOT have -- a branch moved back to an
/// ancestor still fast-forwards, and two dispatchers that both observed the base both succeed -- so
/// the old value is not optional here, and
/// `a_swap_refuses_unless_the_ref_still_holds_the_expected_value` is what keeps it from being
/// dropped.
///
/// The argv is built here and handed back so the caller records the invocation that RAN rather than
/// a second literal of its own. A Receipt's `gitSteps` is a record of attempted commands; two
/// literals for one command can drift, and the Receipt would then describe a plain update while a
/// compare-and-swap ran, or the reverse, with nothing to notice (Fable).
fn swap_ref<'a>(
    repository: &PinnedRepository,
    branch: &'a str,
    commit: &'a str,
    expected: &'a str,
) -> (Option<String>, [&'a str; 4]) {
    let argv = ["update-ref", branch, commit, expected];
    let swapped = write_pinned(repository, None, None, &argv, None, WRITE_BUDGET);
    (swapped, argv)
}

/// Does every declared pre-image match what the pinned base revision actually holds?
///
/// A create must find nothing at its path; a modify or a delete must find a blob whose bytes hash
/// to the declared pre-image. Read from the base TREE, not from the working tree: the tree is what
/// the change will be applied to.
/// The verdict of the pre-image gate. Three answers, because `Unreadable` is not a disagreement.
enum PreImageVerdict {
    Hold,
    /// Every declared pre-image was READ (or the tree reported no such object) and at least one
    /// disagreed with what the Candidate declares.
    Mismatch,
    /// At least one declared pre-image could not be read. Carries the FIRST such failure, which is
    /// the one that stopped this gate from being able to answer.
    Unreadable(PinnedReadFailure),
    /// R50: the filter-driver inventory could not be established, so no `cat-file` was run at all.
    /// Not a verdict about any pre-image -- it is why this gate never got to ask.
    InventoryUnestablished(FilterInventoryFailure),
}

fn pre_images_hold(
    repository: &PinnedRepository,
    base: &str,
    decided: &mut Decided,
    before_read: &dyn Fn(),
) -> PreImageVerdict {
    let records: Vec<(Option<String>, String)> = decided
        .per_mutation
        .iter()
        .map(|record| (record.pre_image.clone(), record.path.clone()))
        .collect();
    let mut held = true;
    let mut unreadable: Option<PinnedReadFailure> = None;
    let mut observed: Vec<(String, ObservedPreImage)> = Vec::new();
    for (declared_pre_image, declared_path) in records {
        let path = format!("{base}:{declared_path}");
        let cat_file = ["cat-file", "blob", path.as_str()];
        before_read();
        let read = match read_pinned_bytes(repository, &cat_file, READ_BUDGET) {
            Ok(read) => read,
            // R50: no child ran, because the hardening it would have run under could not be
            // established. That is not a fact about this pre-image, and it is not this gate's
            // refusal to name -- it stops the dispatch with the inventory's own outcome.
            Err(failure) => {
                decided.record_step(GitStepName::CatFile, &cat_file, false);
                return PreImageVerdict::InventoryUnestablished(failure);
            }
        };
        decided.record_step(GitStepName::CatFile, &cat_file, read.bytes().is_some());
        // R46.8: THE OBSERVATION IS RECORDED FIRST, for every read that produced one, and the
        // verdict is then read off the table. Four rounds of this lane fixed the contract one cell
        // at a time -- the declared-present branches in item 6, the declared-present-absent branch
        // in item 8 -- and each time the cell nobody had written was the one that bricked a store.
        // Recording and deciding are separate steps now, so a cell cannot be decided without its
        // evidence.
        let observation = match &read {
            PinnedRead::Bytes(bytes) => Some(ObservedPreImage::Bytes {
                digest: hex(&crate::proposal::sha256(bytes)),
            }),
            PinnedRead::NotFound => Some(ObservedPreImage::Absent),
            // A read that FAILED establishes nothing, so it observes nothing. It is not a cell of
            // this table at all: it refuses as `PreImageUnreadable` below.
            PinnedRead::Unreadable(_) => None,
        };
        if let Some(value) = &observation {
            observed.push((declared_path.clone(), value.clone()));
        }
        let record_holds = match (&declared_pre_image, &observation) {
            // The two cells that HOLD: a declared absence the tree agrees is absent, and a
            // declared digest the tree's bytes hash to.
            (None, Some(ObservedPreImage::Absent)) => true,
            (Some(declared), Some(ObservedPreImage::Bytes { digest })) => digest == declared,
            // The three cells that MISMATCH, each carrying the observation above:
            //   (present, Bytes != declared) · (present, Absent) · (absent, Bytes)
            (None, Some(ObservedPreImage::Bytes { .. }))
            | (Some(_), Some(ObservedPreImage::Absent)) => false,
            // R46: the read failed. This gate cannot say whether the pre-image holds, so it does
            // not say it does and it does not say it disagrees. The first such failure is kept
            // because it is the one that stopped the gate answering; later reads still run, so
            // the Receipt's step list stays the invocations this dispatch made.
            (_, None) => {
                if let PinnedRead::Unreadable(failure) = &read {
                    unreadable.get_or_insert_with(|| failure.clone());
                }
                false
            }
        };
        // Every declared path is read, not just the ones up to the first disagreement, so the
        // Receipt's step list is the invocations this dispatch made rather than a prefix of them.
        held = held && record_holds;
    }
    for (path, digest) in observed {
        if let Some(record) = decided
            .per_mutation
            .iter_mut()
            .find(|record| record.path == path)
        {
            record.observed_pre_image = Some(digest);
        }
    }
    match (unreadable, held) {
        (Some(failure), _) => PreImageVerdict::Unreadable(failure),
        (None, true) => PreImageVerdict::Hold,
        (None, false) => PreImageVerdict::Mismatch,
    }
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
    decided: &mut Decided,
) -> Result<bool, DispatchError> {
    let (device, inode) = repository.identity();
    let recorded: Option<String> = connection
        .query_row(
            "SELECT provenance_token FROM beacon_repositories
             WHERE workspace_id = ?1 AND device = ?2 AND inode = ?3",
            params![
                workspace_id.to_string(),
                i64::try_from(device).unwrap_or(i64::MAX),
                i64::try_from(inode).unwrap_or(i64::MAX),
            ],
            |row| row.get(0),
        )
        .optional()?;
    let Some(recorded) = recorded else {
        return Ok(false);
    };
    // The token, read from the repository itself through the pinned descriptor. Device and inode
    // are reusable after a deletion, so the row alone proves "a directory Beacon once made had
    // these numbers" rather than "this is that directory" (gpt1). The read is a gate's own Git
    // invocation, so it is recorded like every other one.
    let token = read_provenance_token(repository);
    decided.record_step(
        GitStepName::Config,
        &provenance_token_arguments(),
        token.is_some(),
    );
    Ok(token.as_deref() == Some(recorded.as_str()))
}

/// The provenance token this repository carries, read through the descriptor it was pinned by.
pub(crate) fn read_provenance_token(repository: &PinnedRepository) -> Option<String> {
    read_pinned(repository, &provenance_token_arguments(), READ_BUDGET)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// One owner for the invocation, so what the Receipt records cannot drift from what ran.
fn provenance_token_arguments() -> [&'static str; 5] {
    [
        "config",
        "--file",
        ".git/config",
        "--get",
        PROVENANCE_TOKEN_KEY,
    ]
}

/// Where a Beacon-made copy carries the token that says Beacon made it.
pub(crate) const PROVENANCE_TOKEN_KEY: &str = "beacon.provenanceToken";

/// Record a repository Beacon made, from the descriptor it has held since it made it.
///
/// The repository is passed in already pinned rather than opened from a pathname here: a review
/// found the window between copying to a path and reopening that path, which is long enough for the
/// directory recorded to be a different one from the directory created (gpt1). The token is read
/// back through that same descriptor, so the row and the repository agree at the moment of writing.
pub(crate) fn record_beacon_repository(
    connection: &Connection,
    workspace_id: Uuid,
    repository: &PinnedRepository,
    now: i64,
) -> Result<(), DispatchError> {
    let (device, inode) = repository.identity();
    let token = read_provenance_token(repository).ok_or(DispatchError::InvalidStore)?;
    connection.execute(
        "INSERT INTO beacon_repositories (
             workspace_id, path, device, inode, origin, provenance_token, created_at_ms
         ) VALUES (?1, ?2, ?3, ?4, 'sample_project_copy', ?5, ?6)",
        params![
            workspace_id.to_string(),
            repository.path().to_string_lossy(),
            i64::try_from(device).unwrap_or(i64::MAX),
            i64::try_from(inode).unwrap_or(i64::MAX),
            token,
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
            LandingObservation::NothingPrepared,
        );
        write_receipt(connection, workspace_id, run_id, &receipt)?;
    }

    for prepared in rows {
        let workspace_id =
            Uuid::parse_str(&prepared.workspace_id).map_err(|_| DispatchError::InvalidStore)?;
        let run_id = Uuid::parse_str(&prepared.run_id).map_err(|_| DispatchError::InvalidStore)?;
        let project_id =
            Uuid::parse_str(&prepared.project_id).map_err(|_| DispatchError::InvalidStore)?;
        let (outcome, observation) = observe_landing(&prepared);
        let started_at_ms =
            u64::try_from(prepared.started_at_ms).map_err(|_| DispatchError::InvalidStore)?;
        let mut receipt = interrupted_receipt(
            run_id,
            workspace_id,
            project_id,
            started_at_ms,
            outcome.clone(),
            Some(&prepared),
            observation,
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
    observation: LandingObservation,
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
    // The same derivation as the live path, from the same observation: a ref that names the
    // prepared commit is a ref that names its objects, whatever this attempt's outcome was.
    if prepared.is_some() && observation == LandingObservation::BranchDoesNotReachPreparedCommit {
        not_verified.push(NotVerified::UnreferencedObjectResidue);
    }
    not_verified.push(NotVerified::ReconstructedAfterInterruption);
    if prepared.is_none() {
        // The interruption landed before the claim: whether any object was written is unknown,
        // and if one was, nothing can name it. Derived from the absence of the preparation rather
        // than passed alongside it, so the two cannot disagree.
        not_verified.push(NotVerified::ObjectResidueUnknown);
    }
    DispatchReceipt {
        version: RECEIPT_VERSION,
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
        // A reconstruction never ran the pre-image gate, so it carries no read failure.
        pre_image_read_failure: None,
        filter_inventory_failure: None,
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
fn observe_landing(prepared: &PreparedDispatch) -> (DispatchOutcome, LandingObservation) {
    let unobserved = (
        DispatchOutcome::LandingUnobserved,
        LandingObservation::Unobserved,
    );
    let Ok(repository) = PinnedRepository::open(Path::new(&prepared.source)) else {
        return unobserved;
    };
    let (device, inode) = repository.identity();
    // The identity the attempt pinned, or nothing: a directory that now has a different identity is
    // a different repository, and reading its ref would answer a question about the wrong tree.
    if i64::try_from(device).unwrap_or(i64::MAX) != prepared.device
        || i64::try_from(inode).unwrap_or(i64::MAX) != prepared.inode
    {
        return unobserved;
    }
    match read_pinned(
        &repository,
        &["rev-parse", "--verify", &prepared.branch],
        READ_BUDGET,
    ) {
        Some(head) if head.trim() == prepared.prepared_commit => (
            DispatchOutcome::Applied,
            LandingObservation::BranchReachesPreparedCommit,
        ),
        // The branch is back at the base. The outcome is a refusal, and whether this attempt's
        // objects are still reachable is a separate question, answered by the same read the live
        // path uses rather than assumed from the tip (R14).
        Some(head) if head.trim() == prepared.pinned_base_revision => (
            DispatchOutcome::LandingRefused,
            reconstructed_reachability(&repository, prepared, head.trim()),
        ),
        // The branch holds something else entirely: the attempt may have landed and been moved
        // afterwards, or never landed. That is exactly what "unobserved" means, and an outcome
        // nobody could establish makes no residue claim in either direction (gpt2, s4f).
        Some(_) | None => unobserved,
    }
}

/// The reachability half of a reconstructed observation, with no `Decided` to record a step on.
///
/// Startup writes a Receipt for an attempt that never wrote one, so there is no step list to append
/// to; the answer still has to be earned by the same read rather than inferred from the tip.
fn reconstructed_reachability(
    repository: &PinnedRepository,
    prepared: &PreparedDispatch,
    observed: &str,
) -> LandingObservation {
    if observed == prepared.prepared_commit {
        return LandingObservation::BranchReachesPreparedCommit;
    }
    match read_pinned_exit(
        repository,
        &[
            "merge-base",
            "--is-ancestor",
            &prepared.prepared_commit,
            observed,
        ],
        READ_BUDGET,
    ) {
        Some(0) => LandingObservation::BranchReachesPreparedCommit,
        Some(1) => LandingObservation::BranchDoesNotReachPreparedCommit,
        _ => LandingObservation::Unobserved,
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
    if !(3..=RECEIPT_VERSION).contains(&receipt.version)
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
    if i64::from(receipt.version) != version
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
/// Is this record's `(declared, observed)` pair one of the three cells that mean MISMATCH?
///
/// R46.8, the contract as ONE table rather than per-branch arithmetic. Five cells, and which side
/// they fall on is the whole of what a mismatch Receipt claims:
///
/// | declared | observed        | verdict                     |
/// |----------|-----------------|-----------------------------|
/// | present  | `Bytes` ≠       | mismatch, carrying `Bytes`  |
/// | present  | `Absent`        | mismatch, carrying `Absent` |
/// | absent   | `Bytes`         | mismatch, carrying `Bytes`  — the create conflict |
/// | absent   | `Absent`        | holds → `Applied`           |
/// | present  | `Bytes` =       | holds → `Applied`           |
///
/// A missing observation is no cell at all: at version 4 it is incoherent, and the caller reads a
/// version-3 Receipt as unrecorded before it ever asks this (R46.7.1).
fn is_mismatch_cell(record: &MutationRecord) -> bool {
    match (&record.pre_image, &record.observed_pre_image) {
        (Some(declared), Some(ObservedPreImage::Bytes { digest })) => digest != declared,
        (Some(_), Some(ObservedPreImage::Absent))
        | (None, Some(ObservedPreImage::Bytes { .. })) => true,
        (None, Some(ObservedPreImage::Absent)) | (_, None) => false,
    }
}

fn verify_outcome_invariants(receipt: &DispatchReceipt) -> Result<(), DispatchError> {
    let names = |token: NotVerified| receipt.not_verified.contains(&token);
    let reconstructed = names(NotVerified::ReconstructedAfterInterruption);
    // R54 (sol1's cold read): THE ERA RULE IS TWO-SIDED. R46.7.1 reads a version-3 Receipt's
    // missing observation as "unrecorded" and accepts it -- but nothing checked that a version-3
    // Receipt was actually unrecorded, so a hand-written hybrid carrying the version-4-only
    // `observedPreImage` was accepted as legacy. An era marker that only licenses absence is not a
    // marker; it is a hole shaped like one.
    //
    // Outcome-independent, because the field belongs to the R46 ERA rather than to one arm: a
    // version-3 `Applied` carrying it is the same forgery as a version-3 mismatch carrying it.
    if receipt.version < RECEIPT_VERSION
        && receipt
            .per_mutation
            .iter()
            .any(|record| record.observed_pre_image.is_some())
    {
        return Err(DispatchError::InvalidStore);
    }

    // R46.6(b), astra's HOLD: the field's doc comment says `pre_image_read_failure` belongs to
    // `PreImageUnreadable` and to nothing else, and the owner did not hold it to that. A promise
    // only a comment makes is one a stored Receipt can break -- an unreadable refusal carrying no
    // subcase says a read failed and refuses to say which, and any other outcome carrying one
    // names a read failure it did not have.
    if (receipt.outcome == DispatchOutcome::PreImageUnreadable)
        != receipt.pre_image_read_failure.is_some()
    {
        return Err(DispatchError::InvalidStore);
    }
    // R50: the same rule for the same reason, one gate earlier. `FilterInventoryUnreadable`
    // carries WHY the inventory could not be read and every other outcome carries nothing, so a
    // refusal that will not say why is as incoherent as an `Applied` naming a read failure it
    // never had. Two independent fields rather than one reused: the two failures are about
    // different reads, and a Receipt that could carry either in one slot could not say which.
    if (receipt.outcome == DispatchOutcome::FilterInventoryUnreadable)
        != receipt.filter_inventory_failure.is_some()
    {
        return Err(DispatchError::InvalidStore);
    }
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
                && !receipt.checkout_updated
                && names(NotVerified::LandingResultUnknown)
        }
        // R46.5: a mismatch Receipt must carry the digest it read, and that digest must DISAGREE
        // with the declared one. A mismatch whose observed digest equals the declared one is
        // incoherent -- it records the comparison holding and the outcome refusing -- and a
        // mismatch carrying none asserts a comparison it kept no evidence of. Both refuse here as
        // `InvalidStore`, the same way every other incoherent Receipt does.
        DispatchOutcome::PreImageMismatch => {
            // R46.7.1: keyed on the Receipt's VERSION, the house era marker, never on whether the
            // field happens to be there. A version-3 mismatch predates the field and is read as
            // UNRECORDED -- accepted as it stands, because inventing a digest for it would be a
            // fabrication and refusing it would make every historical refusal unreadable after an
            // upgrade (the lockout R46.7 named, in the other direction).
            let observation = if receipt.version >= RECEIPT_VERSION {
                // R46.9 (astra): `any(is_mismatch_cell)` alone let a Receipt carry an UNRECORDED
                // row as long as some OTHER row mismatched -- the five-cell table held per row and
                // said nothing about the Receipt as a whole. Every mutation of a version-4
                // mismatch was read, so every one has an observation; a holding cell is a perfectly
                // valid row (the gate reads every declared path, not a prefix), and at least one
                // row has to occupy a mismatch cell or the outcome is not what happened.
                receipt
                    .per_mutation
                    .iter()
                    .all(|record| record.observed_pre_image.is_some())
                    && receipt.per_mutation.iter().any(is_mismatch_cell)
            } else {
                true
            };
            receipt.resulting_revision.is_none()
                && !receipt.checkout_updated
                && names(NotVerified::ChangeNotApplied)
                && observation
        }
        // Every other arm refused. Objects it wrote may survive -- that is what the residue token
        // is for, and a diff digest measured over a commit that was never published is still a
        // measurement of that commit -- but nothing it prepared may be presented as a landing.
        _ => {
            receipt.resulting_revision.is_none()
                && !receipt.checkout_updated
                && names(NotVerified::ChangeNotApplied)
            // No residue requirement: the token is derived from the observation now, and a refusal
            // whose branch was observed to name the prepared commit correctly makes no claim.
            // Requiring it here is what forced the false claim in the first place.
        }
    };
    if coherent {
        Ok(())
    } else {
        Err(DispatchError::InvalidStore)
    }
}

/// Check a Receipt against the durable claim the attempt committed before its swap.
///
/// The claim is the independent record: it is committed in its own transaction, before the one
/// that performs the swap, and is never rewritten. So the arms that claim the swap ran must have
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
            swap_ref(&repository, branch, &second, &first).0.is_none(),
            "a swap expecting a stale value succeeded"
        );
        assert_eq!(git(&root, &["rev-parse", "HEAD"]), third);

        // And the positive control: expecting what the ref actually holds moves it.
        assert!(swap_ref(&repository, branch, &second, &third).0.is_some());
        assert_eq!(git(&root, &["rev-parse", branch]), second);

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A Receipt built entirely from fixed inputs, so a second implementation can compare bytes.
    ///
    /// Every value here is chosen, not measured: the UUIDs count up, the digests are constant, the
    /// times are 1 and 2. Two implementations that agree on the encoding produce these bytes.
    fn specimen(outcome: DispatchOutcome) -> DispatchReceipt {
        // The two arms are not the same record with different values: a `RulingRefused` leaves the
        // gate list at G3, so it has no Candidate, no repository, no profile and no objects, and a
        // specimen that carried them would pin bytes this dispatch path cannot emit (gpt2). The
        // members each arm leaves absent are asserted against a real Receipt of the same outcome in
        // `tests/dispatch.rs`.
        let applied = outcome == DispatchOutcome::Applied;
        DispatchReceipt {
            version: RECEIPT_VERSION,
            run_id: Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap(),
            workspace_id: Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap(),
            project_id: Uuid::parse_str("00000000-0000-4000-8000-000000000003").unwrap(),
            actor: DispatchActor::Owner,
            ruling_digest: RulingDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.ruling.v2".to_owned(),
                value: "1".repeat(64),
            },
            proposal_digest: Some(ProposalDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.proposal.v1".to_owned(),
                value: "2".repeat(64),
            }),
            candidate_digest: applied.then(|| CandidateDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.candidate.v1".to_owned(),
                value: "3".repeat(64),
            }),
            outcome: outcome.clone(),
            autonomy_profile: applied.then_some(AutonomyProfile::Supervised),
            charter_revision: 1,
            project_policy_revision: 1,
            repository_revision: 1,
            source_kind: applied.then_some(RepositorySourceKind::LocalPath),
            source: applied.then(|| "/copies/sample-project-specimen".to_owned()),
            source_identity: applied.then_some(SourceIdentity {
                device: 66_306,
                inode: 4_242,
            }),
            branch: applied.then(|| "refs/heads/main".to_owned()),
            pinned_base_revision: Some("4".repeat(40)),
            observed_head: applied.then(|| "5".repeat(40)),
            prepared_commit: applied.then(|| "5".repeat(40)),
            resulting_revision: applied.then(|| "5".repeat(40)),
            diff_digest: applied.then(|| "6".repeat(64)),
            checkout_updated: applied,
            git_steps: if applied {
                vec![GitStep {
                    step: GitStepName::UpdateRef,
                    argv: vec![
                        "update-ref".to_owned(),
                        "refs/heads/main".to_owned(),
                        "5".repeat(40),
                        "4".repeat(40),
                    ],
                    result: GitStepResult::Succeeded,
                }]
            } else {
                Vec::new()
            },
            model_invocations: 0,
            cost_usd_micros: 0,
            per_mutation: if applied {
                vec![MutationRecord {
                    kind: MutationKind::ModifyFile,
                    path: "src/lib.rs".to_owned(),
                    pre_image: Some("7".repeat(64)),
                    post_image: Some("8".repeat(64)),
                    // An APPLIED specimen: its pre-image held, so the observed digest equals the
                    // declared one and the field is skipped rather than repeating it.
                    observed_pre_image: None,
                    effect: MutationEffect::Written,
                }]
            } else {
                Vec::new()
            },
            started_at_ms: 1,
            ended_at_ms: 2,
            pre_image_read_failure: None,
            filter_inventory_failure: None,
            not_verified: vec![
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
            ]
            .into_iter()
            .chain(if applied {
                Vec::new()
            } else {
                vec![NotVerified::ChangeNotApplied]
            })
            .collect(),
        }
    }

    #[test]
    fn astra_unreadable_receipt_requires_its_failure() {
        let receipt = specimen(DispatchOutcome::PreImageUnreadable);
        assert!(
            verify_outcome_invariants(&receipt).is_err(),
            "R46.OWNER.UNREADABLE_REQUIRES_FAILURE"
        );
    }

    #[test]
    fn astra_other_outcome_rejects_a_pre_image_failure() {
        let mut receipt = specimen(DispatchOutcome::RulingRefused);
        receipt.pre_image_read_failure = Some(PinnedReadFailure::TimedOut);
        assert!(
            verify_outcome_invariants(&receipt).is_err(),
            "R46.OWNER.OTHER_OUTCOME_REJECTS_FAILURE"
        );
    }

    /// R50: the same owner rule, for the read one gate earlier.
    #[test]
    fn r50_inventory_receipt_requires_its_failure() {
        let receipt = specimen(DispatchOutcome::FilterInventoryUnreadable);
        assert!(
            verify_outcome_invariants(&receipt).is_err(),
            "R50.OWNER.INVENTORY_REQUIRES_FAILURE"
        );
    }

    #[test]
    fn r50_other_outcome_rejects_an_inventory_failure() {
        let mut receipt = specimen(DispatchOutcome::RulingRefused);
        receipt.filter_inventory_failure = Some(FilterInventoryFailure::Unreadable);
        assert!(
            verify_outcome_invariants(&receipt).is_err(),
            "R50.OWNER.OTHER_OUTCOME_REJECTS_INVENTORY_FAILURE"
        );
    }

    /// R50: the two failure fields belong to two different outcomes and never to each other's.
    ///
    /// One field reused for both reads could not say WHICH read failed, which is the whole reason
    /// R47's subcase was the wrong shape. This is the assertion that keeps them apart.
    #[test]
    fn r50_the_two_read_failures_are_not_interchangeable() {
        let mut inventory = specimen(DispatchOutcome::FilterInventoryUnreadable);
        inventory.pre_image_read_failure = Some(PinnedReadFailure::TimedOut);
        assert!(
            verify_outcome_invariants(&inventory).is_err(),
            "R50.OWNER.INVENTORY_OUTCOME_REJECTS_PRE_IMAGE_FAILURE"
        );

        let mut unreadable = specimen(DispatchOutcome::PreImageUnreadable);
        unreadable.pre_image_read_failure = Some(PinnedReadFailure::TimedOut);
        unreadable.filter_inventory_failure = Some(FilterInventoryFailure::Unreadable);
        assert!(
            verify_outcome_invariants(&unreadable).is_err(),
            "R50.OWNER.PRE_IMAGE_OUTCOME_REJECTS_INVENTORY_FAILURE"
        );

        // ...and the coherent pair is accepted, so the rule refuses the wrong shape rather than
        // every shape.
        let mut coherent = specimen(DispatchOutcome::FilterInventoryUnreadable);
        coherent.filter_inventory_failure = Some(FilterInventoryFailure::Refused {
            message: "fatal: unable to access '/proc/self/fd/100/config': Permission denied"
                .to_owned(),
        });
        assert!(
            verify_outcome_invariants(&coherent).is_ok(),
            "R50.OWNER.COHERENT_INVENTORY_RECEIPT_IS_ACCEPTED"
        );
    }

    /// R50: the retired R47 subcases still DECODE, so Receipts already recorded stay readable.
    ///
    /// `PinnedReadFailure::FilterInventory*` are retained on `DispatchOutcome::Admissible`'s terms:
    /// no new dispatch produces them, and deleting them would make a stored Receipt undecodable --
    /// the lockout R46.7 named. A Receipt from the R46..R50 builds is still coherent.
    #[test]
    fn r50_a_receipt_from_the_retired_subcase_path_still_decodes_and_is_coherent() {
        for retired in [
            PinnedReadFailure::FilterInventoryUnreadable,
            PinnedReadFailure::FilterInventoryRefused {
                message: "fatal: unable to read config file".to_owned(),
            },
        ] {
            let mut receipt = specimen(DispatchOutcome::PreImageUnreadable);
            receipt.pre_image_read_failure = Some(retired.clone());
            let encoded = serde_json::to_string(&receipt).unwrap();
            let decoded: DispatchReceipt = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded.pre_image_read_failure, Some(retired));
            assert!(
                verify_outcome_invariants(&decoded).is_ok(),
                "a Receipt recorded before R50 stopped being readable"
            );
        }
    }

    /// The members a Receipt of this outcome leaves absent, in declaration order.
    ///
    /// The same list is asserted against a real Receipt of the same outcome in
    /// `tests/dispatch.rs`, which is what ties a pinned vector to a producible record.
    fn absent_members(receipt: &DispatchReceipt) -> Vec<String> {
        let value: serde_json::Value = serde_json::to_value(receipt).unwrap();
        let object = value.as_object().unwrap();
        object
            .iter()
            .filter(|(_, value)| value.is_null())
            .map(|(name, _)| name.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// The members a `RulingRefused` leaves absent, asserted here and against a real Receipt of
    /// the same outcome in `tests/dispatch.rs`.
    const REFUSED_ABSENT: &[&str] = &[
        "autonomyProfile",
        "branch",
        "candidateDigest",
        "diffDigest",
        "observedHead",
        "preparedCommit",
        "resultingRevision",
        "source",
        "sourceIdentity",
        "sourceKind",
    ];

    #[test]
    fn an_unknown_landing_is_never_reported_as_byte_identical() {
        // `repository_effect` answers `Unknown` for a `landing_result_unknown` Receipt because of
        // the token, and this is the scenario where the token is the ONLY thing that can: a record
        // that names no prepared commit falls through to the byte-identical arm otherwise, and
        // saying a repository is unchanged when the landing's result is unknown is a false
        // statement rather than a weak one.
        //
        // The scenario exists because C41 added a catch-all `Unknown` arm for prepared commits,
        // which masked the token guard everywhere the two agreed -- the mutant survived until this
        // test named the case only the guard can see (C37's rule, applied to its own guard).
        let mut receipt = specimen(DispatchOutcome::RulingRefused);
        receipt.outcome = DispatchOutcome::LandingUnobserved;
        receipt.prepared_commit = None;
        receipt.resulting_revision = None;
        receipt.not_verified = vec![NotVerified::LandingResultUnknown];
        assert_eq!(receipt.repository_effect(), RepositoryEffect::Unknown);

        // The control: without the unknown token, the same shape IS byte-identical.
        receipt.not_verified = Vec::new();
        assert_eq!(receipt.repository_effect(), RepositoryEffect::ByteIdentical);
    }

    #[test]
    fn the_applied_receipt_specimen_is_exactly_these_bytes() {
        // The vector a second implementation matches against. A cross-lab review built a Receipt
        // from the ADR's field list and produced different bytes for the same facts, which is what
        // happens when a record has a field list instead of an encoding: member order, the shape of
        // a digest envelope, and whether an absent fact is `null` or missing are all decisions, and
        // this is where they are pinned.
        let bytes = serde_json::to_string(&specimen(DispatchOutcome::Applied)).unwrap();
        assert_eq!(bytes.len(), 2048);
        assert_eq!(
            hex(&crate::proposal::sha256(bytes.as_bytes())),
            "fe71ea0e9fa2d8d45baedc9104e02f0dd3d680d75ae3b63cfe56405d389cabfb"
        );
        assert_eq!(bytes, APPLIED_SPECIMEN);
        // An Applied establishes every fact, so nothing in it is absent -- and it is a record this
        // build could have written.
        assert!(absent_members(&specimen(DispatchOutcome::Applied)).is_empty());
        verify_outcome_invariants(&specimen(DispatchOutcome::Applied)).unwrap();
    }

    /// R46.9 (astra's joint read): the evidence rule is about the RECEIPT, not one lucky row.
    ///
    /// The five-cell table holds per mutation; `any(is_mismatch_cell)` then accepted a Receipt
    /// carrying an unrecorded row as long as some OTHER row mismatched. Every declared path of a
    /// version-4 mismatch was read -- the gate reads all of them, not a prefix -- so every row has
    /// an observation, and a HOLDING row is perfectly valid beside a mismatching one.
    #[test]
    fn every_mutation_of_a_version_four_mismatch_carries_its_observation() {
        let row = |path: &str, observed: Option<ObservedPreImage>| MutationRecord {
            kind: MutationKind::ModifyFile,
            path: path.to_owned(),
            pre_image: Some("7".repeat(64)),
            post_image: Some("8".repeat(64)),
            observed_pre_image: observed,
            effect: MutationEffect::NotAttempted,
        };
        let receipt_with = |rows: Vec<MutationRecord>| {
            let mut receipt = specimen(DispatchOutcome::PreImageMismatch);
            receipt.per_mutation = rows;
            receipt
        };
        let mismatching = Some(ObservedPreImage::Bytes {
            digest: "9".repeat(64),
        });
        let holding = Some(ObservedPreImage::Bytes {
            digest: "7".repeat(64),
        });

        // Both rows recorded, one of them a mismatch: coherent, and the second row HOLDING is not
        // a defect -- it is what reading every declared path looks like.
        verify_outcome_invariants(&receipt_with(vec![
            row("src/lib.rs", mismatching.clone()),
            row("src/other.rs", holding),
        ]))
        .expect("a recorded holding row beside a recorded mismatch is coherent");

        // One row UNRECORDED: the Receipt says a comparison happened for a path it kept no
        // evidence about, and the other row's mismatch does not cover for it.
        assert!(
            matches!(
                verify_outcome_invariants(&receipt_with(vec![
                    row("src/lib.rs", mismatching),
                    row("src/other.rs", None),
                ])),
                Err(DispatchError::InvalidStore)
            ),
            "a version-4 mismatch carrying an unrecorded mutation was accepted"
        );
    }

    /// R54 (sol1's cold read): the era rule is TWO-SIDED -- a version-3 Receipt carries NO
    /// observation at all, not merely "no observation required".
    ///
    /// R46.7.1 reads a version-3 Receipt's missing observation as UNRECORDED and accepts it, and
    /// nothing checked that such a Receipt was actually unrecorded: a hand-written hybrid carrying
    /// the version-4-only field was read as a legacy record and believed. An era marker that only
    /// licenses absence is not a marker; it is a hole shaped like one.
    ///
    /// The field belongs to the ERA rather than to one arm, so the rule is outcome-independent: an
    /// `Applied` at version 3 carrying it is the same forgery as a mismatch at version 3 carrying
    /// it, and ONE row carrying it is enough -- a Receipt does not become half a version.
    #[test]
    fn a_version_three_receipt_carries_no_observation_at_all() {
        let row = |observed: Option<ObservedPreImage>| MutationRecord {
            kind: MutationKind::ModifyFile,
            path: "src/lib.rs".to_owned(),
            pre_image: Some("7".repeat(64)),
            post_image: Some("8".repeat(64)),
            observed_pre_image: observed,
            effect: MutationEffect::NotAttempted,
        };
        let legacy = |rows: Vec<MutationRecord>| {
            let mut receipt = specimen(DispatchOutcome::PreImageMismatch);
            receipt.version = 3;
            receipt.per_mutation = rows;
            receipt
        };

        // R46.7.1 unchanged: the shape a real pre-R46 store holds.
        verify_outcome_invariants(&legacy(vec![row(None), row(None)]))
            .expect("a version-3 mismatch recording nothing is unrecorded, not incoherent");

        // Either observed value is version-4-only vocabulary. A version-3 record cannot have
        // emitted either one, so neither is evidence -- it is a claim about an era that had no
        // such field.
        for forged in [
            ObservedPreImage::Bytes {
                digest: "9".repeat(64),
            },
            ObservedPreImage::Absent,
        ] {
            assert!(
                matches!(
                    verify_outcome_invariants(&legacy(vec![row(Some(forged.clone()))])),
                    Err(DispatchError::InvalidStore)
                ),
                "a version-3 Receipt carrying a version-4 observation was accepted"
            );
        }

        // ANY row, not every row: a hybrid that leaves the first row bare is the shape that slips
        // past a rule quantified the other way.
        assert!(
            matches!(
                verify_outcome_invariants(&legacy(vec![
                    row(None),
                    row(Some(ObservedPreImage::Absent))
                ])),
                Err(DispatchError::InvalidStore)
            ),
            "a version-3 Receipt whose second row carried an observation was accepted"
        );

        // Outcome-independent: nothing about this is particular to the mismatch arm.
        let mut applied = specimen(DispatchOutcome::Applied);
        applied.version = 3;
        applied.per_mutation = vec![row(Some(ObservedPreImage::Bytes {
            digest: "7".repeat(64),
        }))];
        assert!(
            matches!(
                verify_outcome_invariants(&applied),
                Err(DispatchError::InvalidStore)
            ),
            "a version-3 applied Receipt carrying a version-4 observation was accepted"
        );

        // And it does not reach forward. A CURRENT-era Receipt carrying the field is the ordinary
        // case -- the whole point of R46 -- so a rule one version too wide would refuse every
        // Receipt this binary writes.
        let mut current = specimen(DispatchOutcome::PreImageMismatch);
        current.per_mutation = vec![row(Some(ObservedPreImage::Bytes {
            digest: "9".repeat(64),
        }))];
        verify_outcome_invariants(&current)
            .expect("a version-4 mismatch carrying what it read is the current era, not a forgery");

        // Outside the mismatch arm the current era owes no observation, which is what an
        // unreadable pre-image leaves behind (R46.8) -- absence is not the marker, version is.
        verify_outcome_invariants(&specimen(DispatchOutcome::Applied))
            .expect("a version-4 Receipt without an observation is the current era, not a forgery");
    }

    /// R46.5: a `PreImageMismatch` Receipt owes its own record the digest it compared against.
    ///
    /// The Receipt owner refuses both incoherent shapes the same way every other incoherent
    /// Receipt is refused -- `DispatchError::InvalidStore` out of `verify_outcome_invariants`,
    /// which is what a reconciling reader calls before it believes a stored refusal.
    #[test]
    fn a_mismatch_receipt_owes_the_digest_it_read() {
        let mismatch = |observed: Option<ObservedPreImage>| {
            let mut receipt = specimen(DispatchOutcome::PreImageMismatch);
            receipt.per_mutation = vec![MutationRecord {
                kind: MutationKind::ModifyFile,
                path: "src/lib.rs".to_owned(),
                pre_image: Some("7".repeat(64)),
                post_image: Some("8".repeat(64)),
                observed_pre_image: observed,
                effect: MutationEffect::NotAttempted,
            }];
            receipt
        };

        // A digest that DISAGREES: the comparison happened and the outcome follows from it.
        verify_outcome_invariants(&mismatch(Some(ObservedPreImage::Bytes {
            digest: "9".repeat(64),
        })))
        .expect("a mismatch carrying what it read is coherent");

        // R46.7: an ABSENCE where something was declared is equally truthful -- the path is not
        // there, which is exactly why the declared pre-image does not hold.
        verify_outcome_invariants(&mismatch(Some(ObservedPreImage::Absent)))
            .expect("a truthful absent pre-image is a coherent mismatch");

        // NONE: the Receipt asserts a comparison and keeps no evidence of one.
        assert!(
            matches!(
                verify_outcome_invariants(&mismatch(None)),
                Err(DispatchError::InvalidStore)
            ),
            "a mismatch that recorded nothing it read was accepted"
        );

        // EQUAL to the declared digest: the comparison held and the outcome refused. Both cannot
        // be true, and this is the shape a plausible-looking forgery would take.
        assert!(
            matches!(
                verify_outcome_invariants(&mismatch(Some(ObservedPreImage::Bytes {
                    digest: "7".repeat(64)
                }))),
                Err(DispatchError::InvalidStore)
            ),
            "a mismatch whose observed digest equals the declared one was accepted"
        );

        // R46.7: `Absent` against a DECLARED ABSENCE is what holding looks like, not a mismatch.
        let mut declared_absent = mismatch(Some(ObservedPreImage::Absent));
        declared_absent.per_mutation[0].pre_image = None;
        assert!(
            matches!(
                verify_outcome_invariants(&declared_absent),
                Err(DispatchError::InvalidStore)
            ),
            "a mismatch declaring absence and observing absence was accepted"
        );

        // R46.7.1: a version-3 Receipt predates the field. Unrecorded, not a liar.
        let mut legacy = mismatch(None);
        legacy.version = 3;
        verify_outcome_invariants(&legacy)
            .expect("a version-3 mismatch without the field is unrecorded, not incoherent");
    }

    #[test]
    fn the_refused_receipt_specimen_is_exactly_these_bytes() {
        // The same thirty members, with everything the refusal did not reach carried as `null`. A
        // `RulingRefused` leaves the gate list at G3, so this specimen has no Candidate, no
        // repository, no profile and no objects: pinning bytes for a shape the dispatch path cannot
        // produce would be a vector for a record that does not exist (gpt2).
        let bytes = serde_json::to_string(&specimen(DispatchOutcome::RulingRefused)).unwrap();
        assert_eq!(bytes.len(), 1301);
        assert_eq!(
            hex(&crate::proposal::sha256(bytes.as_bytes())),
            "d55faf2cf80d698a88ba30aba5c71464948209841d3e535b234ff0df4019726f"
        );
        assert_eq!(bytes, REFUSED_SPECIMEN);
        assert_eq!(
            absent_members(&specimen(DispatchOutcome::RulingRefused)),
            REFUSED_ABSENT
        );
        verify_outcome_invariants(&specimen(DispatchOutcome::RulingRefused)).unwrap();
    }

    const APPLIED_SPECIMEN: &str = concat!(
        r#"{"version":4,"runId":"00000000-0000-4000-8000-000000000001","workspaceId":"00000000-0000-4000-80"#,
        r#"00-000000000002","projectId":"00000000-0000-4000-8000-000000000003","actor":"owner","rulingDiges"#,
        r#"t":{"algorithm":"sha256","canonicalizer":"beacon.ruling.v2","value":"111111111111111111111111111"#,
        r#"1111111111111111111111111111111111111"},"proposalDigest":{"algorithm":"sha256","canonicalizer":""#,
        r#"beacon.proposal.v1","value":"2222222222222222222222222222222222222222222222222222222222222222"},"#,
        r#""candidateDigest":{"algorithm":"sha256","canonicalizer":"beacon.candidate.v1","value":"333333333"#,
        r#"3333333333333333333333333333333333333333333333333333333"},"outcome":"applied","autonomyProfile":"#,
        r#""supervised","charterRevision":1,"projectPolicyRevision":1,"repositoryRevision":1,"sourceKind":""#,
        r#"local_path","source":"/copies/sample-project-specimen","sourceIdentity":{"device":66306,"inode":"#,
        r#"4242},"branch":"refs/heads/main","pinnedBaseRevision":"4444444444444444444444444444444444444444""#,
        r#","observedHead":"5555555555555555555555555555555555555555","preparedCommit":"5555555555555555555"#,
        r#"555555555555555555555","resultingRevision":"5555555555555555555555555555555555555555","diffDiges"#,
        r#"t":"6666666666666666666666666666666666666666666666666666666666666666","checkoutUpdated":true,"gi"#,
        r#"tSteps":[{"step":"update_ref","argv":["update-ref","refs/heads/main","55555555555555555555555555"#,
        r#"55555555555555","4444444444444444444444444444444444444444"],"result":"succeeded"}],"modelInvocat"#,
        r#"ions":0,"costUsdMicros":0,"perMutation":[{"kind":"modify_file","path":"src/lib.rs","preImage":"7"#,
        r#"777777777777777777777777777777777777777777777777777777777777777","postImage":"888888888888888888"#,
        r#"8888888888888888888888888888888888888888888888","effect":"written"}],"startedAtMs":1,"endedAtMs""#,
        r#":2,"notVerified":["execution_policy_constraint","sandbox_constraint","network_constraint","model"#,
        r#"_provider_constraint","command_allowlist_constraint","tool_allowlist_constraint","evaluations_co"#,
        r#"nstraint","non_author_evaluation_of_candidate","safety_snapshot","base_revision_reachability","d"#,
        r#"eclared_checks_against_result"]}"#
    );

    const REFUSED_SPECIMEN: &str = concat!(
        r#"{"version":4,"runId":"00000000-0000-4000-8000-000000000001","workspaceId":"00000000-0000-4000-80"#,
        r#"00-000000000002","projectId":"00000000-0000-4000-8000-000000000003","actor":"owner","rulingDiges"#,
        r#"t":{"algorithm":"sha256","canonicalizer":"beacon.ruling.v2","value":"111111111111111111111111111"#,
        r#"1111111111111111111111111111111111111"},"proposalDigest":{"algorithm":"sha256","canonicalizer":""#,
        r#"beacon.proposal.v1","value":"2222222222222222222222222222222222222222222222222222222222222222"},"#,
        r#""candidateDigest":null,"outcome":"ruling_refused","autonomyProfile":null,"charterRevision":1,"pr"#,
        r#"ojectPolicyRevision":1,"repositoryRevision":1,"sourceKind":null,"source":null,"sourceIdentity":n"#,
        r#"ull,"branch":null,"pinnedBaseRevision":"4444444444444444444444444444444444444444","observedHead""#,
        r#":null,"preparedCommit":null,"resultingRevision":null,"diffDigest":null,"checkoutUpdated":false,""#,
        r#"gitSteps":[],"modelInvocations":0,"costUsdMicros":0,"perMutation":[],"startedAtMs":1,"endedAtMs""#,
        r#":2,"notVerified":["execution_policy_constraint","sandbox_constraint","network_constraint","model"#,
        r#"_provider_constraint","command_allowlist_constraint","tool_allowlist_constraint","evaluations_co"#,
        r#"nstraint","non_author_evaluation_of_candidate","safety_snapshot","base_revision_reachability","d"#,
        r#"eclared_checks_against_result","change_not_applied"]}"#
    );

    // -----------------------------------------------------------------------------------------
    // R46 item 3: the dispatch outcome vocabulary is CLOSED, and every variant has a row.
    //
    // Modelled on the R42S2 TRANSPORT witness. The `match` is exhaustive, so a variant added
    // without a row does not compile; the `rows()` list is separate, so a row DELETED from it
    // fails `every_dispatch_outcome_has_a_row` rather than passing quietly. Both halves are
    // needed: one catches an addition, the other catches a removal.
    macro_rules! outcome_table {
        ( $( $case:literal | $pattern:pat => $token:literal , $build:expr ; )+ ) => {
            fn expected_outcome(value: &DispatchOutcome) -> (&'static str, &'static str) {
                match value {
                    $( $pattern => ($case, $token), )+
                }
            }

            fn outcome_rows() -> Vec<(&'static str, DispatchOutcome)> {
                vec![$(($case, $build)),+]
            }
        };
    }

    outcome_table! {
        "R46.OUTCOME.RulingUnreadable" | DispatchOutcome::RulingUnreadable => "ruling_unreadable", DispatchOutcome::RulingUnreadable;
        "R46.OUTCOME.RulingRefused" | DispatchOutcome::RulingRefused => "ruling_refused", DispatchOutcome::RulingRefused;
        "R46.OUTCOME.RulingSuperseded" | DispatchOutcome::RulingSuperseded => "ruling_superseded", DispatchOutcome::RulingSuperseded;
        "R46.OUTCOME.RulingDrifted" | DispatchOutcome::RulingDrifted => "ruling_drifted", DispatchOutcome::RulingDrifted;
        "R46.OUTCOME.ProposalDigestMismatch" | DispatchOutcome::ProposalDigestMismatch => "proposal_digest_mismatch", DispatchOutcome::ProposalDigestMismatch;
        "R46.OUTCOME.ProposalNotReady" | DispatchOutcome::ProposalNotReady => "proposal_not_ready", DispatchOutcome::ProposalNotReady;
        "R46.OUTCOME.CandidateAbsent" | DispatchOutcome::CandidateAbsent => "candidate_absent", DispatchOutcome::CandidateAbsent;
        "R46.OUTCOME.CandidateMismatch" | DispatchOutcome::CandidateMismatch => "candidate_mismatch", DispatchOutcome::CandidateMismatch;
        "R46.OUTCOME.ContentDigestMismatch" | DispatchOutcome::ContentDigestMismatch => "content_digest_mismatch", DispatchOutcome::ContentDigestMismatch;
        "R46.OUTCOME.ContentOverBounds" | DispatchOutcome::ContentOverBounds => "content_over_bounds", DispatchOutcome::ContentOverBounds;
        "R46.OUTCOME.SourceNotLocal" | DispatchOutcome::SourceNotLocal => "source_not_local", DispatchOutcome::SourceNotLocal;
        "R46.OUTCOME.SourceNotBeaconMade" | DispatchOutcome::SourceNotBeaconMade => "source_not_beacon_made", DispatchOutcome::SourceNotBeaconMade;
        "R46.OUTCOME.RepositoryHeadMoved" | DispatchOutcome::RepositoryHeadMoved => "repository_head_moved", DispatchOutcome::RepositoryHeadMoved;
        "R46.OUTCOME.SourceDetached" | DispatchOutcome::SourceDetached => "source_detached", DispatchOutcome::SourceDetached;
        "R46.OUTCOME.SourceDirty" | DispatchOutcome::SourceDirty => "source_dirty", DispatchOutcome::SourceDirty;
        "R46.OUTCOME.AutonomyProfileRefuses" | DispatchOutcome::AutonomyProfileRefuses => "autonomy_profile_refuses", DispatchOutcome::AutonomyProfileRefuses;
        "R46.OUTCOME.InstallationPaused" | DispatchOutcome::InstallationPaused => "installation_paused", DispatchOutcome::InstallationPaused;
        "R46.OUTCOME.PreImageMismatch" | DispatchOutcome::PreImageMismatch => "pre_image_mismatch", DispatchOutcome::PreImageMismatch;
        "R46.OUTCOME.PreImageUnreadable" | DispatchOutcome::PreImageUnreadable => "pre_image_unreadable", DispatchOutcome::PreImageUnreadable;
        "R50.OUTCOME.FilterInventoryUnreadable" | DispatchOutcome::FilterInventoryUnreadable => "filter_inventory_unreadable", DispatchOutcome::FilterInventoryUnreadable;
        "R46.OUTCOME.WallClockExceeded" | DispatchOutcome::WallClockExceeded => "wall_clock_exceeded", DispatchOutcome::WallClockExceeded;
        "R46.OUTCOME.AlreadyDispatched" | DispatchOutcome::AlreadyDispatched => "already_dispatched", DispatchOutcome::AlreadyDispatched;
        "R46.OUTCOME.Admissible" | DispatchOutcome::Admissible => "admissible", DispatchOutcome::Admissible;
        "R46.OUTCOME.ObjectWriteFailed" | DispatchOutcome::ObjectWriteFailed => "object_write_failed", DispatchOutcome::ObjectWriteFailed;
        "R46.OUTCOME.EffectDoesNotMatchDeclaration" | DispatchOutcome::EffectDoesNotMatchDeclaration => "effect_does_not_match_declaration", DispatchOutcome::EffectDoesNotMatchDeclaration;
        "R46.OUTCOME.LandingRefused" | DispatchOutcome::LandingRefused => "landing_refused", DispatchOutcome::LandingRefused;
        "R46.OUTCOME.LandingUnobserved" | DispatchOutcome::LandingUnobserved => "landing_unobserved", DispatchOutcome::LandingUnobserved;
        "R46.OUTCOME.Applied" | DispatchOutcome::Applied => "applied", DispatchOutcome::Applied;
    }

    #[test]
    fn every_dispatch_outcome_has_a_row_and_its_own_token() {
        let rows = outcome_rows();
        assert_eq!(
            rows.len(),
            28,
            "a dispatch outcome was added or a row was deleted: the vocabulary is closed (R42/R46)"
        );
        let mut tokens = std::collections::BTreeSet::new();
        for (case, outcome) in &rows {
            let (named_case, token) = expected_outcome(outcome);
            assert_eq!(
                named_case, *case,
                "the row's case name does not match the table"
            );
            assert_eq!(
                outcome.as_str(),
                token,
                "{case}: the outward token does not match the pinned table"
            );
            assert!(
                tokens.insert(token),
                "{case}: two outcomes share the token {token}, so a reader cannot tell them apart"
            );
        }
        assert_eq!(tokens.len(), 28);
    }

    // -----------------------------------------------------------------------------------------
    // R50 item 3: caller-facing text, in the audiences a dispatch outcome actually has.
    //
    // The expected lines are written HERE, independently, and never derived from the outcome's own
    // name or from `caller_line`'s source (R42). A wording change is a deliberate edit of both
    // halves; a wording change in one half alone is a failure.
    // -----------------------------------------------------------------------------------------
    const CALLER_LINES: &[(&str, DispatchOutcome, &str)] = &[
        (
            "R50.LINE.RulingUnreadable",
            DispatchOutcome::RulingUnreadable,
            "the Ruling record does not agree with itself",
        ),
        (
            "R50.LINE.RulingRefused",
            DispatchOutcome::RulingRefused,
            "the Ruling refused this Proposal",
        ),
        (
            "R50.LINE.RulingSuperseded",
            DispatchOutcome::RulingSuperseded,
            "a later Ruling reverses this one",
        ),
        (
            "R50.LINE.RulingDrifted",
            DispatchOutcome::RulingDrifted,
            "a revision the Ruling recorded has moved",
        ),
        (
            "R50.LINE.ProposalDigestMismatch",
            DispatchOutcome::ProposalDigestMismatch,
            "the stored Proposal does not hash to the digest the Ruling names",
        ),
        (
            "R50.LINE.ProposalNotReady",
            DispatchOutcome::ProposalNotReady,
            "a live precondition of the Proposal does not hold",
        ),
        (
            "R50.LINE.CandidateAbsent",
            DispatchOutcome::CandidateAbsent,
            "no Candidate authorizes any bytes for this Proposal",
        ),
        (
            "R50.LINE.CandidateMismatch",
            DispatchOutcome::CandidateMismatch,
            "the Candidate the Ruling names is not the Candidate the Proposal has",
        ),
        (
            "R50.LINE.ContentDigestMismatch",
            DispatchOutcome::ContentDigestMismatch,
            "the supplied content does not hash to what the Candidate declares",
        ),
        (
            "R50.LINE.ContentOverBounds",
            DispatchOutcome::ContentOverBounds,
            "the supplied content exceeds a bound the Proposal declared",
        ),
        (
            "R50.LINE.SourceNotLocal",
            DispatchOutcome::SourceNotLocal,
            "the Repository Binding does not name a local path",
        ),
        (
            "R50.LINE.SourceNotBeaconMade",
            DispatchOutcome::SourceNotBeaconMade,
            "the bound repository is not a disposable copy Beacon made",
        ),
        (
            "R50.LINE.RepositoryHeadMoved",
            DispatchOutcome::RepositoryHeadMoved,
            "the repository's head is not the base revision this Ruling pinned",
        ),
        (
            "R50.LINE.SourceDetached",
            DispatchOutcome::SourceDetached,
            "the repository's head is not on a branch, so there is no ref to move",
        ),
        (
            "R50.LINE.SourceDirty",
            DispatchOutcome::SourceDirty,
            "the repository holds uncommitted or untracked content",
        ),
        (
            "R50.LINE.AutonomyProfileRefuses",
            DispatchOutcome::AutonomyProfileRefuses,
            "the Project's autonomy profile does not admit dispatch",
        ),
        (
            "R50.LINE.InstallationPaused",
            DispatchOutcome::InstallationPaused,
            "the Installation is paused",
        ),
        (
            "R50.LINE.PreImageMismatch",
            DispatchOutcome::PreImageMismatch,
            "a declared pre-image is not what the base revision holds",
        ),
        (
            "R50.LINE.PreImageUnreadable",
            DispatchOutcome::PreImageUnreadable,
            "a declared pre-image could not be read, so nothing was compared",
        ),
        (
            "R50.LINE.FilterInventoryUnreadable",
            DispatchOutcome::FilterInventoryUnreadable,
            "the repository's filter configuration could not be read, so no Git command could be run safely",
        ),
        (
            "R50.LINE.WallClockExceeded",
            DispatchOutcome::WallClockExceeded,
            "the attempt exceeded the time bound the Proposal declared",
        ),
        (
            "R50.LINE.AlreadyDispatched",
            DispatchOutcome::AlreadyDispatched,
            "this Ruling has already been dispatched",
        ),
        (
            "R50.LINE.Admissible",
            DispatchOutcome::Admissible,
            "every gate held and the act was not attempted",
        ),
        (
            "R50.LINE.ObjectWriteFailed",
            DispatchOutcome::ObjectWriteFailed,
            "an object, tree or commit could not be written",
        ),
        (
            "R50.LINE.EffectDoesNotMatchDeclaration",
            DispatchOutcome::EffectDoesNotMatchDeclaration,
            "the prepared commit does not match what the Candidate declares",
        ),
        (
            "R50.LINE.LandingRefused",
            DispatchOutcome::LandingRefused,
            "the branch no longer held the pinned base at the moment of the swap",
        ),
        (
            "R50.LINE.LandingUnobserved",
            DispatchOutcome::LandingUnobserved,
            "the result of the swap could not be established",
        ),
        (
            "R50.LINE.Applied",
            DispatchOutcome::Applied,
            "the branch names the prepared commit",
        ),
    ];

    /// Audience 2 (the in-process API): every outcome has one fixed line, and it is that line.
    #[test]
    fn r50_every_dispatch_outcome_has_its_own_fixed_caller_line() {
        assert_eq!(
            CALLER_LINES.len(),
            outcome_rows().len(),
            "the caller-line table and the outcome vocabulary disagree about how many there are"
        );
        let mut seen = std::collections::BTreeSet::new();
        for (case, outcome, expected) in CALLER_LINES {
            assert_eq!(
                outcome.caller_line(),
                *expected,
                "{case}: the line shown to a caller is not the line this table pins"
            );
            assert!(
                seen.insert(*expected),
                "{case}: two outcomes share a line, so a caller cannot tell them apart"
            );
        }
        // ...and the table covers the vocabulary, not a subset of it: an outcome added without a
        // line would compile (the match is exhaustive, so it could not), but a line DELETED here
        // would leave a row uncovered, which the length check above and this set catch together.
        let lined: std::collections::BTreeSet<&str> = CALLER_LINES
            .iter()
            .map(|(_, outcome, _)| outcome.as_str())
            .collect();
        for (case, outcome) in outcome_rows() {
            assert!(
                lined.contains(outcome.as_str()),
                "{case}: this outcome has no row in the caller-line table"
            );
        }
    }

    /// R42's exclusions, asserted over the WHOLE output rather than trusted to review.
    #[test]
    fn r50_no_caller_line_carries_anything_the_product_did_not_write() {
        for (case, _, line) in CALLER_LINES {
            assert!(!line.is_empty(), "{case}: an empty line is not a refusal");
            // No interpolation of any kind reaches these: they are `&'static str` by type, and
            // these assertions are what keeps a later edit from smuggling one in through `format!`
            // and a `const` that no longer is one.
            for forbidden in ['{', '}', '/', '\\', '"', '`'] {
                assert!(
                    !line.contains(forbidden),
                    "{case}: the line carries {forbidden:?}, which is how a path, a quoted \
                     identifier or a format placeholder gets in"
                );
            }
            // A possessive apostrophe is English; a PAIR of them is a quoted identifier, which is
            // the shape git uses for the path and revision it names. So the rule is the pair, not
            // the character -- stated this way because the first form of this assertion failed on
            // "the repository's head" and softening it to allow quoting would have been the wrong
            // repair.
            assert!(
                line.matches('\'').count() <= 1,
                "{case}: the line quotes something, and a quoted token is an identifier"
            );
            // git's own diagnostics, the R50 case's own hazard: the message is EVIDENCE in
            // `filter_inventory_failure`, never a sentence a caller is shown.
            for forbidden in ["fatal:", "warning:", "hint:", "error:", "/proc/", ".git"] {
                assert!(
                    !line.contains(forbidden),
                    "{case}: the line carries {forbidden:?} -- git's text is evidence, not wording"
                );
            }
            assert!(
                !line
                    .chars()
                    .any(|character| character.is_ascii_hexdigit() && character.is_ascii_digit()),
                "{case}: the line carries a digit, which is how a digest or an identity gets in"
            );
        }
    }

    /// Audience 1 (startup / the owning reader): the token IS the row key, and it is stable.
    #[test]
    fn r50_the_inventory_outcome_token_is_its_own_row_key() {
        assert_eq!(
            DispatchOutcome::FilterInventoryUnreadable.as_str(),
            "filter_inventory_unreadable"
        );
        assert_ne!(
            DispatchOutcome::FilterInventoryUnreadable.as_str(),
            DispatchOutcome::PreImageUnreadable.as_str(),
            "the inventory refusal is not the pre-image one and must not share its key"
        );
        assert_eq!(
            DispatchOutcome::FilterInventoryUnreadable.run_state(),
            "refused"
        );
    }

    /// Audience 3 (the wire and the desktop): ABSENT, and this is the witness that it stays absent.
    ///
    /// R42's third audience does not exist for a dispatch outcome, by design rather than by
    /// omission: ADR 0039 admits dispatch from one named Operator surface only,
    /// `no_transport_command_can_reach_dispatch_or_pause` is the committed inventory of that, and
    /// the Briefing wire carries two Receipt kinds, neither of them dispatch. So the thing to
    /// witness is the absence -- a dispatch-shaped projection must not decode -- with the two real
    /// kinds as the control, so the assertion cannot pass because the parser refuses everything.
    #[test]
    fn r50_no_dispatch_outcome_can_reach_the_briefing_wire() {
        use crate::recent_receipts::RecentReceiptOutcome;
        for control in [
            r#"{"kind":"check_preflight","outcome":"verified"}"#,
            r#"{"kind":"declared_checks","summary":"completed"}"#,
        ] {
            serde_json::from_str::<RecentReceiptOutcome>(control)
                .expect("the control kinds must decode, or this witness proves nothing");
        }
        for (case, outcome, _) in CALLER_LINES {
            for shape in [
                format!(
                    r#"{{"kind":"proposal_dispatch","outcome":"{}"}}"#,
                    outcome.as_str()
                ),
                format!(
                    r#"{{"kind":"check_preflight","outcome":"{}"}}"#,
                    outcome.as_str()
                ),
            ] {
                assert!(
                    serde_json::from_str::<RecentReceiptOutcome>(&shape).is_err(),
                    "{case}: a dispatch outcome decoded into the Briefing wire vocabulary"
                );
            }
        }
    }

    /// R46: the new outcome is its own row, and it is NOT the mismatch it used to be reported as.
    #[test]
    fn pre_image_unreadable_is_its_own_outcome_and_not_a_mismatch() {
        assert_eq!(
            DispatchOutcome::PreImageUnreadable.as_str(),
            "pre_image_unreadable"
        );
        assert_ne!(
            DispatchOutcome::PreImageUnreadable.as_str(),
            DispatchOutcome::PreImageMismatch.as_str()
        );
        // Both are refusals; the Run state does not distinguish them and must not be asked to.
        assert_eq!(DispatchOutcome::PreImageUnreadable.run_state(), "refused");
        // Every read-failure subcase has a name of its own, so a Receipt can say WHICH read failed.
        //
        // R47 made this witness EXHAUSTIVE, the way the outcome table above already is. It listed
        // five of six -- `ReadRefused` was left out because it carries data -- so R46's own new
        // subcase was the one variant no row covered, and R47 could have added a seventh the same
        // way. The `match` is what closes it: a variant added without a row does not compile, and
        // the separate `rows` list is what catches a row deleted from it.
        fn expected_subcase(value: &PinnedReadFailure) -> &'static str {
            match value {
                PinnedReadFailure::FilterInventoryUnreadable => "filter_inventory_unreadable",
                PinnedReadFailure::FilterInventoryRefused { .. } => "filter_inventory_refused",
                PinnedReadFailure::ChildFailed => "child_failed",
                PinnedReadFailure::Signalled => "signalled",
                PinnedReadFailure::TimedOut => "timed_out",
                PinnedReadFailure::PinBrokenAfterRead => "pin_broken_after_read",
                PinnedReadFailure::ReadRefused { .. } => "read_refused",
                PinnedReadFailure::SilentExit { .. } => "silent_exit",
            }
        }
        let subcases = [
            PinnedReadFailure::FilterInventoryUnreadable,
            // R47: the inventory read git REFUSED, carrying what git said. Its own subcase, so a
            // Receipt distinguishes "the hardening could not be established, and here is why" from
            // an inventory lost to a broken pin or a child that never started.
            PinnedReadFailure::FilterInventoryRefused {
                message: "fatal: unable to access '/proc/self/fd/100/config': Permission denied"
                    .to_owned(),
            },
            PinnedReadFailure::ChildFailed,
            PinnedReadFailure::Signalled,
            PinnedReadFailure::TimedOut,
            PinnedReadFailure::PinBrokenAfterRead,
            PinnedReadFailure::SilentExit { code: 128 },
            PinnedReadFailure::ReadRefused {
                message: "fatal: not a git repository".to_owned(),
            },
        ];
        assert_eq!(
            subcases.len(),
            8,
            "a read-failure subcase was added or a row was deleted: the vocabulary is closed"
        );
        let names: std::collections::BTreeSet<&str> = subcases
            .iter()
            .map(|subcase| {
                assert_eq!(
                    subcase.name(),
                    expected_subcase(subcase),
                    "the outward name does not match the pinned table: {subcase:?}"
                );
                subcase.name()
            })
            .collect();
        assert_eq!(names.len(), 8, "two read failures share a name: {names:?}");
        for name in names {
            assert!(
                !name.contains(' '),
                "a name is a name, not a rendering: {name}"
            );
        }
    }
}
