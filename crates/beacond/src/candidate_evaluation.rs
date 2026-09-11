//! Read-only historical context for a proposed Outcome-fidelity Evaluation.
//!
//! This context proves the prepared candidate and its admitted non-author Residents. Current
//! eligibility is not measured and the full frozen source packet is not collected. No value here
//! permits a request, invokes a model, records an Evaluation, or changes any domain record.

use std::fmt;
use std::sync::Arc;

use rusqlite::TransactionBehavior;
use uuid::Uuid;

use crate::artifact::{
    ArtifactContext, ArtifactError, VerifiedArtifactRecord, artifact_within_context_read_limits,
};
use crate::evaluation::Evaluator;
use crate::mission::{
    MissionError, MissionView, mission_within_context_read_limits, verified_mission_admission,
};
use crate::operator_account::{OperatorAccountError, OperatorAccountService};
use crate::outcome::SourceDigest;
use crate::outcome_store::{
    CandidateContextReadPreflight, ManifestStoreRefusal, StoredOutcomeManifest,
    candidate_context_read_preflight, verified_candidate_context_within,
};

use crate::local_model_activation::verified_activation_chain_within;
use crate::resident_configuration::{
    configuration_within_context_read_limits, verified_current_configuration_within,
};
use crate::resident_lifecycle::{
    lifecycle_within_context_read_limits, verified_resident_lifecycle_within,
};
use crate::{
    BeaconCapability, LocalModelActivation, LocalModelActivationError, LocalModelActivationState,
    ResidentAgentView, ResidentConfiguration, ResidentConfigurationError,
    ResidentConfigurationState, ResidentLifecycle, ResidentLifecycleError,
};

const RUBRIC_BYTES: &[u8] = include_bytes!("../resources/evaluation/outcome-fidelity-v1.txt");

/// Historical admission and current execution eligibility are different evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentEvaluationEligibility {
    NotMeasured,
}

/// The candidate's own document is not the full frozen source packet needed for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationSourcePacket {
    NotCollected,
}

/// Closed factual mismatches in stable order. An empty set is no availability or authority verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurrentEvaluatorGap {
    ResidentNotActive,
    ConfigurationCleared,
    ConfigurationChanged,
    MissingArtifactEvaluate,
    ActivationSuperseded,
    ActivationInactive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentActivationEvidence {
    configured: LocalModelActivation,
    latest_for_resource: LocalModelActivation,
}
impl CurrentActivationEvidence {
    pub fn configured(&self) -> &LocalModelActivation {
        &self.configured
    }
    pub fn latest_for_resource(&self) -> &LocalModelActivation {
        &self.latest_for_resource
    }
}

/// Current owner records beside the frozen admitted identity, never permission to request work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentEvaluatorEvidence {
    admitted: Evaluator,
    resident: ResidentAgentView,
    current_configuration: ResidentConfiguration,
    activation: Option<CurrentActivationEvidence>,
    gaps: Vec<CurrentEvaluatorGap>,
}
impl CurrentEvaluatorEvidence {
    pub fn admitted(&self) -> &Evaluator {
        &self.admitted
    }
    pub fn resident(&self) -> &ResidentAgentView {
        &self.resident
    }
    pub fn current_configuration(&self) -> &ResidentConfiguration {
        &self.current_configuration
    }
    pub fn activation(&self) -> Option<&CurrentActivationEvidence> {
        self.activation.as_ref()
    }
    pub fn gaps(&self) -> &[CurrentEvaluatorGap] {
        &self.gaps
    }
}

/// Maintainer-authored draft criteria, supplied as a proposal rather than accepted product Policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedOutcomeFidelityRubric {
    digest: SourceDigest,
}

impl ProposedOutcomeFidelityRubric {
    pub fn id(&self) -> &'static str {
        "outcome-fidelity-draft"
    }

    pub fn version(&self) -> u32 {
        1
    }

    pub fn bytes(&self) -> &'static [u8] {
        RUBRIC_BYTES
    }

    pub fn digest(&self) -> &SourceDigest {
        &self.digest
    }
}

/// Exact owning records in one snapshot. This is no request-readiness or authority verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateEvaluationContext {
    manifest: StoredOutcomeManifest,
    candidate: VerifiedArtifactRecord,
    mission_admission: MissionView,
    admitted_non_author_residents: Vec<Evaluator>,
    current_evaluator_evidence: Vec<CurrentEvaluatorEvidence>,
    rubric: ProposedOutcomeFidelityRubric,
}

