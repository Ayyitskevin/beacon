#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use beacond::{
    AccessOrigin, BeaconCapability, BeaconCapabilityDescriptor, ExportedOutcome,
    LiveDataEncryptionReport, LocalErrorCode, LocalModelActivation, LocalModelActivationReview,
    LocalModelDiscoveryReport, LocalSupervisorClient, LocalTransportError, MissionDraft,
    MissionView, OperatorSessionPolicy, OperatorSessionStatus, PlatformSessionMonitorStatus,
    PreflightReceipt, ProjectCharter, ProjectCharterDraft, ProjectDraft, ProjectPolicy,
    ProjectPolicyDraft, ProjectRepository, ProjectSummary, ProjectView, ProviderFeature,
    PublicationSummary, RecentReceiptPage, RepositoryDraft, ResidentAgentView,
    ResidentConfiguration, SampleProjectInstallResult, SampleProjectStatus, SecretString,
    StarterOrganizationView, StarterResidentDraft, TeamDraft, TeamView,
    initialize_local_auth_token,
};
use serde::Serialize;
use tauri::{Manager, State};

struct DesktopState {
    supervisor: LocalSupervisorClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PreferredMethod {
    Passkey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PasskeyUnavailableReason {
    UnsupportedLinuxWebkitgtk,
    NativeAdapterNotAvailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PasskeyCapability {
    available: bool,
    reason: PasskeyUnavailableReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PasswordScope {
    SameMachine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PasswordFallbackCapability {
    available: bool,
    scope: PasswordScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticationCapabilities {
    preferred_method: PreferredMethod,
    passkey: PasskeyCapability,
    password_fallback: PasswordFallbackCapability,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryKeyOutput {
    recovery_key: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandError {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_ms: Option<u64>,
}

impl From<LocalTransportError> for CommandError {
    fn from(error: LocalTransportError) -> Self {
        let (code, message, retry_after_ms) = match error {
            LocalTransportError::Remote {
                code: LocalErrorCode::PasswordTooShort,
                ..
            } => ("password_too_short", "Use at least 15 characters.", None),
            LocalTransportError::Remote {
                code: LocalErrorCode::PasswordTooLong,
                ..
            } => (
                "password_too_long",
                "Use no more than 1,024 characters.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::PasswordConfirmationMismatch,
                ..
            } => (
                "password_confirmation_mismatch",
                "The password confirmation does not match.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::RecoveryNotConfirmed,
                ..
            } => (
                "recovery_not_confirmed",
                "Confirm that the Recovery Key was saved offline.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::AuthenticationFailed,
                ..
            } => (
                "authentication_failed",
                "That credential was not accepted.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::AuthenticationThrottled,
                retry_after_ms,
            } => (
                "authentication_throttled",
                "Wait before trying again.",
                retry_after_ms,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::CredentialDisabled,
                ..
            } => (
                "credential_disabled",
                "Use the other local credential to recover this account.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::InvalidSessionTimeout,
                ..
            } => (
                "invalid_session_timeout",
                "Choose a timeout from 1 to 1,440 minutes.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::UnsupportedStarterProposal,
                ..
            } => (
                "unsupported_starter_proposal",
                "Reload the current Starter Organization proposal.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::InvalidStarterRoster,
                ..
            } => (
                "invalid_starter_roster",
                "Select each role once and use Display Names from 1 to 80 characters.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::StarterDecisionRecorded,
                ..
            } => (
                "starter_decision_recorded",
                "The Starter Organization was already decided.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::StarterOrganizationUnavailable,
                ..
            } => (
                "starter_organization_unavailable",
                "The Starter Organization state could not be verified.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::ResidentNotFound,
                ..
            } => (
                "resident_not_found",
                "That Resident is no longer available in this Workspace.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::InvalidResidentTransition,
                ..
            } => (
                "invalid_resident_transition",
                "That Resident lifecycle action is not available from the current state.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::StaleResidentDecision,
                ..
            } => (
                "stale_resident_decision",
                "The Resident changed. Reload the roster before trying again.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::ResidentLifecycleUnavailable,
                ..
            } => (
                "resident_lifecycle_unavailable",
                "Beacon could not verify Resident lifecycle state.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::InvalidResidentConfiguration,
                ..
            } => (
                "invalid_resident_configuration",
                "Review the Resident, active model, completion approval, and Capability selection.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::StaleResidentConfiguration,
                ..
            } => (
                "stale_resident_configuration",
                "The Resident configuration changed. Reload it before trying again.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::ResidentConfigurationUnavailable,
                ..
            } => (
                "resident_configuration_unavailable",
                "Beacon could not verify Resident configuration state.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::InvalidProject,
                ..
            } => (
                "invalid_project",
                "Review the Project name and every Charter field.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::ProjectNotFound,
                ..
            } => (
                "project_not_found",
                "That Project is no longer available in this Workspace.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::StaleCharterRevision,
                ..
            } => (
                "stale_charter_revision",
                "Reload the current Charter before publishing a new revision.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::UnchangedCharter,
                ..
            } => (
                "unchanged_charter",
                "Change at least one Charter field before publishing a revision.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::ProjectUnavailable,
                ..
            } => (
                "project_unavailable",
                "Beacon could not verify the Project state.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::InvalidProjectPolicy,
                ..
            } => (
                "invalid_project_policy",
                "Review the Lead capabilities and every Track Record evidence threshold.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::StaleProjectPolicy,
                ..
            } => (
                "stale_project_policy",
                "Reload the current Policy revision before publishing a new one.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::UnchangedProjectPolicy,
                ..
            } => (
                "unchanged_project_policy",
                "Change at least one Team Lead selection criterion before publishing.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::ProjectPolicyUnavailable,
                ..
            } => (
                "project_policy_unavailable",
                "Beacon could not verify the Project Policy state.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::InvalidProjectRepository,
                ..
            } => (
                "invalid_project_repository",
                "Review the repository source, the 40-character base revision, and every declared check.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::StaleProjectRepository,
                ..
            } => (
                "stale_project_repository",
                "Reload the current Repository revision before binding a new one.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::UnchangedProjectRepository,
                ..
            } => (
                "unchanged_project_repository",
                "Change the source, the base revision, or a declared check before binding.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::ProjectRepositoryUnavailable,
                ..
            } => (
                "project_repository_unavailable",
                "Beacon could not verify the Project Repository state.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::InvalidTeam,
                ..
            } => (
                "invalid_team",
                "Choose a Project, a Team name, and distinct eligible Residents.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::TeamNotFound,
                ..
            } => (
                "team_not_found",
                "That Team is no longer available in this Workspace.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::StaleTeamMemberConfiguration,
                ..
            } => (
                "stale_team_member_configuration",
                "A Resident configuration changed. Reload the Team form before trying again.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::TeamMemberIneligible,
                ..
            } => (
                "team_member_ineligible",
                "A selected Resident is no longer eligible for Team membership.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::TeamUnavailable,
                ..
            } => (
                "team_unavailable",
                "Beacon could not verify Team state.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::LocalModelUnavailable,
                ..
            } => (
                "local_model_unavailable",
                "The local model provider could not be verified.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::LocalModelNotFound,
                ..
            } => (
                "local_model_not_found",
                "That exact local model is no longer installed.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::LocalModelChanged,
                ..
            } => (
                "local_model_changed",
                "The local model changed. Review it again before activation.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::InvalidLocalModelApproval,
                ..
            } => (
                "invalid_local_model_approval",
                "Select at least one advertised feature and review the model again.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::LocalModelAlreadyActive,
                ..
            } => (
                "local_model_already_active",
                "This local model is already active in the Workspace.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::StaleLocalModelDecision,
                ..
            } => (
                "stale_local_model_decision",
                "The activation changed. Reload it before deactivating.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::LocalModelActivationUnavailable,
                ..
            } => (
                "local_model_activation_unavailable",
                "Beacon could not verify local-model activation state.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::RepositoryNotBound,
                ..
            } => (
                "repository_not_bound",
                "Bind a repository to this Project before preflighting its checks.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::PreflightUnavailable,
                ..
            } => (
                "preflight_unavailable",
                "Beacon could not record the preflight.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::SampleProjectUnavailable,
                ..
            } => (
                "sample_project_unavailable",
                "Beacon refused the Sample Project installation. No Project was published.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::InvalidMission,
                ..
            } => (
                "invalid_mission",
                "Review the Mission outcome, success conditions, Team, and exact member evidence.",
                None,
            ),
            // A locked session is not an unavailable service, and saying so was the difference
            // between an operator re-authenticating and an operator filing a bug. Split out of the
            // group below for the reason the Policy and Mission arms record: each refusal must
            // carry its own code, or the matching `case` in session.ts is unreachable.
            LocalTransportError::Remote {
                code: LocalErrorCode::NotAuthenticated,
                ..
            } => ("not_authenticated", "Unlock Beacon to continue.", None),
            LocalTransportError::Remote {
                code: LocalErrorCode::OutcomeAbsent,
                ..
            } => (
                "outcome_absent",
                "This Mission has no published Outcome Document.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::OutcomeUnavailable,
                ..
            } => (
                "outcome_unverified",
                "Beacon could not verify this Mission's Outcome Document.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::MissionNotFound,
                ..
            } => (
                "mission_not_found",
                "That Mission is no longer available in this Workspace.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::StaleMissionTeam,
                ..
            } => (
                "stale_mission_team",
                "The Team or a Resident configuration changed. Reload before admitting the Mission.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::MissionMemberIneligible,
                ..
            } => (
                "mission_member_ineligible",
                "A Team member is no longer eligible for Mission admission.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::MissionUnavailable,
                ..
            } => (
                "mission_unavailable",
                "Beacon could not verify Mission state.",
                None,
            ),
            // Declared, not omitted. These codes have no `case` in session.ts
            // today, so they surface under the generic message by decision.
            // Listing them here instead of `_` means a new `LocalErrorCode`
            // variant fails this build until someone either gives it a message
            // or adds it to this list. The Policy and Mission tables both
            // shipped with unreachable `case` strings because a wildcard hid
            // the gap; the compiler is the guard now.
            LocalTransportError::Remote {
                code:
                    LocalErrorCode::SetupNotStarted
                    | LocalErrorCode::AccountAlreadyExists
                    | LocalErrorCode::AccountMissing
                    | LocalErrorCode::RemotePasswordNotAllowed
                    | LocalErrorCode::AccountServiceUnavailable
                    | LocalErrorCode::UnauthorizedClient
                    | LocalErrorCode::ProtocolError,
                ..
            }
            | LocalTransportError::Io(_)
            | LocalTransportError::Json(_)
            | LocalTransportError::InvalidTokenFile
            | LocalTransportError::InsecurePermissions
            | LocalTransportError::Protocol
            | LocalTransportError::Supervisor(_)
            | LocalTransportError::Account(_)
            | LocalTransportError::StarterOrganization(_)
            | LocalTransportError::ResidentLifecycle(_)
            | LocalTransportError::ResidentConfiguration(_)
            | LocalTransportError::Project(_)
            | LocalTransportError::ProjectPolicy(_)
            | LocalTransportError::ProjectRepository(_)
            | LocalTransportError::Team(_)
            | LocalTransportError::Mission(_)
            | LocalTransportError::CheckPreflight(_)
            | LocalTransportError::RecentReceipt(_)
            | LocalTransportError::SampleProject(_)
            | LocalTransportError::ResourceDiscovery(_)
            | LocalTransportError::LocalModelActivation(_)
            | LocalTransportError::Outcome(_) => (
                "account_service_unavailable",
                "The local account service is unavailable.",
                None,
            ),
        };
        Self {
            code,
            message,
            retry_after_ms,
        }
    }
}

fn authentication_capabilities_value() -> AuthenticationCapabilities {
    AuthenticationCapabilities {
        preferred_method: PreferredMethod::Passkey,
        passkey: PasskeyCapability {
            available: false,
            reason: if cfg!(target_os = "linux") {
                PasskeyUnavailableReason::UnsupportedLinuxWebkitgtk
            } else {
                PasskeyUnavailableReason::NativeAdapterNotAvailable
            },
        },
        password_fallback: PasswordFallbackCapability {
            available: true,
            scope: PasswordScope::SameMachine,
        },
    }
}

fn session_status_value(state: &DesktopState) -> Result<OperatorSessionStatus, CommandError> {
    state.supervisor.status().map_err(Into::into)
}

fn session_policy_value(state: &DesktopState) -> Result<OperatorSessionPolicy, CommandError> {
    state.supervisor.session_policy().map_err(Into::into)
}

fn set_session_timeout_value(
    state: &DesktopState,
    inactivity_timeout_minutes: u16,
) -> Result<OperatorSessionPolicy, CommandError> {
    state
        .supervisor
        .set_session_timeout_minutes(inactivity_timeout_minutes)
        .map_err(Into::into)
}

fn platform_session_monitor_status_value(
    state: &DesktopState,
) -> Result<PlatformSessionMonitorStatus, CommandError> {
    state
        .supervisor
        .platform_session_monitor_status()
        .map_err(Into::into)
}

fn begin_password_setup_value(
    state: &DesktopState,
    password: SecretString,
    confirmation: SecretString,
) -> Result<RecoveryKeyOutput, CommandError> {
    state
        .supervisor
        .begin_password_setup(password, confirmation)
        .map(|recovery_key| RecoveryKeyOutput {
            recovery_key: recovery_key.into_string(),
        })
        .map_err(Into::into)
}

fn complete_operator_setup_value(
    state: &DesktopState,
    recovery_saved: bool,
) -> Result<OperatorSessionStatus, CommandError> {
    state
        .supervisor
        .complete_operator_setup(recovery_saved)
        .map_err(Into::into)
}

fn starter_organization_value(
    state: &DesktopState,
) -> Result<StarterOrganizationView, CommandError> {
    state.supervisor.starter_organization().map_err(Into::into)
}

fn accept_starter_organization_value(
    state: &DesktopState,
    proposal_version: u16,
    residents: Vec<StarterResidentDraft>,
) -> Result<StarterOrganizationView, CommandError> {
    state
        .supervisor
        .accept_starter_organization(proposal_version, residents)
        .map_err(Into::into)
}

fn discard_starter_organization_value(
    state: &DesktopState,
    proposal_version: u16,
) -> Result<StarterOrganizationView, CommandError> {
    state
        .supervisor
        .discard_starter_organization(proposal_version)
        .map_err(Into::into)
}

fn residents_value(state: &DesktopState) -> Result<Vec<ResidentAgentView>, CommandError> {
    state.supervisor.residents().map_err(Into::into)
}

fn activate_resident_value(
    state: &DesktopState,
    resident_id: uuid::Uuid,
    expected_decision_id: Option<uuid::Uuid>,
) -> Result<ResidentAgentView, CommandError> {
    state
        .supervisor
        .activate_resident(resident_id, expected_decision_id)
        .map_err(Into::into)
}

fn suspend_resident_value(
    state: &DesktopState,
    resident_id: uuid::Uuid,
    expected_decision_id: Option<uuid::Uuid>,
) -> Result<ResidentAgentView, CommandError> {
    state
        .supervisor
        .suspend_resident(resident_id, expected_decision_id)
        .map_err(Into::into)
}

fn reactivate_resident_value(
    state: &DesktopState,
    resident_id: uuid::Uuid,
    expected_decision_id: Option<uuid::Uuid>,
) -> Result<ResidentAgentView, CommandError> {
    state
        .supervisor
        .reactivate_resident(resident_id, expected_decision_id)
        .map_err(Into::into)
}

fn retire_resident_value(
    state: &DesktopState,
    resident_id: uuid::Uuid,
    expected_decision_id: Option<uuid::Uuid>,
) -> Result<ResidentAgentView, CommandError> {
    state
        .supervisor
        .retire_resident(resident_id, expected_decision_id)
        .map_err(Into::into)
}

fn resident_capability_catalog_value(
    state: &DesktopState,
) -> Result<Vec<BeaconCapabilityDescriptor>, CommandError> {
    state
        .supervisor
        .resident_capability_catalog()
        .map_err(Into::into)
}

fn resident_configurations_value(
    state: &DesktopState,
) -> Result<Vec<ResidentConfiguration>, CommandError> {
    state
        .supervisor
        .resident_configurations()
        .map_err(Into::into)
}

fn configure_resident_value(
    state: &DesktopState,
    resident_id: uuid::Uuid,
    expected_decision_id: Option<uuid::Uuid>,
    activation_id: uuid::Uuid,
    capabilities: Vec<BeaconCapability>,
) -> Result<ResidentConfiguration, CommandError> {
    state
        .supervisor
        .configure_resident(
            resident_id,
            expected_decision_id,
            activation_id,
            capabilities,
        )
        .map_err(Into::into)
}

fn clear_resident_configuration_value(
    state: &DesktopState,
    resident_id: uuid::Uuid,
    expected_decision_id: uuid::Uuid,
) -> Result<ResidentConfiguration, CommandError> {
    state
        .supervisor
        .clear_resident_configuration(resident_id, expected_decision_id)
        .map_err(Into::into)
}

fn projects_value(state: &DesktopState) -> Result<Vec<ProjectSummary>, CommandError> {
    state.supervisor.projects().map_err(Into::into)
}

fn sample_project_status_value(state: &DesktopState) -> Result<SampleProjectStatus, CommandError> {
    state.supervisor.sample_project_status().map_err(Into::into)
}

fn install_sample_project_value(
    state: &DesktopState,
) -> Result<SampleProjectInstallResult, CommandError> {
    state
        .supervisor
        .install_sample_project()
        .map_err(Into::into)
}

fn project_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
) -> Result<ProjectView, CommandError> {
    state.supervisor.project(project_id).map_err(Into::into)
}

fn project_charter_history_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
) -> Result<Vec<ProjectCharter>, CommandError> {
    state
        .supervisor
        .project_charter_history(project_id)
        .map_err(Into::into)
}

fn create_project_value(
    state: &DesktopState,
    draft: ProjectDraft,
) -> Result<ProjectView, CommandError> {
    state.supervisor.create_project(draft).map_err(Into::into)
}

fn revise_project_charter_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
    expected_current_revision: u32,
    draft: ProjectCharterDraft,
) -> Result<ProjectView, CommandError> {
    state
        .supervisor
        .revise_project_charter(project_id, expected_current_revision, draft)
        .map_err(Into::into)
}

fn project_policy_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
) -> Result<Option<ProjectPolicy>, CommandError> {
    state
        .supervisor
        .project_policy(project_id)
        .map_err(Into::into)
}

fn project_policy_history_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
) -> Result<Vec<ProjectPolicy>, CommandError> {
    state
        .supervisor
        .project_policy_history(project_id)
        .map_err(Into::into)
}

fn mission_publications_value(
    state: &DesktopState,
    mission_id: uuid::Uuid,
) -> Result<Vec<PublicationSummary>, CommandError> {
    state
        .supervisor
        .mission_publications(mission_id)
        .map_err(Into::into)
}

fn outcome_document_value(
    state: &DesktopState,
    mission_id: uuid::Uuid,
    publication_version: Option<u32>,
) -> Result<ExportedOutcome, CommandError> {
    state
        .supervisor
        .outcome_document(mission_id, publication_version)
        .map_err(Into::into)
}

fn publish_project_policy_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
    expected_charter_revision: u32,
    expected_policy_revision: Option<u32>,
    draft: ProjectPolicyDraft,
) -> Result<ProjectPolicy, CommandError> {
    state
        .supervisor
        .publish_project_policy(
            project_id,
            expected_charter_revision,
            expected_policy_revision,
            draft,
        )
        .map_err(Into::into)
}

fn current_project_repository_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
) -> Result<Option<ProjectRepository>, CommandError> {
    state
        .supervisor
        .current_project_repository(project_id)
        .map_err(Into::into)
}

fn project_repository_history_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
) -> Result<Vec<ProjectRepository>, CommandError> {
    state
        .supervisor
        .project_repository_history(project_id)
        .map_err(Into::into)
}

