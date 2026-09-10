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

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use data_encoding::BASE64;
use rusqlite::{Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mission_impact::{
    ClassificationActor, ClassificationBasis, ClassificationRecordError, ClassificationSupersedes,
    ImpactLevel, MalformedClassification, MissionImpactClassification,
    MissionImpactClassificationV1, MissionImpactClassifiedPayload, PolicyRevisionRef,
    classify_within, verified_classification_within,
};
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
    /// What the STORED record says decided. A new write is always `operator_declared` -- ADR 0044
    /// gives `policy_rule` a tag and no path, so it is not an input and cannot be one -- but the
    /// shipped canonical-byte store admits a `policy_rule` record with a rule id
    /// (`mission_impact.rs:332-343`), and a read of one must not relabel it.
    pub basis: String,
    /// The rule identity a `policy_rule` record names, `null` for an Operator declaration.
    /// Required as a KEY: absent and null are different facts, and only one of them is recorded.
    ///
    /// Projecting it is not a claim that a rule was applied or evaluated -- only that the record
    /// says so.
    #[serde(deserialize_with = "required_explicit_null")]
    pub rule_id: Option<String>,
    pub reason: String,
    pub policy: PolicyCitationDto,
    pub classified_by: String,
    pub classified_at_ms: u64,
    pub supersedes: SupersedesDto,
    /// Content identity, whole. A reclassification needs the predecessor's digest, and it should
    /// come from a previous answer rather than a second round trip.
    pub digest: ClassificationDigestDto,
}