impl CandidateEvaluationContext {
    pub fn manifest(&self) -> &StoredOutcomeManifest {
        &self.manifest
    }

    pub fn candidate(&self) -> &VerifiedArtifactRecord {
        &self.candidate
    }

    pub fn mission_admission(&self) -> &MissionView {
        &self.mission_admission
    }

    /// The exact historical admission configurations, excluding the accountable author by Resident
    /// identity. An empty set means that this Mission admitted no other Resident, not absent data.
    pub fn admitted_non_author_residents(&self) -> &[Evaluator] {
        &self.admitted_non_author_residents
    }

    pub fn current_evaluator_evidence(&self) -> &[CurrentEvaluatorEvidence] {
        &self.current_evaluator_evidence
    }

    pub fn current_eligibility(&self) -> CurrentEvaluationEligibility {
        CurrentEvaluationEligibility::NotMeasured
    }

    pub fn source_packet(&self) -> EvaluationSourcePacket {
        EvaluationSourcePacket::NotCollected
    }

    pub fn rubric(&self) -> &ProposedOutcomeFidelityRubric {
        &self.rubric
    }
}

/// Fixed outward names; nested owner/driver details remain in Debug and source only.
#[derive(Debug)]
pub enum CandidateEvaluationError {
    Account(OperatorAccountError),
    InvalidManifestDigest,
    ReadLimit,
    Artifact(ArtifactError),
    Manifest(ManifestStoreRefusal),
    Mission(MissionError),
    Resident(ResidentLifecycleError),
    Configuration(ResidentConfigurationError),
    Activation(LocalModelActivationError),
    MissionAbsent,
    ScopeMismatch,
    Store(rusqlite::Error),
}

impl CandidateEvaluationError {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::InvalidManifestDigest => "InvalidManifestDigest",
            Self::ReadLimit => "ReadLimit",
            Self::Artifact(_) => "Artifact",
            Self::Manifest(_) => "Manifest",
            Self::Mission(_) => "Mission",
            Self::Resident(_) => "Resident",
            Self::Configuration(_) => "Configuration",
            Self::Activation(_) => "Activation",
            Self::MissionAbsent => "MissionAbsent",
            Self::ScopeMismatch => "ScopeMismatch",
            Self::Store(_) => "Store",
        }
    }
}

impl fmt::Display for CandidateEvaluationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl std::error::Error for CandidateEvaluationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Account(error) => Some(error),
            Self::Artifact(error) => Some(error),
            Self::Manifest(error) => Some(error),
            Self::Mission(error) => Some(error),
            Self::Resident(error) => Some(error),
            Self::Configuration(error) => Some(error),
            Self::Activation(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::InvalidManifestDigest
            | Self::ReadLimit
            | Self::MissionAbsent
            | Self::ScopeMismatch => None,
        }
    }
}

impl From<OperatorAccountError> for CandidateEvaluationError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for CandidateEvaluationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