fn bind_project_repository_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
    expected_charter_revision: u32,
    expected_prior_repository_revision: Option<u32>,
    draft: RepositoryDraft,
) -> Result<ProjectRepository, CommandError> {
    state
        .supervisor
        .bind_project_repository(
            project_id,
            expected_charter_revision,
            expected_prior_repository_revision,
            draft,
        )
        .map_err(Into::into)
}

fn preflight_project_checks_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
) -> Result<PreflightReceipt, CommandError> {
    state
        .supervisor
        .preflight_project_checks(project_id)
        .map_err(Into::into)
}

/// Briefing's recent Receipts: one Workspace-scoped, bounded read across every
/// Run kind. The Project-scoped Receipt command this replaces is gone.
fn recent_receipts_value(
    state: &DesktopState,
    window: u32,
) -> Result<RecentReceiptPage, CommandError> {
    state.supervisor.recent_receipts(window).map_err(Into::into)
}

fn teams_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
) -> Result<Vec<TeamView>, CommandError> {
    state.supervisor.teams(project_id).map_err(Into::into)
}

fn team_value(state: &DesktopState, team_id: uuid::Uuid) -> Result<TeamView, CommandError> {
    state.supervisor.team(team_id).map_err(Into::into)
}

fn form_team_value(state: &DesktopState, draft: TeamDraft) -> Result<TeamView, CommandError> {
    state.supervisor.form_team(draft).map_err(Into::into)
}