/// What one Mission's classification state looks like in ONE snapshot.
///
/// Four fields and no more: no record count, no history, no "current" verdict and no
/// `isHighImpact`. `publishedPolicy` is what a NEW classification would cite; `latest.policy` is
/// what the newest existing one DID cite, and the two differ whenever Policy has moved since.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClassificationObservation {
    pub mission_id: Uuid,
    /// The chain head this snapshot was taken at, as a canonical nonnegative decimal string in the
    /// `u64` domain. A string because the boundary is JavaScript: above 2^53 a JSON number is not
    /// the value any more, and a cursor that silently rounds is worse than no cursor.
    pub observed_at_event_seq: String,
    /// The latest VERIFIED published Policy of this Mission's Project, in this same snapshot.
    /// `null` is a proved absence -- this Project has published none -- never a failed read.
    #[serde(deserialize_with = "required_explicit_null")]
    pub published_policy: Option<PolicyCitationDto>,
    /// The highest proven classification version in this snapshot, whole. `null` is a proved
    /// absence, never an orphan Event or an unreadable row.
    #[serde(deserialize_with = "required_explicit_null")]
    pub latest: Option<ClassificationView>,
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
        self.accounts.with_authenticated_store(|connection, now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let outcome = self.derive_and_record(&transaction, &request, now);
            match outcome {
                Ok(view) => {
                    transaction.commit()?;
                    Ok(Ok(view))
                }
                // The transaction is dropped unwritten: a refusal writes nothing.
                Err(refusal) => Ok::<_, OperatorAccountError>(Err(refusal)),
            }
        })?
    }

    fn derive_and_record(
        &self,
        transaction: &Transaction<'_>,
        request: &ClassificationIntentRequest,
        now: i64,
    ) -> Result<ClassificationView, ClassificationCommandError> {
        // The REQUEST's own domains first. A digest that is not a digest is a malformed request
        // whatever the store holds; reporting it as `PredecessorUnproven` because the ladder
        // happened to be empty would send a caller to look for a record they never named.
        let impact = impact_from(&request.impact)?;
        let named_predecessor = match &request.supersedes {
            SupersedesDto::None => None,
            SupersedesDto::Record { predecessor_digest } => Some(digest64(
                predecessor_digest,
                "supersedes.predecessorDigest",
            )?),
        };

        // The Mission, through its OWN verified admission -- not the `missions` projection column
        // `mission_project_of` reads. An unknown Mission and another Workspace's are one answer.
        let mission = crate::mission::verified_mission_admission(
            transaction,
            self.workspace_id,
            request.mission_id,
        )
        .map_err(|error| unavailable(Unprovable::MissionUnverified(format!("{error:?}"))))?
        .ok_or_else(|| unavailable(Unprovable::MissionAbsent))?;

        // The Policy citation, PROVEN against its own `project.policy_published` Event.
        let (policy_revision, charter_revision) =
            crate::project_policy::verified_latest_policy_citation(
                transaction,
                self.workspace_id,
                mission.project_id,
            )
            .map_err(|error| unavailable(Unprovable::PolicyBindingUnproven(format!("{error:?}"))))?
            .ok_or(ClassificationCommandError::NoPolicyPublished)?;
        if request.expected_policy_revision != Some(policy_revision) {
            return Err(ClassificationCommandError::StalePolicyRevision);
        }

        // The ladder, proven before ANY of it is judged.
        let ladder = self.proven_ladder(transaction, request.mission_id)?;
        let (version, supersedes) = self.ladder_position(named_predecessor, &ladder)?;

        let record = MissionImpactClassificationV1 {
            workspace_id: self.workspace_id,
            mission_id: request.mission_id,
            classification_id: Uuid::new_v4(),
            version,
            impact,
            // Fixed. ADR 0044 gives `policy_rule` a tag and no path, so there is no input for it
            // and `rule_id` is absent by construction rather than by a caller remembering.
            basis: ClassificationBasis::OperatorDeclared,
            policy: PolicyRevisionRef {
                project_id: mission.project_id,
                policy_revision,
                charter_revision,
            },
            rule_id: None,
            reason: request.reason.clone(),
            classified_by: ClassificationActor::Operator,
            classified_at_ms: u64::try_from(now).map_err(|_| malformed("classifiedAtMs"))?,
            supersedes,
        };
        // The codec's own grammar is the validator; its classes are PROJECTED, not flattened.
        let bytes = record.canonical_bytes().map_err(codec_refusal)?;

        let recorded =
            classify_within(transaction, self.workspace_id, &bytes).map_err(write_refusal)?;
        if recorded.payload() != &record {
            return Err(unavailable(Unprovable::WriteReadBackDisagrees));
        }
        // The record just written proves itself the same way every older one had to.
        self.prove_owning_event(transaction, &recorded)?;
        Ok(view_of(&recorded))
    }

    /// What this Mission's classification state is, in ONE Deferred snapshot, writing nothing.
    ///
    /// The same proof the writer uses, reused rather than re-spelled: an adjacent read path with
    /// its own idea of what "proven" means is two answers to one question.
    pub fn observe(
        &self,
        mission_id: Uuid,
    ) -> Result<ClassificationObservation, ClassificationCommandError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let outcome = self.observed_within(&transaction, mission_id);
            match outcome {
                Ok(observation) => {
                    transaction.commit()?;
                    Ok(Ok(observation))
                }
                Err(refusal) => Ok::<_, OperatorAccountError>(Err(refusal)),
            }
        })?
    }

    fn observed_within(
        &self,
        transaction: &Transaction<'_>,
        mission_id: Uuid,
    ) -> Result<ClassificationObservation, ClassificationCommandError> {
        // Scope first: nothing about a Mission this Workspace cannot see is answerable, and an
        // unknown Mission and a foreign one are the same answer.
        let mission =
            crate::mission::verified_mission_admission(transaction, self.workspace_id, mission_id)
                .map_err(|error| unavailable(Unprovable::MissionUnverified(format!("{error:?}"))))?
                .ok_or_else(|| unavailable(Unprovable::MissionAbsent))?;

        let published_policy = crate::project_policy::verified_latest_policy_citation(
            transaction,
            self.workspace_id,
            mission.project_id,
        )
        .map_err(|error| unavailable(Unprovable::PolicyBindingUnproven(format!("{error:?}"))))?
        .map(|(policy_revision, charter_revision)| PolicyCitationDto {
            project_id: mission.project_id,
            policy_revision,
            charter_revision,
        });

        let ladder = self.proven_ladder(transaction, mission_id)?;
        let latest = ladder.last().map(view_of);

        // The cursor, in its own domain, stringified exactly. No lossy cast anywhere on this path.
        let head: i64 = transaction.query_row(
            "SELECT coalesce(max(sequence), 0) FROM events WHERE workspace_id = ?1",
            rusqlite::params![self.workspace_id.to_string()],
            |row| row.get(0),
        )?;
        let head = u64::try_from(head).map_err(|_| {
            unavailable(Unprovable::LadderUnprovable(format!(
                "chain head {head} is outside the sequence domain"
            )))
        })?;

        Ok(ClassificationObservation {
            mission_id,
            observed_at_event_seq: head.to_string(),
            published_policy,
            latest,
        })
    }

    /// Every classification of one Mission, each proven by its bytes AND by its owning Event, in
    /// proven-version order.
    ///
    /// Discovery selects candidates; it decides nothing. An unreadable row is `Unavailable`, never
    /// an empty ladder -- collapsing those two would let a corrupt latest row turn a
    /// reclassification into a silent "first" classification at the wrong version.
    fn proven_ladder(
        &self,
        transaction: &Transaction<'_>,
        mission_id: Uuid,
    ) -> Result<Vec<MissionImpactClassification>, ClassificationCommandError> {
        let candidates = {
            let mut statement = transaction.prepare(
                "SELECT digest_value, version, policy_revision, charter_revision, classified_at_ms
                 FROM mission_impact_classifications
                 WHERE workspace_id = ?1 AND mission_id = ?2",
            )?;
            statement
                .query_map(
                    rusqlite::params![self.workspace_id.to_string(), mission_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            [
                                row.get::<_, i64>(1)?,
                                row.get::<_, i64>(2)?,
                                row.get::<_, i64>(3)?,
                            ],
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        // The RAW stored domain of every column the shipped row loader narrows with an `as` cast
        // (`mission_impact.rs:764-769`), before that loader is asked anything. A projected column at
        // `value + 2^32` truncates back to the original and passes the column comparison, so the
        // narrowed value cannot be the thing that proves the domain (root's probe 3, which names the
        // two revision columns; `version` and `classified_at_ms` are the other two casts and are
        // checked here in the same place rather than left as a known sibling).
        for (digest, revisions, classified_at_ms) in &candidates {
            for (column, value) in ["version", "policy_revision", "charter_revision"]
                .into_iter()
                .zip(revisions)
            {
                if u32::try_from(*value)
                    .ok()
                    .filter(|proven| *proven >= 1)
                    .is_none()
                {
                    return Err(unavailable(Unprovable::LadderUnprovable(format!(
                        "{digest} carries {column} {value}"
                    ))));
                }
            }
            if u64::try_from(*classified_at_ms).is_err() {
                return Err(unavailable(Unprovable::LadderUnprovable(format!(
                    "{digest} carries classified_at_ms {classified_at_ms}"
                ))));
            }
        }

        let mut proven = Vec::with_capacity(candidates.len());
        for (digest, _, _) in &candidates {
            let record = verified_classification_within(transaction, self.workspace_id, digest)
                .map_err(|error| {
                    unavailable(Unprovable::LadderUnprovable(format!("{digest}: {error:?}")))
                })?
                .ok_or_else(|| {
                    unavailable(Unprovable::LadderUnprovable(format!("{digest} vanished")))
                })?;
            self.prove_owning_event(transaction, &record)?;
            proven.push(record);
        }

        // The OTHER direction, before anything treats this history as empty: an Event naming a
        // classification this store does not hold is a broken history, not an absent one.
        let known: BTreeSet<&str> = proven
            .iter()
            .map(|record| record.digest().value.as_str())
            .collect();
        let named = {
            // `$.mission_id`, NOT `$.missionId`: `MissionImpactClassifiedPayload` carries no
            // `rename_all` (`mission_impact.rs:1097-1099`), so its keys are the field names. The
            // camelCase spelling belongs to the completion payload next door, and using it here
            // matched nothing -- an Event left behind by a deleted row was invisible, and this
            // owner then recorded a fresh version 1 onto a broken history (root's probe 1).
            let mut statement = transaction.prepare(
                "SELECT json_extract(payload_json, '$.digest.value') FROM events
                 WHERE workspace_id = ?1 AND kind = 'mission.impact_classified'
                   AND json_extract(payload_json, '$.mission_id') = ?2",
            )?;
            statement
                .query_map(
                    rusqlite::params![self.workspace_id.to_string(), mission_id.to_string()],
                    |row| row.get::<_, Option<String>>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        for digest in named {
            let digest = digest.ok_or_else(|| {
                unavailable(Unprovable::OrphanClassificationEvent(
                    "no digest".to_owned(),
                ))
            })?;
            if !known.contains(digest.as_str()) {
                return Err(unavailable(Unprovable::OrphanClassificationEvent(digest)));
            }
        }

        // The order is the PROVEN payload's, not the column's.
        proven.sort_by_key(|record| record.payload().version);
        Ok(proven)
    }

    /// Exactly one `mission.impact_classified` Event, agreeing with the row's retained bytes and
    /// with every selector it projects.
    ///
    /// `verified_classification_within` proves the row against its own bytes and does not look at
    /// the log at all. This is the half it does not do, added at THIS command's perimeter only.
    fn prove_owning_event(
        &self,
        transaction: &Transaction<'_>,
        record: &MissionImpactClassification,
    ) -> Result<(), ClassificationCommandError> {
        let digest = record.digest().value.as_str();
        let unproven = |detail: String| {
            unavailable(Unprovable::ClassificationEventUnproven(format!(
                "{digest}: {detail}"
            )))
        };
        let payloads = {
            let mut statement = transaction.prepare(
                "SELECT sequence, occurred_at_ms, payload_json FROM events
                 WHERE workspace_id = ?1 AND kind = 'mission.impact_classified'
                   AND json_extract(payload_json, '$.digest.value') = ?2",
            )?;
            statement
                .query_map(
                    rusqlite::params![self.workspace_id.to_string(), digest],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        // Zero is a row the log does not account for; more than one is two claims about one record.
        let [(sequence, occurred_at_ms, payload_json)] = payloads.as_slice() else {
            return Err(unproven(format!("{} owning Events", payloads.len())));
        };
        // The Event's own columns, in their own domains, before anything is read out of them.
        if *sequence < 1 {
            return Err(unproven(format!("sequence {sequence}")));
        }
        let occurred_at_ms = u64::try_from(*occurred_at_ms)
            .map_err(|_| unproven(format!("occurred_at_ms {occurred_at_ms}")))?;
        let event: MissionImpactClassifiedPayload = serde_json::from_str(payload_json)
            .map_err(|error| unproven(format!("payload: {error}")))?;
        // The WHOLE content identity, not the hex alone: an algorithm or a canonicalizer that says
        // something else describes a different digest of the same bytes (root's probe 2).
        let claimed = record.digest();
        if (
            event.digest.algorithm.as_str(),
            event.digest.canonicalizer.as_str(),
            event.digest.value.as_str(),
        ) != (
            claimed.algorithm.as_str(),
            claimed.canonicalizer.as_str(),
            claimed.value.as_str(),
        ) {
            return Err(unproven("the digest triple disagrees".to_owned()));
        }
        let bytes = BASE64
            .decode(event.canonical_bytes_base64.as_bytes())
            .map_err(|error| unproven(format!("retained bytes: {error}")))?;
        if bytes != record.canonical_bytes() {
            return Err(unproven("retained bytes disagree".to_owned()));
        }
        let payload = record.payload();
        let observed = (
            event.workspace_id.as_str(),
            event.classification_id.as_str(),
            event.mission_id.as_str(),
            event.project_id.as_str(),
            event.version,
            event.impact.as_str(),
            event.basis.as_str(),
            event.policy_revision,
            event.charter_revision,
            event.classified_at_ms,
        );
        let workspace = payload.workspace_id.to_string();
        let classification = payload.classification_id.to_string();
        let mission = payload.mission_id.to_string();
        let project = payload.policy.project_id.to_string();
        let expected = (
            workspace.as_str(),
            classification.as_str(),
            mission.as_str(),
            project.as_str(),
            payload.version,
            payload.impact.as_str(),
            payload.basis.as_str(),
            payload.policy.policy_revision,
            payload.policy.charter_revision,
            payload.classified_at_ms,
        );
        if observed != expected {
            return Err(unproven("a selector disagrees".to_owned()));
        }
        // The writer appends the Event AT the record's own instant
        // (`mission_impact.rs:1283-1290`), so a log entry claiming another time is not this
        // record's owning Event.
        if occurred_at_ms != payload.classified_at_ms {
            return Err(unproven(format!("occurred_at_ms {occurred_at_ms}")));
        }
        Ok(())
    }

    /// Initial or reclassification, decided only from PROVEN records.
    fn ladder_position(
        &self,
        named_predecessor: Option<String>,
        ladder: &[MissionImpactClassification],
    ) -> Result<(u32, ClassificationSupersedes), ClassificationCommandError> {
        match (named_predecessor, ladder.last()) {
            (None, None) => Ok((1, ClassificationSupersedes::None)),
            (None, Some(_)) => Err(ClassificationCommandError::AlreadyClassified),
            (Some(_), None) => Err(ClassificationCommandError::PredecessorUnproven),
            (Some(named), Some(latest)) => {
                if !ladder.iter().any(|record| record.digest().value == named) {
                    return Err(ClassificationCommandError::PredecessorUnproven);
                }
                if latest.digest().value != named {
                    return Err(ClassificationCommandError::PredecessorNotLatest);
                }
                // `checked_add`: wrapping would produce version 0, which no legitimate record has
                // and which sorts below the whole ladder.
                let next = latest
                    .payload()
                    .version
                    .checked_add(1)
                    .ok_or(ClassificationCommandError::VersionCeilingReached)?;
                Ok((next, ClassificationSupersedes::Record(named)))
            }
        }
    }
}

use ClassificationCommandUnavailable as Unprovable;

fn unavailable(cause: ClassificationCommandUnavailable) -> ClassificationCommandError {
    ClassificationCommandError::Unavailable(cause)
}

fn malformed(field: &'static str) -> ClassificationCommandError {
    ClassificationCommandError::MalformedRequest(field)
}

fn impact_from(token: &str) -> Result<ImpactLevel, ClassificationCommandError> {
    match token {
        "high" => Ok(ImpactLevel::High),
        "ordinary" => Ok(ImpactLevel::Ordinary),
        // No third arm and no default: ADR 0044 refuses `unclassified`, and a default-to-ordinary
        // would be omission achieving what removing `omit` from the appendix API prevented.
        _ => Err(malformed("impact")),
    }
}

fn digest64(value: &str, field: &'static str) -> Result<String, ClassificationCommandError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(malformed(field));
    }
    Ok(value.to_owned())
}

/// The codec's refusal, projected into this owner's small truthful set.
///
/// Every arm is the codec's own class, and only the genuinely length-shaped one reaches a
/// length-shaped caller line.
fn codec_refusal(refusal: MalformedClassification) -> ClassificationCommandError {
    match refusal {
        MalformedClassification::TextTooLong => ClassificationCommandError::TextOverLimit,
        MalformedClassification::EmptyText => malformed("reason.empty"),
        MalformedClassification::UntrimmedText => malformed("reason.untrimmed"),
        MalformedClassification::ControlCharacter => malformed("reason.control"),
        MalformedClassification::SupersedesDigest => malformed("supersedes.predecessorDigest"),
        MalformedClassification::Revision => malformed("policy.revision"),
        MalformedClassification::RuleId | MalformedClassification::RuleIdBasisMismatch => {
            malformed("basis.ruleId")
        }
        MalformedClassification::OverLimitLength => malformed("record.overLimit"),
        MalformedClassification::RecordTooLarge => malformed("record.tooLarge"),
        MalformedClassification::Magic
        | MalformedClassification::Truncated
        | MalformedClassification::TrailingBytes
        | MalformedClassification::UnknownTag
        | MalformedClassification::InvalidUtf8 => malformed("canonical_bytes"),
    }
}

/// The store writer's refusal, projected. Its account arm keeps its own meaning; its malformed arm
/// goes back through the codec projection so one class cannot get two caller lines.
fn write_refusal(refusal: ClassificationRecordError) -> ClassificationCommandError {
    match refusal {
        ClassificationRecordError::Account(error) => ClassificationCommandError::Account(error),
        ClassificationRecordError::Malformed(class) => codec_refusal(class),
        other => unavailable(Unprovable::Write(format!("{other:?}"))),
    }
}

fn view_of(record: &MissionImpactClassification) -> ClassificationView {
    let payload = record.payload();
    ClassificationView {
        mission_id: payload.mission_id,
        classification_id: payload.classification_id,
        version: payload.version,
        impact: payload.impact.as_str().to_owned(),
        // The ACTUAL stored basis and rule identity. A legacy `policy_rule` record keeps both.
        basis: payload.basis.as_str().to_owned(),
        rule_id: payload.rule_id.clone(),
        reason: payload.reason.clone(),
        policy: PolicyCitationDto {
            project_id: payload.policy.project_id,
            policy_revision: payload.policy.policy_revision,
            charter_revision: payload.policy.charter_revision,
        },
        classified_by: match payload.classified_by {
            ClassificationActor::Operator => "operator".to_owned(),
        },
        classified_at_ms: payload.classified_at_ms,
        supersedes: match &payload.supersedes {
            ClassificationSupersedes::None => SupersedesDto::None,
            ClassificationSupersedes::Record(digest) => SupersedesDto::Record {
                predecessor_digest: digest.clone(),
            },
        },
        digest: ClassificationDigestDto {
            algorithm: record.digest().algorithm.clone(),
            canonicalizer: record.digest().canonicalizer.clone(),
            value: record.digest().value.clone(),
        },
    }
}