pub struct CandidateEvaluationService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl CandidateEvaluationService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Read the held candidate in one authenticated snapshot. Absent and foreign selectors have
    /// the same answer; malformed selectors and broken owner evidence never become absence.
    pub fn context(
        &self,
        manifest_digest: &str,
    ) -> Result<Option<CandidateEvaluationContext>, CandidateEvaluationError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            if manifest_digest.len() != 64
                || !manifest_digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(CandidateEvaluationError::InvalidManifestDigest);
            }
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            match candidate_context_read_preflight(&transaction, self.workspace_id, manifest_digest)
                .map_err(CandidateEvaluationError::Manifest)?
            {
                CandidateContextReadPreflight::Absent => return Ok(None),
                CandidateContextReadPreflight::OverLimit => return Err(CandidateEvaluationError::ReadLimit),
                CandidateContextReadPreflight::Keys { candidate_digest, mission_id } => {
                    if !artifact_within_context_read_limits(&transaction, self.workspace_id, &candidate_digest)
                        .map_err(CandidateEvaluationError::Artifact)?
                        || !mission_within_context_read_limits(&transaction, self.workspace_id, mission_id)
                            .map_err(CandidateEvaluationError::Mission)?
                    {
                        return Err(CandidateEvaluationError::ReadLimit);
                    }
                }
            }
            let (manifest, candidate) = match verified_candidate_context_within(
                &transaction,
                self.workspace_id,
                manifest_digest,
            ) {
                Ok(records) => records,
                Err(ManifestStoreRefusal::Absent | ManifestStoreRefusal::CrossWorkspace) => {
                    return Ok(None);
                }
                Err(error) => return Err(CandidateEvaluationError::Manifest(error)),
            };
            let mission_admission = verified_mission_admission(
                &transaction,
                self.workspace_id,
                manifest.manifest().mission_id,
            )
            .map_err(CandidateEvaluationError::Mission)?
            .ok_or(CandidateEvaluationError::MissionAbsent)?;
            if !matches!(candidate.payload().context,
                ArtifactContext::Mission { project_id, mission_id }
                    if project_id == mission_admission.project_id && mission_id == mission_admission.id)
            {
                return Err(CandidateEvaluationError::ScopeMismatch);
            }
            let admitted_non_author_residents: Vec<Evaluator> = mission_admission
                .members
                .iter()
                .filter(|member| member.resident_id != candidate.payload().accountable_resident_id)
                .map(|member| Evaluator {
                    resident_id: member.resident_id,
                    configuration_id: member.admission_configuration_id,
                })
                .collect();
            let mut current_evaluator_evidence = Vec::new();
            for admitted in &admitted_non_author_residents {
                if !lifecycle_within_context_read_limits(&transaction, self.workspace_id, admitted.resident_id)
                    .map_err(CandidateEvaluationError::Resident)?
                    || !configuration_within_context_read_limits(&transaction, self.workspace_id, admitted.resident_id)
                        .map_err(CandidateEvaluationError::Configuration)? {
                    return Err(CandidateEvaluationError::ReadLimit);
                }
                let resident = verified_resident_lifecycle_within(
                    &transaction, self.workspace_id, admitted.resident_id,
                ).map_err(CandidateEvaluationError::Resident)?;
                let current_configuration = verified_current_configuration_within(
                    &transaction, self.workspace_id, admitted.resident_id,
                ).map_err(CandidateEvaluationError::Configuration)?;
                let mut gaps = Vec::new();
                if resident.lifecycle != ResidentLifecycle::Active {
                    gaps.push(CurrentEvaluatorGap::ResidentNotActive);
                }
                if current_configuration.state == ResidentConfigurationState::Cleared {
                    gaps.push(CurrentEvaluatorGap::ConfigurationCleared);
                }
                if current_configuration.id != admitted.configuration_id {
                    gaps.push(CurrentEvaluatorGap::ConfigurationChanged);
                }
                if !current_configuration.binding.as_ref().is_some_and(|binding|
                    binding.capabilities.contains(&BeaconCapability::ArtifactEvaluate)) {
                    gaps.push(CurrentEvaluatorGap::MissingArtifactEvaluate);
                }
                let activation = if let Some(binding) = &current_configuration.binding {
                    let (configured, latest_for_resource) = verified_activation_chain_within(
                        &transaction, self.workspace_id, binding.activation_id,
                    ).map_err(CandidateEvaluationError::Activation)?;
                    if configured.id != latest_for_resource.id {
                        gaps.push(CurrentEvaluatorGap::ActivationSuperseded);
                    }
                    if latest_for_resource.state == LocalModelActivationState::Inactive {
                        gaps.push(CurrentEvaluatorGap::ActivationInactive);
                    }
                    Some(CurrentActivationEvidence { configured, latest_for_resource })
                } else { None };
                current_evaluator_evidence.push(CurrentEvaluatorEvidence {
                    admitted: *admitted, resident, current_configuration, activation, gaps,
                });
            }
            let rubric = ProposedOutcomeFidelityRubric {
                digest: SourceDigest {
                    algorithm: "sha256".to_owned(),
                    canonicalizer: "beacon.raw.v1".to_owned(),
                    value: data_encoding::HEXLOWER.encode(&crate::proposal::sha256(RUBRIC_BYTES)),
                },
            };
            transaction.commit()?;
            Ok(Some(CandidateEvaluationContext {
                manifest,
                candidate,
                mission_admission,
                admitted_non_author_residents,
                current_evaluator_evidence,
                rubric,
            }))
        })
    }
}