fn missions_value(
    state: &DesktopState,
    project_id: uuid::Uuid,
) -> Result<Vec<MissionView>, CommandError> {
    state.supervisor.missions(project_id).map_err(Into::into)
}

fn mission_value(
    state: &DesktopState,
    mission_id: uuid::Uuid,
) -> Result<MissionView, CommandError> {
    state.supervisor.mission(mission_id).map_err(Into::into)
}

fn admit_mission_value(
    state: &DesktopState,
    draft: MissionDraft,
) -> Result<MissionView, CommandError> {
    state.supervisor.admit_mission(draft).map_err(Into::into)
}

fn discover_local_models_value(
    state: &DesktopState,
) -> Result<LocalModelDiscoveryReport, CommandError> {
    state.supervisor.discover_local_models().map_err(Into::into)
}

fn live_data_encryption_status_value(
    state: &DesktopState,
) -> Result<LiveDataEncryptionReport, CommandError> {
    state
        .supervisor
        .live_data_encryption_status()
        .map_err(Into::into)
}

fn inspect_local_model_value(
    state: &DesktopState,
    model_name: String,
    expected_digest: String,
) -> Result<LocalModelActivationReview, CommandError> {
    state
        .supervisor
        .inspect_local_model(model_name, expected_digest)
        .map_err(Into::into)
}

fn activate_local_model_value(
    state: &DesktopState,
    review: LocalModelActivationReview,
    approved_features: Vec<ProviderFeature>,
) -> Result<LocalModelActivation, CommandError> {
    state
        .supervisor
        .activate_local_model(review, approved_features)
        .map_err(Into::into)
}

fn local_model_activations_value(
    state: &DesktopState,
) -> Result<Vec<LocalModelActivation>, CommandError> {
    state
        .supervisor
        .local_model_activations()
        .map_err(Into::into)
}

fn deactivate_local_model_value(
    state: &DesktopState,
    expected_activation_id: uuid::Uuid,
) -> Result<LocalModelActivation, CommandError> {
    state
        .supervisor
        .deactivate_local_model(expected_activation_id)
        .map_err(Into::into)
}

// Tauri command registration and typed Result/value transport:
// https://v2.tauri.app/develop/calling-rust/#commands
#[tauri::command]
fn authentication_capabilities() -> AuthenticationCapabilities {
    authentication_capabilities_value()
}

#[tauri::command]
fn session_status(state: State<'_, DesktopState>) -> Result<OperatorSessionStatus, CommandError> {
    session_status_value(&state)
}

#[tauri::command]
fn session_policy(state: State<'_, DesktopState>) -> Result<OperatorSessionPolicy, CommandError> {
    session_policy_value(&state)
}

#[tauri::command]
fn set_session_timeout(
    state: State<'_, DesktopState>,
    inactivity_timeout_minutes: u16,
) -> Result<OperatorSessionPolicy, CommandError> {
    set_session_timeout_value(&state, inactivity_timeout_minutes)
}

#[tauri::command]
fn record_operator_activity(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    state
        .supervisor
        .record_operator_activity()
        .map_err(Into::into)
}

#[tauri::command]
fn platform_session_monitor_status(
    state: State<'_, DesktopState>,
) -> Result<PlatformSessionMonitorStatus, CommandError> {
    platform_session_monitor_status_value(&state)
}

#[tauri::command]
fn begin_password_setup(
    state: State<'_, DesktopState>,
    password: String,
    confirmation: String,
) -> Result<RecoveryKeyOutput, CommandError> {
    begin_password_setup_value(
        &state,
        SecretString::new(password),
        SecretString::new(confirmation),
    )
}

#[tauri::command]
fn complete_operator_setup(
    state: State<'_, DesktopState>,
    recovery_saved: bool,
) -> Result<OperatorSessionStatus, CommandError> {
    complete_operator_setup_value(&state, recovery_saved)
}

#[tauri::command]
fn authenticate_password(
    state: State<'_, DesktopState>,
    password: String,
) -> Result<OperatorSessionStatus, CommandError> {
    state
        .supervisor
        .authenticate_password(SecretString::new(password), AccessOrigin::SameMachine)
        .map_err(Into::into)
}

#[tauri::command]
fn recover_operator_account(
    state: State<'_, DesktopState>,
    recovery_key: String,
    new_password: String,
    confirmation: String,
) -> Result<RecoveryKeyOutput, CommandError> {
    state
        .supervisor
        .recover_with_key(
            SecretString::new(recovery_key),
            SecretString::new(new_password),
            SecretString::new(confirmation),
            AccessOrigin::SameMachine,
        )
        .map(|replacement| RecoveryKeyOutput {
            recovery_key: replacement.into_string(),
        })
        .map_err(Into::into)
}

#[tauri::command]
fn lock_session(state: State<'_, DesktopState>) -> Result<OperatorSessionStatus, CommandError> {
    state.supervisor.lock().map_err(Into::into)
}

#[tauri::command]
fn starter_organization(
    state: State<'_, DesktopState>,
) -> Result<StarterOrganizationView, CommandError> {
    starter_organization_value(&state)
}

