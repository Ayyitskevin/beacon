//! The authenticated Operator owner for Mission impact classification.
//!
//! `MissionImpactService::classify` takes **canonical bytes** (`mission_impact.rs:1140-1170`): the
//! caller assembles the whole record, including its identity, its ladder position, the Policy
//! citation and the clock. That is a store-level submit contract, not something an Operator can be
//! handed. This module is the missing derivation layer -- the classification twin of what
//! `mission_completion_command.rs` is for completions.
//!
//! The Operator supplies FIVE things and nothing else: which Mission, `high` or `ordinary`, why,
//! which Policy revision they were looking at, and -- for a reclassification -- which record they
//! are replacing. Everything else is derived here from proven store state.
//!
//! **Scope, deliberately.** One write. No reader, no `history()`, no `current(mission_id)` and no
//! `is_high_impact` (ADR 0044 refuses the last two by name). No completion prerequisite is added
//! and no Outcome policy changes: a classification recorded after a completion's boundary does not
//! and must not change that completion's Outcome (R23.4).

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mission_impact::MissionImpactClassificationV1;
use crate::operator_account::{OperatorAccountError, OperatorAccountService};

// ---------------------------------------------------------------------------------------------
// The request DTO. Five keys. Strict: unknown fields, unknown tags and out-of-domain values refuse.
// ---------------------------------------------------------------------------------------------

/// Require the KEY, permit the value to be `null`.
///
/// Serde's own missing-field path (`serde/src/private/de.rs:24-58`) hands a type a deserializer
/// that supports only `deserialize_option`, so ANY type that forwards to `Option` -- including a
/// transparent newtype over one -- accepts an omitted key. Measured on the C4 lane, where both of
/// those shapes were tried and both accepted an omitted key. A field-level `deserialize_with` takes
/// a different branch in the derive and returns `missing field` instead.
fn required_explicit_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// Which record this classification replaces, if any.
///
/// Tagged rather than a nullable digest: "I am not replacing anything" and "I am replacing this"
/// are two different statements, and a null digest would spell the first as an absence of the
/// second.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SupersedesDto {
    None,
    Record { predecessor_digest: String },
}

/// Exactly what an Operator decides. Nothing here is derived and nothing here defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClassificationIntentRequest {
    pub mission_id: Uuid,
    /// `high` or `ordinary`. **No default and no third arm** -- ADR 0044 refuses `unclassified`,
    /// and a default-to-ordinary would be omission achieving what removing `omit` from the appendix
    /// API was meant to prevent.
    pub impact: String,
    /// The Operator's own words, 1..=2000 scalars. Inside the hashed bytes.
    pub reason: String,
    /// The Policy revision the Operator was looking at when they decided, echoed back so a revision
    /// published mid-form cannot silently change what the record says decided. Explicit `null` is
    /// the claim "I saw no published Policy" -- a different statement from a forgotten key.
    ///
    /// The shape is `ProjectPolicyService::publish`'s own optimistic guard
    /// (`project_policy.rs:378-397`), not a new invention.
    #[serde(deserialize_with = "required_explicit_null")]
    pub expected_policy_revision: Option<u32>,
    pub supersedes: SupersedesDto,
}