#[tauri::command]
fn accept_starter_organization(
    state: State<'_, DesktopState>,
    proposal_version: u16,
    residents: Vec<StarterResidentDraft>,
) -> Result<StarterOrganizationView, CommandError> {
    accept_starter_organization_value(&state, proposal_version, residents)
}

#[tauri::command]
fn discard_starter_organization(
    state: State<'_, DesktopState>,
    proposal_version: u16,
) -> Result<StarterOrganizationView, CommandError> {
    discard_starter_organization_value(&state, proposal_version)
}

#[tauri::command]
fn residents(state: State<'_, DesktopState>) -> Result<Vec<ResidentAgentView>, CommandError> {
    residents_value(&state)
}

#[tauri::command]
fn activate_resident(
    state: State<'_, DesktopState>,
    resident_id: uuid::Uuid,
    expected_decision_id: Option<uuid::Uuid>,
) -> Result<ResidentAgentView, CommandError> {
    activate_resident_value(&state, resident_id, expected_decision_id)
}

#[tauri::command]
fn suspend_resident(
    state: State<'_, DesktopState>,
    resident_id: uuid::Uuid,
    expected_decision_id: Option<uuid::Uuid>,
) -> Result<ResidentAgentView, CommandError> {
    suspend_resident_value(&state, resident_id, expected_decision_id)
}

#[tauri::command]
fn reactivate_resident(
    state: State<'_, DesktopState>,
    resident_id: uuid::Uuid,
    expected_decision_id: Option<uuid::Uuid>,
) -> Result<ResidentAgentView, CommandError> {
    reactivate_resident_value(&state, resident_id, expected_decision_id)
}

#[tauri::command]
fn retire_resident(
    state: State<'_, DesktopState>,
    resident_id: uuid::Uuid,
    expected_decision_id: Option<uuid::Uuid>,
) -> Result<ResidentAgentView, CommandError> {
    retire_resident_value(&state, resident_id, expected_decision_id)
}

#[tauri::command]
fn resident_capability_catalog(
    state: State<'_, DesktopState>,
) -> Result<Vec<BeaconCapabilityDescriptor>, CommandError> {
    resident_capability_catalog_value(&state)
}

#[tauri::command]
fn resident_configurations(
    state: State<'_, DesktopState>,
) -> Result<Vec<ResidentConfiguration>, CommandError> {
    resident_configurations_value(&state)
}

#[tauri::command]
fn configure_resident(
    state: State<'_, DesktopState>,
    resident_id: uuid::Uuid,
    expected_decision_id: Option<uuid::Uuid>,
    activation_id: uuid::Uuid,
    capabilities: Vec<BeaconCapability>,
) -> Result<ResidentConfiguration, CommandError> {
    configure_resident_value(
        &state,
        resident_id,
        expected_decision_id,
        activation_id,
        capabilities,
    )
}

#[tauri::command]
fn clear_resident_configuration(
    state: State<'_, DesktopState>,
    resident_id: uuid::Uuid,
    expected_decision_id: uuid::Uuid,
) -> Result<ResidentConfiguration, CommandError> {
    clear_resident_configuration_value(&state, resident_id, expected_decision_id)
}

#[tauri::command]
fn projects(state: State<'_, DesktopState>) -> Result<Vec<ProjectSummary>, CommandError> {
    projects_value(&state)
}

#[tauri::command]
fn sample_project_status(
    state: State<'_, DesktopState>,
) -> Result<SampleProjectStatus, CommandError> {
    sample_project_status_value(&state)
}

#[tauri::command]
fn install_sample_project(
    state: State<'_, DesktopState>,
) -> Result<SampleProjectInstallResult, CommandError> {
    install_sample_project_value(&state)
}

#[tauri::command]
fn project(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
) -> Result<ProjectView, CommandError> {
    project_value(&state, project_id)
}

#[tauri::command]
fn project_charter_history(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
) -> Result<Vec<ProjectCharter>, CommandError> {
    project_charter_history_value(&state, project_id)
}

#[tauri::command]
fn create_project(
    state: State<'_, DesktopState>,
    draft: ProjectDraft,
) -> Result<ProjectView, CommandError> {
    create_project_value(&state, draft)
}

#[tauri::command]
fn revise_project_charter(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
    expected_current_revision: u32,
    draft: ProjectCharterDraft,
) -> Result<ProjectView, CommandError> {
    revise_project_charter_value(&state, project_id, expected_current_revision, draft)
}

#[tauri::command]
fn project_policy(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
) -> Result<Option<ProjectPolicy>, CommandError> {
    project_policy_value(&state, project_id)
}

#[tauri::command]
fn project_policy_history(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
) -> Result<Vec<ProjectPolicy>, CommandError> {
    project_policy_history_value(&state, project_id)
}

#[tauri::command]
fn mission_publications(
    state: State<'_, DesktopState>,
    mission_id: uuid::Uuid,
) -> Result<Vec<PublicationSummary>, CommandError> {
    mission_publications_value(&state, mission_id)
}

/// The exact stored Markdown of one published Outcome. `version` selects a superseded version; its
/// absence means the current one.
///
/// The desktop layer adds no access check, in keeping with every other command: the check lives in
/// `OutcomeService`, which scopes by Workspace and requires an authenticated session.
#[tauri::command]
fn outcome_document(
    state: State<'_, DesktopState>,
    mission_id: uuid::Uuid,
    version: Option<u32>,
) -> Result<ExportedOutcome, CommandError> {
    outcome_document_value(&state, mission_id, version)
}

#[tauri::command]
fn publish_project_policy(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
    expected_charter_revision: u32,
    expected_policy_revision: Option<u32>,
    draft: ProjectPolicyDraft,
) -> Result<ProjectPolicy, CommandError> {
    publish_project_policy_value(
        &state,
        project_id,
        expected_charter_revision,
        expected_policy_revision,
        draft,
    )
}

#[tauri::command]
fn current_project_repository(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
) -> Result<Option<ProjectRepository>, CommandError> {
    current_project_repository_value(&state, project_id)
}

#[tauri::command]
fn project_repository_history(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
) -> Result<Vec<ProjectRepository>, CommandError> {
    project_repository_history_value(&state, project_id)
}

#[tauri::command]
fn bind_project_repository(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
    expected_charter_revision: u32,
    expected_prior_repository_revision: Option<u32>,
    draft: RepositoryDraft,
) -> Result<ProjectRepository, CommandError> {
    bind_project_repository_value(
        &state,
        project_id,
        expected_charter_revision,
        expected_prior_repository_revision,
        draft,
    )
}

#[tauri::command]
fn preflight_project_checks(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
) -> Result<PreflightReceipt, CommandError> {
    preflight_project_checks_value(&state, project_id)
}

#[tauri::command]
fn recent_receipts(
    state: State<'_, DesktopState>,
    window: u32,
) -> Result<RecentReceiptPage, CommandError> {
    recent_receipts_value(&state, window)
}

#[tauri::command]
fn teams(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
) -> Result<Vec<TeamView>, CommandError> {
    teams_value(&state, project_id)
}

#[tauri::command]
fn team(state: State<'_, DesktopState>, team_id: uuid::Uuid) -> Result<TeamView, CommandError> {
    team_value(&state, team_id)
}

#[tauri::command]
fn form_team(state: State<'_, DesktopState>, draft: TeamDraft) -> Result<TeamView, CommandError> {
    form_team_value(&state, draft)
}

#[tauri::command]
fn missions(
    state: State<'_, DesktopState>,
    project_id: uuid::Uuid,
) -> Result<Vec<MissionView>, CommandError> {
    missions_value(&state, project_id)
}

#[tauri::command]
fn mission(
    state: State<'_, DesktopState>,
    mission_id: uuid::Uuid,
) -> Result<MissionView, CommandError> {
    mission_value(&state, mission_id)
}

#[tauri::command]
fn admit_mission(
    state: State<'_, DesktopState>,
    draft: MissionDraft,
) -> Result<MissionView, CommandError> {
    admit_mission_value(&state, draft)
}

#[tauri::command]
fn discover_local_models(
    state: State<'_, DesktopState>,
) -> Result<LocalModelDiscoveryReport, CommandError> {
    discover_local_models_value(&state)
}

#[tauri::command]
fn live_data_encryption_status(
    state: State<'_, DesktopState>,
) -> Result<LiveDataEncryptionReport, CommandError> {
    live_data_encryption_status_value(&state)
}

#[tauri::command]
fn inspect_local_model(
    state: State<'_, DesktopState>,
    model_name: String,
    expected_digest: String,
) -> Result<LocalModelActivationReview, CommandError> {
    inspect_local_model_value(&state, model_name, expected_digest)
}

#[tauri::command]
fn activate_local_model(
    state: State<'_, DesktopState>,
    review: LocalModelActivationReview,
    approved_features: Vec<ProviderFeature>,
) -> Result<LocalModelActivation, CommandError> {
    activate_local_model_value(&state, review, approved_features)
}

#[tauri::command]
fn local_model_activations(
    state: State<'_, DesktopState>,
) -> Result<Vec<LocalModelActivation>, CommandError> {
    local_model_activations_value(&state)
}

#[tauri::command]
fn deactivate_local_model(
    state: State<'_, DesktopState>,
    expected_activation_id: uuid::Uuid,
) -> Result<LocalModelActivation, CommandError> {
    deactivate_local_model_value(&state, expected_activation_id)
}

fn prepare_runtime_directory(path: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::other(
            "Beacon runtime path is not a directory",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn migrate_legacy_app_data(data_dir: &Path) -> Result<(), std::io::Error> {
    let Some(parent) = data_dir.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Beacon app-data path has no parent directory",
        ));
    };
    if data_dir.file_name().and_then(|name| name.to_str()) != Some("dev.beacon.phase0") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Beacon app-data path has an unexpected identifier",
        ));
    }

    let legacy_dir = parent.join("dev.mosaic.phase0");
    let legacy_metadata = match fs::symlink_metadata(&legacy_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if data_dir.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "Both Beacon and legacy Mosaic app-data directories exist; refusing to choose",
        ));
    }
    if legacy_metadata.file_type().is_symlink() || !legacy_metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Legacy Mosaic app-data path is not a real directory",
        ));
    }
    if legacy_metadata.uid() != unsafe { libc::geteuid() }
        || legacy_metadata.permissions().mode() & 0o077 != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Legacy Mosaic app-data directory is not private to the current user",
        ));
    }

    let _supervisor_guard = acquire_legacy_supervisor_guard(&legacy_dir)?;
    let legacy_names = [
        ("mosaic.sqlite3", "beacon.sqlite3"),
        ("mosaic.sqlite3-wal", "beacon.sqlite3-wal"),
        ("mosaic.sqlite3-shm", "beacon.sqlite3-shm"),
        ("runtime/mosaicd.log", "runtime/beacond.log"),
    ];
    for (old_name, new_name) in legacy_names {
        if legacy_dir.join(old_name).exists() && legacy_dir.join(new_name).exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("legacy app data contains both {old_name} and {new_name}"),
            ));
        }
    }

    let mut renamed = Vec::new();
    for (old_name, new_name) in legacy_names {
        let old_path = legacy_dir.join(old_name);
        let new_path = legacy_dir.join(new_name);
        if old_path.exists() && !new_path.exists() {
            if let Err(error) = fs::rename(&old_path, &new_path) {
                rollback_legacy_names(&renamed);
                return Err(error);
            }
            renamed.push((old_path, new_path));
        }
    }
    if let Err(error) = fs::rename(&legacy_dir, data_dir) {
        rollback_legacy_names(&renamed);
        return Err(error);
    }
    Ok(())
}

fn acquire_legacy_supervisor_guard(legacy_dir: &Path) -> Result<Option<fs::File>, std::io::Error> {
    let lock_path = legacy_dir.join("runtime/supervisor.lock");
    let metadata = match fs::symlink_metadata(&lock_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Legacy supervisor lock is not a private regular file",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock_path)?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "Close the legacy Mosaic desktop before Beacon adopts its app data",
        ));
    }
    Ok(Some(file))
}

fn rollback_legacy_names(renamed: &[(PathBuf, PathBuf)]) {
    for (old_path, new_path) in renamed.iter().rev() {
        let _ = fs::rename(new_path, old_path);
    }
}

fn supervisor_binary() -> Result<PathBuf, std::io::Error> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("BEACON_SUPERVISOR_BIN") {
        return Ok(PathBuf::from(path));
    }
    let current = std::env::current_exe()?;
    Ok(current.with_file_name("beacond"))
}