/// The record that was just written, read back through its owning reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClassificationView {
    pub mission_id: Uuid,
    pub classification_id: Uuid,
    pub version: u32,
    pub impact: String,
    /// Fixed `operator_declared` in v1. ADR 0044 gives `policy_rule` a tag and no path, so it is
    /// not an input and cannot be one.
    pub basis: String,
    pub reason: String,
    pub policy: PolicyCitationDto,
    pub classified_by: String,
    pub classified_at_ms: u64,
    pub supersedes: SupersedesDto,
    /// Content identity, whole. A reclassification needs the predecessor's digest, and it should
    /// come from a previous answer rather than a second round trip.
    pub digest: ClassificationDigestDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyCitationDto {
    pub project_id: Uuid,
    pub policy_revision: u32,
    /// The Policy revision's OWN charter revision, never the Project's current one. They differ
    /// whenever the Charter has moved since that Policy was published.
    pub charter_revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClassificationDigestDto {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

// ---------------------------------------------------------------------------------------------
// Refusals -- a closed outward set; every detail lives in Debug and source (R42)
// ---------------------------------------------------------------------------------------------

/// Why a classification command refused.
///
/// `Display` is ONE independent literal per arm and carries nothing a caller typed. Driver text,
/// digests, revision numbers and the store's own reasons stay in `Debug` and `source`.
#[derive(Debug)]
pub enum ClassificationCommandError {
    Account(OperatorAccountError),
    /// The Policy revision the Operator echoed is not the current one.
    StalePolicyRevision,
    /// This Project has never published a Policy revision, so there is nothing to cite. Distinct
    /// from `StalePolicyRevision`: "you named the wrong revision" and "there are none" send an
    /// Operator to two different places.
    NoPolicyPublished,
    /// `supersedes: none` on a Mission that already has a classification.
    AlreadyClassified,
    /// The named predecessor is real and proven, and is not the newest version.
    PredecessorNotLatest,
    /// The named predecessor is missing, unverifiable, another Workspace's, or another Mission's.
    PredecessorUnproven,
    /// The ladder's newest version is `u32::MAX`. Refused rather than wrapped: `checked_add` would
    /// otherwise produce version 0, which no legitimate record has and which sorts below everything.
    VersionCeilingReached,
    /// `reason` exceeds the codec's own scalar bound.
    TextOverLimit,
    /// A malformed identity, digest, tag, or text class in the request.
    MalformedRequest(&'static str),
    /// The Mission, the Policy binding or the existing ladder could not be proven.
    Unavailable(ClassificationCommandUnavailable),
}

/// What actually went wrong. Never projected outward.
#[derive(Debug)]
pub enum ClassificationCommandUnavailable {
    Store(rusqlite::Error),
    /// The Mission is not admitted in this Workspace. Outwardly indistinguishable from a Mission
    /// this caller may not see.
    MissionAbsent,
    /// The Mission's admission Event does not verify. Its owner's reason, repeated.
    MissionUnverified(String),
    /// A row in this Mission's classification ladder does not re-prove. **Never an empty ladder**:
    /// unreadable and absent are different states, and collapsing them would let a corrupt latest
    /// row turn a reclassification into a silent "first" classification.
    LadderUnprovable(String),
    /// The cited Policy revision's row and its `project.policy_published` Event disagree, or the
    /// Event is missing or duplicated.
    PolicyBindingUnproven(String),
    /// A ladder row's `mission.impact_classified` Event is missing, duplicated, or disagrees with
    /// the row's retained bytes or fields.
    ClassificationEventUnproven(String),
    /// A `mission.impact_classified` Event names a classification this store does not hold. The
    /// history is broken, not empty.
    OrphanClassificationEvent(String),
    /// The store refused the write. Its own reason, carried whole.
    Write(String),
    /// The record just written does not read back as what was written.
    WriteReadBackDisagrees,
}

impl ClassificationCommandError {
    /// The closed outward token. Eleven arms, and a caller sees nothing else.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::StalePolicyRevision => "StalePolicyRevision",
            Self::NoPolicyPublished => "NoPolicyPublished",
            Self::AlreadyClassified => "AlreadyClassified",
            Self::PredecessorNotLatest => "PredecessorNotLatest",
            Self::PredecessorUnproven => "PredecessorUnproven",
            Self::VersionCeilingReached => "VersionCeilingReached",
            Self::TextOverLimit => "TextOverLimit",
            Self::MalformedRequest(_) => "MalformedRequest",
            Self::Unavailable(_) => "Unavailable",
        }
    }
}

impl fmt::Display for ClassificationCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl std::error::Error for ClassificationCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Account(error) => Some(error),
            _ => None,
        }
    }
}

impl From<OperatorAccountError> for ClassificationCommandError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for ClassificationCommandError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Unavailable(ClassificationCommandUnavailable::Store(error))
    }
}

// ---------------------------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------------------------

/// The authenticated Operator classification owner.
///
/// The Workspace comes from this service, never from client JSON.
pub struct MissionImpactCommandService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl MissionImpactCommandService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Record one classification of one Mission, in ONE Immediate transaction.
    ///
    /// Everything the write depends on is read inside that transaction and PROVEN before anything
    /// is judged: the Mission's admission, the current Policy revision against its own Event, and
    /// every ladder row against its bytes and its owning Event. Discovery columns choose candidates;
    /// they never decide.
    ///
    /// **NOT IMPLEMENTED.** The skeleton exists so the first public-service bar can be written and
    /// executed RED against the real contract rather than against a guess.
    pub fn classify(
        &self,
        request: ClassificationIntentRequest,
    ) -> Result<ClassificationView, ClassificationCommandError> {
        let _ = (&self.accounts, &self.workspace_id, &request);
        let _: Option<MissionImpactClassificationV1> = None;
        todo!("C5.OWNER: the classification owner is not implemented yet")
    }
}