fn start_or_connect_supervisor(
    data_dir: &Path,
) -> Result<LocalSupervisorClient, Box<dyn std::error::Error>> {
    let runtime_dir = data_dir.join("runtime");
    prepare_runtime_directory(&runtime_dir)?;
    let socket_path = runtime_dir.join("beacond.sock");
    let token_path = runtime_dir.join("client.token");
    let lock_path = runtime_dir.join("supervisor.lock");
    let store_path = data_dir.join("beacon.sqlite3");
    initialize_local_auth_token(&token_path)?;
    let client = LocalSupervisorClient::connect(&socket_path, &token_path)?;
    if client.status().is_ok() {
        return Ok(client);
    }

    let mut log_options = OpenOptions::new();
    log_options
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let log = log_options.open(runtime_dir.join("beacond.log"))?;
    log.set_permissions(fs::Permissions::from_mode(0o600))?;
    let log_error = log.try_clone()?;
    let mut child = Command::new(supervisor_binary()?)
        .arg("serve")
        .arg("--store")
        .arg(&store_path)
        .arg("--socket")
        .arg(&socket_path)
        .arg("--auth-token")
        .arg(&token_path)
        .arg("--lock")
        .arg(&lock_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_error))
        .spawn()?;

    for _ in 0..100 {
        if client.status().is_ok() {
            return Ok(client);
        }
        if let Some(status) = child.try_wait()? {
            return Err(std::io::Error::other(format!(
                "beacond exited before accepting local commands: {status}"
            ))
            .into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "beacond did not accept local commands within five seconds",
    )
    .into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Tauri 2.11 resolves an application-specific local-data directory from the bundle
            // identifier: https://docs.rs/tauri/2.11.5/tauri/path/struct.PathResolver.html#method.app_local_data_dir
            let data_dir = app.path().app_local_data_dir()?;
            migrate_legacy_app_data(&data_dir)?;
            prepare_runtime_directory(&data_dir)?;
            let supervisor = start_or_connect_supervisor(&data_dir)?;
            app.manage(DesktopState { supervisor });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            authentication_capabilities,
            session_status,
            session_policy,
            set_session_timeout,
            record_operator_activity,
            platform_session_monitor_status,
            begin_password_setup,
            complete_operator_setup,
            authenticate_password,
            recover_operator_account,
            lock_session,
            starter_organization,
            accept_starter_organization,
            discard_starter_organization,
            residents,
            activate_resident,
            suspend_resident,
            reactivate_resident,
            retire_resident,
            resident_capability_catalog,
            resident_configurations,
            configure_resident,
            clear_resident_configuration,
            projects,
            sample_project_status,
            install_sample_project,
            project,
            project_charter_history,
            create_project,
            revise_project_charter,
            project_policy,
            project_policy_history,
            publish_project_policy,
            current_project_repository,
            project_repository_history,
            bind_project_repository,
            preflight_project_checks,
            recent_receipts,
            teams,
            team,
            form_team,
            missions,
            mission,
            admit_mission,
            mission_publications,
            outcome_document,
            discover_local_models,
            live_data_encryption_status,
            inspect_local_model,
            activate_local_model,
            local_model_activations,
            deactivate_local_model
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the Beacon Phase 0 desktop shell");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;

    use beacond::{
        AutonomyProfile, BeaconCapability, DeclaredCheck, LocalServerConfig, LocalSupervisorClient,
        LocalSupervisorServer, MissionDraft, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft,
        RepositoryDraft, RepositorySourceKind, ResidentLifecycle, SampleProjectInstallOutcome,
        SampleProjectStatus, SecretString, StarterResidentDraft, StarterRole,
        TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule,
        TeamLeadSelectionPolicy, TrackRecordEvidenceKind, TrackRecordRequirement,
        initialize_local_auth_token,
    };
    use serde_json::Value;
    use uuid::Uuid;

    use super::{
        DesktopState, accept_starter_organization_value, activate_resident_value,
        admit_mission_value, authentication_capabilities_value, begin_password_setup_value,
        bind_project_repository_value, complete_operator_setup_value, create_project_value,
        current_project_repository_value, install_sample_project_value, migrate_legacy_app_data,
        mission_value, outcome_document_value, platform_session_monitor_status_value,
        preflight_project_checks_value, prepare_runtime_directory, project_charter_history_value,
        project_policy_history_value, project_policy_value, project_repository_history_value,
        project_value, projects_value, publish_project_policy_value, reactivate_resident_value,
        residents_value, retire_resident_value, revise_project_charter_value,
        sample_project_status_value, session_policy_value, session_status_value,
        set_session_timeout_value, starter_organization_value, suspend_resident_value,
    };

    #[test]
    fn legacy_app_data_moves_once_and_keeps_the_existing_store() {
        let parent = std::env::temp_dir().join(format!("beacon-migration-{}", Uuid::new_v4()));
        let legacy = parent.join("dev.mosaic.phase0");
        let current = parent.join("dev.beacon.phase0");
        fs::create_dir_all(&legacy).unwrap();
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(legacy.join("mosaic.sqlite3"), b"existing workspace").unwrap();
        fs::write(legacy.join("mosaic.sqlite3-wal"), b"existing wal").unwrap();

        migrate_legacy_app_data(&current).unwrap();

        assert!(!legacy.exists());
        assert_eq!(
            fs::read(current.join("beacon.sqlite3")).unwrap(),
            b"existing workspace"
        );
        assert_eq!(
            fs::read(current.join("beacon.sqlite3-wal")).unwrap(),
            b"existing wal"
        );
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn legacy_app_data_migration_refuses_ambiguous_dual_directories() {
        let parent = std::env::temp_dir().join(format!("beacon-migration-{}", Uuid::new_v4()));
        let legacy = parent.join("dev.mosaic.phase0");
        let current = parent.join("dev.beacon.phase0");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&current).unwrap();
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o700)).unwrap();

        let error = migrate_legacy_app_data(&current).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(legacy.is_dir());
        assert!(current.is_dir());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn legacy_app_data_migration_refuses_a_symlink_source() {
        use std::os::unix::fs::symlink;

        let parent = std::env::temp_dir().join(format!("beacon-migration-{}", Uuid::new_v4()));
        let target = parent.join("target");
        let legacy = parent.join("dev.mosaic.phase0");
        let current = parent.join("dev.beacon.phase0");
        fs::create_dir_all(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&target, &legacy).unwrap();

        let error = migrate_legacy_app_data(&current).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(legacy.is_symlink());
        assert!(target.is_dir());
        fs::remove_file(legacy).unwrap();
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn legacy_app_data_migration_refuses_a_running_supervisor() {
        use std::fs::OpenOptions;
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;

        let parent = std::env::temp_dir().join(format!("beacon-migration-{}", Uuid::new_v4()));
        let legacy = parent.join("dev.mosaic.phase0");
        let current = parent.join("dev.beacon.phase0");
        let runtime = legacy.join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(runtime.join("supervisor.lock"))
            .unwrap();
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );

        let error = migrate_legacy_app_data(&current).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(legacy.is_dir());
        assert!(!current.exists());
        drop(lock);
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn app_data_directory_is_owner_only() {
        let root = std::env::temp_dir().join(format!("beacon-data-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o775)).unwrap();

        prepare_runtime_directory(&root).unwrap();

        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Registration is not permission.
    ///
    /// A command reaches `generate_handler!` and compiles; the capability is a
    /// separate file, and `build.rs` generates its permission from a third
    /// list. Nothing ties the three together, so a command can ship registered
    /// and ungranted -- reachable from the interface, refused by the ACL, and
    /// reported to the operator as a generic account error. The list below is
    /// derived from the source rather than written by hand, so a future
    /// handler cannot ship that way without this test failing.
    ///
    /// The needles are assembled with `concat!` so this parser does not match
    /// its own source text.
    fn registered_commands(source: &str) -> Vec<String> {
        let handler = concat!("generate_handler", "![");
        let body = source
            .split_once(handler)
            .expect("lib.rs registers commands")
            .1
            .split_once(']')
            .expect("the registration list closes")
            .0;
        body.split(',')
            .map(|entry| entry.trim().to_owned())
            .filter(|entry| !entry.is_empty())
            .collect()
    }

    fn attributed_commands(source: &str) -> Vec<String> {
        let attribute = concat!("#[tauri::", "command]");
        source
            .split(attribute)
            .skip(1)
            .filter_map(|rest| {
                let after = rest.split_once("fn ")?.1;
                Some(after.split_once('(')?.0.trim().to_owned())
            })
            .collect()
    }

    fn declared_commands(build_source: &str) -> Vec<String> {
        let body = build_source
            .split_once("COMMANDS: &[&str] = &[")
            .expect("build.rs declares its command list")
            .1
            .split_once("];")
            .expect("the declaration closes")
            .0;
        body.split(',')
            .filter_map(|entry| {
                let entry = entry.trim();
                entry
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .map(str::to_owned)
            })
            .collect()
    }

    #[test]
    fn no_registered_command_can_ship_without_its_permission() {
        let source = include_str!("lib.rs");
        let registered = registered_commands(source);
        let attributed = attributed_commands(source);
        let declared = declared_commands(include_str!("../build.rs"));
        let capability: Value =
            serde_json::from_str(include_str!("../capabilities/main-window.json")).unwrap();
        let granted = capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|permission| permission.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();

        assert!(
            registered.len() > 40,
            "the registration list did not parse: {registered:?}"
        );

        // Every command the interface can reach must be granted, and must have
        // a permission for the capability to name.
        let ungranted = registered
            .iter()
            .filter(|command| !granted.contains(&format!("allow-{}", command.replace('_', "-"))))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            ungranted,
            Vec::<String>::new(),
            "registered but not granted by capabilities/main-window.json"
        );
        let undeclared = registered
            .iter()
            .filter(|command| !declared.contains(command))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            undeclared,
            Vec::<String>::new(),
            "registered but absent from build.rs COMMANDS, so no permission exists"
        );

        // And nothing is granted that the interface cannot reach, so a stale
        // grant cannot outlive the command it was written for.
        let unregistered = declared
            .iter()
            .filter(|command| !registered.contains(command))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            unregistered,
            Vec::<String>::new(),
            "declared in build.rs COMMANDS but not registered"
        );
        let unattributed = registered
            .iter()
            .filter(|command| !attributed.contains(command))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            unattributed,
            Vec::<String>::new(),
            "registered but not defined as a Tauri command in this file"
        );
    }

    #[test]
    fn main_window_capability_grants_every_registered_account_command() {
        let capability: Value =
            serde_json::from_str(include_str!("../capabilities/main-window.json")).unwrap();
        let permissions = capability["permissions"].as_array().unwrap();
        let permissions = permissions
            .iter()
            .map(|permission| permission.as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            permissions,
            [
                "allow-authentication-capabilities",
                "allow-session-status",
                "allow-session-policy",
                "allow-set-session-timeout",
                "allow-record-operator-activity",
                "allow-platform-session-monitor-status",
                "allow-begin-password-setup",
                "allow-complete-operator-setup",
                "allow-authenticate-password",
                "allow-recover-operator-account",
                "allow-lock-session",
                "allow-starter-organization",
                "allow-accept-starter-organization",
                "allow-discard-starter-organization",
                "allow-residents",
                "allow-activate-resident",
                "allow-suspend-resident",
                "allow-reactivate-resident",
                "allow-retire-resident",
                "allow-resident-capability-catalog",
                "allow-resident-configurations",
                "allow-configure-resident",
                "allow-clear-resident-configuration",
                "allow-projects",
                "allow-sample-project-status",
                "allow-install-sample-project",
                "allow-project",
                "allow-project-charter-history",
                "allow-create-project",
                "allow-revise-project-charter",
                "allow-project-policy",
                "allow-project-policy-history",
                "allow-publish-project-policy",
                "allow-current-project-repository",
                "allow-project-repository-history",
                "allow-bind-project-repository",
                "allow-preflight-project-checks",
                "allow-recent-receipts",
                "allow-teams",
                "allow-team",
                "allow-form-team",
                "allow-missions",
                "allow-mission",
                "allow-admit-mission",
                "allow-mission-publications",
                "allow-outcome-document",
                "allow-discover-local-models",
                "allow-live-data-encryption-status",
                "allow-inspect-local-model",
                "allow-activate-local-model",
                "allow-local-model-activations",
                "allow-deactivate-local-model",
            ]
        );
    }

    #[test]
    fn blue_locked_outcome_export_reaches_renderer_as_not_authenticated() {
        let state = TestState::new();
        let refusal = outcome_document_value(&state.state, Uuid::new_v4(), None).unwrap_err();
        assert_eq!(refusal.code, "not_authenticated");
    }

    /// The other half of blue's P1: an unlocked store with no publication must say so, and must not
    /// say the account service is unavailable either.
    #[test]
    fn an_absent_outcome_reaches_the_renderer_as_outcome_absent() {
        let state = TestState::new();
        state
            .state
            .supervisor
            .begin_password_setup(
                SecretString::new("a long local passphrase".to_owned()),
                SecretString::new("a long local passphrase".to_owned()),
            )
            .unwrap();
        state
            .state
            .supervisor
            .complete_operator_setup(true)
            .unwrap();
        let refusal = outcome_document_value(&state.state, Uuid::new_v4(), None).unwrap_err();
        assert_eq!(refusal.code, "outcome_absent");
    }

    struct TestState {
        state: DesktopState,
        root: PathBuf,
        running: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl TestState {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("beacon-desktop-{}", Uuid::new_v4()));
            prepare_runtime_directory(&root).unwrap();
            let socket = root.join("beacond.sock");
            let token = root.join("client.token");
            initialize_local_auth_token(&token).unwrap();
            let server = LocalSupervisorServer::bind(LocalServerConfig {
                store_path: root.join("beacon.sqlite3"),
                initial_workspace_name: "Personal".to_owned(),
                socket_path: socket.clone(),
                auth_token_path: token.clone(),
                lock_path: root.join("supervisor.lock"),
                local_model_endpoint: beacond::LocalModelEndpoint::ollama_default(),
            })
            .unwrap();
            let running = Arc::new(AtomicBool::new(true));
            let server_running = Arc::clone(&running);
            let thread = std::thread::spawn(move || server.serve_until(server_running).unwrap());
            let supervisor = LocalSupervisorClient::connect(socket, token).unwrap();
            Self {
                state: DesktopState { supervisor },
                root,
                running,
                thread: Some(thread),
            }
        }
    }

    impl Drop for TestState {
        fn drop(&mut self) {
            self.running.store(false, Ordering::Release);
            self.thread.take().unwrap().join().unwrap();
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn signed_out_status_exposes_no_workspace_data_or_operator_command() {
        let state = TestState::new();
        let status = serde_json::to_value(session_status_value(&state.state).unwrap()).unwrap();

        assert_eq!(status["phase"], "operator_setup_required");
        assert_eq!(status["workspaceAccess"], false);
        assert_eq!(status["operatorCommands"], false);
        assert_eq!(
            status
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["operatorCommands", "phase", "workspaceAccess"]
        );
        assert!(!contains_workspace_material(&status));
    }

    #[test]
    fn capability_output_is_explicit_about_the_missing_native_passkey_adapter() {
        let capabilities = serde_json::to_value(authentication_capabilities_value()).unwrap();

        assert_eq!(capabilities["preferredMethod"], "passkey");
        assert_eq!(capabilities["passkey"]["available"], false);
        assert_eq!(
            capabilities["passkey"]["reason"],
            "unsupported_linux_webkitgtk"
        );
        assert_eq!(capabilities["passwordFallback"]["scope"], "same_machine");
        assert_eq!(
            capabilities
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["passkey", "passwordFallback", "preferredMethod"]
        );
    }

    #[test]
    fn command_adapter_returns_the_recovery_key_once_then_an_exact_unlocked_status() {
        let state = TestState::new();
        let output = begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        let recovery = serde_json::to_value(output).unwrap();
        assert_eq!(recovery.as_object().unwrap().len(), 1);
        assert_eq!(
            recovery["recoveryKey"]
                .as_str()
                .unwrap()
                .replace('-', "")
                .len(),
            32
        );

        let status =
            serde_json::to_value(complete_operator_setup_value(&state.state, true).unwrap())
                .unwrap();
        assert_eq!(status["phase"], "unlocked");
        assert_eq!(status["workspaceAccess"], true);
        assert_eq!(status["operatorCommands"], true);
        assert_eq!(status.as_object().unwrap().len(), 3);

        let policy =
            serde_json::to_value(set_session_timeout_value(&state.state, 75).unwrap()).unwrap();
        assert_eq!(policy["inactivityTimeoutMinutes"], 75);
        assert_eq!(policy.as_object().unwrap().len(), 1);
        assert_eq!(
            session_policy_value(&state.state)
                .unwrap()
                .inactivity_timeout_minutes,
            75
        );
        let monitor =
            serde_json::to_value(platform_session_monitor_status_value(&state.state).unwrap())
                .unwrap();
        assert_eq!(monitor["screenLock"], "starting");
        assert_eq!(monitor["suspend"], "starting");
    }

    #[test]
    fn command_adapter_installs_and_reports_the_opt_in_sample_project() {
        let state = TestState::new();
        begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        complete_operator_setup_value(&state.state, true).unwrap();

        assert!(matches!(
            sample_project_status_value(&state.state).unwrap(),
            SampleProjectStatus::Available {
                bundle_version: 1,
                ..
            }
        ));
        let installed = install_sample_project_value(&state.state).unwrap();
        assert_eq!(installed.outcome, SampleProjectInstallOutcome::Installed);
        assert_eq!(installed.installation.charter_revision, 1);
        assert_eq!(installed.installation.repository_revision, 1);
        assert!(matches!(
            sample_project_status_value(&state.state).unwrap(),
            SampleProjectStatus::Installed { installation }
                if installation == installed.installation
        ));
    }

    #[test]
    fn command_adapter_returns_and_accepts_the_closed_starter_roster() {
        let state = TestState::new();
        begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        complete_operator_setup_value(&state.state, true).unwrap();

        let proposed = starter_organization_value(&state.state).unwrap();
        assert_eq!(proposed.templates.len(), 4);
        assert!(proposed.residents.is_empty());
        let accepted = accept_starter_organization_value(
            &state.state,
            proposed.proposal_version,
            vec![
                StarterResidentDraft {
                    role: StarterRole::Lead,
                    display_name: Some("Northstar".to_owned()),
                },
                StarterResidentDraft {
                    role: StarterRole::Builder,
                    display_name: None,
                },
                StarterResidentDraft {
                    role: StarterRole::Reviewer,
                    display_name: None,
                },
            ],
        )
        .unwrap();
        let value = serde_json::to_value(accepted).unwrap();
        assert_eq!(value["decision"], "accepted");
        assert_eq!(value["residents"].as_array().unwrap().len(), 3);
        assert_eq!(value["residents"][0]["lifecycle"], "inactive");

        let residents = residents_value(&state.state).unwrap();
        assert_eq!(residents.len(), 3);
        let resident_id = residents[0].id;
        let active = activate_resident_value(&state.state, resident_id, None).unwrap();
        assert_eq!(active.lifecycle, ResidentLifecycle::Active);
        let suspended =
            suspend_resident_value(&state.state, resident_id, active.latest_decision_id).unwrap();
        assert_eq!(suspended.lifecycle, ResidentLifecycle::Suspended);
        let reactivated =
            reactivate_resident_value(&state.state, resident_id, suspended.latest_decision_id)
                .unwrap();
        assert_eq!(reactivated.lifecycle, ResidentLifecycle::Active);
        let retired =
            retire_resident_value(&state.state, resident_id, reactivated.latest_decision_id)
                .unwrap();
        let resident_value = serde_json::to_value(&retired).unwrap();
        assert_eq!(resident_value["lifecycle"], "retired");
        assert_eq!(resident_value.as_object().unwrap().len(), 8);
        assert!(resident_value.get("authority").is_none());
    }

    #[test]
    fn command_adapter_creates_and_reads_an_exact_project_charter() {
        let state = TestState::new();
        begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        complete_operator_setup_value(&state.state, true).unwrap();

        assert!(projects_value(&state.state).unwrap().is_empty());
        let created = create_project_value(
            &state.state,
            ProjectDraft {
                name: "Beacon Alpha".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Build the agent office.".to_owned(),
                    desired_outcomes: vec!["Ship a verified vertical slice".to_owned()],
                    constraints: vec!["Keep Workspace data local".to_owned()],
                    prohibitions: vec!["Do not deploy publicly".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            },
        )
        .unwrap();
        let value = serde_json::to_value(&created).unwrap();
        assert_eq!(value["name"], "Beacon Alpha");
        assert_eq!(value["charter"]["revision"], 1);
        assert_eq!(value["charter"]["budgetBoundary"], "no_spend");
        assert_eq!(value["charter"]["externalEffects"], "prohibited");
        assert_eq!(project_value(&state.state, created.id).unwrap(), created);
        assert_eq!(
            project_charter_history_value(&state.state, created.id).unwrap(),
            std::slice::from_ref(&created.charter)
        );
        let revised = revise_project_charter_value(
            &state.state,
            created.id,
            1,
            ProjectCharterDraft {
                purpose: "Build the agent office, beginning with software.".to_owned(),
                desired_outcomes: vec!["Ship a governed revision".to_owned()],
                constraints: vec!["Keep Workspace data local".to_owned()],
                prohibitions: vec!["Do not deploy publicly".to_owned()],
                autonomy_profile: AutonomyProfile::Observe,
            },
        )
        .unwrap();
        assert_eq!(revised.charter.revision, 2);
        assert_eq!(revised.charter.autonomy_profile, AutonomyProfile::Observe);
        assert_eq!(
            project_charter_history_value(&state.state, created.id)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(projects_value(&state.state).unwrap().len(), 1);
    }

    #[test]
    fn command_adapter_publishes_and_reads_project_policy_without_selecting_a_lead() {
        let state = TestState::new();
        begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        complete_operator_setup_value(&state.state, true).unwrap();
        let project = create_project_value(
            &state.state,
            ProjectDraft {
                name: "Policy Project".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Build the agent office.".to_owned(),
                    desired_outcomes: vec!["Ship a governed vertical slice".to_owned()],
                    constraints: vec!["Keep Workspace data local".to_owned()],
                    prohibitions: vec!["Do not deploy publicly".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            },
        )
        .unwrap();
        assert!(
            project_policy_value(&state.state, project.id)
                .unwrap()
                .is_none()
        );

        let policy = publish_project_policy_value(
            &state.state,
            project.id,
            project.charter.revision,
            None,
            ProjectPolicyDraft {
                team_lead_selection: TeamLeadSelectionPolicy {
                    required_capabilities: vec![BeaconCapability::WorkCoordinateMission],
                    availability_rule: TeamLeadAvailabilityRule::CurrentTeamEligibility,
                    track_record_requirements: vec![TrackRecordRequirement {
                        evidence_kind: TrackRecordEvidenceKind::MissionOutcome,
                        minimum_count: 1,
                    }],
                    conflict_rule: TeamLeadConflictRule::RequireNoDeclaredConflict,
                    ambiguity_rule: TeamLeadAmbiguityRule::RequireOperatorDecision,
                },
            },
        )
        .unwrap();
        let value = serde_json::to_value(&policy).unwrap();
        assert_eq!(value["revision"], 1);
        assert_eq!(value["authoredBy"], "operator");
        assert_eq!(
            value["teamLeadSelection"]["availabilityRule"],
            "current_team_eligibility"
        );
        assert!(value.get("leadResidentId").is_none());
        assert_eq!(
            project_policy_value(&state.state, project.id).unwrap(),
            Some(policy)
        );
        assert_eq!(
            project_policy_history_value(&state.state, project.id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn policy_refusals_reach_the_interface_under_their_own_code() {
        let state = TestState::new();
        begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        complete_operator_setup_value(&state.state, true).unwrap();
        let project = create_project_value(
            &state.state,
            ProjectDraft {
                name: "Policy Project".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Build the agent office.".to_owned(),
                    desired_outcomes: vec!["Ship a governed vertical slice".to_owned()],
                    constraints: vec!["Keep Workspace data local".to_owned()],
                    prohibitions: vec!["Do not deploy publicly".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            },
        )
        .unwrap();
        let valid = || ProjectPolicyDraft {
            team_lead_selection: TeamLeadSelectionPolicy {
                required_capabilities: vec![BeaconCapability::WorkCoordinateMission],
                availability_rule: TeamLeadAvailabilityRule::CurrentTeamEligibility,
                track_record_requirements: vec![TrackRecordRequirement {
                    evidence_kind: TrackRecordEvidenceKind::MissionOutcome,
                    minimum_count: 1,
                }],
                conflict_rule: TeamLeadConflictRule::RequireNoDeclaredConflict,
                ambiguity_rule: TeamLeadAmbiguityRule::RequireOperatorDecision,
            },
        };

        // Before this change every one of these fell through to
        // "account_service_unavailable", so the matching `case` strings in
        // session.ts were unreachable. Each refusal must now carry its own code.
        let mut invalid = valid();
        invalid.team_lead_selection.required_capabilities.clear();
        let refusal = publish_project_policy_value(
            &state.state,
            project.id,
            project.charter.revision,
            None,
            invalid,
        )
        .unwrap_err();
        assert_eq!(refusal.code, "invalid_project_policy");

        publish_project_policy_value(
            &state.state,
            project.id,
            project.charter.revision,
            None,
            valid(),
        )
        .unwrap();
        let unchanged = publish_project_policy_value(
            &state.state,
            project.id,
            project.charter.revision,
            Some(1),
            valid(),
        )
        .unwrap_err();
        assert_eq!(unchanged.code, "unchanged_project_policy");

        let stale = publish_project_policy_value(
            &state.state,
            project.id,
            project.charter.revision,
            None,
            valid(),
        )
        .unwrap_err();
        assert_eq!(stale.code, "stale_project_policy");
    }

    #[test]
    fn mission_refusals_reach_the_interface_under_their_own_code() {
        let state = TestState::new();
        begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        complete_operator_setup_value(&state.state, true).unwrap();
        let project = create_project_value(&state.state, repository_project_draft()).unwrap();

        // Before this change every Mission refusal fell through to
        // "account_service_unavailable", so the five Mission `case` strings in
        // session.ts were unreachable. The two refusals reachable without a
        // local-model activation fixture are proven end to end here; the other
        // three arms are held by the exhaustive match rather than a wildcard.
        let missing = mission_value(&state.state, Uuid::new_v4()).unwrap_err();
        assert_eq!(missing.code, "mission_not_found");

        let invalid = admit_mission_value(
            &state.state,
            MissionDraft {
                project_id: project.id,
                expected_charter_revision: project.charter.revision,
                team_id: Uuid::new_v4(),
                desired_outcome: String::new(),
                success_conditions: vec!["Relevant gates pass".to_owned()],
                constraints: Vec::new(),
                prohibitions: Vec::new(),
                members: Vec::new(),
            },
        )
        .unwrap_err();
        assert_eq!(invalid.code, "invalid_mission");
    }

    #[test]
    fn preflight_refusals_reach_the_interface_under_their_own_code() {
        let state = TestState::new();
        begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        complete_operator_setup_value(&state.state, true).unwrap();
        let project = create_project_value(&state.state, repository_project_draft()).unwrap();

        // A Project with no Repository Binding has nothing to preflight. Before
        // the exhaustive error table this refusal reached the operator as
        // "the local account service is unavailable".
        let unbound = preflight_project_checks_value(&state.state, project.id).unwrap_err();
        assert_eq!(unbound.code, "repository_not_bound");

        let missing = preflight_project_checks_value(&state.state, Uuid::new_v4()).unwrap_err();
        assert_eq!(missing.code, "project_not_found");
    }

    fn repository_project_draft() -> ProjectDraft {
        ProjectDraft {
            name: "Repository Project".to_owned(),
            charter: ProjectCharterDraft {
                purpose: "Build the agent office.".to_owned(),
                desired_outcomes: vec!["Ship a governed vertical slice".to_owned()],
                constraints: vec!["Keep Workspace data local".to_owned()],
                prohibitions: vec!["Do not deploy publicly".to_owned()],
                autonomy_profile: AutonomyProfile::Supervised,
            },
        }
    }

    /// The declaration under test: a fabricated local path and a synthetic commit id, neither
    /// of which names anything that exists. The adapter never opens the path — this is a
    /// record, not a checkout.
    fn sample_repository_draft() -> RepositoryDraft {
        RepositoryDraft {
            source_kind: RepositorySourceKind::LocalPath,
            source: "/home/operator/projects/sample-tracker".to_owned(),
            base_revision: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_owned(),
            declared_checks: vec![DeclaredCheck {
                name: "pytest".to_owned(),
                argv: vec![
                    "pytest".to_owned(),
                    "tests/test_invariants.py".to_owned(),
                    "-q".to_owned(),
                ],
                working_directory: ".".to_owned(),
            }],
        }
    }

    #[test]
    fn command_adapter_binds_and_reads_project_repository_without_opening_it() {
        let state = TestState::new();
        begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        complete_operator_setup_value(&state.state, true).unwrap();
        let project = create_project_value(&state.state, repository_project_draft()).unwrap();
        assert!(
            current_project_repository_value(&state.state, project.id)
                .unwrap()
                .is_none()
        );

        let repository = bind_project_repository_value(
            &state.state,
            project.id,
            project.charter.revision,
            None,
            sample_repository_draft(),
        )
        .unwrap();

        let value = serde_json::to_value(&repository).unwrap();
        assert_eq!(value["revision"], 1);
        assert_eq!(value["authorKind"], "operator");
        assert_eq!(value["sourceKind"], "local_path");
        assert_eq!(value["source"], "/home/operator/projects/sample-tracker");
        assert_eq!(
            value["baseRevision"],
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
        );
        assert_eq!(value["declaredChecks"][0]["name"], "pytest");
        assert_eq!(value["declaredChecks"][0]["workingDirectory"], ".");
        assert_eq!(value["appliesToCurrentCharter"], true);
        assert!(value.get("worktreePath").is_none());
        assert!(value.get("checkResults").is_none());
        assert_eq!(
            current_project_repository_value(&state.state, project.id).unwrap(),
            Some(repository)
        );
        assert_eq!(
            project_repository_history_value(&state.state, project.id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn repository_refusals_reach_the_interface_under_their_own_code() {
        let state = TestState::new();
        begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        complete_operator_setup_value(&state.state, true).unwrap();
        let project = create_project_value(&state.state, repository_project_draft()).unwrap();

        let mut invalid = sample_repository_draft();
        invalid.base_revision = "0123456".to_owned();
        let refusal = bind_project_repository_value(
            &state.state,
            project.id,
            project.charter.revision,
            None,
            invalid,
        )
        .unwrap_err();
        assert_eq!(refusal.code, "invalid_project_repository");

        bind_project_repository_value(
            &state.state,
            project.id,
            project.charter.revision,
            None,
            sample_repository_draft(),
        )
        .unwrap();
        let unchanged = bind_project_repository_value(
            &state.state,
            project.id,
            project.charter.revision,
            Some(1),
            sample_repository_draft(),
        )
        .unwrap_err();
        assert_eq!(unchanged.code, "unchanged_project_repository");
        let stale = bind_project_repository_value(
            &state.state,
            project.id,
            project.charter.revision,
            None,
            sample_repository_draft(),
        )
        .unwrap_err();
        assert_eq!(stale.code, "stale_project_repository");
    }

    fn contains_workspace_material(value: &Value) -> bool {
        value.to_string().to_lowercase().contains("workspace_id")
            || value.to_string().to_lowercase().contains("workspace_name")
    }
}
