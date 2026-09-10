#![cfg(unix)]

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use data_encoding::BASE32_NOPAD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::{
    AccessOrigin, BeaconCapability, BeaconCapabilityDescriptor, CheckPreflightError,
    CheckPreflightService, DiskEncryptionService, ExportedOutcome, LiveDataEncryptionReport,
    MissionDraft, MissionError, MissionService, MissionView, OperatorAccountError,
    OperatorAccountService, OperatorSessionPolicy, OperatorSessionStatus, OutcomeService,
    PlatformSessionMonitor, PlatformSessionMonitorStatus, PreflightReceipt, ProjectCharter,
    ProjectCharterDraft, ProjectDraft, ProjectError, ProjectPolicy, ProjectPolicyDraft,
    ProjectPolicyError, ProjectPolicyService, ProjectRepository, ProjectRepositoryError,
    ProjectRepositoryService, ProjectService, ProjectSummary, ProjectView, PublicationSummary,
    PublishRefusal, RecoveryKey, RepositoryDraft, ResidentAgentView, ResidentConfiguration,
    ResidentConfigurationError, ResidentConfigurationService, ResidentLifecycleError,
    ResidentLifecycleService, ResourceDiscoveryError, ResourceDiscoveryService, SampleProjectError,
    SampleProjectInstallResult, SampleProjectService, SampleProjectStatus, SecretString,
    StarterOrganizationError, StarterOrganizationService, StarterOrganizationView,
    StarterResidentDraft, StartupConfig, Supervisor, TeamDraft, TeamError, TeamService, TeamView,
};

const PROTOCOL_VERSION: u16 = 1;
const AUTH_TOKEN_BYTES: usize = 32;
const MAX_MESSAGE_BYTES: u64 = 64 * 1024;
/// The completion family's REQUEST parser ceiling (`C4-transport-decision.md` 2-3).
///
/// A finite parser ceiling, not a resource grant: the only two commands that may use it are
/// `PreviewMissionCompletion` and `RecordMissionCompletion`, whose argument is one
/// `CompletionIntentRequest`. A conservative upper bound on that request's compact JSON --
/// `J <= 2*C + (J - 2*C)`, since permitted escaping at most doubles a scalar's UTF-8 bytes and the
/// owner caps `C` at 65,536 canonical bytes -- is 147,094 plus independently counted envelope
/// overhead; this constant covers that with margin, and `completion_ceiling_covers_the_grammar`
/// fails if the grammar ever outgrows it.
///
/// Nothing else moves: `MAX_MESSAGE_BYTES` remains every other command's accepted domain, its
/// refusal, and its refusal ORDER.
const MAX_COMPLETION_REQUEST_BYTES: u64 = 160 * 1024;
/// The completion family's RESPONSE parser ceiling, for `PreviewMissionCompletion` ALONE.
///
/// `RecordMissionCompletion` answers with one token and `GetMissionCompletionIntent` with a token or
/// null, so both keep the 64 KiB domain. Only the preview echoes the whole normalized request, plus
/// its bindings and the Mission's declared condition texts.
const MAX_COMPLETION_PREVIEW_RESPONSE_BYTES: u64 = 192 * 1024;
const SERVER_REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
// Exact activation review makes three independently bounded provider reads (tags, show, tags).
// Keep transport overhead outside that six-second worst case so a provider error is returned as
// a domain result rather than being mistaken for a broken supervisor socket.
const CLIENT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(7);
/// The deadline for commands that materialize a tree or run a subprocess.
///
/// `CLIENT_RESPONSE_TIMEOUT` above is a **socket read timeout**, so when it expires the client gets
/// `EAGAIN` -- `Io(WouldBlock)` -- while the supervisor is still working. Sized for the provider
/// reads, it is the wrong budget for `InstallSampleProject`, which copies a bundle and runs `git`:
/// that command's wall time scales with disk and process-spawn contention rather than with CPU, so
/// under a full parallel gate it inflates far past a budget a six-second worst case justified. Four
/// recorded flakes, all on that one command, all `Io(Os { code: 11, kind: WouldBlock })`.
///
/// A retry on `WouldBlock` would be the wrong shape here, and the distinction is the whole
/// diagnosis: `SO_RCVTIMEO` IS the bounded wait, so retrying only re-expresses a longer deadline in
/// a second place that can disagree with this one. (TF-1's ETXTBSY retry was the opposite case --
/// there the kernel refused an operation that would later succeed, and waiting was the only way to
/// make progress.)
const CLIENT_SLOW_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct LocalServerConfig {
    pub store_path: PathBuf,
    pub initial_workspace_name: String,
    pub socket_path: PathBuf,
    pub auth_token_path: PathBuf,
    pub lock_path: PathBuf,
    pub local_model_endpoint: crate::LocalModelEndpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalErrorCode {
    PasswordTooShort,
    PasswordTooLong,
    PasswordConfirmationMismatch,
    RecoveryNotConfirmed,
    SetupNotStarted,
    AccountAlreadyExists,
    AccountMissing,
    AuthenticationFailed,
    AuthenticationThrottled,
    CredentialDisabled,
    RemotePasswordNotAllowed,
    NotAuthenticated,
    InvalidSessionTimeout,
    UnsupportedStarterProposal,
    InvalidStarterRoster,
    StarterDecisionRecorded,
    StarterOrganizationUnavailable,
    ResidentNotFound,
    InvalidResidentTransition,
    StaleResidentDecision,
    ResidentLifecycleUnavailable,
    InvalidResidentConfiguration,
    StaleResidentConfiguration,
    ResidentConfigurationUnavailable,
    InvalidProject,
    ProjectNotFound,
    StaleCharterRevision,
    UnchangedCharter,
    ProjectUnavailable,
    InvalidProjectPolicy,
    StaleProjectPolicy,
    UnchangedProjectPolicy,
    ProjectPolicyUnavailable,
    InvalidProjectRepository,
    StaleProjectRepository,
    UnchangedProjectRepository,
    ProjectRepositoryUnavailable,
    RepositoryNotBound,
    PreflightUnavailable,
    SampleProjectUnavailable,
    InvalidTeam,
    TeamNotFound,
    StaleTeamMemberConfiguration,
    TeamMemberIneligible,
    TeamUnavailable,
    InvalidMission,
    MissionNotFound,
    StaleMissionTeam,
    MissionMemberIneligible,
    MissionUnavailable,
    MissionCompletionUnavailable,
    CompletionIntentChanged,
    CompletionAlreadyRecorded,
    CompletionResidentNotInAdmissionRoster,
    CompletionConditionCoverageIncomplete,
    CompletionSupportOverLimit,
    CompletionTextOverLimit,
    CompletionSpendReasonRequired,
    CompletionInvalidDetails,
    OutcomeAbsent,
    OutcomeUnavailable,
    LocalModelUnavailable,
    LocalModelNotFound,
    LocalModelChanged,
    InvalidLocalModelApproval,
    LocalModelAlreadyActive,
    StaleLocalModelDecision,
    LocalModelActivationUnavailable,
    AccountServiceUnavailable,
    UnauthorizedClient,
    ProtocolError,
}

#[derive(Debug)]
pub enum LocalTransportError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidTokenFile,
    InsecurePermissions,
    Protocol,
    Remote {
        code: LocalErrorCode,
        retry_after_ms: Option<u64>,
    },
    Supervisor(crate::SupervisorError),
    Account(OperatorAccountError),
    StarterOrganization(StarterOrganizationError),
    ResidentLifecycle(ResidentLifecycleError),
    ResidentConfiguration(ResidentConfigurationError),
    Project(ProjectError),
    ProjectPolicy(ProjectPolicyError),
    ProjectRepository(ProjectRepositoryError),
    CheckPreflight(CheckPreflightError),
    RecentReceipt(crate::RecentReceiptError),
    SampleProject(SampleProjectError),
    Team(TeamError),
    Mission(MissionError),
    MissionCompletionRead(crate::MissionCompletionReadError),
    MissionCompletionCommand(crate::CompletionCommandError),
    ResourceDiscovery(ResourceDiscoveryError),
    LocalModelActivation(crate::LocalModelActivationError),
    Outcome(PublishRefusal),
}

/// The account fault's NAME, for a caller of the local transport (R42).
///
/// A third caller-owned copy of this projection, beside `mission_impact.rs`'s and
/// `outcome_publication.rs`'s. Deliberate: each caller owns the text it emits, and this one's
/// audience -- an in-process transport caller who may not own the row at all -- is the narrowest
/// of the three. `OperatorAccountError` has no `name()` of its own and this slice's fence forbids
/// adding one to `operator_account.rs`, so the match lives here and is exhaustive: a new account
/// fault is a compile error at this caller, not a new sentence nobody read.
fn account_tag(error: &OperatorAccountError) -> &'static str {
    match error {
        OperatorAccountError::PasswordTooShort => "PasswordTooShort",
        OperatorAccountError::PasswordTooLong => "PasswordTooLong",
        OperatorAccountError::PasswordConfirmationMismatch => "PasswordConfirmationMismatch",
        OperatorAccountError::RecoveryNotConfirmed => "RecoveryNotConfirmed",
        OperatorAccountError::SetupNotStarted => "SetupNotStarted",
        OperatorAccountError::AccountAlreadyExists => "AccountAlreadyExists",
        OperatorAccountError::AccountMissing => "AccountMissing",
        OperatorAccountError::AuthenticationFailed => "AuthenticationFailed",
        OperatorAccountError::AuthenticationThrottled { .. } => "AuthenticationThrottled",
        OperatorAccountError::CredentialDisabled => "CredentialDisabled",
        OperatorAccountError::RemotePasswordNotAllowed => "RemotePasswordNotAllowed",
        OperatorAccountError::NotAuthenticated => "NotAuthenticated",
        OperatorAccountError::InvalidSessionTimeout => "InvalidSessionTimeout",
        OperatorAccountError::CredentialProcessing => "CredentialProcessing",
        OperatorAccountError::InvalidStore => "InvalidStore",
        OperatorAccountError::ClockBeforeUnixEpoch => "ClockBeforeUnixEpoch",
        OperatorAccountError::Store(_) => "Store",
        OperatorAccountError::StateUnavailable => "StateUnavailable",
    }
}

/// Which part of a repository draft was refused. A closed three-name vocabulary; the draft's own
/// content -- source URL, revision, declared checks -- never reaches the caller line.
fn invalid_draft_tag(reason: &crate::InvalidRepositoryDraft) -> &'static str {
    match reason {
        crate::InvalidRepositoryDraft::Source => "Source",
        crate::InvalidRepositoryDraft::BaseRevision => "BaseRevision",
        crate::InvalidRepositoryDraft::DeclaredChecks => "DeclaredChecks",
    }
}

/// `RecentReceiptError` has no `name()` and its file is outside this slice's fence, so the three
/// names are written here.
fn recent_receipt_tag(error: &crate::RecentReceiptError) -> String {
    match error {
        crate::RecentReceiptError::Account(cause) => format!("Account.{}", account_tag(cause)),
        crate::RecentReceiptError::InvalidStore => "InvalidStore".to_owned(),
        crate::RecentReceiptError::Store(_) => "Store".to_owned(),
    }
}

/// `ResourceDiscoveryError` likewise. `EndpointNotLoopback` carries the endpoint it refused; the
/// name says which check failed without repeating the address.
fn resource_discovery_tag(error: &ResourceDiscoveryError) -> String {
    match error {
        ResourceDiscoveryError::Account(cause) => format!("Account.{}", account_tag(cause)),
        ResourceDiscoveryError::EndpointNotLoopback => "EndpointNotLoopback".to_owned(),
    }
}

fn starter_organization_tag(error: &StarterOrganizationError) -> String {
    match error {
        StarterOrganizationError::Account(cause) => format!("Account.{}", account_tag(cause)),
        named @ (StarterOrganizationError::UnsupportedProposalVersion
        | StarterOrganizationError::EmptyRoster
        | StarterOrganizationError::DuplicateRole
        | StarterOrganizationError::InvalidDisplayName
        | StarterOrganizationError::DecisionAlreadyRecorded
        | StarterOrganizationError::InvalidStore
        | StarterOrganizationError::Store(_)
        | StarterOrganizationError::Json(_)) => named.name().to_owned(),
    }
}

fn resident_lifecycle_tag(error: &ResidentLifecycleError) -> String {
    match error {
        ResidentLifecycleError::Account(cause) => format!("Account.{}", account_tag(cause)),
        named @ (ResidentLifecycleError::NotFound
        | ResidentLifecycleError::InvalidTransition
        | ResidentLifecycleError::StaleDecision
        | ResidentLifecycleError::InvalidStore
        | ResidentLifecycleError::Store(_)
        | ResidentLifecycleError::Json(_)) => named.name().to_owned(),
    }
}

fn resident_configuration_tag(error: &ResidentConfigurationError) -> String {
    match error {
        ResidentConfigurationError::Account(cause) => format!("Account.{}", account_tag(cause)),
        named @ (ResidentConfigurationError::ResidentNotFound
        | ResidentConfigurationError::ResidentRetired
        | ResidentConfigurationError::ModelActivationUnavailable
        | ResidentConfigurationError::ModelMissingCompletion
        | ResidentConfigurationError::InvalidCapabilities
        | ResidentConfigurationError::StaleDecision
        | ResidentConfigurationError::AlreadyCleared
        | ResidentConfigurationError::InvalidStore
        | ResidentConfigurationError::Store(_)
        | ResidentConfigurationError::Json(_)) => named.name().to_owned(),
    }
}

fn project_tag(error: &ProjectError) -> String {
    match error {
        ProjectError::Account(cause) => format!("Account.{}", account_tag(cause)),
        ProjectError::StaleCharterRevision { expected, current } => {
            format!("StaleCharterRevision(expected={expected},current={current})")
        }
        named @ (ProjectError::InvalidName
        | ProjectError::InvalidPurpose
        | ProjectError::InvalidDesiredOutcomes
        | ProjectError::InvalidConstraint
        | ProjectError::InvalidProhibition
        | ProjectError::DuplicateCharterEntry
        | ProjectError::UnchangedCharter
        | ProjectError::NotFound
        | ProjectError::InvalidStore
        | ProjectError::Store(_)
        | ProjectError::Json(_)) => named.name().to_owned(),
    }
}

fn project_policy_tag(error: &ProjectPolicyError) -> String {
    match error {
        ProjectPolicyError::Account(cause) => format!("Account.{}", account_tag(cause)),
        ProjectPolicyError::StaleCharterRevision { expected, current } => {
            format!("StaleCharterRevision(expected={expected},current={current})")
        }
        named @ (ProjectPolicyError::ProjectNotFound
        | ProjectPolicyError::StalePolicyRevision
        | ProjectPolicyError::InvalidRequiredCapabilities
        | ProjectPolicyError::InvalidTrackRecordRequirements
        | ProjectPolicyError::UnchangedPolicy
        | ProjectPolicyError::InvalidStore
        | ProjectPolicyError::Store(_)
        | ProjectPolicyError::Json(_)) => named.name().to_owned(),
    }
}

fn project_repository_tag(error: &ProjectRepositoryError) -> String {
    match error {
        ProjectRepositoryError::Account(cause) => format!("Account.{}", account_tag(cause)),
        ProjectRepositoryError::StaleCharterRevision { expected, current } => {
            format!("StaleCharterRevision(expected={expected},current={current})")
        }
        ProjectRepositoryError::InvalidDraft(reason) => {
            format!("InvalidDraft.{}", invalid_draft_tag(reason))
        }
        named @ (ProjectRepositoryError::ProjectNotFound
        | ProjectRepositoryError::StaleRepositoryRevision
        | ProjectRepositoryError::UnchangedRepository
        | ProjectRepositoryError::InvalidStore
        | ProjectRepositoryError::Store(_)
        | ProjectRepositoryError::Json(_)) => named.name().to_owned(),
    }
}

fn check_preflight_tag(error: &CheckPreflightError) -> String {
    match error {
        CheckPreflightError::Account(cause) => format!("Account.{}", account_tag(cause)),
        named @ (CheckPreflightError::ProjectNotFound
        | CheckPreflightError::RepositoryNotBound
        | CheckPreflightError::InvalidStore
        | CheckPreflightError::Store(_)
        | CheckPreflightError::Json(_)) => named.name().to_owned(),
    }
}

fn sample_project_tag(error: &SampleProjectError) -> String {
    match error {
        SampleProjectError::Account(cause) => format!("Account.{}", account_tag(cause)),
        named @ (SampleProjectError::NotInstalled
        | SampleProjectError::UnsafeDestination
        | SampleProjectError::BundleMismatch
        | SampleProjectError::MaterializationFailed
        | SampleProjectError::TamperedInstallation
        | SampleProjectError::InvalidStore
        | SampleProjectError::Store(_)
        | SampleProjectError::Io(_)
        | SampleProjectError::Json(_)) => named.name().to_owned(),
    }
}

fn team_tag(error: &TeamError) -> String {
    match error {
        TeamError::Account(cause) => format!("Account.{}", account_tag(cause)),
        named @ (TeamError::ProjectNotFound
        | TeamError::TeamNotFound
        | TeamError::InvalidName
        | TeamError::InvalidMembers
        | TeamError::ResidentConfigurationUnavailable
        | TeamError::StaleResidentConfiguration
        | TeamError::ResidentIneligible(_)
        | TeamError::InvalidStore
        | TeamError::Store(_)
        | TeamError::Json(_)) => named.name().to_owned(),
    }
}

fn mission_tag(error: &MissionError) -> String {
    match error {
        MissionError::Account(cause) => format!("Account.{}", account_tag(cause)),
        MissionError::StaleCharterRevision { expected, current } => {
            format!("StaleCharterRevision(expected={expected},current={current})")
        }
        named @ (MissionError::ProjectNotFound
        | MissionError::TeamNotFound
        | MissionError::MissionNotFound
        | MissionError::InvalidDesiredOutcome
        | MissionError::InvalidSuccessConditions
        | MissionError::InvalidConstraint
        | MissionError::InvalidProhibition
        | MissionError::DuplicateMissionEntry
        | MissionError::InvalidMembers
        | MissionError::StaleTeamEligibility
        | MissionError::ResidentIneligible(_)
        | MissionError::InvalidStore
        | MissionError::Store(_)
        | MissionError::Json(_)) => named.name().to_owned(),
    }
}

fn local_model_activation_tag(error: &crate::LocalModelActivationError) -> String {
    match error {
        crate::LocalModelActivationError::Account(cause) => {
            format!("Account.{}", account_tag(cause))
        }
        named @ (crate::LocalModelActivationError::ResourceUnavailable
        | crate::LocalModelActivationError::InvalidProviderResponse
        | crate::LocalModelActivationError::ModelNotFound
        | crate::LocalModelActivationError::ModelDrift
        | crate::LocalModelActivationError::InvalidApproval
        | crate::LocalModelActivationError::AlreadyActive
        | crate::LocalModelActivationError::StaleDecision
        | crate::LocalModelActivationError::InvalidStore
        | crate::LocalModelActivationError::Store(_)
        | crate::LocalModelActivationError::Json(_)) => named.name().to_owned(),
    }
}

impl fmt::Display for LocalTransportError {
    /// R42: one closed line per arm. The child's own `Display` is the child's business -- it is
    /// written for whoever owns that row -- and a transport caller may own none of it. Every arm
    /// that used to interpolate `{error}` now interpolates a NAME through a private borrowed
    /// selector, and every original value stays in the derived `Debug`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => write!(formatter, "local transport I/O failed: Io"),
            Self::Json(_) => write!(formatter, "local transport message was invalid: Json"),
            Self::InvalidTokenFile => write!(formatter, "local transport token is invalid"),
            Self::InsecurePermissions => {
                write!(
                    formatter,
                    "local transport path permissions are not private"
                )
            }
            Self::Protocol => write!(formatter, "local transport protocol mismatch"),
            // Reviewed: `LocalErrorCode` is 62 unit variants with no payload, so its `Debug` is a
            // closed name vocabulary. Pinned by the code table in the wire witness -- a
            // payload-bearing code would break that table before it could reach this line.
            Self::Remote { code, .. } => write!(formatter, "local supervisor refused: {code:?}"),
            Self::Supervisor(error) => {
                write!(
                    formatter,
                    "supervisor startup failed: {}",
                    crate::supervisor_caller_tag(error)
                )
            }
            Self::Account(error) => {
                write!(formatter, "account service failed: {}", account_tag(error))
            }
            Self::StarterOrganization(error) => {
                write!(
                    formatter,
                    "starter organization failed: {}",
                    starter_organization_tag(error)
                )
            }
            Self::ResidentLifecycle(error) => {
                write!(
                    formatter,
                    "Resident lifecycle failed: {}",
                    resident_lifecycle_tag(error)
                )
            }
            Self::ResidentConfiguration(error) => {
                write!(
                    formatter,
                    "Resident configuration failed: {}",
                    resident_configuration_tag(error)
                )
            }
            Self::Project(error) => {
                write!(
                    formatter,
                    "Project operation failed: {}",
                    project_tag(error)
                )
            }
            Self::ProjectPolicy(error) => {
                write!(
                    formatter,
                    "Project Policy failed: {}",
                    project_policy_tag(error)
                )
            }
            // Already closed by slice 1: the publisher's own line is the reviewed closed
            // projection, so forwarding it is R37.4 delegation, not disclosure.
            Self::Outcome(error) => write!(formatter, "Outcome failed: {error}"),
            Self::ProjectRepository(error) => {
                write!(
                    formatter,
                    "Project Repository failed: {}",
                    project_repository_tag(error)
                )
            }
            Self::CheckPreflight(error) => {
                write!(
                    formatter,
                    "check preflight failed: {}",
                    check_preflight_tag(error)
                )
            }
            Self::RecentReceipt(error) => {
                write!(
                    formatter,
                    "recent Receipts failed: {}",
                    recent_receipt_tag(error)
                )
            }
            Self::SampleProject(error) => {
                write!(
                    formatter,
                    "Sample Project failed: {}",
                    sample_project_tag(error)
                )
            }
            Self::Team(error) => write!(formatter, "Team operation failed: {}", team_tag(error)),
            // ONE closed literal: `MissionCompletionReadError`'s own Display is already a closed
            // token, and nothing beneath it -- driver text, a path, a note an operator wrote -- is
            // allowed onto this line.
            Self::MissionCompletionRead(error) => {
                write!(
                    formatter,
                    "Mission completion read failed: {}",
                    error.name()
                )
            }
            // ONE closed literal, the owner's own outward token. `MalformedRequest` and
            // `DuplicateSetEntry` each carry the field they refused; that field stays in the derived
            // `Debug`, never on this line and never in the wire code.
            Self::MissionCompletionCommand(error) => {
                write!(
                    formatter,
                    "Mission completion command failed: {}",
                    error.name()
                )
            }
            Self::Mission(error) => {
                write!(
                    formatter,
                    "Mission operation failed: {}",
                    mission_tag(error)
                )
            }
            Self::ResourceDiscovery(error) => {
                write!(
                    formatter,
                    "resource discovery failed: {}",
                    resource_discovery_tag(error)
                )
            }
            Self::LocalModelActivation(error) => {
                write!(
                    formatter,
                    "local model activation failed: {}",
                    local_model_activation_tag(error)
                )
            }
        }
    }
}
impl std::error::Error for LocalTransportError {}

impl From<std::io::Error> for LocalTransportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for LocalTransportError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<crate::SupervisorError> for LocalTransportError {
    fn from(error: crate::SupervisorError) -> Self {
        Self::Supervisor(error)
    }
}

impl From<OperatorAccountError> for LocalTransportError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<StarterOrganizationError> for LocalTransportError {
    fn from(error: StarterOrganizationError) -> Self {
        Self::StarterOrganization(error)
    }
}

impl From<ResidentLifecycleError> for LocalTransportError {
    fn from(error: ResidentLifecycleError) -> Self {
        Self::ResidentLifecycle(error)
    }
}

impl From<ResidentConfigurationError> for LocalTransportError {
    fn from(error: ResidentConfigurationError) -> Self {
        Self::ResidentConfiguration(error)
    }
}

impl From<ProjectError> for LocalTransportError {
    fn from(error: ProjectError) -> Self {
        Self::Project(error)
    }
}

impl From<ProjectPolicyError> for LocalTransportError {
    fn from(error: ProjectPolicyError) -> Self {
        Self::ProjectPolicy(error)
    }
}

impl From<PublishRefusal> for LocalTransportError {
    fn from(error: PublishRefusal) -> Self {
        Self::Outcome(error)
    }
}

impl From<ProjectRepositoryError> for LocalTransportError {
    fn from(error: ProjectRepositoryError) -> Self {
        Self::ProjectRepository(error)
    }
}

impl From<CheckPreflightError> for LocalTransportError {
    fn from(error: CheckPreflightError) -> Self {
        Self::CheckPreflight(error)
    }
}

impl From<crate::RecentReceiptError> for LocalTransportError {
    fn from(error: crate::RecentReceiptError) -> Self {
        Self::RecentReceipt(error)
    }
}

impl From<SampleProjectError> for LocalTransportError {
    fn from(error: SampleProjectError) -> Self {
        Self::SampleProject(error)
    }
}

impl From<TeamError> for LocalTransportError {
    fn from(error: TeamError) -> Self {
        Self::Team(error)
    }
}

impl From<crate::MissionCompletionReadError> for LocalTransportError {
    fn from(error: crate::MissionCompletionReadError) -> Self {
        Self::MissionCompletionRead(error)
    }
}

impl From<crate::CompletionCommandError> for LocalTransportError {
    fn from(error: crate::CompletionCommandError) -> Self {
        Self::MissionCompletionCommand(error)
    }
}

impl From<MissionError> for LocalTransportError {
    fn from(error: MissionError) -> Self {
        Self::Mission(error)
    }
}

impl From<ResourceDiscoveryError> for LocalTransportError {
    fn from(error: ResourceDiscoveryError) -> Self {
        Self::ResourceDiscovery(error)
    }
}

impl From<crate::LocalModelActivationError> for LocalTransportError {
    fn from(error: crate::LocalModelActivationError) -> Self {
        Self::LocalModelActivation(error)
    }
}

struct LocalAuthToken(Zeroizing<[u8; AUTH_TOKEN_BYTES]>);

impl LocalAuthToken {
    fn generate() -> Result<Self, LocalTransportError> {
        let mut value = Zeroizing::new([0_u8; AUTH_TOKEN_BYTES]);
        getrandom::fill(value.as_mut()).map_err(|_| LocalTransportError::InvalidTokenFile)?;
        Ok(Self(value))
    }

    fn encode(&self) -> Zeroizing<String> {
        Zeroizing::new(BASE32_NOPAD.encode(self.0.as_ref()))
    }

    fn decode(value: &str) -> Result<Self, LocalTransportError> {
        let decoded = BASE32_NOPAD
            .decode(value.as_bytes())
            .map_err(|_| LocalTransportError::InvalidTokenFile)?;
        let token: [u8; AUTH_TOKEN_BYTES] = decoded
            .try_into()
            .map_err(|_| LocalTransportError::InvalidTokenFile)?;
        Ok(Self(Zeroizing::new(token)))
    }

    fn matches_encoded(&self, candidate: &str) -> bool {
        Self::decode(candidate)
            .map(|candidate| bool::from(self.0.ct_eq(candidate.0.as_ref())))
            .unwrap_or(false)
    }
}

pub fn initialize_local_auth_token(path: impl AsRef<Path>) -> Result<(), LocalTransportError> {
    let path = path.as_ref();
    verify_private_parent(path)?;
    if path.exists() {
        load_local_auth_token(path)?;
        return Ok(());
    }

    let token = LocalAuthToken::generate()?;
    let encoded = token.encode();
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(encoded.as_bytes())?;
            file.sync_all()?;
            ensure_private_regular_file(&file)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            load_local_auth_token(path).map(|_| ())
        }
        Err(error) => Err(error.into()),
    }
}

fn load_local_auth_token(path: &Path) -> Result<LocalAuthToken, LocalTransportError> {
    verify_private_parent(path)?;
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path)?;
    ensure_private_regular_file(&file)?;
    let mut encoded = Zeroizing::new(String::new());
    file.take(128).read_to_string(&mut encoded)?;
    if encoded.len() != 52 {
        return Err(LocalTransportError::InvalidTokenFile);
    }
    LocalAuthToken::decode(&encoded)
}

fn verify_private_parent(path: &Path) -> Result<(), LocalTransportError> {
    let parent = path
        .parent()
        .ok_or(LocalTransportError::InsecurePermissions)?;
    let metadata = fs::metadata(parent)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(LocalTransportError::InsecurePermissions);
    }
    Ok(())
}

fn ensure_private_regular_file(file: &File) -> Result<(), LocalTransportError> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(LocalTransportError::InsecurePermissions);
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct WireSecret(#[serde(with = "zeroizing_string")] Zeroizing<String>);

impl WireSecret {
    fn from_secret(value: SecretString) -> Self {
        Self(value.into_inner())
    }

    fn into_secret(self) -> SecretString {
        SecretString::from_inner(self.0)
    }

    fn from_recovery(value: RecoveryKey) -> Self {
        Self(value.into_inner())
    }

    fn into_recovery(self) -> RecoveryKey {
        RecoveryKey::from_inner(self.0)
    }

    fn expose(&self) -> &str {
        self.0.as_str()
    }
}

mod zeroizing_string {
    use super::*;

    pub fn serialize<S>(value: &Zeroizing<String>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(value.as_str())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Zeroizing<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Zeroizing::new)
    }
}

/// Which commands may exceed `MAX_MESSAGE_BYTES` as a REQUEST.
///
/// Exactly the two that carry a `CompletionIntentRequest`. `GetMissionCompletionIntent` is
/// deliberately absent: its argument is one UUID, so granting it the larger domain would widen a
/// parser ceiling for a command that cannot need it.
fn accepts_completion_sized_request(command: &LocalCommand) -> bool {
    matches!(
        command,
        LocalCommand::PreviewMissionCompletion { .. }
            | LocalCommand::RecordMissionCompletion { .. }
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestEnvelope {
    protocol: u16,
    token: WireSecret,
    command: LocalCommand,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
enum LocalCommand {
    Status,
    SessionPolicy,
    SetSessionTimeout {
        inactivity_timeout_minutes: u16,
    },
    PlatformSessionMonitorStatus,
    BeginPasswordSetup {
        password: WireSecret,
        confirmation: WireSecret,
    },
    CompleteOperatorSetup {
        recovery_saved: bool,
    },
    AuthenticatePassword {
        password: WireSecret,
        origin: AccessOrigin,
    },
    RecoverWithKey {
        recovery_key: WireSecret,
        new_password: WireSecret,
        confirmation: WireSecret,
        origin: AccessOrigin,
    },
    Lock,
    RecordOperatorActivity,
    StarterOrganization,
    AcceptStarterOrganization {
        proposal_version: u16,
        residents: Vec<StarterResidentDraft>,
    },
    DiscardStarterOrganization {
        proposal_version: u16,
    },
    Residents,
    ActivateResident {
        resident_id: uuid::Uuid,
        expected_decision_id: Option<uuid::Uuid>,
    },
    SuspendResident {
        resident_id: uuid::Uuid,
        expected_decision_id: Option<uuid::Uuid>,
    },
    ReactivateResident {
        resident_id: uuid::Uuid,
        expected_decision_id: Option<uuid::Uuid>,
    },
    RetireResident {
        resident_id: uuid::Uuid,
        expected_decision_id: Option<uuid::Uuid>,
    },
    ResidentCapabilityCatalog,
    ResidentConfigurations,
    ConfigureResident {
        resident_id: uuid::Uuid,
        expected_decision_id: Option<uuid::Uuid>,
        activation_id: uuid::Uuid,
        capabilities: Vec<BeaconCapability>,
    },
    ClearResidentConfiguration {
        resident_id: uuid::Uuid,
        expected_decision_id: uuid::Uuid,
    },
    Projects,
    SampleProjectStatus,
    InstallSampleProject,
    Project {
        project_id: uuid::Uuid,
    },
    ProjectCharterHistory {
        project_id: uuid::Uuid,
    },
    CreateProject {
        draft: ProjectDraft,
    },
    ReviseProjectCharter {
        project_id: uuid::Uuid,
        expected_current_revision: u32,
        draft: ProjectCharterDraft,
    },
    ProjectPolicy {
        project_id: uuid::Uuid,
    },
    ProjectPolicyHistory {
        project_id: uuid::Uuid,
    },
    MissionPublications {
        mission_id: uuid::Uuid,
    },
    OutcomeDocument {
        mission_id: uuid::Uuid,
        publication_version: Option<u32>,
    },
    PublishProjectPolicy {
        project_id: uuid::Uuid,
        expected_charter_revision: u32,
        expected_policy_revision: Option<u32>,
        draft: ProjectPolicyDraft,
    },
    CurrentProjectRepository {
        project_id: uuid::Uuid,
    },
    ProjectRepositoryHistory {
        project_id: uuid::Uuid,
    },
    BindProjectRepository {
        project_id: uuid::Uuid,
        expected_charter_revision: u32,
        expected_prior_repository_revision: Option<u32>,
        draft: RepositoryDraft,
    },
    PreflightProjectChecks {
        project_id: uuid::Uuid,
    },
    RecentReceipts {
        window: u32,
    },
    Teams {
        project_id: uuid::Uuid,
    },
    Team {
        team_id: uuid::Uuid,
    },
    FormTeam {
        draft: TeamDraft,
    },
    Missions {
        project_id: uuid::Uuid,
    },
    Mission {
        mission_id: uuid::Uuid,
    },
    MissionCompletion {
        mission_id: uuid::Uuid,
    },
    /// Derive the whole intended record and its reviewed-intent token. Writes nothing.
    PreviewMissionCompletion {
        request: crate::CompletionIntentRequest,
    },
    /// Record the reviewed completion. The token is what the Operator reviewed, not a fresh one.
    RecordMissionCompletion {
        request: crate::CompletionIntentRequest,
        reviewed_intent_sha256: String,
    },
    /// The token of the completion visible for this Mission in one snapshot, or null.
    GetMissionCompletionIntent {
        mission_id: uuid::Uuid,
    },
    AdmitMission {
        draft: MissionDraft,
    },
    DiscoverLocalModels,
    LiveDataEncryptionStatus,
    InspectLocalModel {
        model_name: String,
        expected_digest: String,
    },
    ActivateLocalModel {
        review: crate::LocalModelActivationReview,
        approved_features: Vec<crate::ProviderFeature>,
    },
    LocalModelActivations,
    DeactivateLocalModel {
        expected_activation_id: uuid::Uuid,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseEnvelope {
    protocol: u16,
    result: Option<LocalResult>,
    error: Option<WireError>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum LocalResult {
    SessionStatus {
        status: OperatorSessionStatus,
    },
    SessionPolicy {
        policy: OperatorSessionPolicy,
    },
    PlatformSessionMonitorStatus {
        status: PlatformSessionMonitorStatus,
    },
    RecoveryKey {
        recovery_key: WireSecret,
    },
    StarterOrganization {
        organization: StarterOrganizationView,
    },
    Residents {
        residents: Vec<ResidentAgentView>,
    },
    Resident {
        resident: ResidentAgentView,
    },
    ResidentCapabilityCatalog {
        capabilities: Vec<BeaconCapabilityDescriptor>,
    },
    ResidentConfigurations {
        configurations: Vec<ResidentConfiguration>,
    },
    ResidentConfiguration {
        configuration: ResidentConfiguration,
    },
    Projects {
        projects: Vec<ProjectSummary>,
    },
    SampleProjectStatus {
        status: SampleProjectStatus,
    },
    SampleProjectInstallResult {
        result: SampleProjectInstallResult,
    },
    Project {
        project: ProjectView,
    },
    ProjectCharterHistory {
        charters: Vec<ProjectCharter>,
    },
    ProjectPolicy {
        policy: Option<ProjectPolicy>,
    },
    ProjectPolicyHistory {
        policies: Vec<ProjectPolicy>,
    },
    MissionPublications {
        publications: Vec<PublicationSummary>,
    },
    OutcomeDocument {
        document: ExportedOutcome,
    },
    ProjectRepository {
        repository: Option<ProjectRepository>,
    },
    ProjectRepositoryHistory {
        repositories: Vec<ProjectRepository>,
    },
    PreflightReceipt {
        receipt: PreflightReceipt,
    },
    RecentReceipts {
        page: crate::RecentReceiptPage,
    },
    Teams {
        teams: Vec<TeamView>,
    },
    Team {
        team: TeamView,
    },
    Missions {
        missions: Vec<MissionView>,
    },
    MissionCompletion {
        completion: Option<crate::CompletionView>,
    },
    MissionCompletionPreview {
        preview: crate::CompletionPreview,
    },
    MissionCompletionAcknowledgement {
        intent: crate::ReviewedIntent,
    },
    MissionCompletionIntent {
        intent: Option<crate::ReviewedIntent>,
    },
    Mission {
        mission: MissionView,
    },
    LocalModelDiscovery {
        report: crate::LocalModelDiscoveryReport,
    },
    LiveDataEncryption {
        report: LiveDataEncryptionReport,
    },
    LocalModelActivationReview {
        review: crate::LocalModelActivationReview,
    },
    LocalModelActivation {
        activation: crate::LocalModelActivation,
    },
    LocalModelActivations {
        activations: Vec<crate::LocalModelActivation>,
    },
    Empty,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireError {
    code: LocalErrorCode,
    retry_after_ms: Option<u64>,
}

pub struct LocalSupervisorServer {
    listener: UnixListener,
    socket_path: PathBuf,
    _lock_file: File,
    token: LocalAuthToken,
    accounts: Arc<OperatorAccountService>,
    starter_organization: StarterOrganizationService,
    resident_lifecycle: ResidentLifecycleService,
    resident_configuration: ResidentConfigurationService,
    projects: ProjectService,
    sample_projects: SampleProjectService,
    project_policies: ProjectPolicyService,
    project_repositories: ProjectRepositoryService,
    check_preflights: CheckPreflightService,
    recent_receipts: crate::RecentReceiptService,
    teams: TeamService,
    missions: MissionService,
    mission_completions: crate::MissionCompletionReadService,
    completion_commands: crate::MissionCompletionCommandService,
    outcomes: OutcomeService,
    resource_discovery: ResourceDiscoveryService,
    disk_encryption: DiskEncryptionService,
    local_model_activation: crate::LocalModelActivationService,
    platform_monitor: PlatformSessionMonitor,
}

impl LocalSupervisorServer {
    pub fn bind(config: LocalServerConfig) -> Result<Self, LocalTransportError> {
        verify_private_parent(&config.socket_path)?;
        initialize_local_auth_token(&config.auth_token_path)?;
        let lock_file = acquire_supervisor_lock(&config.lock_path)?;
        if config.socket_path.exists() {
            let metadata = fs::symlink_metadata(&config.socket_path)?;
            if !metadata.file_type().is_socket() {
                return Err(LocalTransportError::Protocol);
            }
            fs::remove_file(&config.socket_path)?;
        }
        let report = Supervisor::bootstrap(StartupConfig {
            store_path: config.store_path.clone(),
            initial_workspace_name: config.initial_workspace_name,
        })?;
        let accounts = Arc::new(OperatorAccountService::open(&config.store_path)?);
        let starter_organization =
            StarterOrganizationService::new(Arc::clone(&accounts), report.workspace.id);
        let resident_lifecycle =
            ResidentLifecycleService::new(Arc::clone(&accounts), report.workspace.id);
        let resident_configuration =
            ResidentConfigurationService::new(Arc::clone(&accounts), report.workspace.id);
        let projects = ProjectService::new(Arc::clone(&accounts), report.workspace.id);
        let sample_projects = SampleProjectService::new(
            Arc::clone(&accounts),
            report.workspace.id,
            &config.store_path,
        )?;
        let project_policies =
            ProjectPolicyService::new(Arc::clone(&accounts), report.workspace.id);
        let project_repositories =
            ProjectRepositoryService::new(Arc::clone(&accounts), report.workspace.id);
        let check_preflights =
            CheckPreflightService::new(Arc::clone(&accounts), report.workspace.id);
        let recent_receipts =
            crate::RecentReceiptService::new(Arc::clone(&accounts), report.workspace.id);
        let teams = TeamService::new(Arc::clone(&accounts), report.workspace.id);
        let missions = MissionService::new(Arc::clone(&accounts), report.workspace.id);
        let mission_completions =
            crate::MissionCompletionReadService::new(Arc::clone(&accounts), report.workspace.id);
        let completion_commands =
            crate::MissionCompletionCommandService::new(Arc::clone(&accounts), report.workspace.id);
        let outcomes = OutcomeService::new(Arc::clone(&accounts), report.workspace.id);
        let resource_discovery =
            ResourceDiscoveryService::new(Arc::clone(&accounts), config.local_model_endpoint);
        let disk_encryption =
            DiskEncryptionService::new(Arc::clone(&accounts), config.store_path.clone());
        let local_model_activation = crate::LocalModelActivationService::new(
            Arc::clone(&accounts),
            report.workspace.id,
            config.local_model_endpoint,
        );
        let token = load_local_auth_token(&config.auth_token_path)?;
        let listener = UnixListener::bind(&config.socket_path)?;
        fs::set_permissions(&config.socket_path, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(&config.socket_path)?;
        if !metadata.file_type().is_socket() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(LocalTransportError::InsecurePermissions);
        }
        Ok(Self {
            listener,
            socket_path: config.socket_path,
            _lock_file: lock_file,
            token,
            accounts,
            starter_organization,
            resident_lifecycle,
            resident_configuration,
            projects,
            sample_projects,
            project_policies,
            project_repositories,
            check_preflights,
            recent_receipts,
            teams,
            missions,
            mission_completions,
            completion_commands,
            outcomes,
            resource_discovery,
            disk_encryption,
            local_model_activation,
            platform_monitor: PlatformSessionMonitor::new(),
        })
    }

    pub fn serve(self) -> Result<(), LocalTransportError> {
        self.platform_monitor.start(Arc::clone(&self.accounts));
        loop {
            let (stream, _) = self.listener.accept()?;
            self.handle_connection(stream);
        }
    }

    pub fn serve_until(self, running: Arc<AtomicBool>) -> Result<(), LocalTransportError> {
        self.listener.set_nonblocking(true)?;
        while running.load(Ordering::Acquire) {
            match self.listener.accept() {
                Ok((stream, _)) => self.handle_connection(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn handle_connection(&self, mut stream: UnixStream) {
        let response = configure_stream_deadlines(&stream, SERVER_REQUEST_TIMEOUT)
            .and_then(|()| self.read_request(&mut stream))
            .and_then(|request| self.dispatch(request))
            .unwrap_or_else(response_from_transport_error);
        if let Ok(serialized) = serde_json::to_vec(&response) {
            let serialized = Zeroizing::new(serialized);
            let _ = stream.write_all(&serialized);
        }
    }

    /// Read one request, refusing by SIZE before anything else and by FAMILY before anything but the
    /// parse.
    ///
    /// The command name lives inside the JSON (`RequestEnvelope` is parsed as a whole), so the size
    /// decision cannot be made per command before reading. The order below is what keeps every old
    /// command's behaviour identical anyway:
    ///
    /// 1. over the completion ceiling -> `Protocol`, before any parse, as today;
    /// 2. parse ONCE; a parse failure on a body over the OLD ceiling is `Protocol` -- that body
    ///    could not be read at all before, so its refusal must not become `Json`; an old-size parse
    ///    failure stays `Json`;
    /// 3. a successfully parsed NON-completion command over the old ceiling -> `Protocol`, BEFORE
    ///    the protocol-version and token checks, which is exactly where it was refused before;
    /// 4. then the original checks, unchanged.
    ///
    /// Disclosed consequence (`C4-transport-decision.md` 2, 4): an oversized valid old body now
    /// costs one parse, and an unauthenticated peer can make this read up to
    /// `MAX_COMPLETION_REQUEST_BYTES + 1` bytes and parse up to `MAX_COMPLETION_REQUEST_BYTES` of
    /// them before the token check, where the old ceiling was 65,537/65,536. Kevin gates that before
    /// publication.
    fn read_request(&self, stream: &mut UnixStream) -> Result<LocalCommand, LocalTransportError> {
        let mut bytes = Zeroizing::new(Vec::new());
        stream
            .take(MAX_COMPLETION_REQUEST_BYTES + 1)
            .read_to_end(&mut bytes)?;
        let received = bytes.len() as u64;
        if received > MAX_COMPLETION_REQUEST_BYTES {
            return Err(LocalTransportError::Protocol);
        }
        let request: RequestEnvelope = match serde_json::from_slice(&bytes) {
            Ok(request) => request,
            Err(_) if received > MAX_MESSAGE_BYTES => {
                return Err(LocalTransportError::Protocol);
            }
            Err(error) => return Err(LocalTransportError::Json(error)),
        };
        if received > MAX_MESSAGE_BYTES && !accepts_completion_sized_request(&request.command) {
            return Err(LocalTransportError::Protocol);
        }
        if request.protocol != PROTOCOL_VERSION {
            return Err(LocalTransportError::Protocol);
        }
        if !self.token.matches_encoded(request.token.expose()) {
            return Err(LocalTransportError::Remote {
                code: LocalErrorCode::UnauthorizedClient,
                retry_after_ms: None,
            });
        }
        Ok(request.command)
    }

    fn dispatch(&self, command: LocalCommand) -> Result<ResponseEnvelope, LocalTransportError> {
        let result = match command {
            LocalCommand::Status => LocalResult::SessionStatus {
                status: self.accounts.status()?,
            },
            LocalCommand::SessionPolicy => LocalResult::SessionPolicy {
                policy: self.accounts.session_policy()?,
            },
            LocalCommand::SetSessionTimeout {
                inactivity_timeout_minutes,
            } => LocalResult::SessionPolicy {
                policy: self
                    .accounts
                    .set_session_timeout_minutes(inactivity_timeout_minutes)?,
            },
            LocalCommand::PlatformSessionMonitorStatus => {
                LocalResult::PlatformSessionMonitorStatus {
                    status: self.platform_monitor.status(),
                }
            }
            LocalCommand::BeginPasswordSetup {
                password,
                confirmation,
            } => LocalResult::RecoveryKey {
                recovery_key: WireSecret::from_recovery(
                    self.accounts
                        .begin_password_setup(password.into_secret(), confirmation.into_secret())?,
                ),
            },
            LocalCommand::CompleteOperatorSetup { recovery_saved } => LocalResult::SessionStatus {
                status: self.accounts.complete_operator_setup(recovery_saved)?,
            },
            LocalCommand::AuthenticatePassword { password, origin } => LocalResult::SessionStatus {
                status: self
                    .accounts
                    .authenticate_password(password.into_secret(), origin)?,
            },
            LocalCommand::RecoverWithKey {
                recovery_key,
                new_password,
                confirmation,
                origin,
            } => LocalResult::RecoveryKey {
                recovery_key: WireSecret::from_recovery(self.accounts.recover_with_key(
                    recovery_key.into_secret(),
                    new_password.into_secret(),
                    confirmation.into_secret(),
                    origin,
                )?),
            },
            LocalCommand::Lock => LocalResult::SessionStatus {
                status: self.accounts.lock()?,
            },
            LocalCommand::RecordOperatorActivity => {
                self.accounts.record_operator_activity()?;
                LocalResult::Empty
            }
            LocalCommand::StarterOrganization => LocalResult::StarterOrganization {
                organization: self.starter_organization.view()?,
            },
            LocalCommand::AcceptStarterOrganization {
                proposal_version,
                residents,
            } => LocalResult::StarterOrganization {
                organization: self
                    .starter_organization
                    .accept(proposal_version, residents)?,
            },
            LocalCommand::DiscardStarterOrganization { proposal_version } => {
                LocalResult::StarterOrganization {
                    organization: self.starter_organization.discard(proposal_version)?,
                }
            }
            LocalCommand::Residents => LocalResult::Residents {
                residents: self.resident_lifecycle.list()?,
            },
            LocalCommand::ActivateResident {
                resident_id,
                expected_decision_id,
            } => LocalResult::Resident {
                resident: self
                    .resident_lifecycle
                    .activate(resident_id, expected_decision_id)?,
            },
            LocalCommand::SuspendResident {
                resident_id,
                expected_decision_id,
            } => LocalResult::Resident {
                resident: self
                    .resident_lifecycle
                    .suspend(resident_id, expected_decision_id)?,
            },
            LocalCommand::ReactivateResident {
                resident_id,
                expected_decision_id,
            } => LocalResult::Resident {
                resident: self
                    .resident_lifecycle
                    .reactivate(resident_id, expected_decision_id)?,
            },
            LocalCommand::RetireResident {
                resident_id,
                expected_decision_id,
            } => LocalResult::Resident {
                resident: self
                    .resident_lifecycle
                    .retire(resident_id, expected_decision_id)?,
            },
            LocalCommand::ResidentCapabilityCatalog => LocalResult::ResidentCapabilityCatalog {
                capabilities: self.resident_configuration.catalog()?,
            },
            LocalCommand::ResidentConfigurations => LocalResult::ResidentConfigurations {
                configurations: self.resident_configuration.list()?,
            },
            LocalCommand::ConfigureResident {
                resident_id,
                expected_decision_id,
                activation_id,
                capabilities,
            } => LocalResult::ResidentConfiguration {
                configuration: self.resident_configuration.configure(
                    resident_id,
                    expected_decision_id,
                    activation_id,
                    capabilities,
                )?,
            },
            LocalCommand::ClearResidentConfiguration {
                resident_id,
                expected_decision_id,
            } => LocalResult::ResidentConfiguration {
                configuration: self
                    .resident_configuration
                    .clear(resident_id, expected_decision_id)?,
            },
            LocalCommand::Projects => LocalResult::Projects {
                projects: self.projects.list()?,
            },
            LocalCommand::SampleProjectStatus => LocalResult::SampleProjectStatus {
                status: self.sample_projects.status()?,
            },
            LocalCommand::InstallSampleProject => LocalResult::SampleProjectInstallResult {
                result: self.sample_projects.install()?,
            },
            LocalCommand::Project { project_id } => LocalResult::Project {
                project: self.projects.get(project_id)?,
            },
            LocalCommand::ProjectCharterHistory { project_id } => {
                LocalResult::ProjectCharterHistory {
                    charters: self.projects.charter_history(project_id)?,
                }
            }
            LocalCommand::CreateProject { draft } => LocalResult::Project {
                project: self.projects.create(draft)?,
            },
            LocalCommand::ReviseProjectCharter {
                project_id,
                expected_current_revision,
                draft,
            } => LocalResult::Project {
                project: self.projects.revise_charter(
                    project_id,
                    expected_current_revision,
                    draft,
                )?,
            },
            LocalCommand::ProjectPolicy { project_id } => LocalResult::ProjectPolicy {
                policy: self.project_policies.current(project_id)?,
            },
            LocalCommand::ProjectPolicyHistory { project_id } => {
                LocalResult::ProjectPolicyHistory {
                    policies: self.project_policies.history(project_id)?,
                }
            }
            LocalCommand::MissionPublications { mission_id } => LocalResult::MissionPublications {
                publications: self.outcomes.publication_history(mission_id)?,
            },
            LocalCommand::OutcomeDocument {
                mission_id,
                publication_version,
            } => LocalResult::OutcomeDocument {
                document: match publication_version {
                    // The bytes come through export, so the surface and any other caller read the
                    // document by one path rather than two.
                    None => self.outcomes.export_markdown(mission_id)?,
                    Some(version) => self.outcomes.export_markdown_version(mission_id, version)?,
                },
            },
            LocalCommand::PublishProjectPolicy {
                project_id,
                expected_charter_revision,
                expected_policy_revision,
                draft,
            } => LocalResult::ProjectPolicy {
                policy: Some(self.project_policies.publish(
                    project_id,
                    expected_charter_revision,
                    expected_policy_revision,
                    draft,
                )?),
            },
            LocalCommand::CurrentProjectRepository { project_id } => {
                LocalResult::ProjectRepository {
                    repository: self.project_repositories.current(project_id)?,
                }
            }
            LocalCommand::PreflightProjectChecks { project_id } => LocalResult::PreflightReceipt {
                receipt: self.check_preflights.preflight(project_id)?,
            },
            LocalCommand::RecentReceipts { window } => LocalResult::RecentReceipts {
                page: self.recent_receipts.recent(window)?,
            },
            LocalCommand::ProjectRepositoryHistory { project_id } => {
                LocalResult::ProjectRepositoryHistory {
                    repositories: self.project_repositories.history(project_id)?,
                }
            }
            LocalCommand::BindProjectRepository {
                project_id,
                expected_charter_revision,
                expected_prior_repository_revision,
                draft,
            } => LocalResult::ProjectRepository {
                repository: Some(self.project_repositories.bind(
                    project_id,
                    expected_charter_revision,
                    expected_prior_repository_revision,
                    draft,
                )?),
            },
            LocalCommand::Teams { project_id } => LocalResult::Teams {
                teams: self.teams.list(project_id)?,
            },
            LocalCommand::Team { team_id } => LocalResult::Team {
                team: self.teams.get(team_id)?,
            },
            LocalCommand::FormTeam { draft } => LocalResult::Team {
                team: self.teams.form(draft)?,
            },
            LocalCommand::Missions { project_id } => LocalResult::Missions {
                missions: self.missions.list(project_id)?,
            },
            LocalCommand::Mission { mission_id } => LocalResult::Mission {
                mission: self.missions.get(mission_id)?,
            },
            LocalCommand::MissionCompletion { mission_id } => LocalResult::MissionCompletion {
                completion: self.mission_completions.get(mission_id)?,
            },
            LocalCommand::PreviewMissionCompletion { request } => {
                LocalResult::MissionCompletionPreview {
                    preview: self.completion_commands.preview(request)?,
                }
            }
            LocalCommand::RecordMissionCompletion {
                request,
                reviewed_intent_sha256,
            } => LocalResult::MissionCompletionAcknowledgement {
                intent: self
                    .completion_commands
                    .record(request, &reviewed_intent_sha256)?,
            },
            LocalCommand::GetMissionCompletionIntent { mission_id } => {
                LocalResult::MissionCompletionIntent {
                    intent: self.completion_commands.get_intent(mission_id)?,
                }
            }
            LocalCommand::AdmitMission { draft } => LocalResult::Mission {
                mission: self.missions.admit(draft)?,
            },
            LocalCommand::DiscoverLocalModels => LocalResult::LocalModelDiscovery {
                report: self.resource_discovery.discover_local_models()?,
            },
            LocalCommand::LiveDataEncryptionStatus => LocalResult::LiveDataEncryption {
                report: self.disk_encryption.status()?,
            },
            LocalCommand::InspectLocalModel {
                model_name,
                expected_digest,
            } => LocalResult::LocalModelActivationReview {
                review: self
                    .local_model_activation
                    .inspect(&model_name, &expected_digest)?,
            },
            LocalCommand::ActivateLocalModel {
                review,
                approved_features,
            } => LocalResult::LocalModelActivation {
                activation: self
                    .local_model_activation
                    .activate(review, approved_features)?,
            },
            LocalCommand::LocalModelActivations => LocalResult::LocalModelActivations {
                activations: self.local_model_activation.list()?,
            },
            LocalCommand::DeactivateLocalModel {
                expected_activation_id,
            } => LocalResult::LocalModelActivation {
                activation: self
                    .local_model_activation
                    .deactivate(expected_activation_id)?,
            },
        };
        Ok(ResponseEnvelope {
            protocol: PROTOCOL_VERSION,
            result: Some(result),
            error: None,
        })
    }
}

fn acquire_supervisor_lock(path: &Path) -> Result<File, LocalTransportError> {
    verify_private_parent(path)?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path)?;
    ensure_private_regular_file(&file)?;
    use std::os::fd::AsRawFd;
    // The lock is held for the server lifetime. A replacement process can only remove a stale
    // socket after the kernel releases this lock, so two supervisors cannot unlink each other.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

impl Drop for LocalSupervisorServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

#[derive(Clone)]
pub struct LocalSupervisorClient {
    socket_path: PathBuf,
    token_path: PathBuf,
}

impl LocalSupervisorClient {
    pub fn connect(
        socket_path: impl AsRef<Path>,
        auth_token_path: impl AsRef<Path>,
    ) -> Result<Self, LocalTransportError> {
        load_local_auth_token(auth_token_path.as_ref())?;
        Ok(Self {
            socket_path: socket_path.as_ref().to_owned(),
            token_path: auth_token_path.as_ref().to_owned(),
        })
    }

    pub fn status(&self) -> Result<OperatorSessionStatus, LocalTransportError> {
        expect_status(self.send(LocalCommand::Status)?)
    }

    pub fn session_policy(&self) -> Result<OperatorSessionPolicy, LocalTransportError> {
        expect_policy(self.send(LocalCommand::SessionPolicy)?)
    }

    pub fn set_session_timeout_minutes(
        &self,
        inactivity_timeout_minutes: u16,
    ) -> Result<OperatorSessionPolicy, LocalTransportError> {
        expect_policy(self.send(LocalCommand::SetSessionTimeout {
            inactivity_timeout_minutes,
        })?)
    }

    pub fn platform_session_monitor_status(
        &self,
    ) -> Result<PlatformSessionMonitorStatus, LocalTransportError> {
        match self.send(LocalCommand::PlatformSessionMonitorStatus)? {
            LocalResult::PlatformSessionMonitorStatus { status } => Ok(status),
            _ => Err(LocalTransportError::Protocol),
        }
    }

    pub fn begin_password_setup(
        &self,
        password: SecretString,
        confirmation: SecretString,
    ) -> Result<RecoveryKey, LocalTransportError> {
        expect_recovery(self.send(LocalCommand::BeginPasswordSetup {
            password: WireSecret::from_secret(password),
            confirmation: WireSecret::from_secret(confirmation),
        })?)
    }

    pub fn complete_operator_setup(
        &self,
        recovery_saved: bool,
    ) -> Result<OperatorSessionStatus, LocalTransportError> {
        expect_status(self.send(LocalCommand::CompleteOperatorSetup { recovery_saved })?)
    }

    pub fn authenticate_password(
        &self,
        password: SecretString,
        origin: AccessOrigin,
    ) -> Result<OperatorSessionStatus, LocalTransportError> {
        expect_status(self.send(LocalCommand::AuthenticatePassword {
            password: WireSecret::from_secret(password),
            origin,
        })?)
    }

    pub fn recover_with_key(
        &self,
        recovery_key: SecretString,
        new_password: SecretString,
        confirmation: SecretString,
        origin: AccessOrigin,
    ) -> Result<RecoveryKey, LocalTransportError> {
        expect_recovery(self.send(LocalCommand::RecoverWithKey {
            recovery_key: WireSecret::from_secret(recovery_key),
            new_password: WireSecret::from_secret(new_password),
            confirmation: WireSecret::from_secret(confirmation),
            origin,
        })?)
    }

    pub fn lock(&self) -> Result<OperatorSessionStatus, LocalTransportError> {
        expect_status(self.send(LocalCommand::Lock)?)
    }

    pub fn record_operator_activity(&self) -> Result<(), LocalTransportError> {
        match self.send(LocalCommand::RecordOperatorActivity)? {
            LocalResult::Empty => Ok(()),
            _ => Err(LocalTransportError::Protocol),
        }
    }

    pub fn starter_organization(&self) -> Result<StarterOrganizationView, LocalTransportError> {
        expect_starter_organization(self.send(LocalCommand::StarterOrganization)?)
    }

    pub fn accept_starter_organization(
        &self,
        proposal_version: u16,
        residents: Vec<StarterResidentDraft>,
    ) -> Result<StarterOrganizationView, LocalTransportError> {
        expect_starter_organization(self.send(LocalCommand::AcceptStarterOrganization {
            proposal_version,
            residents,
        })?)
    }

    pub fn discard_starter_organization(
        &self,
        proposal_version: u16,
    ) -> Result<StarterOrganizationView, LocalTransportError> {
        expect_starter_organization(
            self.send(LocalCommand::DiscardStarterOrganization { proposal_version })?,
        )
    }

    pub fn residents(&self) -> Result<Vec<ResidentAgentView>, LocalTransportError> {
        expect_residents(self.send(LocalCommand::Residents)?)
    }

    pub fn activate_resident(
        &self,
        resident_id: uuid::Uuid,
        expected_decision_id: Option<uuid::Uuid>,
    ) -> Result<ResidentAgentView, LocalTransportError> {
        expect_resident(self.send(LocalCommand::ActivateResident {
            resident_id,
            expected_decision_id,
        })?)
    }

    pub fn suspend_resident(
        &self,
        resident_id: uuid::Uuid,
        expected_decision_id: Option<uuid::Uuid>,
    ) -> Result<ResidentAgentView, LocalTransportError> {
        expect_resident(self.send(LocalCommand::SuspendResident {
            resident_id,
            expected_decision_id,
        })?)
    }

    pub fn reactivate_resident(
        &self,
        resident_id: uuid::Uuid,
        expected_decision_id: Option<uuid::Uuid>,
    ) -> Result<ResidentAgentView, LocalTransportError> {
        expect_resident(self.send(LocalCommand::ReactivateResident {
            resident_id,
            expected_decision_id,
        })?)
    }

    pub fn retire_resident(
        &self,
        resident_id: uuid::Uuid,
        expected_decision_id: Option<uuid::Uuid>,
    ) -> Result<ResidentAgentView, LocalTransportError> {
        expect_resident(self.send(LocalCommand::RetireResident {
            resident_id,
            expected_decision_id,
        })?)
    }

    pub fn resident_capability_catalog(
        &self,
    ) -> Result<Vec<BeaconCapabilityDescriptor>, LocalTransportError> {
        expect_resident_capability_catalog(self.send(LocalCommand::ResidentCapabilityCatalog)?)
    }

    pub fn resident_configurations(
        &self,
    ) -> Result<Vec<ResidentConfiguration>, LocalTransportError> {
        expect_resident_configurations(self.send(LocalCommand::ResidentConfigurations)?)
    }

    pub fn configure_resident(
        &self,
        resident_id: uuid::Uuid,
        expected_decision_id: Option<uuid::Uuid>,
        activation_id: uuid::Uuid,
        capabilities: Vec<BeaconCapability>,
    ) -> Result<ResidentConfiguration, LocalTransportError> {
        expect_resident_configuration(self.send(LocalCommand::ConfigureResident {
            resident_id,
            expected_decision_id,
            activation_id,
            capabilities,
        })?)
    }

    pub fn clear_resident_configuration(
        &self,
        resident_id: uuid::Uuid,
        expected_decision_id: uuid::Uuid,
    ) -> Result<ResidentConfiguration, LocalTransportError> {
        expect_resident_configuration(self.send(LocalCommand::ClearResidentConfiguration {
            resident_id,
            expected_decision_id,
        })?)
    }

    pub fn projects(&self) -> Result<Vec<ProjectSummary>, LocalTransportError> {
        expect_projects(self.send(LocalCommand::Projects)?)
    }

    pub fn sample_project_status(&self) -> Result<SampleProjectStatus, LocalTransportError> {
        expect_sample_project_status(self.send(LocalCommand::SampleProjectStatus)?)
    }

    pub fn install_sample_project(
        &self,
    ) -> Result<SampleProjectInstallResult, LocalTransportError> {
        expect_sample_project_install_result(self.send_within(
            LocalCommand::InstallSampleProject,
            CLIENT_SLOW_COMMAND_TIMEOUT,
        )?)
    }

    pub fn project(&self, project_id: uuid::Uuid) -> Result<ProjectView, LocalTransportError> {
        expect_project(self.send(LocalCommand::Project { project_id })?)
    }

    pub fn project_charter_history(
        &self,
        project_id: uuid::Uuid,
    ) -> Result<Vec<ProjectCharter>, LocalTransportError> {
        expect_project_charter_history(
            self.send(LocalCommand::ProjectCharterHistory { project_id })?,
        )
    }

    pub fn create_project(&self, draft: ProjectDraft) -> Result<ProjectView, LocalTransportError> {
        expect_project(self.send(LocalCommand::CreateProject { draft })?)
    }

    pub fn revise_project_charter(
        &self,
        project_id: uuid::Uuid,
        expected_current_revision: u32,
        draft: ProjectCharterDraft,
    ) -> Result<ProjectView, LocalTransportError> {
        expect_project(self.send(LocalCommand::ReviseProjectCharter {
            project_id,
            expected_current_revision,
            draft,
        })?)
    }

    pub fn project_policy(
        &self,
        project_id: uuid::Uuid,
    ) -> Result<Option<ProjectPolicy>, LocalTransportError> {
        expect_project_policy(self.send(LocalCommand::ProjectPolicy { project_id })?)
    }

    pub fn project_policy_history(
        &self,
        project_id: uuid::Uuid,
    ) -> Result<Vec<ProjectPolicy>, LocalTransportError> {
        expect_project_policy_history(self.send(LocalCommand::ProjectPolicyHistory { project_id })?)
    }

    pub fn mission_publications(
        &self,
        mission_id: uuid::Uuid,
    ) -> Result<Vec<PublicationSummary>, LocalTransportError> {
        expect_mission_publications(self.send(LocalCommand::MissionPublications { mission_id })?)
    }

    pub fn outcome_document(
        &self,
        mission_id: uuid::Uuid,
        publication_version: Option<u32>,
    ) -> Result<ExportedOutcome, LocalTransportError> {
        expect_outcome_document(self.send(LocalCommand::OutcomeDocument {
            mission_id,
            publication_version,
        })?)
    }

    pub fn publish_project_policy(
        &self,
        project_id: uuid::Uuid,
        expected_charter_revision: u32,
        expected_policy_revision: Option<u32>,
        draft: ProjectPolicyDraft,
    ) -> Result<ProjectPolicy, LocalTransportError> {
        expect_project_policy(self.send(LocalCommand::PublishProjectPolicy {
            project_id,
            expected_charter_revision,
            expected_policy_revision,
            draft,
        })?)?
        .ok_or(LocalTransportError::Protocol)
    }

    pub fn current_project_repository(
        &self,
        project_id: uuid::Uuid,
    ) -> Result<Option<ProjectRepository>, LocalTransportError> {
        expect_project_repository(self.send(LocalCommand::CurrentProjectRepository { project_id })?)
    }

    pub fn preflight_project_checks(
        &self,
        project_id: uuid::Uuid,
    ) -> Result<PreflightReceipt, LocalTransportError> {
        expect_preflight_receipt(self.send(LocalCommand::PreflightProjectChecks { project_id })?)
    }

    pub fn recent_receipts(
        &self,
        window: u32,
    ) -> Result<crate::RecentReceiptPage, LocalTransportError> {
        expect_recent_receipts(self.send(LocalCommand::RecentReceipts { window })?)
    }

    pub fn project_repository_history(
        &self,
        project_id: uuid::Uuid,
    ) -> Result<Vec<ProjectRepository>, LocalTransportError> {
        expect_project_repository_history(
            self.send(LocalCommand::ProjectRepositoryHistory { project_id })?,
        )
    }

    pub fn bind_project_repository(
        &self,
        project_id: uuid::Uuid,
        expected_charter_revision: u32,
        expected_prior_repository_revision: Option<u32>,
        draft: RepositoryDraft,
    ) -> Result<ProjectRepository, LocalTransportError> {
        expect_project_repository(self.send(LocalCommand::BindProjectRepository {
            project_id,
            expected_charter_revision,
            expected_prior_repository_revision,
            draft,
        })?)?
        .ok_or(LocalTransportError::Protocol)
    }

    pub fn teams(&self, project_id: uuid::Uuid) -> Result<Vec<TeamView>, LocalTransportError> {
        expect_teams(self.send(LocalCommand::Teams { project_id })?)
    }

    pub fn team(&self, team_id: uuid::Uuid) -> Result<TeamView, LocalTransportError> {
        expect_team(self.send(LocalCommand::Team { team_id })?)
    }

    pub fn form_team(&self, draft: TeamDraft) -> Result<TeamView, LocalTransportError> {
        expect_team(self.send(LocalCommand::FormTeam { draft })?)
    }

    pub fn missions(
        &self,
        project_id: uuid::Uuid,
    ) -> Result<Vec<MissionView>, LocalTransportError> {
        expect_missions(self.send(LocalCommand::Missions { project_id })?)
    }

    /// The verified completion of one Mission, or an honest absence. Absence is `None`; a read that
    /// could not be verified is an error and is never converted into `None`.
    pub fn mission_completion(
        &self,
        mission_id: uuid::Uuid,
    ) -> Result<Option<crate::CompletionView>, LocalTransportError> {
        expect_mission_completion(self.send(LocalCommand::MissionCompletion { mission_id })?)
    }

    /// Derive the completion the Operator would record, and its reviewed-intent token.
    ///
    /// The EXACT owner DTO crosses the socket in both directions: this method builds no record,
    /// re-validates no field the owner owns, and reads nothing back out of the preview.
    pub fn preview_mission_completion(
        &self,
        request: crate::CompletionIntentRequest,
    ) -> Result<crate::CompletionPreview, LocalTransportError> {
        expect_mission_completion_preview(
            self.send(LocalCommand::PreviewMissionCompletion { request })?,
        )
    }

    /// Record the reviewed completion. `reviewed_intent_sha256` is the token the Operator reviewed;
    /// it is forwarded unchanged, never recomputed here.
    pub fn record_mission_completion(
        &self,
        request: crate::CompletionIntentRequest,
        reviewed_intent_sha256: String,
    ) -> Result<crate::ReviewedIntent, LocalTransportError> {
        expect_mission_completion_acknowledgement(self.send(
            LocalCommand::RecordMissionCompletion {
                request,
                reviewed_intent_sha256,
            },
        )?)
    }

    /// The token of the completion visible for this Mission in one snapshot, or `None`.
    pub fn get_mission_completion_intent(
        &self,
        mission_id: uuid::Uuid,
    ) -> Result<Option<crate::ReviewedIntent>, LocalTransportError> {
        expect_mission_completion_intent(
            self.send(LocalCommand::GetMissionCompletionIntent { mission_id })?,
        )
    }

    pub fn mission(&self, mission_id: uuid::Uuid) -> Result<MissionView, LocalTransportError> {
        expect_mission(self.send(LocalCommand::Mission { mission_id })?)
    }

    pub fn admit_mission(&self, draft: MissionDraft) -> Result<MissionView, LocalTransportError> {
        expect_mission(self.send(LocalCommand::AdmitMission { draft })?)
    }

    pub fn discover_local_models(
        &self,
    ) -> Result<crate::LocalModelDiscoveryReport, LocalTransportError> {
        expect_local_model_discovery(self.send(LocalCommand::DiscoverLocalModels)?)
    }

    pub fn live_data_encryption_status(
        &self,
    ) -> Result<LiveDataEncryptionReport, LocalTransportError> {
        expect_live_data_encryption(self.send(LocalCommand::LiveDataEncryptionStatus)?)
    }

    pub fn inspect_local_model(
        &self,
        model_name: String,
        expected_digest: String,
    ) -> Result<crate::LocalModelActivationReview, LocalTransportError> {
        expect_local_model_activation_review(self.send(LocalCommand::InspectLocalModel {
            model_name,
            expected_digest,
        })?)
    }

    pub fn activate_local_model(
        &self,
        review: crate::LocalModelActivationReview,
        approved_features: Vec<crate::ProviderFeature>,
    ) -> Result<crate::LocalModelActivation, LocalTransportError> {
        expect_local_model_activation(self.send(LocalCommand::ActivateLocalModel {
            review,
            approved_features,
        })?)
    }

    pub fn local_model_activations(
        &self,
    ) -> Result<Vec<crate::LocalModelActivation>, LocalTransportError> {
        expect_local_model_activations(self.send(LocalCommand::LocalModelActivations)?)
    }

    pub fn deactivate_local_model(
        &self,
        expected_activation_id: uuid::Uuid,
    ) -> Result<crate::LocalModelActivation, LocalTransportError> {
        expect_local_model_activation(self.send(LocalCommand::DeactivateLocalModel {
            expected_activation_id,
        })?)
    }

    fn send(&self, command: LocalCommand) -> Result<LocalResult, LocalTransportError> {
        self.send_within(command, CLIENT_RESPONSE_TIMEOUT)
    }

    fn send_within(
        &self,
        command: LocalCommand,
        response_timeout: Duration,
    ) -> Result<LocalResult, LocalTransportError> {
        let token = load_local_auth_token(&self.token_path)?;
        // Chosen from the command while it is still ours to read; moving it into the envelope first
        // would leave the ceiling to be guessed from something else.
        let response_ceiling = response_max(&command);
        let request = RequestEnvelope {
            protocol: PROTOCOL_VERSION,
            token: WireSecret(token.encode()),
            command,
        };
        let serialized = Zeroizing::new(serde_json::to_vec(&request)?);
        let mut stream = UnixStream::connect(&self.socket_path)?;
        configure_stream_deadlines(&stream, response_timeout)?;
        stream.write_all(&serialized)?;
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut bytes = Zeroizing::new(Vec::new());
        stream.take(response_ceiling + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > response_ceiling {
            return Err(LocalTransportError::Protocol);
        }
        let response: ResponseEnvelope = serde_json::from_slice(&bytes)?;
        if response.protocol != PROTOCOL_VERSION {
            return Err(LocalTransportError::Protocol);
        }
        match (response.result, response.error) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err(LocalTransportError::Remote {
                code: error.code,
                retry_after_ms: error.retry_after_ms,
            }),
            _ => Err(LocalTransportError::Protocol),
        }
    }
}

/// The RESPONSE ceiling for one outgoing command, chosen from the command BEFORE it is moved into
/// the envelope.
///
/// Exhaustive and named, with no catch-all: a new `LocalCommand` arm does not compile until someone
/// states its response domain, so a command cannot inherit a widened ceiling by being forgotten.
/// Only the preview echoes a whole normalized request; `RecordMissionCompletion` answers with one
/// token and `GetMissionCompletionIntent` with a token or null, so both stay at the old domain.
fn response_max(command: &LocalCommand) -> u64 {
    match command {
        LocalCommand::PreviewMissionCompletion { .. } => MAX_COMPLETION_PREVIEW_RESPONSE_BYTES,
        LocalCommand::Status
        | LocalCommand::SessionPolicy
        | LocalCommand::SetSessionTimeout { .. }
        | LocalCommand::PlatformSessionMonitorStatus
        | LocalCommand::BeginPasswordSetup { .. }
        | LocalCommand::CompleteOperatorSetup { .. }
        | LocalCommand::AuthenticatePassword { .. }
        | LocalCommand::RecoverWithKey { .. }
        | LocalCommand::Lock
        | LocalCommand::RecordOperatorActivity
        | LocalCommand::StarterOrganization
        | LocalCommand::AcceptStarterOrganization { .. }
        | LocalCommand::DiscardStarterOrganization { .. }
        | LocalCommand::Residents
        | LocalCommand::ActivateResident { .. }
        | LocalCommand::SuspendResident { .. }
        | LocalCommand::ReactivateResident { .. }
        | LocalCommand::RetireResident { .. }
        | LocalCommand::ResidentCapabilityCatalog
        | LocalCommand::ResidentConfigurations
        | LocalCommand::ConfigureResident { .. }
        | LocalCommand::ClearResidentConfiguration { .. }
        | LocalCommand::Projects
        | LocalCommand::SampleProjectStatus
        | LocalCommand::InstallSampleProject
        | LocalCommand::Project { .. }
        | LocalCommand::ProjectCharterHistory { .. }
        | LocalCommand::CreateProject { .. }
        | LocalCommand::ReviseProjectCharter { .. }
        | LocalCommand::ProjectPolicy { .. }
        | LocalCommand::ProjectPolicyHistory { .. }
        | LocalCommand::MissionPublications { .. }
        | LocalCommand::OutcomeDocument { .. }
        | LocalCommand::PublishProjectPolicy { .. }
        | LocalCommand::CurrentProjectRepository { .. }
        | LocalCommand::ProjectRepositoryHistory { .. }
        | LocalCommand::BindProjectRepository { .. }
        | LocalCommand::PreflightProjectChecks { .. }
        | LocalCommand::RecentReceipts { .. }
        | LocalCommand::Teams { .. }
        | LocalCommand::Team { .. }
        | LocalCommand::FormTeam { .. }
        | LocalCommand::Missions { .. }
        | LocalCommand::Mission { .. }
        | LocalCommand::MissionCompletion { .. }
        | LocalCommand::RecordMissionCompletion { .. }
        | LocalCommand::GetMissionCompletionIntent { .. }
        | LocalCommand::AdmitMission { .. }
        | LocalCommand::DiscoverLocalModels
        | LocalCommand::LiveDataEncryptionStatus
        | LocalCommand::InspectLocalModel { .. }
        | LocalCommand::ActivateLocalModel { .. }
        | LocalCommand::LocalModelActivations
        | LocalCommand::DeactivateLocalModel { .. } => MAX_MESSAGE_BYTES,
    }
}

fn configure_stream_deadlines(
    stream: &UnixStream,
    timeout: Duration,
) -> Result<(), LocalTransportError> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(())
}

fn expect_status(result: LocalResult) -> Result<OperatorSessionStatus, LocalTransportError> {
    match result {
        LocalResult::SessionStatus { status } => Ok(status),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_policy(result: LocalResult) -> Result<OperatorSessionPolicy, LocalTransportError> {
    match result {
        LocalResult::SessionPolicy { policy } => Ok(policy),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_recovery(result: LocalResult) -> Result<RecoveryKey, LocalTransportError> {
    match result {
        LocalResult::RecoveryKey { recovery_key } => Ok(recovery_key.into_recovery()),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_starter_organization(
    result: LocalResult,
) -> Result<StarterOrganizationView, LocalTransportError> {
    match result {
        LocalResult::StarterOrganization { organization } => Ok(organization),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_residents(result: LocalResult) -> Result<Vec<ResidentAgentView>, LocalTransportError> {
    match result {
        LocalResult::Residents { residents } => Ok(residents),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_resident(result: LocalResult) -> Result<ResidentAgentView, LocalTransportError> {
    match result {
        LocalResult::Resident { resident } => Ok(resident),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_resident_capability_catalog(
    result: LocalResult,
) -> Result<Vec<BeaconCapabilityDescriptor>, LocalTransportError> {
    match result {
        LocalResult::ResidentCapabilityCatalog { capabilities } => Ok(capabilities),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_resident_configurations(
    result: LocalResult,
) -> Result<Vec<ResidentConfiguration>, LocalTransportError> {
    match result {
        LocalResult::ResidentConfigurations { configurations } => Ok(configurations),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_resident_configuration(
    result: LocalResult,
) -> Result<ResidentConfiguration, LocalTransportError> {
    match result {
        LocalResult::ResidentConfiguration { configuration } => Ok(configuration),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_projects(result: LocalResult) -> Result<Vec<ProjectSummary>, LocalTransportError> {
    match result {
        LocalResult::Projects { projects } => Ok(projects),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_sample_project_status(
    result: LocalResult,
) -> Result<SampleProjectStatus, LocalTransportError> {
    match result {
        LocalResult::SampleProjectStatus { status } => Ok(status),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_sample_project_install_result(
    result: LocalResult,
) -> Result<SampleProjectInstallResult, LocalTransportError> {
    match result {
        LocalResult::SampleProjectInstallResult { result } => Ok(result),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_project(result: LocalResult) -> Result<ProjectView, LocalTransportError> {
    match result {
        LocalResult::Project { project } => Ok(project),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_project_charter_history(
    result: LocalResult,
) -> Result<Vec<ProjectCharter>, LocalTransportError> {
    match result {
        LocalResult::ProjectCharterHistory { charters } => Ok(charters),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_project_policy(
    result: LocalResult,
) -> Result<Option<ProjectPolicy>, LocalTransportError> {
    match result {
        LocalResult::ProjectPolicy { policy } => Ok(policy),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_project_policy_history(
    result: LocalResult,
) -> Result<Vec<ProjectPolicy>, LocalTransportError> {
    match result {
        LocalResult::ProjectPolicyHistory { policies } => Ok(policies),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_mission_publications(
    result: LocalResult,
) -> Result<Vec<PublicationSummary>, LocalTransportError> {
    match result {
        LocalResult::MissionPublications { publications } => Ok(publications),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_outcome_document(result: LocalResult) -> Result<ExportedOutcome, LocalTransportError> {
    match result {
        LocalResult::OutcomeDocument { document } => Ok(document),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_project_repository(
    result: LocalResult,
) -> Result<Option<ProjectRepository>, LocalTransportError> {
    match result {
        LocalResult::ProjectRepository { repository } => Ok(repository),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_preflight_receipt(result: LocalResult) -> Result<PreflightReceipt, LocalTransportError> {
    match result {
        LocalResult::PreflightReceipt { receipt } => Ok(receipt),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_recent_receipts(
    result: LocalResult,
) -> Result<crate::RecentReceiptPage, LocalTransportError> {
    match result {
        LocalResult::RecentReceipts { page } => Ok(page),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_project_repository_history(
    result: LocalResult,
) -> Result<Vec<ProjectRepository>, LocalTransportError> {
    match result {
        LocalResult::ProjectRepositoryHistory { repositories } => Ok(repositories),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_teams(result: LocalResult) -> Result<Vec<TeamView>, LocalTransportError> {
    match result {
        LocalResult::Teams { teams } => Ok(teams),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_team(result: LocalResult) -> Result<TeamView, LocalTransportError> {
    match result {
        LocalResult::Team { team } => Ok(team),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_mission_completion(
    result: LocalResult,
) -> Result<Option<crate::CompletionView>, LocalTransportError> {
    match result {
        LocalResult::MissionCompletion { completion } => Ok(completion),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_mission_completion_preview(
    result: LocalResult,
) -> Result<crate::CompletionPreview, LocalTransportError> {
    match result {
        LocalResult::MissionCompletionPreview { preview } => Ok(preview),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_mission_completion_acknowledgement(
    result: LocalResult,
) -> Result<crate::ReviewedIntent, LocalTransportError> {
    match result {
        LocalResult::MissionCompletionAcknowledgement { intent } => Ok(intent),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_mission_completion_intent(
    result: LocalResult,
) -> Result<Option<crate::ReviewedIntent>, LocalTransportError> {
    match result {
        LocalResult::MissionCompletionIntent { intent } => Ok(intent),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_missions(result: LocalResult) -> Result<Vec<MissionView>, LocalTransportError> {
    match result {
        LocalResult::Missions { missions } => Ok(missions),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_mission(result: LocalResult) -> Result<MissionView, LocalTransportError> {
    match result {
        LocalResult::Mission { mission } => Ok(mission),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_local_model_discovery(
    result: LocalResult,
) -> Result<crate::LocalModelDiscoveryReport, LocalTransportError> {
    match result {
        LocalResult::LocalModelDiscovery { report } => Ok(report),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_live_data_encryption(
    result: LocalResult,
) -> Result<LiveDataEncryptionReport, LocalTransportError> {
    match result {
        LocalResult::LiveDataEncryption { report } => Ok(report),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_local_model_activation_review(
    result: LocalResult,
) -> Result<crate::LocalModelActivationReview, LocalTransportError> {
    match result {
        LocalResult::LocalModelActivationReview { review } => Ok(review),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_local_model_activation(
    result: LocalResult,
) -> Result<crate::LocalModelActivation, LocalTransportError> {
    match result {
        LocalResult::LocalModelActivation { activation } => Ok(activation),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_local_model_activations(
    result: LocalResult,
) -> Result<Vec<crate::LocalModelActivation>, LocalTransportError> {
    match result {
        LocalResult::LocalModelActivations { activations } => Ok(activations),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn response_from_transport_error(error: LocalTransportError) -> ResponseEnvelope {
    let wire_error = match error {
        LocalTransportError::Account(error) => wire_error_from_account(error),
        LocalTransportError::StarterOrganization(error) => wire_error_from_starter(error),
        LocalTransportError::ResidentLifecycle(error) => wire_error_from_resident(error),
        LocalTransportError::ResidentConfiguration(error) => {
            wire_error_from_resident_configuration(error)
        }
        LocalTransportError::Project(error) => wire_error_from_project(error),
        LocalTransportError::ProjectPolicy(error) => wire_error_from_project_policy(error),
        LocalTransportError::Outcome(error) => wire_error_from_outcome(error),
        LocalTransportError::ProjectRepository(error) => wire_error_from_project_repository(error),
        LocalTransportError::Team(error) => wire_error_from_team(error),
        LocalTransportError::CheckPreflight(error) => wire_error_from_check_preflight(error),
        LocalTransportError::RecentReceipt(error) => wire_error_from_recent_receipt(error),
        LocalTransportError::SampleProject(error) => wire_error_from_sample_project(error),
        LocalTransportError::Mission(error) => wire_error_from_mission(error),
        LocalTransportError::MissionCompletionRead(error) => {
            wire_error_from_mission_completion(error)
        }
        LocalTransportError::MissionCompletionCommand(error) => {
            wire_error_from_completion_command(error)
        }
        LocalTransportError::ResourceDiscovery(ResourceDiscoveryError::Account(error)) => {
            wire_error_from_account(error)
        }
        LocalTransportError::ResourceDiscovery(ResourceDiscoveryError::EndpointNotLoopback) => {
            WireError {
                code: LocalErrorCode::ProtocolError,
                retry_after_ms: None,
            }
        }
        LocalTransportError::LocalModelActivation(error) => {
            wire_error_from_local_model_activation(error)
        }
        LocalTransportError::Remote {
            code,
            retry_after_ms,
        } => WireError {
            code,
            retry_after_ms,
        },
        // Was `_`. Same six inputs, same result -- named so that a NEW transport variant is a
        // compile error here instead of silently joining the coarsest wire code the protocol has
        // (R42). Deliberately NOT a finer diagnosis: changing what these six report would change
        // wire semantics, which is outside this slice.
        LocalTransportError::Io(_)
        | LocalTransportError::Json(_)
        | LocalTransportError::InvalidTokenFile
        | LocalTransportError::InsecurePermissions
        | LocalTransportError::Protocol
        | LocalTransportError::Supervisor(_) => WireError {
            code: LocalErrorCode::ProtocolError,
            retry_after_ms: None,
        },
    };
    ResponseEnvelope {
        protocol: PROTOCOL_VERSION,
        result: None,
        error: Some(wire_error),
    }
}

fn wire_error_from_resident(error: ResidentLifecycleError) -> WireError {
    let code = match error {
        ResidentLifecycleError::Account(error) => return wire_error_from_account(error),
        ResidentLifecycleError::NotFound => LocalErrorCode::ResidentNotFound,
        ResidentLifecycleError::InvalidTransition => LocalErrorCode::InvalidResidentTransition,
        ResidentLifecycleError::StaleDecision => LocalErrorCode::StaleResidentDecision,
        ResidentLifecycleError::InvalidStore
        | ResidentLifecycleError::Store(_)
        | ResidentLifecycleError::Json(_) => LocalErrorCode::ResidentLifecycleUnavailable,
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_resident_configuration(error: ResidentConfigurationError) -> WireError {
    let code = match error {
        ResidentConfigurationError::Account(error) => return wire_error_from_account(error),
        ResidentConfigurationError::ResidentNotFound => LocalErrorCode::ResidentNotFound,
        ResidentConfigurationError::ResidentRetired
        | ResidentConfigurationError::ModelActivationUnavailable
        | ResidentConfigurationError::ModelMissingCompletion
        | ResidentConfigurationError::InvalidCapabilities
        | ResidentConfigurationError::AlreadyCleared => {
            LocalErrorCode::InvalidResidentConfiguration
        }
        ResidentConfigurationError::StaleDecision => LocalErrorCode::StaleResidentConfiguration,
        ResidentConfigurationError::InvalidStore
        | ResidentConfigurationError::Store(_)
        | ResidentConfigurationError::Json(_) => LocalErrorCode::ResidentConfigurationUnavailable,
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_local_model_activation(error: crate::LocalModelActivationError) -> WireError {
    use crate::LocalModelActivationError;

    let code = match error {
        LocalModelActivationError::Account(error) => return wire_error_from_account(error),
        LocalModelActivationError::ResourceUnavailable
        | LocalModelActivationError::InvalidProviderResponse => {
            LocalErrorCode::LocalModelUnavailable
        }
        LocalModelActivationError::ModelNotFound => LocalErrorCode::LocalModelNotFound,
        LocalModelActivationError::ModelDrift => LocalErrorCode::LocalModelChanged,
        LocalModelActivationError::InvalidApproval => LocalErrorCode::InvalidLocalModelApproval,
        LocalModelActivationError::AlreadyActive => LocalErrorCode::LocalModelAlreadyActive,
        LocalModelActivationError::StaleDecision => LocalErrorCode::StaleLocalModelDecision,
        LocalModelActivationError::InvalidStore
        | LocalModelActivationError::Store(_)
        | LocalModelActivationError::Json(_) => LocalErrorCode::LocalModelActivationUnavailable,
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_project(error: ProjectError) -> WireError {
    let code = match error {
        ProjectError::Account(error) => return wire_error_from_account(error),
        ProjectError::InvalidName
        | ProjectError::InvalidPurpose
        | ProjectError::InvalidDesiredOutcomes
        | ProjectError::InvalidConstraint
        | ProjectError::InvalidProhibition
        | ProjectError::DuplicateCharterEntry => LocalErrorCode::InvalidProject,
        ProjectError::NotFound => LocalErrorCode::ProjectNotFound,
        ProjectError::StaleCharterRevision { .. } => LocalErrorCode::StaleCharterRevision,
        ProjectError::UnchangedCharter => LocalErrorCode::UnchangedCharter,
        ProjectError::InvalidStore | ProjectError::Store(_) | ProjectError::Json(_) => {
            LocalErrorCode::ProjectUnavailable
        }
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_project_repository(error: ProjectRepositoryError) -> WireError {
    let code = match error {
        ProjectRepositoryError::Account(error) => return wire_error_from_account(error),
        ProjectRepositoryError::ProjectNotFound => LocalErrorCode::ProjectNotFound,
        ProjectRepositoryError::StaleCharterRevision { .. } => LocalErrorCode::StaleCharterRevision,
        ProjectRepositoryError::StaleRepositoryRevision => LocalErrorCode::StaleProjectRepository,
        ProjectRepositoryError::InvalidDraft(_) => LocalErrorCode::InvalidProjectRepository,
        ProjectRepositoryError::UnchangedRepository => LocalErrorCode::UnchangedProjectRepository,
        ProjectRepositoryError::InvalidStore
        | ProjectRepositoryError::Store(_)
        | ProjectRepositoryError::Json(_) => LocalErrorCode::ProjectRepositoryUnavailable,
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

/// A preflight that produced a Receipt is a success, whatever its outcome: the
/// operator asked whether execution would be admissible and got an answer.
/// These codes are only for the cases where no preflight could be attempted.
/// No new `LocalErrorCode`: a cross-kind read fails for the same two reasons a
/// preflight read does -- a closed session, or a store it cannot trust. The
/// desktop translates `preflight_unavailable` at the recent-Receipts seam,
/// because "Beacon could not record the preflight" is false for a Query that
/// records nothing.
fn wire_error_from_recent_receipt(error: crate::RecentReceiptError) -> WireError {
    let code = match error {
        crate::RecentReceiptError::Account(error) => return wire_error_from_account(error),
        crate::RecentReceiptError::InvalidStore | crate::RecentReceiptError::Store(_) => {
            LocalErrorCode::PreflightUnavailable
        }
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_check_preflight(error: CheckPreflightError) -> WireError {
    let code = match error {
        CheckPreflightError::Account(error) => return wire_error_from_account(error),
        CheckPreflightError::ProjectNotFound => LocalErrorCode::ProjectNotFound,
        CheckPreflightError::RepositoryNotBound => LocalErrorCode::RepositoryNotBound,
        CheckPreflightError::InvalidStore
        | CheckPreflightError::Store(_)
        | CheckPreflightError::Json(_) => LocalErrorCode::PreflightUnavailable,
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_sample_project(error: SampleProjectError) -> WireError {
    let code = match error {
        SampleProjectError::Account(error) => return wire_error_from_account(error),
        SampleProjectError::NotInstalled
        | SampleProjectError::UnsafeDestination
        | SampleProjectError::BundleMismatch
        | SampleProjectError::MaterializationFailed
        | SampleProjectError::TamperedInstallation
        | SampleProjectError::InvalidStore
        | SampleProjectError::Store(_)
        | SampleProjectError::Io(_)
        | SampleProjectError::Json(_) => LocalErrorCode::SampleProjectUnavailable,
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

/// An Outcome refusal on the wire, EXHAUSTIVELY -- no wildcard, so a new refusal is a compile error
/// here rather than a silent fall into "unavailable" (blue's P1, and the rule the Policy and Mission
/// arms already carry: each refusal must arrive with its own code).
///
/// `OutcomeAbsent` covers a Mission with no publication AND a Mission in another Workspace, with one
/// code for both (R29.6): a distinct code would confirm to a caller who may not read the Workspace
/// that the record exists.
fn wire_error_from_outcome(error: PublishRefusal) -> WireError {
    let code = match error {
        // An account refusal is the account module's to report, exactly as every other wire
        // mapping does.
        PublishRefusal::Account(error) => return wire_error_from_account(error),
        PublishRefusal::Absent => LocalErrorCode::OutcomeAbsent,
        // Everything below means a stored record did not prove, or a pairing was refused. They are
        // one code on the wire because they are one thing to an operator -- this document cannot be
        // served -- and the detail belongs in the log, not in a renderer string.
        // A document that will not render is the same sentence to an operator: this document
        // cannot be served. The renderer's own refusal is named in the log, not on the wire.
        PublishRefusal::Pdf(_)
        | PublishRefusal::Seam(_)
        | PublishRefusal::NoManifestForCandidate
        | PublishRefusal::Manifest(_)
        | PublishRefusal::ManifestMissionDiffers { .. }
        | PublishRefusal::Completion(_)
        | PublishRefusal::AppendixRequired
        | PublishRefusal::VersionGap { .. }
        | PublishRefusal::AlreadyPublished
        | PublishRefusal::MissionBindingBroken { .. }
        | PublishRefusal::CandidateIsNotMissionScoped { .. }
        | PublishRefusal::QualifyingVerdictNotMeets
        | PublishRefusal::HistoryNotContiguous { .. }
        | PublishRefusal::Unverified(_)
        | PublishRefusal::Store(_) => LocalErrorCode::OutcomeUnavailable,
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_project_policy(error: ProjectPolicyError) -> WireError {
    let code = match error {
        ProjectPolicyError::Account(error) => return wire_error_from_account(error),
        ProjectPolicyError::ProjectNotFound => LocalErrorCode::ProjectNotFound,
        ProjectPolicyError::StaleCharterRevision { .. } => LocalErrorCode::StaleCharterRevision,
        ProjectPolicyError::StalePolicyRevision => LocalErrorCode::StaleProjectPolicy,
        ProjectPolicyError::InvalidRequiredCapabilities
        | ProjectPolicyError::InvalidTrackRecordRequirements => {
            LocalErrorCode::InvalidProjectPolicy
        }
        ProjectPolicyError::UnchangedPolicy => LocalErrorCode::UnchangedProjectPolicy,
        ProjectPolicyError::InvalidStore
        | ProjectPolicyError::Store(_)
        | ProjectPolicyError::Json(_) => LocalErrorCode::ProjectPolicyUnavailable,
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_team(error: TeamError) -> WireError {
    let code = match error {
        TeamError::Account(error) => return wire_error_from_account(error),
        TeamError::ProjectNotFound => LocalErrorCode::ProjectNotFound,
        TeamError::TeamNotFound => LocalErrorCode::TeamNotFound,
        TeamError::InvalidName
        | TeamError::InvalidMembers
        | TeamError::ResidentConfigurationUnavailable => LocalErrorCode::InvalidTeam,
        TeamError::StaleResidentConfiguration => LocalErrorCode::StaleTeamMemberConfiguration,
        TeamError::ResidentIneligible(_) => LocalErrorCode::TeamMemberIneligible,
        TeamError::InvalidStore | TeamError::Store(_) | TeamError::Json(_) => {
            LocalErrorCode::TeamUnavailable
        }
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

/// One closed outward answer for every verification failure, and the account's own semantics kept.
///
/// The real cause never reaches this projection: it stays in the error's `Debug` and `source`, which
/// is where R42 puts driver text, paths and user data.
fn wire_error_from_mission_completion(error: crate::MissionCompletionReadError) -> WireError {
    match error {
        crate::MissionCompletionReadError::Account(error) => wire_error_from_account(error),
        crate::MissionCompletionReadError::Unavailable(_) => WireError {
            code: LocalErrorCode::MissionCompletionUnavailable,
            retry_after_ms: None,
        },
    }
}

/// The completion family's closed outward answer.
///
/// Every arm names a code the caller already has fixed copy for. Two things this deliberately does
/// NOT do: it never forwards the field name inside `MalformedRequest`/`DuplicateSetEntry` (that
/// stays in `Debug` and `source`, R42), and it never calls a malformed identity, digest, tag,
/// integer or duplicated set entry "text over limit" -- a grammar failure disguised as a length
/// failure would send the Operator to shorten text that was never the problem.
fn wire_error_from_completion_command(error: crate::CompletionCommandError) -> WireError {
    let code = match error {
        crate::CompletionCommandError::Account(error) => return wire_error_from_account(error),
        crate::CompletionCommandError::IntentChanged => LocalErrorCode::CompletionIntentChanged,
        crate::CompletionCommandError::AlreadyRecorded => LocalErrorCode::CompletionAlreadyRecorded,
        crate::CompletionCommandError::ResidentNotInAdmissionRoster => {
            LocalErrorCode::CompletionResidentNotInAdmissionRoster
        }
        crate::CompletionCommandError::ConditionCoverageIncomplete => {
            LocalErrorCode::CompletionConditionCoverageIncomplete
        }
        crate::CompletionCommandError::SupportOverLimit => LocalErrorCode::CompletionSupportOverLimit,
        crate::CompletionCommandError::TextOverLimit => LocalErrorCode::CompletionTextOverLimit,
        crate::CompletionCommandError::SpendReasonRequired => {
            LocalErrorCode::CompletionSpendReasonRequired
        }
        crate::CompletionCommandError::MalformedRequest(_)
        | crate::CompletionCommandError::DuplicateSetEntry(_) => {
            LocalErrorCode::CompletionInvalidDetails
        }
        // The same code C3's read already uses, with C3's exact sentence: an unverifiable Mission
        // completion is one condition, and the caller knows which operation it asked for.
        crate::CompletionCommandError::Unavailable(_) => LocalErrorCode::MissionCompletionUnavailable,
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_mission(error: MissionError) -> WireError {
    let code = match error {
        MissionError::Account(error) => return wire_error_from_account(error),
        MissionError::ProjectNotFound => LocalErrorCode::ProjectNotFound,
        MissionError::TeamNotFound => LocalErrorCode::TeamNotFound,
        MissionError::MissionNotFound => LocalErrorCode::MissionNotFound,
        MissionError::StaleCharterRevision { .. } => LocalErrorCode::StaleCharterRevision,
        MissionError::InvalidDesiredOutcome
        | MissionError::InvalidSuccessConditions
        | MissionError::InvalidConstraint
        | MissionError::InvalidProhibition
        | MissionError::DuplicateMissionEntry
        | MissionError::InvalidMembers => LocalErrorCode::InvalidMission,
        MissionError::StaleTeamEligibility => LocalErrorCode::StaleMissionTeam,
        MissionError::ResidentIneligible(_) => LocalErrorCode::MissionMemberIneligible,
        MissionError::InvalidStore | MissionError::Store(_) | MissionError::Json(_) => {
            LocalErrorCode::MissionUnavailable
        }
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_starter(error: StarterOrganizationError) -> WireError {
    let code = match error {
        StarterOrganizationError::Account(error) => return wire_error_from_account(error),
        StarterOrganizationError::UnsupportedProposalVersion => {
            LocalErrorCode::UnsupportedStarterProposal
        }
        StarterOrganizationError::EmptyRoster
        | StarterOrganizationError::DuplicateRole
        | StarterOrganizationError::InvalidDisplayName => LocalErrorCode::InvalidStarterRoster,
        StarterOrganizationError::DecisionAlreadyRecorded => {
            LocalErrorCode::StarterDecisionRecorded
        }
        StarterOrganizationError::InvalidStore
        | StarterOrganizationError::Store(_)
        | StarterOrganizationError::Json(_) => LocalErrorCode::StarterOrganizationUnavailable,
    };
    WireError {
        code,
        retry_after_ms: None,
    }
}

fn wire_error_from_account(error: OperatorAccountError) -> WireError {
    let (code, retry_after_ms) = match error {
        OperatorAccountError::PasswordTooShort => (LocalErrorCode::PasswordTooShort, None),
        OperatorAccountError::PasswordTooLong => (LocalErrorCode::PasswordTooLong, None),
        OperatorAccountError::PasswordConfirmationMismatch => {
            (LocalErrorCode::PasswordConfirmationMismatch, None)
        }
        OperatorAccountError::RecoveryNotConfirmed => (LocalErrorCode::RecoveryNotConfirmed, None),
        OperatorAccountError::SetupNotStarted => (LocalErrorCode::SetupNotStarted, None),
        OperatorAccountError::AccountAlreadyExists => (LocalErrorCode::AccountAlreadyExists, None),
        OperatorAccountError::AccountMissing => (LocalErrorCode::AccountMissing, None),
        OperatorAccountError::AuthenticationFailed => (LocalErrorCode::AuthenticationFailed, None),
        OperatorAccountError::AuthenticationThrottled { retry_after_ms } => (
            LocalErrorCode::AuthenticationThrottled,
            Some(retry_after_ms),
        ),
        OperatorAccountError::CredentialDisabled => (LocalErrorCode::CredentialDisabled, None),
        OperatorAccountError::RemotePasswordNotAllowed => {
            (LocalErrorCode::RemotePasswordNotAllowed, None)
        }
        OperatorAccountError::NotAuthenticated => (LocalErrorCode::NotAuthenticated, None),
        OperatorAccountError::InvalidSessionTimeout => {
            (LocalErrorCode::InvalidSessionTimeout, None)
        }
        // Was `_`. Same five inputs, same result; a new account fault now has to be classified
        // here rather than inheriting "the account service is unavailable" by default.
        OperatorAccountError::CredentialProcessing
        | OperatorAccountError::InvalidStore
        | OperatorAccountError::ClockBeforeUnixEpoch
        | OperatorAccountError::Store(_)
        | OperatorAccountError::StateUnavailable => {
            (LocalErrorCode::AccountServiceUnavailable, None)
        }
    };
    WireError {
        code,
        retry_after_ms,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use uuid::Uuid;

    use super::{
        LocalCommand, LocalErrorCode, LocalServerConfig, LocalSupervisorClient,
        LocalSupervisorServer, LocalTransportError, MAX_COMPLETION_PREVIEW_RESPONSE_BYTES,
        MAX_COMPLETION_REQUEST_BYTES, MAX_MESSAGE_BYTES, ResponseEnvelope, WireError,
        accepts_completion_sized_request, initialize_local_auth_token,
        response_from_transport_error, response_max,
    };
    use crate::outcome::MalformedOutcome;
    use crate::outcome_pdf::PdfRefusal;
    use crate::outcome_store::ManifestStoreRefusal;
    use crate::outcome_typst::TypstRefusal;
    use crate::publication_seam::PublicationSeamRefusal;
    use crate::{
        ArtifactError, CandidateError, ClassificationStoreError, CompletionReadRefusal,
        EvaluationError, LocalModelActivationError, MalformedClassification, PredecessorFault,
        ProposalError, RulingError, StartupPass, SupervisorError, TotalsMismatch,
    };
    use crate::{
        CheckPreflightError, LocalModelEndpoint, MissionError, OperatorAccountError, ProjectError,
        ProjectPolicyError, ProjectRepositoryError, PublishRefusal, ResidentConfigurationError,
        ResidentLifecycleError, ResourceDiscoveryError, SampleProjectError,
        SampleProjectInstallOutcome, SampleProjectStatus, SecretString, StarterOrganizationError,
        TeamError,
    };

    #[test]
    fn sample_project_query_and_command_are_authenticated_over_local_transport() {
        let root =
            std::env::temp_dir().join(format!("beacond-sample-transport-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let store_path = root.join("beacon.sqlite3");
        let socket_path = root.join("beacond.sock");
        let token_path = root.join("beacond.token");
        let lock_path = root.join("beacond.lock");
        initialize_local_auth_token(&token_path).unwrap();
        let server = LocalSupervisorServer::bind(LocalServerConfig {
            store_path,
            initial_workspace_name: "Personal".to_owned(),
            socket_path: socket_path.clone(),
            auth_token_path: token_path.clone(),
            lock_path,
            local_model_endpoint: LocalModelEndpoint::ollama_default(),
        })
        .unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let server_thread = thread::spawn(move || server.serve_until(thread_running).unwrap());
        let client = LocalSupervisorClient::connect(&socket_path, &token_path).unwrap();

        assert!(matches!(
            client.sample_project_status(),
            Err(LocalTransportError::Remote {
                code: LocalErrorCode::NotAuthenticated,
                ..
            })
        ));
        assert!(matches!(
            client.install_sample_project(),
            Err(LocalTransportError::Remote {
                code: LocalErrorCode::NotAuthenticated,
                ..
            })
        ));
        client
            .begin_password_setup(
                SecretString::new("a long local passphrase".to_owned()),
                SecretString::new("a long local passphrase".to_owned()),
            )
            .unwrap();
        client.complete_operator_setup(true).unwrap();
        assert!(matches!(
            client.sample_project_status().unwrap(),
            SampleProjectStatus::Available { .. }
        ));
        let installed = client.install_sample_project().unwrap();
        assert_eq!(installed.outcome, SampleProjectInstallOutcome::Installed);

        running.store(false, Ordering::Release);
        server_thread.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// A command slower than the default deadline is ANSWERED, not reported as a broken socket.
    ///
    /// R41.4. `CLIENT_RESPONSE_TIMEOUT` is a socket read timeout, so when it expires the client
    /// gets `EAGAIN` -- `Io(WouldBlock)` -- while the supervisor is still working, and the desktop
    /// flattens that to `account_service_unavailable`. Four recorded flakes, all on
    /// `InstallSampleProject`, the one command that copies a tree and runs `git`.
    ///
    /// # Two peers, because the two arms need different things
    ///
    /// The install arm needs an answer that arrives late: the stub waits 7.5 s, past the default
    /// deadline and far inside the slow one, then replies.
    ///
    /// The ordinary arm needs the client's read to end for exactly one reason, and astra's cold
    /// read caught the first version measuring the wrong interval. A stub that replies on a timer
    /// starts that timer when it finishes READING the request, while the client's budget starts at
    /// its own blocking read -- so any pause between the two (astra's probe uses 700 ms) leaves the
    /// answer arriving INSIDE the budget and flips "it still times out" into a pass for the wrong
    /// reason. The second peer therefore never replies at all. It holds the connection open, so the
    /// read can only end by `SO_RCVTIMEO`, and the assertion holds however the scheduler behaves.
    #[test]
    fn a_command_slower_than_the_default_deadline_is_answered_not_refused() {
        use std::io::{Read as _, Write as _};
        use std::os::unix::net::{UnixListener, UnixStream};

        /// Cleans up even when an assertion unwinds -- a failing run used to leave its socket
        /// directory behind (astra's note).
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        let root = std::env::temp_dir().join(format!("beacond-slow-command-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let scratch = Scratch(root.clone());
        let socket_path = root.join("supervisor.sock");
        let token_path = root.join("auth.token");
        super::initialize_local_auth_token(&token_path).unwrap();

        // The constant reaches the socket as a READ timeout. Everything below rests on that, so it
        // is read back from the kernel rather than assumed (astra's probe does the same).
        let (probe, _probe_peer) = UnixStream::pair().unwrap();
        super::configure_stream_deadlines(&probe, super::CLIENT_RESPONSE_TIMEOUT).unwrap();
        assert_eq!(
            probe.read_timeout().unwrap(),
            Some(super::CLIENT_RESPONSE_TIMEOUT),
            "the default deadline is not the socket's read timeout"
        );

        let listener = UnixListener::bind(&socket_path).unwrap();
        let holding = Arc::new(AtomicBool::new(true));
        let stub_holding = Arc::clone(&holding);
        let stub = thread::spawn(move || {
            // First connection: the install. Answer late, but answer.
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).unwrap();
            thread::sleep(std::time::Duration::from_millis(7_500));
            let response = super::ResponseEnvelope {
                protocol: super::PROTOCOL_VERSION,
                result: None,
                error: Some(super::WireError {
                    code: LocalErrorCode::AccountMissing,
                    retry_after_ms: None,
                }),
            };
            let _ = stream.write_all(&serde_json::to_vec(&response).unwrap());
            drop(stream);

            // Second connection: read the request and NEVER reply, holding it open so the client's
            // read can end only by its own deadline.
            let (held, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            (&held).read_to_end(&mut request).unwrap();
            while stub_holding.load(Ordering::Acquire) {
                thread::sleep(std::time::Duration::from_millis(25));
            }
            drop(held);
        });

        let client = LocalSupervisorClient::connect(&socket_path, &token_path).unwrap();

        // The slow command waits, and comes back with the server's own answer.
        assert!(
            matches!(
                client.install_sample_project(),
                Err(LocalTransportError::Remote {
                    code: LocalErrorCode::AccountMissing,
                    ..
                })
            ),
            "the install did not survive its own deadline"
        );

        // The ordinary command against a peer that never answers gives up on its own deadline --
        // which is what says the longer budget is scoped to the slow command rather than relaxing
        // every deadline.
        let ordinary = client.sample_project_status();
        holding.store(false, Ordering::Release);
        assert!(
            matches!(
                ordinary,
                Err(LocalTransportError::Io(ref error)) if error.kind() == std::io::ErrorKind::WouldBlock
            ),
            "the default deadline was relaxed for every command, not just the slow one: \
             {ordinary:?}"
        );

        stub.join().unwrap();
        drop(scratch);
    }

    /// Every arm of the wire vocabulary, read out of this file's own source.
    fn command_vocabulary(source: &str) -> Vec<&str> {
        source
            .split("enum LocalCommand {")
            .nth(1)
            .expect("the wire vocabulary is an enum in this file")
            .split("\n}")
            .next()
            .expect("the enum closes")
            .lines()
            .map(str::trim)
            .filter(|line| line.ends_with(',') || line.ends_with(" {"))
            .collect()
    }

    /// Is any arm a way to reach dispatch, Pause or Resume?
    fn reaches_dispatch(arms: &[&str]) -> Option<String> {
        arms.iter()
            .find(|arm| {
                let lowered = arm.to_lowercase();
                lowered.contains("dispatch")
                    || lowered.contains("pause")
                    || lowered.contains("resume")
            })
            .map(|arm| (*arm).to_owned())
    }

    #[test]
    fn no_transport_command_can_reach_dispatch_or_pause() {
        // ADR 0039 requires dispatch to be reachable only from one named Operator surface, and this
        // build exposes none. The executable inventory is the wire vocabulary itself: without it,
        // adding a `Dispatch` arm would be a silent widening of what can mutate a repository -- the
        // absence is true today and would be unguarded tomorrow (gpt1). Pause is included because
        // it gates dispatch, and a remote Resume would be a remote way past that gate.
        let arms = command_vocabulary(include_str!("local_transport.rs"));
        assert!(arms.len() > 20, "the inventory did not find the vocabulary");
        assert_eq!(reaches_dispatch(&arms), None);
    }

    #[test]
    fn the_surface_inventory_would_notice_a_dispatch_arm() {
        // The inventory's own control. Mutating the real enum cannot demonstrate this -- every
        // exhaustive match in the crate would stop compiling first -- so the predicate is exercised
        // against a synthetic vocabulary instead.
        let synthetic =
            "enum LocalCommand {\n    Status,\n    DispatchProposal { ruling: String },\n}\n";
        let arms = command_vocabulary(synthetic);
        assert_eq!(arms.len(), 2);
        assert_eq!(
            reaches_dispatch(&arms).as_deref(),
            Some("DispatchProposal { ruling: String },")
        );

        // And Resume, for the same reason.
        let synthetic = "enum LocalCommand {\n    Status,\n    ResumeWork,\n}\n";
        assert_eq!(
            reaches_dispatch(&command_vocabulary(synthetic)).as_deref(),
            Some("ResumeWork,")
        );
    }

    // -------------------------------------------------------------------------------------
    // R42 slice 2, commit 2: the transport's caller-facing text is closed.
    // -------------------------------------------------------------------------------------

    /// Two fixed synthetic markers, each placed before any newline so that "is it in the line"
    /// is a substring question and never an escaping question.
    const NEEDLE_A: &str = "R42S2_PRIVATE_A";
    /// The field name a malformed/duplicate completion refusal carries. A needle, not a message: an
    /// outward line or wire code containing it would be the owner's private diagnostic escaping.
    const NEEDLE_FIELD: &str = "R42S2_PRIVATE_FIELD";
    const NEEDLE_B: &str = "R42S2_PRIVATE_B";

    /// Approved numeric context, chosen here before any error is built and pairwise distinct so a
    /// swapped pair cannot pass.
    const FIRST_EXPECTED: u32 = 7;
    const FIRST_CURRENT: u32 = 11;
    const SECOND_EXPECTED: u32 = 13;
    const SECOND_CURRENT: u32 = 17;
    const THROTTLE_MS: u64 = 4242;

    fn payload_a() -> String {
        format!("{NEEDLE_A} /home/example/private.txt column=private_column\r\nsecond-line")
    }

    fn payload_b() -> String {
        format!("{NEEDLE_B} /home/example/other.txt column=other_column\r\nsecond-line")
    }

    fn driver_error(needle: &str) -> rusqlite::Error {
        rusqlite::Error::InvalidParameterName(format!("{needle} :private_binding"))
    }

    fn io_error(needle: &str) -> std::io::Error {
        std::io::Error::other(format!("{needle} /home/example/private.txt"))
    }

    fn json_error(needle: &str) -> serde_json::Error {
        serde::de::Error::custom(format!("{needle} /home/example/private.txt"))
    }

    /// One declarative row table per vocabulary, expanded into BOTH the exhaustive expected match
    /// and the constructed fixtures. Wildcard-free: a new variant anywhere in this tree fails to
    /// compile here until its row supplies a constructor, an independently written expected
    /// literal, and its declared markers. No table reads a production `Display` or `name()`.
    macro_rules! closed_table {
        (
            $expected_fn:ident / $rows_fn:ident : $ty:path,
            $( $case:literal | $pattern:pat => $line:expr , $build:expr , $markers:expr ; )+
        ) => {
            fn $expected_fn(value: &$ty) -> (&'static str, String) {
                match value {
                    $( $pattern => ($case, String::from($line)), )+
                }
            }

            #[allow(dead_code)]
            fn $rows_fn() -> Vec<(&'static str, $ty, &'static [&'static str])> {
                vec![$(($case, $build, $markers)),+]
            }
        };
    }

    // ---- generated child tables ----
    closed_table! {
        expected_account / account_rows: OperatorAccountError,
        "PasswordTooShort" | OperatorAccountError::PasswordTooShort
            => "PasswordTooShort",
            OperatorAccountError::PasswordTooShort, &[];
        "PasswordTooLong" | OperatorAccountError::PasswordTooLong
            => "PasswordTooLong",
            OperatorAccountError::PasswordTooLong, &[];
        "PasswordConfirmationMismatch" | OperatorAccountError::PasswordConfirmationMismatch
            => "PasswordConfirmationMismatch",
            OperatorAccountError::PasswordConfirmationMismatch, &[];
        "RecoveryNotConfirmed" | OperatorAccountError::RecoveryNotConfirmed
            => "RecoveryNotConfirmed",
            OperatorAccountError::RecoveryNotConfirmed, &[];
        "SetupNotStarted" | OperatorAccountError::SetupNotStarted
            => "SetupNotStarted",
            OperatorAccountError::SetupNotStarted, &[];
        "AccountAlreadyExists" | OperatorAccountError::AccountAlreadyExists
            => "AccountAlreadyExists",
            OperatorAccountError::AccountAlreadyExists, &[];
        "AccountMissing" | OperatorAccountError::AccountMissing
            => "AccountMissing",
            OperatorAccountError::AccountMissing, &[];
        "AuthenticationFailed" | OperatorAccountError::AuthenticationFailed
            => "AuthenticationFailed",
            OperatorAccountError::AuthenticationFailed, &[];
        "AuthenticationThrottled" | OperatorAccountError::AuthenticationThrottled { .. }
            => "AuthenticationThrottled",
            OperatorAccountError::AuthenticationThrottled { retry_after_ms: THROTTLE_MS }, &[];
        "CredentialDisabled" | OperatorAccountError::CredentialDisabled
            => "CredentialDisabled",
            OperatorAccountError::CredentialDisabled, &[];
        "RemotePasswordNotAllowed" | OperatorAccountError::RemotePasswordNotAllowed
            => "RemotePasswordNotAllowed",
            OperatorAccountError::RemotePasswordNotAllowed, &[];
        "NotAuthenticated" | OperatorAccountError::NotAuthenticated
            => "NotAuthenticated",
            OperatorAccountError::NotAuthenticated, &[];
        "InvalidSessionTimeout" | OperatorAccountError::InvalidSessionTimeout
            => "InvalidSessionTimeout",
            OperatorAccountError::InvalidSessionTimeout, &[];
        "CredentialProcessing" | OperatorAccountError::CredentialProcessing
            => "CredentialProcessing",
            OperatorAccountError::CredentialProcessing, &[];
        "InvalidStore" | OperatorAccountError::InvalidStore
            => "InvalidStore",
            OperatorAccountError::InvalidStore, &[];
        "ClockBeforeUnixEpoch" | OperatorAccountError::ClockBeforeUnixEpoch
            => "ClockBeforeUnixEpoch",
            OperatorAccountError::ClockBeforeUnixEpoch, &[];
        "Store" | OperatorAccountError::Store(_)
            => "Store",
            OperatorAccountError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "StateUnavailable" | OperatorAccountError::StateUnavailable
            => "StateUnavailable",
            OperatorAccountError::StateUnavailable, &[];
    }

    closed_table! {
        expected_draft / draft_rows: crate::InvalidRepositoryDraft,
        "Source" | crate::InvalidRepositoryDraft::Source
            => "Source", crate::InvalidRepositoryDraft::Source, &[];
        "BaseRevision" | crate::InvalidRepositoryDraft::BaseRevision
            => "BaseRevision", crate::InvalidRepositoryDraft::BaseRevision, &[];
        "DeclaredChecks" | crate::InvalidRepositoryDraft::DeclaredChecks
            => "DeclaredChecks", crate::InvalidRepositoryDraft::DeclaredChecks, &[];
    }

    closed_table! {
        expected_starter / starter_rows: StarterOrganizationError,
        "Account" | StarterOrganizationError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            StarterOrganizationError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "UnsupportedProposalVersion" | StarterOrganizationError::UnsupportedProposalVersion
            => "UnsupportedProposalVersion",
            StarterOrganizationError::UnsupportedProposalVersion, &[];
        "EmptyRoster" | StarterOrganizationError::EmptyRoster
            => "EmptyRoster",
            StarterOrganizationError::EmptyRoster, &[];
        "DuplicateRole" | StarterOrganizationError::DuplicateRole
            => "DuplicateRole",
            StarterOrganizationError::DuplicateRole, &[];
        "InvalidDisplayName" | StarterOrganizationError::InvalidDisplayName
            => "InvalidDisplayName",
            StarterOrganizationError::InvalidDisplayName, &[];
        "DecisionAlreadyRecorded" | StarterOrganizationError::DecisionAlreadyRecorded
            => "DecisionAlreadyRecorded",
            StarterOrganizationError::DecisionAlreadyRecorded, &[];
        "InvalidStore" | StarterOrganizationError::InvalidStore
            => "InvalidStore",
            StarterOrganizationError::InvalidStore, &[];
        "Store" | StarterOrganizationError::Store(_)
            => "Store",
            StarterOrganizationError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | StarterOrganizationError::Json(_)
            => "Json",
            StarterOrganizationError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_lifecycle / lifecycle_rows: ResidentLifecycleError,
        "Account" | ResidentLifecycleError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            ResidentLifecycleError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "NotFound" | ResidentLifecycleError::NotFound
            => "NotFound",
            ResidentLifecycleError::NotFound, &[];
        "InvalidTransition" | ResidentLifecycleError::InvalidTransition
            => "InvalidTransition",
            ResidentLifecycleError::InvalidTransition, &[];
        "StaleDecision" | ResidentLifecycleError::StaleDecision
            => "StaleDecision",
            ResidentLifecycleError::StaleDecision, &[];
        "InvalidStore" | ResidentLifecycleError::InvalidStore
            => "InvalidStore",
            ResidentLifecycleError::InvalidStore, &[];
        "Store" | ResidentLifecycleError::Store(_)
            => "Store",
            ResidentLifecycleError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | ResidentLifecycleError::Json(_)
            => "Json",
            ResidentLifecycleError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_configuration / configuration_rows: ResidentConfigurationError,
        "Account" | ResidentConfigurationError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            ResidentConfigurationError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "ResidentNotFound" | ResidentConfigurationError::ResidentNotFound
            => "ResidentNotFound",
            ResidentConfigurationError::ResidentNotFound, &[];
        "ResidentRetired" | ResidentConfigurationError::ResidentRetired
            => "ResidentRetired",
            ResidentConfigurationError::ResidentRetired, &[];
        "ModelActivationUnavailable" | ResidentConfigurationError::ModelActivationUnavailable
            => "ModelActivationUnavailable",
            ResidentConfigurationError::ModelActivationUnavailable, &[];
        "ModelMissingCompletion" | ResidentConfigurationError::ModelMissingCompletion
            => "ModelMissingCompletion",
            ResidentConfigurationError::ModelMissingCompletion, &[];
        "InvalidCapabilities" | ResidentConfigurationError::InvalidCapabilities
            => "InvalidCapabilities",
            ResidentConfigurationError::InvalidCapabilities, &[];
        "StaleDecision" | ResidentConfigurationError::StaleDecision
            => "StaleDecision",
            ResidentConfigurationError::StaleDecision, &[];
        "AlreadyCleared" | ResidentConfigurationError::AlreadyCleared
            => "AlreadyCleared",
            ResidentConfigurationError::AlreadyCleared, &[];
        "InvalidStore" | ResidentConfigurationError::InvalidStore
            => "InvalidStore",
            ResidentConfigurationError::InvalidStore, &[];
        "Store" | ResidentConfigurationError::Store(_)
            => "Store",
            ResidentConfigurationError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | ResidentConfigurationError::Json(_)
            => "Json",
            ResidentConfigurationError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_project / project_rows: ProjectError,
        "Account" | ProjectError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            ProjectError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "InvalidName" | ProjectError::InvalidName
            => "InvalidName",
            ProjectError::InvalidName, &[];
        "InvalidPurpose" | ProjectError::InvalidPurpose
            => "InvalidPurpose",
            ProjectError::InvalidPurpose, &[];
        "InvalidDesiredOutcomes" | ProjectError::InvalidDesiredOutcomes
            => "InvalidDesiredOutcomes",
            ProjectError::InvalidDesiredOutcomes, &[];
        "InvalidConstraint" | ProjectError::InvalidConstraint
            => "InvalidConstraint",
            ProjectError::InvalidConstraint, &[];
        "InvalidProhibition" | ProjectError::InvalidProhibition
            => "InvalidProhibition",
            ProjectError::InvalidProhibition, &[];
        "DuplicateCharterEntry" | ProjectError::DuplicateCharterEntry
            => "DuplicateCharterEntry",
            ProjectError::DuplicateCharterEntry, &[];
        "StaleCharterRevision" | ProjectError::StaleCharterRevision { expected, current }
            => format!("StaleCharterRevision(expected={expected},current={current})"),
            ProjectError::StaleCharterRevision { expected: FIRST_EXPECTED, current: FIRST_CURRENT }, &[];
        "UnchangedCharter" | ProjectError::UnchangedCharter
            => "UnchangedCharter",
            ProjectError::UnchangedCharter, &[];
        "NotFound" | ProjectError::NotFound
            => "NotFound",
            ProjectError::NotFound, &[];
        "InvalidStore" | ProjectError::InvalidStore
            => "InvalidStore",
            ProjectError::InvalidStore, &[];
        "Store" | ProjectError::Store(_)
            => "Store",
            ProjectError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | ProjectError::Json(_)
            => "Json",
            ProjectError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_policy / policy_rows: ProjectPolicyError,
        "Account" | ProjectPolicyError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            ProjectPolicyError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "ProjectNotFound" | ProjectPolicyError::ProjectNotFound
            => "ProjectNotFound",
            ProjectPolicyError::ProjectNotFound, &[];
        "StaleCharterRevision" | ProjectPolicyError::StaleCharterRevision { expected, current }
            => format!("StaleCharterRevision(expected={expected},current={current})"),
            ProjectPolicyError::StaleCharterRevision { expected: FIRST_EXPECTED, current: FIRST_CURRENT }, &[];
        "StalePolicyRevision" | ProjectPolicyError::StalePolicyRevision
            => "StalePolicyRevision",
            ProjectPolicyError::StalePolicyRevision, &[];
        "InvalidRequiredCapabilities" | ProjectPolicyError::InvalidRequiredCapabilities
            => "InvalidRequiredCapabilities",
            ProjectPolicyError::InvalidRequiredCapabilities, &[];
        "InvalidTrackRecordRequirements" | ProjectPolicyError::InvalidTrackRecordRequirements
            => "InvalidTrackRecordRequirements",
            ProjectPolicyError::InvalidTrackRecordRequirements, &[];
        "UnchangedPolicy" | ProjectPolicyError::UnchangedPolicy
            => "UnchangedPolicy",
            ProjectPolicyError::UnchangedPolicy, &[];
        "InvalidStore" | ProjectPolicyError::InvalidStore
            => "InvalidStore",
            ProjectPolicyError::InvalidStore, &[];
        "Store" | ProjectPolicyError::Store(_)
            => "Store",
            ProjectPolicyError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | ProjectPolicyError::Json(_)
            => "Json",
            ProjectPolicyError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_repository / repository_rows: ProjectRepositoryError,
        "Account" | ProjectRepositoryError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            ProjectRepositoryError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "ProjectNotFound" | ProjectRepositoryError::ProjectNotFound
            => "ProjectNotFound",
            ProjectRepositoryError::ProjectNotFound, &[];
        "StaleCharterRevision" | ProjectRepositoryError::StaleCharterRevision { expected, current }
            => format!("StaleCharterRevision(expected={expected},current={current})"),
            ProjectRepositoryError::StaleCharterRevision { expected: FIRST_EXPECTED, current: FIRST_CURRENT }, &[];
        "StaleRepositoryRevision" | ProjectRepositoryError::StaleRepositoryRevision
            => "StaleRepositoryRevision",
            ProjectRepositoryError::StaleRepositoryRevision, &[];
        "InvalidDraft" | ProjectRepositoryError::InvalidDraft(reason)
            => format!("InvalidDraft.{}", expected_draft(reason).1),
            ProjectRepositoryError::InvalidDraft(crate::InvalidRepositoryDraft::Source), &[];
        "UnchangedRepository" | ProjectRepositoryError::UnchangedRepository
            => "UnchangedRepository",
            ProjectRepositoryError::UnchangedRepository, &[];
        "InvalidStore" | ProjectRepositoryError::InvalidStore
            => "InvalidStore",
            ProjectRepositoryError::InvalidStore, &[];
        "Store" | ProjectRepositoryError::Store(_)
            => "Store",
            ProjectRepositoryError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | ProjectRepositoryError::Json(_)
            => "Json",
            ProjectRepositoryError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_preflight / preflight_rows: CheckPreflightError,
        "Account" | CheckPreflightError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            CheckPreflightError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "ProjectNotFound" | CheckPreflightError::ProjectNotFound
            => "ProjectNotFound",
            CheckPreflightError::ProjectNotFound, &[];
        "RepositoryNotBound" | CheckPreflightError::RepositoryNotBound
            => "RepositoryNotBound",
            CheckPreflightError::RepositoryNotBound, &[];
        "InvalidStore" | CheckPreflightError::InvalidStore
            => "InvalidStore",
            CheckPreflightError::InvalidStore, &[];
        "Store" | CheckPreflightError::Store(_)
            => "Store",
            CheckPreflightError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | CheckPreflightError::Json(_)
            => "Json",
            CheckPreflightError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_receipt / receipt_rows: crate::RecentReceiptError,
        "Account" | crate::RecentReceiptError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            crate::RecentReceiptError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "InvalidStore" | crate::RecentReceiptError::InvalidStore
            => "InvalidStore",
            crate::RecentReceiptError::InvalidStore, &[];
        "Store" | crate::RecentReceiptError::Store(_)
            => "Store",
            crate::RecentReceiptError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_sample / sample_rows: SampleProjectError,
        "Account" | SampleProjectError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            SampleProjectError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "NotInstalled" | SampleProjectError::NotInstalled
            => "NotInstalled",
            SampleProjectError::NotInstalled, &[];
        "UnsafeDestination" | SampleProjectError::UnsafeDestination
            => "UnsafeDestination",
            SampleProjectError::UnsafeDestination, &[];
        "BundleMismatch" | SampleProjectError::BundleMismatch
            => "BundleMismatch",
            SampleProjectError::BundleMismatch, &[];
        "MaterializationFailed" | SampleProjectError::MaterializationFailed
            => "MaterializationFailed",
            SampleProjectError::MaterializationFailed, &[];
        "TamperedInstallation" | SampleProjectError::TamperedInstallation
            => "TamperedInstallation",
            SampleProjectError::TamperedInstallation, &[];
        "InvalidStore" | SampleProjectError::InvalidStore
            => "InvalidStore",
            SampleProjectError::InvalidStore, &[];
        "Store" | SampleProjectError::Store(_)
            => "Store",
            SampleProjectError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Io" | SampleProjectError::Io(_)
            => "Io",
            SampleProjectError::Io(io_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | SampleProjectError::Json(_)
            => "Json",
            SampleProjectError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_team / team_rows: TeamError,
        "Account" | TeamError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            TeamError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "ProjectNotFound" | TeamError::ProjectNotFound
            => "ProjectNotFound",
            TeamError::ProjectNotFound, &[];
        "TeamNotFound" | TeamError::TeamNotFound
            => "TeamNotFound",
            TeamError::TeamNotFound, &[];
        "InvalidName" | TeamError::InvalidName
            => "InvalidName",
            TeamError::InvalidName, &[];
        "InvalidMembers" | TeamError::InvalidMembers
            => "InvalidMembers",
            TeamError::InvalidMembers, &[];
        "ResidentConfigurationUnavailable" | TeamError::ResidentConfigurationUnavailable
            => "ResidentConfigurationUnavailable",
            TeamError::ResidentConfigurationUnavailable, &[];
        "StaleResidentConfiguration" | TeamError::StaleResidentConfiguration
            => "StaleResidentConfiguration",
            TeamError::StaleResidentConfiguration, &[];
        "ResidentIneligible" | TeamError::ResidentIneligible(_)
            => "ResidentIneligible",
            TeamError::ResidentIneligible(first_uuid()), &[];
        "InvalidStore" | TeamError::InvalidStore
            => "InvalidStore",
            TeamError::InvalidStore, &[];
        "Store" | TeamError::Store(_)
            => "Store",
            TeamError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | TeamError::Json(_)
            => "Json",
            TeamError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_completion_read / completion_read_rows: crate::MissionCompletionReadError,
        "Account" | crate::MissionCompletionReadError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            crate::MissionCompletionReadError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "Unavailable" | crate::MissionCompletionReadError::Unavailable(_)
            => "Unavailable",
            crate::MissionCompletionReadError::Unavailable(crate::CompletionUnavailable::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
    }

    closed_table! {
        expected_completion_command / completion_command_rows: crate::CompletionCommandError,
        "Account" | crate::CompletionCommandError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            crate::CompletionCommandError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "IntentChanged" | crate::CompletionCommandError::IntentChanged
            => "IntentChanged", crate::CompletionCommandError::IntentChanged, &[];
        "AlreadyRecorded" | crate::CompletionCommandError::AlreadyRecorded
            => "AlreadyRecorded", crate::CompletionCommandError::AlreadyRecorded, &[];
        "ResidentNotInAdmissionRoster" | crate::CompletionCommandError::ResidentNotInAdmissionRoster
            => "ResidentNotInAdmissionRoster",
            crate::CompletionCommandError::ResidentNotInAdmissionRoster, &[];
        "ConditionCoverageIncomplete" | crate::CompletionCommandError::ConditionCoverageIncomplete
            => "ConditionCoverageIncomplete",
            crate::CompletionCommandError::ConditionCoverageIncomplete, &[];
        "SupportOverLimit" | crate::CompletionCommandError::SupportOverLimit
            => "SupportOverLimit", crate::CompletionCommandError::SupportOverLimit, &[];
        "TextOverLimit" | crate::CompletionCommandError::TextOverLimit
            => "TextOverLimit", crate::CompletionCommandError::TextOverLimit, &[];
        "SpendReasonRequired" | crate::CompletionCommandError::SpendReasonRequired
            => "SpendReasonRequired", crate::CompletionCommandError::SpendReasonRequired, &[];
        "MalformedRequest" | crate::CompletionCommandError::MalformedRequest(_)
            => "MalformedRequest",
            crate::CompletionCommandError::MalformedRequest(NEEDLE_FIELD), &[NEEDLE_FIELD];
        "DuplicateSetEntry" | crate::CompletionCommandError::DuplicateSetEntry(_)
            => "DuplicateSetEntry",
            crate::CompletionCommandError::DuplicateSetEntry(NEEDLE_FIELD), &[NEEDLE_FIELD];
        "Unavailable" | crate::CompletionCommandError::Unavailable(_)
            => "Unavailable",
            crate::CompletionCommandError::Unavailable(crate::CompletionCommandUnavailable::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
    }

    closed_table! {
        expected_mission / mission_rows: MissionError,
        "Account" | MissionError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            MissionError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "ProjectNotFound" | MissionError::ProjectNotFound
            => "ProjectNotFound",
            MissionError::ProjectNotFound, &[];
        "TeamNotFound" | MissionError::TeamNotFound
            => "TeamNotFound",
            MissionError::TeamNotFound, &[];
        "MissionNotFound" | MissionError::MissionNotFound
            => "MissionNotFound",
            MissionError::MissionNotFound, &[];
        "StaleCharterRevision" | MissionError::StaleCharterRevision { expected, current }
            => format!("StaleCharterRevision(expected={expected},current={current})"),
            MissionError::StaleCharterRevision { expected: FIRST_EXPECTED, current: FIRST_CURRENT }, &[];
        "InvalidDesiredOutcome" | MissionError::InvalidDesiredOutcome
            => "InvalidDesiredOutcome",
            MissionError::InvalidDesiredOutcome, &[];
        "InvalidSuccessConditions" | MissionError::InvalidSuccessConditions
            => "InvalidSuccessConditions",
            MissionError::InvalidSuccessConditions, &[];
        "InvalidConstraint" | MissionError::InvalidConstraint
            => "InvalidConstraint",
            MissionError::InvalidConstraint, &[];
        "InvalidProhibition" | MissionError::InvalidProhibition
            => "InvalidProhibition",
            MissionError::InvalidProhibition, &[];
        "DuplicateMissionEntry" | MissionError::DuplicateMissionEntry
            => "DuplicateMissionEntry",
            MissionError::DuplicateMissionEntry, &[];
        "InvalidMembers" | MissionError::InvalidMembers
            => "InvalidMembers",
            MissionError::InvalidMembers, &[];
        "StaleTeamEligibility" | MissionError::StaleTeamEligibility
            => "StaleTeamEligibility",
            MissionError::StaleTeamEligibility, &[];
        "ResidentIneligible" | MissionError::ResidentIneligible(_)
            => "ResidentIneligible",
            MissionError::ResidentIneligible(first_uuid()), &[];
        "InvalidStore" | MissionError::InvalidStore
            => "InvalidStore",
            MissionError::InvalidStore, &[];
        "Store" | MissionError::Store(_)
            => "Store",
            MissionError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | MissionError::Json(_)
            => "Json",
            MissionError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_discovery / discovery_rows: ResourceDiscoveryError,
        "Account" | ResourceDiscoveryError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            ResourceDiscoveryError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "EndpointNotLoopback" | ResourceDiscoveryError::EndpointNotLoopback
            => "EndpointNotLoopback",
            ResourceDiscoveryError::EndpointNotLoopback, &[];
    }

    closed_table! {
        expected_activation / activation_rows: crate::LocalModelActivationError,
        "Account" | crate::LocalModelActivationError::Account(cause)
            => format!("Account.{}", expected_account(cause).1),
            crate::LocalModelActivationError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "ResourceUnavailable" | crate::LocalModelActivationError::ResourceUnavailable
            => "ResourceUnavailable",
            crate::LocalModelActivationError::ResourceUnavailable, &[];
        "InvalidProviderResponse" | crate::LocalModelActivationError::InvalidProviderResponse
            => "InvalidProviderResponse",
            crate::LocalModelActivationError::InvalidProviderResponse, &[];
        "ModelNotFound" | crate::LocalModelActivationError::ModelNotFound
            => "ModelNotFound",
            crate::LocalModelActivationError::ModelNotFound, &[];
        "ModelDrift" | crate::LocalModelActivationError::ModelDrift
            => "ModelDrift",
            crate::LocalModelActivationError::ModelDrift, &[];
        "InvalidApproval" | crate::LocalModelActivationError::InvalidApproval
            => "InvalidApproval",
            crate::LocalModelActivationError::InvalidApproval, &[];
        "AlreadyActive" | crate::LocalModelActivationError::AlreadyActive
            => "AlreadyActive",
            crate::LocalModelActivationError::AlreadyActive, &[];
        "StaleDecision" | crate::LocalModelActivationError::StaleDecision
            => "StaleDecision",
            crate::LocalModelActivationError::StaleDecision, &[];
        "InvalidStore" | crate::LocalModelActivationError::InvalidStore
            => "InvalidStore",
            crate::LocalModelActivationError::InvalidStore, &[];
        "Store" | crate::LocalModelActivationError::Store(_)
            => "Store",
            crate::LocalModelActivationError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | crate::LocalModelActivationError::Json(_)
            => "Json",
            crate::LocalModelActivationError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_code / code_rows: LocalErrorCode,
        "PasswordTooShort" | LocalErrorCode::PasswordTooShort => "PasswordTooShort", LocalErrorCode::PasswordTooShort, &[];
        "PasswordTooLong" | LocalErrorCode::PasswordTooLong => "PasswordTooLong", LocalErrorCode::PasswordTooLong, &[];
        "PasswordConfirmationMismatch" | LocalErrorCode::PasswordConfirmationMismatch => "PasswordConfirmationMismatch", LocalErrorCode::PasswordConfirmationMismatch, &[];
        "RecoveryNotConfirmed" | LocalErrorCode::RecoveryNotConfirmed => "RecoveryNotConfirmed", LocalErrorCode::RecoveryNotConfirmed, &[];
        "SetupNotStarted" | LocalErrorCode::SetupNotStarted => "SetupNotStarted", LocalErrorCode::SetupNotStarted, &[];
        "AccountAlreadyExists" | LocalErrorCode::AccountAlreadyExists => "AccountAlreadyExists", LocalErrorCode::AccountAlreadyExists, &[];
        "AccountMissing" | LocalErrorCode::AccountMissing => "AccountMissing", LocalErrorCode::AccountMissing, &[];
        "AuthenticationFailed" | LocalErrorCode::AuthenticationFailed => "AuthenticationFailed", LocalErrorCode::AuthenticationFailed, &[];
        "AuthenticationThrottled" | LocalErrorCode::AuthenticationThrottled => "AuthenticationThrottled", LocalErrorCode::AuthenticationThrottled, &[];
        "CredentialDisabled" | LocalErrorCode::CredentialDisabled => "CredentialDisabled", LocalErrorCode::CredentialDisabled, &[];
        "RemotePasswordNotAllowed" | LocalErrorCode::RemotePasswordNotAllowed => "RemotePasswordNotAllowed", LocalErrorCode::RemotePasswordNotAllowed, &[];
        "NotAuthenticated" | LocalErrorCode::NotAuthenticated => "NotAuthenticated", LocalErrorCode::NotAuthenticated, &[];
        "InvalidSessionTimeout" | LocalErrorCode::InvalidSessionTimeout => "InvalidSessionTimeout", LocalErrorCode::InvalidSessionTimeout, &[];
        "UnsupportedStarterProposal" | LocalErrorCode::UnsupportedStarterProposal => "UnsupportedStarterProposal", LocalErrorCode::UnsupportedStarterProposal, &[];
        "InvalidStarterRoster" | LocalErrorCode::InvalidStarterRoster => "InvalidStarterRoster", LocalErrorCode::InvalidStarterRoster, &[];
        "StarterDecisionRecorded" | LocalErrorCode::StarterDecisionRecorded => "StarterDecisionRecorded", LocalErrorCode::StarterDecisionRecorded, &[];
        "StarterOrganizationUnavailable" | LocalErrorCode::StarterOrganizationUnavailable => "StarterOrganizationUnavailable", LocalErrorCode::StarterOrganizationUnavailable, &[];
        "ResidentNotFound" | LocalErrorCode::ResidentNotFound => "ResidentNotFound", LocalErrorCode::ResidentNotFound, &[];
        "InvalidResidentTransition" | LocalErrorCode::InvalidResidentTransition => "InvalidResidentTransition", LocalErrorCode::InvalidResidentTransition, &[];
        "StaleResidentDecision" | LocalErrorCode::StaleResidentDecision => "StaleResidentDecision", LocalErrorCode::StaleResidentDecision, &[];
        "ResidentLifecycleUnavailable" | LocalErrorCode::ResidentLifecycleUnavailable => "ResidentLifecycleUnavailable", LocalErrorCode::ResidentLifecycleUnavailable, &[];
        "InvalidResidentConfiguration" | LocalErrorCode::InvalidResidentConfiguration => "InvalidResidentConfiguration", LocalErrorCode::InvalidResidentConfiguration, &[];
        "StaleResidentConfiguration" | LocalErrorCode::StaleResidentConfiguration => "StaleResidentConfiguration", LocalErrorCode::StaleResidentConfiguration, &[];
        "ResidentConfigurationUnavailable" | LocalErrorCode::ResidentConfigurationUnavailable => "ResidentConfigurationUnavailable", LocalErrorCode::ResidentConfigurationUnavailable, &[];
        "InvalidProject" | LocalErrorCode::InvalidProject => "InvalidProject", LocalErrorCode::InvalidProject, &[];
        "ProjectNotFound" | LocalErrorCode::ProjectNotFound => "ProjectNotFound", LocalErrorCode::ProjectNotFound, &[];
        "StaleCharterRevision" | LocalErrorCode::StaleCharterRevision => "StaleCharterRevision", LocalErrorCode::StaleCharterRevision, &[];
        "UnchangedCharter" | LocalErrorCode::UnchangedCharter => "UnchangedCharter", LocalErrorCode::UnchangedCharter, &[];
        "ProjectUnavailable" | LocalErrorCode::ProjectUnavailable => "ProjectUnavailable", LocalErrorCode::ProjectUnavailable, &[];
        "InvalidProjectPolicy" | LocalErrorCode::InvalidProjectPolicy => "InvalidProjectPolicy", LocalErrorCode::InvalidProjectPolicy, &[];
        "StaleProjectPolicy" | LocalErrorCode::StaleProjectPolicy => "StaleProjectPolicy", LocalErrorCode::StaleProjectPolicy, &[];
        "UnchangedProjectPolicy" | LocalErrorCode::UnchangedProjectPolicy => "UnchangedProjectPolicy", LocalErrorCode::UnchangedProjectPolicy, &[];
        "ProjectPolicyUnavailable" | LocalErrorCode::ProjectPolicyUnavailable => "ProjectPolicyUnavailable", LocalErrorCode::ProjectPolicyUnavailable, &[];
        "InvalidProjectRepository" | LocalErrorCode::InvalidProjectRepository => "InvalidProjectRepository", LocalErrorCode::InvalidProjectRepository, &[];
        "StaleProjectRepository" | LocalErrorCode::StaleProjectRepository => "StaleProjectRepository", LocalErrorCode::StaleProjectRepository, &[];
        "UnchangedProjectRepository" | LocalErrorCode::UnchangedProjectRepository => "UnchangedProjectRepository", LocalErrorCode::UnchangedProjectRepository, &[];
        "ProjectRepositoryUnavailable" | LocalErrorCode::ProjectRepositoryUnavailable => "ProjectRepositoryUnavailable", LocalErrorCode::ProjectRepositoryUnavailable, &[];
        "RepositoryNotBound" | LocalErrorCode::RepositoryNotBound => "RepositoryNotBound", LocalErrorCode::RepositoryNotBound, &[];
        "PreflightUnavailable" | LocalErrorCode::PreflightUnavailable => "PreflightUnavailable", LocalErrorCode::PreflightUnavailable, &[];
        "SampleProjectUnavailable" | LocalErrorCode::SampleProjectUnavailable => "SampleProjectUnavailable", LocalErrorCode::SampleProjectUnavailable, &[];
        "InvalidTeam" | LocalErrorCode::InvalidTeam => "InvalidTeam", LocalErrorCode::InvalidTeam, &[];
        "TeamNotFound" | LocalErrorCode::TeamNotFound => "TeamNotFound", LocalErrorCode::TeamNotFound, &[];
        "StaleTeamMemberConfiguration" | LocalErrorCode::StaleTeamMemberConfiguration => "StaleTeamMemberConfiguration", LocalErrorCode::StaleTeamMemberConfiguration, &[];
        "TeamMemberIneligible" | LocalErrorCode::TeamMemberIneligible => "TeamMemberIneligible", LocalErrorCode::TeamMemberIneligible, &[];
        "TeamUnavailable" | LocalErrorCode::TeamUnavailable => "TeamUnavailable", LocalErrorCode::TeamUnavailable, &[];
        "InvalidMission" | LocalErrorCode::InvalidMission => "InvalidMission", LocalErrorCode::InvalidMission, &[];
        "MissionNotFound" | LocalErrorCode::MissionNotFound => "MissionNotFound", LocalErrorCode::MissionNotFound, &[];
        "StaleMissionTeam" | LocalErrorCode::StaleMissionTeam => "StaleMissionTeam", LocalErrorCode::StaleMissionTeam, &[];
        "MissionMemberIneligible" | LocalErrorCode::MissionMemberIneligible => "MissionMemberIneligible", LocalErrorCode::MissionMemberIneligible, &[];
        "MissionUnavailable" | LocalErrorCode::MissionUnavailable => "MissionUnavailable", LocalErrorCode::MissionUnavailable, &[];
        "MissionCompletionUnavailable" | LocalErrorCode::MissionCompletionUnavailable => "MissionCompletionUnavailable", LocalErrorCode::MissionCompletionUnavailable, &[];
        "CompletionIntentChanged" | LocalErrorCode::CompletionIntentChanged => "CompletionIntentChanged", LocalErrorCode::CompletionIntentChanged, &[];
        "CompletionAlreadyRecorded" | LocalErrorCode::CompletionAlreadyRecorded => "CompletionAlreadyRecorded", LocalErrorCode::CompletionAlreadyRecorded, &[];
        "CompletionResidentNotInAdmissionRoster" | LocalErrorCode::CompletionResidentNotInAdmissionRoster => "CompletionResidentNotInAdmissionRoster", LocalErrorCode::CompletionResidentNotInAdmissionRoster, &[];
        "CompletionConditionCoverageIncomplete" | LocalErrorCode::CompletionConditionCoverageIncomplete => "CompletionConditionCoverageIncomplete", LocalErrorCode::CompletionConditionCoverageIncomplete, &[];
        "CompletionSupportOverLimit" | LocalErrorCode::CompletionSupportOverLimit => "CompletionSupportOverLimit", LocalErrorCode::CompletionSupportOverLimit, &[];
        "CompletionTextOverLimit" | LocalErrorCode::CompletionTextOverLimit => "CompletionTextOverLimit", LocalErrorCode::CompletionTextOverLimit, &[];
        "CompletionSpendReasonRequired" | LocalErrorCode::CompletionSpendReasonRequired => "CompletionSpendReasonRequired", LocalErrorCode::CompletionSpendReasonRequired, &[];
        "CompletionInvalidDetails" | LocalErrorCode::CompletionInvalidDetails => "CompletionInvalidDetails", LocalErrorCode::CompletionInvalidDetails, &[];
        "OutcomeAbsent" | LocalErrorCode::OutcomeAbsent => "OutcomeAbsent", LocalErrorCode::OutcomeAbsent, &[];
        "OutcomeUnavailable" | LocalErrorCode::OutcomeUnavailable => "OutcomeUnavailable", LocalErrorCode::OutcomeUnavailable, &[];
        "LocalModelUnavailable" | LocalErrorCode::LocalModelUnavailable => "LocalModelUnavailable", LocalErrorCode::LocalModelUnavailable, &[];
        "LocalModelNotFound" | LocalErrorCode::LocalModelNotFound => "LocalModelNotFound", LocalErrorCode::LocalModelNotFound, &[];
        "LocalModelChanged" | LocalErrorCode::LocalModelChanged => "LocalModelChanged", LocalErrorCode::LocalModelChanged, &[];
        "InvalidLocalModelApproval" | LocalErrorCode::InvalidLocalModelApproval => "InvalidLocalModelApproval", LocalErrorCode::InvalidLocalModelApproval, &[];
        "LocalModelAlreadyActive" | LocalErrorCode::LocalModelAlreadyActive => "LocalModelAlreadyActive", LocalErrorCode::LocalModelAlreadyActive, &[];
        "StaleLocalModelDecision" | LocalErrorCode::StaleLocalModelDecision => "StaleLocalModelDecision", LocalErrorCode::StaleLocalModelDecision, &[];
        "LocalModelActivationUnavailable" | LocalErrorCode::LocalModelActivationUnavailable => "LocalModelActivationUnavailable", LocalErrorCode::LocalModelActivationUnavailable, &[];
        "AccountServiceUnavailable" | LocalErrorCode::AccountServiceUnavailable => "AccountServiceUnavailable", LocalErrorCode::AccountServiceUnavailable, &[];
        "UnauthorizedClient" | LocalErrorCode::UnauthorizedClient => "UnauthorizedClient", LocalErrorCode::UnauthorizedClient, &[];
        "ProtocolError" | LocalErrorCode::ProtocolError => "ProtocolError", LocalErrorCode::ProtocolError, &[];
    }

    closed_table! {
        expected_transport / transport_rows: LocalTransportError,
        "R42S2.TRANSPORT.Io" | LocalTransportError::Io(_)
            => "local transport I/O failed: Io",
            LocalTransportError::Io(io_error(NEEDLE_A)), &[NEEDLE_A];
        "R42S2.TRANSPORT.Json" | LocalTransportError::Json(_)
            => "local transport message was invalid: Json",
            LocalTransportError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
        "R42S2.TRANSPORT.InvalidTokenFile" | LocalTransportError::InvalidTokenFile
            => "local transport token is invalid",
            LocalTransportError::InvalidTokenFile, &[];
        "R42S2.TRANSPORT.InsecurePermissions" | LocalTransportError::InsecurePermissions
            => "local transport path permissions are not private",
            LocalTransportError::InsecurePermissions, &[];
        "R42S2.TRANSPORT.Protocol" | LocalTransportError::Protocol
            => "local transport protocol mismatch",
            LocalTransportError::Protocol, &[];
        "R42S2.TRANSPORT.Remote" | LocalTransportError::Remote { code, .. }
            => format!("local supervisor refused: {}", expected_code(code).1),
            LocalTransportError::Remote { code: LocalErrorCode::ProtocolError, retry_after_ms: None }, &[];
        "R42S2.TRANSPORT.Supervisor" | LocalTransportError::Supervisor(error)
            => format!("supervisor startup failed: {}", expected_supervisor(error).1),
            LocalTransportError::Supervisor(crate::SupervisorError::SchemaMismatch), &[];
        "R42S2.TRANSPORT.Account" | LocalTransportError::Account(error)
            => format!("account service failed: {}", expected_account(error).1),
            LocalTransportError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "R42S2.TRANSPORT.StarterOrganization" | LocalTransportError::StarterOrganization(error)
            => format!("starter organization failed: {}", expected_starter(error).1),
            LocalTransportError::StarterOrganization(StarterOrganizationError::EmptyRoster), &[];
        "R42S2.TRANSPORT.ResidentLifecycle" | LocalTransportError::ResidentLifecycle(error)
            => format!("Resident lifecycle failed: {}", expected_lifecycle(error).1),
            LocalTransportError::ResidentLifecycle(ResidentLifecycleError::NotFound), &[];
        "R42S2.TRANSPORT.ResidentConfiguration" | LocalTransportError::ResidentConfiguration(error)
            => format!("Resident configuration failed: {}", expected_configuration(error).1),
            LocalTransportError::ResidentConfiguration(ResidentConfigurationError::ResidentNotFound), &[];
        "R42S2.TRANSPORT.Project" | LocalTransportError::Project(error)
            => format!("Project operation failed: {}", expected_project(error).1),
            LocalTransportError::Project(ProjectError::NotFound), &[];
        "R42S2.TRANSPORT.ProjectPolicy" | LocalTransportError::ProjectPolicy(error)
            => format!("Project Policy failed: {}", expected_policy(error).1),
            LocalTransportError::ProjectPolicy(ProjectPolicyError::ProjectNotFound), &[];
        "R42S2.TRANSPORT.ProjectRepository" | LocalTransportError::ProjectRepository(error)
            => format!("Project Repository failed: {}", expected_repository(error).1),
            LocalTransportError::ProjectRepository(ProjectRepositoryError::ProjectNotFound), &[];
        "R42S2.TRANSPORT.CheckPreflight" | LocalTransportError::CheckPreflight(error)
            => format!("check preflight failed: {}", expected_preflight(error).1),
            LocalTransportError::CheckPreflight(CheckPreflightError::ProjectNotFound), &[];
        "R42S2.TRANSPORT.RecentReceipt" | LocalTransportError::RecentReceipt(error)
            => format!("recent Receipts failed: {}", expected_receipt(error).1),
            LocalTransportError::RecentReceipt(crate::RecentReceiptError::InvalidStore), &[];
        "R42S2.TRANSPORT.SampleProject" | LocalTransportError::SampleProject(error)
            => format!("Sample Project failed: {}", expected_sample(error).1),
            LocalTransportError::SampleProject(SampleProjectError::NotInstalled), &[];
        "R42S2.TRANSPORT.Team" | LocalTransportError::Team(error)
            => format!("Team operation failed: {}", expected_team(error).1),
            LocalTransportError::Team(TeamError::TeamNotFound), &[];
        "R42S2.TRANSPORT.MissionCompletionRead" | LocalTransportError::MissionCompletionRead(error)
            => format!("Mission completion read failed: {}", expected_completion_read(error).1),
            LocalTransportError::MissionCompletionRead(crate::MissionCompletionReadError::Unavailable(crate::CompletionUnavailable::OrphanCompletedEvent)), &[];
        "R42S2.TRANSPORT.MissionCompletionCommand" | LocalTransportError::MissionCompletionCommand(error)
            => format!("Mission completion command failed: {}", expected_completion_command(error).1),
            LocalTransportError::MissionCompletionCommand(crate::CompletionCommandError::MalformedRequest(NEEDLE_FIELD)), &[NEEDLE_FIELD];
        "R42S2.TRANSPORT.Mission" | LocalTransportError::Mission(error)
            => format!("Mission operation failed: {}", expected_mission(error).1),
            LocalTransportError::Mission(MissionError::MissionNotFound), &[];
        "R42S2.TRANSPORT.ResourceDiscovery" | LocalTransportError::ResourceDiscovery(error)
            => format!("resource discovery failed: {}", expected_discovery(error).1),
            LocalTransportError::ResourceDiscovery(ResourceDiscoveryError::EndpointNotLoopback), &[];
        "R42S2.TRANSPORT.LocalModelActivation" | LocalTransportError::LocalModelActivation(error)
            => format!("local model activation failed: {}", expected_activation(error).1),
            LocalTransportError::LocalModelActivation(crate::LocalModelActivationError::ModelNotFound), &[];
        "R42S2.TRANSPORT.Outcome" | LocalTransportError::Outcome(error)
            => format!("Outcome failed: {}", expected_publish(error).1),
            LocalTransportError::Outcome(PublishRefusal::Absent), &[];
    }

    // ---- the Outcome sub-tree: slice 1's closed publisher/PDF chain, forwarded whole ----------
    //
    // Independent literals again, not calls into `outcome_publication`'s private witness: this
    // caller has to be able to fail on its own when that chain changes.
    const ROW_WIDTH_EXPECTED: usize = 3;
    const ROW_WIDTH_OBSERVED: usize = 5;
    const CEILING_CONFIGURED: u64 = 1024;
    const CEILING_OBSERVED: u64 = 1025;
    const GAP_EXPECTED: u32 = 2;
    const GAP_FOUND: u32 = 4;
    const HISTORY_EXPECTED: u32 = 7;
    const HISTORY_FOUND: u32 = 3;
    const STATIC_NEEDLE_A: &str = "R42S2_PRIVATE_A column=private_column";

    fn first_uuid() -> Uuid {
        Uuid::parse_str("11111111-1111-4111-8111-111111111111").expect("a fixture UUID parses")
    }

    fn second_uuid() -> Uuid {
        Uuid::parse_str("22222222-2222-4222-8222-222222222222").expect("a fixture UUID parses")
    }

    closed_table! {
        expected_seam / seam_rows: PublicationSeamRefusal,
        "EvaluationAbsent" | PublicationSeamRefusal::EvaluationAbsent
            => "no Evaluation in this Workspace has that digest",
            PublicationSeamRefusal::EvaluationAbsent, &[];
        "CandidateAbsent" | PublicationSeamRefusal::CandidateAbsent
            => "no Artifact in this Workspace has that digest",
            PublicationSeamRefusal::CandidateAbsent, &[];
        "SubjectIsNotAnArtifact" | PublicationSeamRefusal::SubjectIsNotAnArtifact
            => "the Evaluation's subject is not an Artifact",
            PublicationSeamRefusal::SubjectIsNotAnArtifact, &[];
        "SubjectIsNotTheCandidate" | PublicationSeamRefusal::SubjectIsNotTheCandidate
            => "the Evaluation is about different bytes than the candidate",
            PublicationSeamRefusal::SubjectIsNotTheCandidate, &[];
        "VerdictIsNotMeets" | PublicationSeamRefusal::VerdictIsNotMeets
            => "the Evaluation's verdict is not `meets`",
            PublicationSeamRefusal::VerdictIsNotMeets, &[];
        "MissionDiffers" | PublicationSeamRefusal::MissionDiffers
            => "the Evaluation's Mission is not this Outcome's Mission",
            PublicationSeamRefusal::MissionDiffers, &[];
        "CitedEnvelopeDisagrees" | PublicationSeamRefusal::CitedEnvelopeDisagrees
            => "the supplied digest envelope is not the one the owning module verified",
            PublicationSeamRefusal::CitedEnvelopeDisagrees, &[];
        "Unverified" | PublicationSeamRefusal::Unverified
            => "a cited record did not verify",
            PublicationSeamRefusal::Unverified, &[];
    }

    closed_table! {
        expected_malformed_outcome / malformed_outcome_rows: MalformedOutcome,
        "Magic" | MalformedOutcome::Magic
            => "magic",
            MalformedOutcome::Magic, &[];
        "Truncated" | MalformedOutcome::Truncated
            => "truncated",
            MalformedOutcome::Truncated, &[];
        "TrailingBytes" | MalformedOutcome::TrailingBytes
            => "trailing_bytes",
            MalformedOutcome::TrailingBytes, &[];
        "UnknownTag" | MalformedOutcome::UnknownTag
            => "unknown_tag",
            MalformedOutcome::UnknownTag, &[];
        "OverLimitLength" | MalformedOutcome::OverLimitLength
            => "over_limit_length",
            MalformedOutcome::OverLimitLength, &[];
        "InvalidUtf8" | MalformedOutcome::InvalidUtf8
            => "invalid_utf8",
            MalformedOutcome::InvalidUtf8, &[];
        "DigestValue" | MalformedOutcome::DigestValue
            => "digest_value",
            MalformedOutcome::DigestValue, &[];
        "DigestEnvelope" | MalformedOutcome::DigestEnvelope
            => "digest_envelope",
            MalformedOutcome::DigestEnvelope, &[];
        "PinnedRevision" | MalformedOutcome::PinnedRevision
            => "pinned_revision",
            MalformedOutcome::PinnedRevision, &[];
        "EmptyText" | MalformedOutcome::EmptyText
            => "empty_text",
            MalformedOutcome::EmptyText, &[];
        "UntrimmedText" | MalformedOutcome::UntrimmedText
            => "untrimmed_text",
            MalformedOutcome::UntrimmedText, &[];
        "TextTooLong" | MalformedOutcome::TextTooLong
            => "text_too_long",
            MalformedOutcome::TextTooLong, &[];
        "ControlCharacter" | MalformedOutcome::ControlCharacter
            => "control_character",
            MalformedOutcome::ControlCharacter, &[];
        "LineBreakInLabel" | MalformedOutcome::LineBreakInLabel
            => "line_break_in_label",
            MalformedOutcome::LineBreakInLabel, &[];
        "Token" | MalformedOutcome::Token
            => "token",
            MalformedOutcome::Token, &[];
        "UnknownVersion" | MalformedOutcome::UnknownVersion
            => "unknown_version",
            MalformedOutcome::UnknownVersion, &[];
        "ReferenceIdentityRequired" | MalformedOutcome::ReferenceIdentityRequired
            => "reference_identity_required",
            MalformedOutcome::ReferenceIdentityRequired, &[];
        "ReferenceIdentityNotCarried" | MalformedOutcome::ReferenceIdentityNotCarried
            => "reference_identity_not_carried",
            MalformedOutcome::ReferenceIdentityNotCarried, &[];
        "RetiredVersion" | MalformedOutcome::RetiredVersion
            => "retired_version",
            MalformedOutcome::RetiredVersion, &[];
        "NonCanonicalOrder" | MalformedOutcome::NonCanonicalOrder
            => "non_canonical_order",
            MalformedOutcome::NonCanonicalOrder, &[];
        "NoSuccessConditions" | MalformedOutcome::NoSuccessConditions
            => "no_success_conditions",
            MalformedOutcome::NoSuccessConditions, &[];
        "MinorUnitsExponent" | MalformedOutcome::MinorUnitsExponent
            => "minor_units_exponent",
            MalformedOutcome::MinorUnitsExponent, &[];
        "SpendCoverage" | MalformedOutcome::SpendCoverage
            => "spend_coverage",
            MalformedOutcome::SpendCoverage, &[];
        "SourceKind" | MalformedOutcome::SourceKind
            => "source_kind",
            MalformedOutcome::SourceKind, &[];
        "CrossWorkspace" | MalformedOutcome::CrossWorkspace
            => "cross_workspace",
            MalformedOutcome::CrossWorkspace, &[];
    }

    closed_table! {
        expected_typst / typst_rows: TypstRefusal,
        "UnknownRendererVersion" | TypstRefusal::UnknownRendererVersion(_)
            => "UnknownRendererVersion",
            TypstRefusal::UnknownRendererVersion(payload_a()), &[NEEDLE_A];
        "UnclosedFrontMatter" | TypstRefusal::UnclosedFrontMatter
            => "UnclosedFrontMatter",
            TypstRefusal::UnclosedFrontMatter, &[];
        "FrontMatterNotKeyValue" | TypstRefusal::FrontMatterNotKeyValue(_)
            => "FrontMatterNotKeyValue",
            TypstRefusal::FrontMatterNotKeyValue(payload_a()), &[NEEDLE_A];
        "UnclosedCodeFence" | TypstRefusal::UnclosedCodeFence
            => "UnclosedCodeFence",
            TypstRefusal::UnclosedCodeFence, &[];
        "HeadingTooDeep" | TypstRefusal::HeadingTooDeep(_)
            => "HeadingTooDeep",
            TypstRefusal::HeadingTooDeep(payload_a()), &[NEEDLE_A];
        "UnescapedConstruct" | TypstRefusal::UnescapedConstruct { .. }
            => "UnescapedConstruct",
            TypstRefusal::UnescapedConstruct { character: '#', line: payload_a() }, &[NEEDLE_A];
        "TableRowWidth" | TypstRefusal::TableRowWidth { expected, observed }
            => format!("TableRowWidth(expected={expected}, observed={observed})"),
            TypstRefusal::TableRowWidth { expected: ROW_WIDTH_EXPECTED, observed: ROW_WIDTH_OBSERVED }, &[];
    }

    closed_table! {
        expected_pdf / pdf_rows: PdfRefusal,
        "NotUtf8" | PdfRefusal::NotUtf8
            => "the exported document is not UTF-8",
            PdfRefusal::NotUtf8, &[];
        "Translate" | PdfRefusal::Translate(refusal)
            => format!("Translate.{}", expected_typst(refusal).1),
            PdfRefusal::Translate(TypstRefusal::UnknownRendererVersion(payload_a())), &[NEEDLE_A];
        "EmbeddedFontSetEmpty" | PdfRefusal::EmbeddedFontSetEmpty
            => "the render profile embeds no font, and this renderer never looks for one on the \
                host",
            PdfRefusal::EmbeddedFontSetEmpty, &[];
        "Compile" | PdfRefusal::Compile(_)
            => "Compile",
            PdfRefusal::Compile(payload_a()), &[NEEDLE_A];
        "Serialize" | PdfRefusal::Serialize(_)
            => "Serialize",
            PdfRefusal::Serialize(payload_b()), &[NEEDLE_B];
    }

    closed_table! {
        expected_manifest / manifest_rows: ManifestStoreRefusal,
        "Absent" | ManifestStoreRefusal::Absent
            => "Absent",
            ManifestStoreRefusal::Absent, &[];
        "CrossWorkspace" | ManifestStoreRefusal::CrossWorkspace
            => "CrossWorkspace",
            ManifestStoreRefusal::CrossWorkspace, &[];
        "BytesDisagree" | ManifestStoreRefusal::BytesDisagree
            => "BytesDisagree",
            ManifestStoreRefusal::BytesDisagree, &[];
        "ColumnDisagrees" | ManifestStoreRefusal::ColumnDisagrees { .. }
            => "ColumnDisagrees",
            ManifestStoreRefusal::ColumnDisagrees { column: STATIC_NEEDLE_A }, &[NEEDLE_A];
        "ManifestTooLarge" | ManifestStoreRefusal::ManifestTooLarge { configured, observed }
            => format!("ManifestTooLarge(configured={configured}, observed={observed})"),
            ManifestStoreRefusal::ManifestTooLarge { configured: CEILING_CONFIGURED, observed: CEILING_OBSERVED }, &[];
        "Malformed" | ManifestStoreRefusal::Malformed(reason)
            => format!("Malformed.{}", expected_malformed_outcome(reason).1),
            ManifestStoreRefusal::Malformed(MalformedOutcome::Magic), &[];
        "Irreconcilable" | ManifestStoreRefusal::Irreconcilable { .. }
            => "Irreconcilable",
            ManifestStoreRefusal::Irreconcilable { detail: payload_a() }, &[NEEDLE_A];
        "CandidateNotAccounted" | ManifestStoreRefusal::CandidateNotAccounted { .. }
            => "CandidateNotAccounted",
            ManifestStoreRefusal::CandidateNotAccounted { reason: STATIC_NEEDLE_A }, &[NEEDLE_A];
        "Store" | ManifestStoreRefusal::Store(_)
            => "Store",
            ManifestStoreRefusal::Store(payload_a()), &[NEEDLE_A];
    }

    closed_table! {
        expected_publish / publish_rows: PublishRefusal,
        "R42S2.OUTCOME.Seam" | PublishRefusal::Seam(refusal)
            => expected_seam(refusal).1,
            PublishRefusal::Seam(PublicationSeamRefusal::EvaluationAbsent), &[];
        "R42S2.OUTCOME.NoManifestForCandidate" | PublishRefusal::NoManifestForCandidate
            => "no prepared manifest names that candidate Artifact",
            PublishRefusal::NoManifestForCandidate, &[];
        "R42S2.OUTCOME.Manifest" | PublishRefusal::Manifest(refusal)
            => format!("outcome manifest refused: {}", expected_manifest(refusal).1),
            PublishRefusal::Manifest(ManifestStoreRefusal::Store(payload_a())), &[NEEDLE_A];
        "R42S2.OUTCOME.ManifestMissionDiffers" | PublishRefusal::ManifestMissionDiffers { .. }
            => "publication refused: ManifestMissionDiffers",
            PublishRefusal::ManifestMissionDiffers { manifest: first_uuid(), requested: second_uuid() }, &[];
        "R42S2.OUTCOME.Completion" | PublishRefusal::Completion(_)
            => "publication refused: Completion",
            PublishRefusal::Completion(payload_a()), &[NEEDLE_A];
        "R42S2.OUTCOME.AppendixRequired" | PublishRefusal::AppendixRequired
            => "Policy classified this Mission high-impact and the candidate carries no \
                verification appendix",
            PublishRefusal::AppendixRequired, &[];
        "R42S2.OUTCOME.VersionGap" | PublishRefusal::VersionGap { expected, found }
            => format!("the next publication version for this Mission is {expected} and {found} was about to be written"),
            PublishRefusal::VersionGap { expected: GAP_EXPECTED, found: GAP_FOUND }, &[];
        "R42S2.OUTCOME.AlreadyPublished" | PublishRefusal::AlreadyPublished
            => "that candidate or Evaluation is already published",
            PublishRefusal::AlreadyPublished, &[];
        "R42S2.OUTCOME.HistoryNotContiguous" | PublishRefusal::HistoryNotContiguous { expected, found }
            => format!("this Mission's publication history is not contiguous from 1: version {expected} is missing and {found} versions are stored"),
            PublishRefusal::HistoryNotContiguous { expected: HISTORY_EXPECTED, found: HISTORY_FOUND }, &[];
        "R42S2.OUTCOME.QualifyingVerdictNotMeets" | PublishRefusal::QualifyingVerdictNotMeets
            => "the Evaluation this publication cites does not carry a `meets` verdict",
            PublishRefusal::QualifyingVerdictNotMeets, &[];
        "R42S2.OUTCOME.MissionBindingBroken" | PublishRefusal::MissionBindingBroken { .. }
            => "publication refused: MissionBindingBroken",
            PublishRefusal::MissionBindingBroken { what: STATIC_NEEDLE_A, named: first_uuid(), relation: second_uuid() }, &[NEEDLE_A];
        "R42S2.OUTCOME.Pdf" | PublishRefusal::Pdf(refusal)
            => format!("the document did not render: {}", expected_pdf(refusal).1),
            PublishRefusal::Pdf(PdfRefusal::Compile(payload_a())), &[NEEDLE_A];
        "R42S2.OUTCOME.Absent" | PublishRefusal::Absent
            => "this Mission has no published Outcome",
            PublishRefusal::Absent, &[];
        "R42S2.OUTCOME.Account" | PublishRefusal::Account(error)
            => format!("publication refused: Account.{}", expected_account(error).1),
            PublishRefusal::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "R42S2.OUTCOME.Unverified" | PublishRefusal::Unverified(_)
            => "publication refused: Unverified",
            PublishRefusal::Unverified(payload_a()), &[NEEDLE_A];
        "R42S2.OUTCOME.Store" | PublishRefusal::Store(_)
            => "publication refused: Store",
            PublishRefusal::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "R42S2.OUTCOME.CandidateIsNotMissionScoped" | PublishRefusal::CandidateIsNotMissionScoped { .. }
            => "publication refused: CandidateIsNotMissionScoped",
            PublishRefusal::CandidateIsNotMissionScoped { relation: second_uuid() }, &[];
    }

    closed_table! {
        expected_proposal / proposal_rows: ProposalError,
        "Account" | ProposalError::Account(..)
            => "Account",
            ProposalError::Account(OperatorAccountError::NotAuthenticated), &[];
        "Oversize" | ProposalError::Oversize
            => "Oversize",
            ProposalError::Oversize, &[];
        "Malformed" | ProposalError::Malformed(..)
            => "Malformed",
            ProposalError::Malformed(crate::MalformedProposal::Magic), &[];
        "NotCanonical" | ProposalError::NotCanonical
            => "NotCanonical",
            ProposalError::NotCanonical, &[];
        "Inconsistent" | ProposalError::Inconsistent(..)
            => "Inconsistent",
            ProposalError::Inconsistent(crate::InconsistentProposal::WorkspaceScope), &[];
        "NotFound" | ProposalError::NotFound
            => "NotFound",
            ProposalError::NotFound, &[];
        "DigestCollision" | ProposalError::DigestCollision
            => "DigestCollision",
            ProposalError::DigestCollision, &[];
        "InvalidStore" | ProposalError::InvalidStore
            => "InvalidStore",
            ProposalError::InvalidStore, &[];
        "Store" | ProposalError::Store(..)
            => "Store",
            ProposalError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | ProposalError::Json(..)
            => "Json",
            ProposalError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_candidate / candidate_rows: CandidateError,
        "Account" | CandidateError::Account(..)
            => "Account",
            CandidateError::Account(OperatorAccountError::NotAuthenticated), &[];
        "Malformed" | CandidateError::Malformed(..)
            => "Malformed",
            CandidateError::Malformed(crate::MalformedCandidate::Magic), &[];
        "Inconsistent" | CandidateError::Inconsistent(..)
            => "Inconsistent",
            CandidateError::Inconsistent(crate::InconsistentCandidate::ProposalNotFound), &[];
        "NotFound" | CandidateError::NotFound
            => "NotFound",
            CandidateError::NotFound, &[];
        "InvalidStore" | CandidateError::InvalidStore
            => "InvalidStore",
            CandidateError::InvalidStore, &[];
        "Store" | CandidateError::Store(..)
            => "Store",
            CandidateError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | CandidateError::Json(..)
            => "Json",
            CandidateError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_artifact / artifact_rows: ArtifactError,
        "Account" | ArtifactError::Account(..)
            => "Account",
            ArtifactError::Account(OperatorAccountError::NotAuthenticated), &[];
        "Malformed" | ArtifactError::Malformed(..)
            => "Malformed",
            ArtifactError::Malformed(crate::MalformedArtifact::Magic), &[];
        "Inconsistent" | ArtifactError::Inconsistent(..)
            => "Inconsistent",
            ArtifactError::Inconsistent(crate::InconsistentArtifact::WorkspaceScope), &[];
        "NotFound" | ArtifactError::NotFound
            => "NotFound",
            ArtifactError::NotFound, &[];
        "DigestCollision" | ArtifactError::DigestCollision
            => "DigestCollision",
            ArtifactError::DigestCollision, &[];
        "InvalidStore" | ArtifactError::InvalidStore
            => "InvalidStore",
            ArtifactError::InvalidStore, &[];
        "SupersessionTooLong" | ArtifactError::SupersessionTooLong { .. }
            => "SupersessionTooLong",
            ArtifactError::SupersessionTooLong { bound: 7 }, &[];
        "Store" | ArtifactError::Store(..)
            => "Store",
            ArtifactError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | ArtifactError::Json(..)
            => "Json",
            ArtifactError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_evaluation / evaluation_rows: EvaluationError,
        "Account" | EvaluationError::Account(..)
            => "Account",
            EvaluationError::Account(OperatorAccountError::NotAuthenticated), &[];
        "Malformed" | EvaluationError::Malformed(..)
            => "Malformed",
            EvaluationError::Malformed(crate::MalformedEvaluation::Magic), &[];
        "Inconsistent" | EvaluationError::Inconsistent(..)
            => "Inconsistent",
            EvaluationError::Inconsistent(crate::InconsistentEvaluation::ProposalNotFound), &[];
        "NotFound" | EvaluationError::NotFound
            => "NotFound",
            EvaluationError::NotFound, &[];
        "InvalidStore" | EvaluationError::InvalidStore
            => "InvalidStore",
            EvaluationError::InvalidStore, &[];
        "SubjectNotStorable" | EvaluationError::SubjectNotStorable
            => "SubjectNotStorable",
            EvaluationError::SubjectNotStorable, &[];
        "SupersessionTooLong" | EvaluationError::SupersessionTooLong { .. }
            => "SupersessionTooLong",
            EvaluationError::SupersessionTooLong { bound: 7 }, &[];
        "Store" | EvaluationError::Store(..)
            => "Store",
            EvaluationError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_ruling / ruling_rows: RulingError,
        "Account" | RulingError::Account(..)
            => "Account",
            RulingError::Account(OperatorAccountError::NotAuthenticated), &[];
        "Malformed" | RulingError::Malformed(..)
            => "Malformed",
            RulingError::Malformed(crate::MalformedRuling::Magic), &[];
        "Inconsistent" | RulingError::Inconsistent(..)
            => "Inconsistent",
            RulingError::Inconsistent(crate::InconsistentRuling::ProposalNotFound), &[];
        "NotFound" | RulingError::NotFound
            => "NotFound",
            RulingError::NotFound, &[];
        "InvalidStore" | RulingError::InvalidStore
            => "InvalidStore",
            RulingError::InvalidStore, &[];
        "Store" | RulingError::Store(..)
            => "Store",
            RulingError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | RulingError::Json(..)
            => "Json",
            RulingError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
    }

    closed_table! {
        expected_completion / completion_rows: CompletionReadRefusal,
        "Absent" | CompletionReadRefusal::Absent
            => "Absent",
            CompletionReadRefusal::Absent, &[];
        "CrossWorkspace" | CompletionReadRefusal::CrossWorkspace
            => "CrossWorkspace",
            CompletionReadRefusal::CrossWorkspace, &[];
        "BytesDisagree" | CompletionReadRefusal::BytesDisagree
            => "BytesDisagree",
            CompletionReadRefusal::BytesDisagree, &[];
        "ColumnDisagrees" | CompletionReadRefusal::ColumnDisagrees(..)
            => "ColumnDisagrees",
            CompletionReadRefusal::ColumnDisagrees(STATIC_NEEDLE_A), &[];
        "SupersessionAbsent" | CompletionReadRefusal::SupersessionAbsent
            => "SupersessionAbsent",
            CompletionReadRefusal::SupersessionAbsent, &[];
        "SupersessionUnverified" | CompletionReadRefusal::SupersessionUnverified(..)
            => "SupersessionUnverified",
            CompletionReadRefusal::SupersessionUnverified(payload_a()), &[NEEDLE_A];
        "SupersessionOtherMission" | CompletionReadRefusal::SupersessionOtherMission
            => "SupersessionOtherMission",
            CompletionReadRefusal::SupersessionOtherMission, &[];
        "SupersessionCycle" | CompletionReadRefusal::SupersessionCycle
            => "SupersessionCycle",
            CompletionReadRefusal::SupersessionCycle, &[];
        "MissionProjectDisagrees" | CompletionReadRefusal::MissionProjectDisagrees(..)
            => "MissionProjectDisagrees",
            CompletionReadRefusal::MissionProjectDisagrees(STATIC_NEEDLE_A), &[];
        "ConditionProjectionDisagrees" | CompletionReadRefusal::ConditionProjectionDisagrees { .. }
            => "ConditionProjectionDisagrees",
            CompletionReadRefusal::ConditionProjectionDisagrees { ordinal: 3, column: STATIC_NEEDLE_A }, &[NEEDLE_A];
        "ConditionsNotExactlyOnce" | CompletionReadRefusal::ConditionsNotExactlyOnce { .. }
            => "ConditionsNotExactlyOnce",
            CompletionReadRefusal::ConditionsNotExactlyOnce { declared: 3, stored: 5 }, &[];
        "DeclaredConditionsMoved" | CompletionReadRefusal::DeclaredConditionsMoved
            => "DeclaredConditionsMoved",
            CompletionReadRefusal::DeclaredConditionsMoved, &[];
        "MissionNotAdmitted" | CompletionReadRefusal::MissionNotAdmitted
            => "MissionNotAdmitted",
            CompletionReadRefusal::MissionNotAdmitted, &[];
        "MissionAdmissionUnreadable" | CompletionReadRefusal::MissionAdmissionUnreadable { .. }
            => "MissionAdmissionUnreadable",
            CompletionReadRefusal::MissionAdmissionUnreadable { sequence: 11 }, &[];
        "BoundaryOutOfRange" | CompletionReadRefusal::BoundaryOutOfRange
            => "BoundaryOutOfRange",
            CompletionReadRefusal::BoundaryOutOfRange, &[];
        "EventCount" | CompletionReadRefusal::EventCount(..)
            => "EventCount",
            CompletionReadRefusal::EventCount(3), &[];
        "Store" | CompletionReadRefusal::Store(..)
            => "Store",
            CompletionReadRefusal::Store(payload_a()), &[NEEDLE_A];
    }

    closed_table! {
        expected_startup_pass / startup_pass_rows: StartupPass,
        "Project" | StartupPass::Project => "Project", StartupPass::Project, &[];
        "Mission" | StartupPass::Mission => "Mission", StartupPass::Mission, &[];
        "MissionImpactClassification" | StartupPass::MissionImpactClassification => "MissionImpactClassification", StartupPass::MissionImpactClassification, &[];
        "OutcomeManifest" | StartupPass::OutcomeManifest => "OutcomeManifest", StartupPass::OutcomeManifest, &[];
        "OutcomePublication" | StartupPass::OutcomePublication => "OutcomePublication", StartupPass::OutcomePublication, &[];
        "MissionCompletion" | StartupPass::MissionCompletion => "MissionCompletion", StartupPass::MissionCompletion, &[];
        "Proposal" | StartupPass::Proposal => "Proposal", StartupPass::Proposal, &[];
        "Evaluation" | StartupPass::Evaluation => "Evaluation", StartupPass::Evaluation, &[];
        "Artifact" | StartupPass::Artifact => "Artifact", StartupPass::Artifact, &[];
        "Ruling" | StartupPass::Ruling => "Ruling", StartupPass::Ruling, &[];
        "Candidate" | StartupPass::Candidate => "Candidate", StartupPass::Candidate, &[];
        "ProjectRepository" | StartupPass::ProjectRepository => "ProjectRepository", StartupPass::ProjectRepository, &[];
        "SampleProject" | StartupPass::SampleProject => "SampleProject", StartupPass::SampleProject, &[];
        "Team" | StartupPass::Team => "Team", StartupPass::Team, &[];
        "ProjectPolicy" | StartupPass::ProjectPolicy => "ProjectPolicy", StartupPass::ProjectPolicy, &[];
        "StarterOrganization" | StartupPass::StarterOrganization => "StarterOrganization", StartupPass::StarterOrganization, &[];
        "ResidentLifecycle" | StartupPass::ResidentLifecycle => "ResidentLifecycle", StartupPass::ResidentLifecycle, &[];
        "ResidentConfiguration" | StartupPass::ResidentConfiguration => "ResidentConfiguration", StartupPass::ResidentConfiguration, &[];
        "LocalModelActivation" | StartupPass::LocalModelActivation => "LocalModelActivation", StartupPass::LocalModelActivation, &[];
        "Run" | StartupPass::Run => "Run", StartupPass::Run, &[];
    }

    closed_table! {
        expected_malformed_classification / malformed_classification_rows: MalformedClassification,
        "Magic" | MalformedClassification::Magic => "magic", MalformedClassification::Magic, &[];
        "Truncated" | MalformedClassification::Truncated => "truncated", MalformedClassification::Truncated, &[];
        "TrailingBytes" | MalformedClassification::TrailingBytes => "trailing_bytes", MalformedClassification::TrailingBytes, &[];
        "UnknownTag" | MalformedClassification::UnknownTag => "unknown_tag", MalformedClassification::UnknownTag, &[];
        "InvalidUtf8" | MalformedClassification::InvalidUtf8 => "invalid_utf8", MalformedClassification::InvalidUtf8, &[];
        "RecordTooLarge" | MalformedClassification::RecordTooLarge => "record_too_large", MalformedClassification::RecordTooLarge, &[];
        "OverLimitLength" | MalformedClassification::OverLimitLength => "over_limit_length", MalformedClassification::OverLimitLength, &[];
        "RuleIdBasisMismatch" | MalformedClassification::RuleIdBasisMismatch => "rule_id_basis_mismatch", MalformedClassification::RuleIdBasisMismatch, &[];
        "RuleId" | MalformedClassification::RuleId => "rule_id", MalformedClassification::RuleId, &[];
        "EmptyText" | MalformedClassification::EmptyText => "empty_text", MalformedClassification::EmptyText, &[];
        "UntrimmedText" | MalformedClassification::UntrimmedText => "untrimmed_text", MalformedClassification::UntrimmedText, &[];
        "TextTooLong" | MalformedClassification::TextTooLong => "text_too_long", MalformedClassification::TextTooLong, &[];
        "ControlCharacter" | MalformedClassification::ControlCharacter => "control_character", MalformedClassification::ControlCharacter, &[];
        "Revision" | MalformedClassification::Revision => "revision", MalformedClassification::Revision, &[];
        "SupersedesDigest" | MalformedClassification::SupersedesDigest => "supersedes_digest", MalformedClassification::SupersedesDigest, &[];
    }

    closed_table! {
        expected_predecessor_fault / predecessor_fault_rows: PredecessorFault,
        "NotStored" | PredecessorFault::NotStored
            => "not_stored",
            PredecessorFault::NotStored, &[];
        "Unverifiable" | PredecessorFault::Unverifiable(_)
            => "unverifiable",
            PredecessorFault::Unverifiable(payload_a()), &[NEEDLE_A];
        "InAnotherWorkspace" | PredecessorFault::InAnotherWorkspace
            => "in_another_workspace",
            PredecessorFault::InAnotherWorkspace, &[];
        "ForAnotherMission" | PredecessorFault::ForAnotherMission
            => "for_another_mission",
            PredecessorFault::ForAnotherMission, &[];
    }

    closed_table! {
        expected_classification / classification_rows: ClassificationStoreError,
        "DigestMismatch" | ClassificationStoreError::DigestMismatch
            => "digest_mismatch", ClassificationStoreError::DigestMismatch, &[];
        "Malformed" | ClassificationStoreError::Malformed(reason)
            => format!("malformed.{}", expected_malformed_classification(reason).1),
            ClassificationStoreError::Malformed(MalformedClassification::Magic), &[];
        "ColumnsDisagree" | ClassificationStoreError::ColumnsDisagree
            => "columns_disagree", ClassificationStoreError::ColumnsDisagree, &[];
        "CitedIdentityMismatch" | ClassificationStoreError::CitedIdentityMismatch
            => "cited_identity_mismatch", ClassificationStoreError::CitedIdentityMismatch, &[];
        "CrossWorkspace" | ClassificationStoreError::CrossWorkspace
            => "cross_workspace", ClassificationStoreError::CrossWorkspace, &[];
        "CopiedBitDisagrees" | ClassificationStoreError::CopiedBitDisagrees
            => "copied_bit_disagrees", ClassificationStoreError::CopiedBitDisagrees, &[];
        "PolicyProjectIsNotTheMissions" | ClassificationStoreError::PolicyProjectIsNotTheMissions
            => "policy_project_is_not_the_missions",
            ClassificationStoreError::PolicyProjectIsNotTheMissions, &[];
        "PolicyRevisionHasAnotherCharter" | ClassificationStoreError::PolicyRevisionHasAnotherCharter
            => "policy_revision_has_another_charter",
            ClassificationStoreError::PolicyRevisionHasAnotherCharter, &[];
        "Predecessor" | ClassificationStoreError::Predecessor { fault, .. }
            => format!("predecessor.{}", expected_predecessor_fault(fault).1),
            ClassificationStoreError::Predecessor {
                digest: digest_b(),
                fault: PredecessorFault::NotStored,
            }, &[];
        "Store" | ClassificationStoreError::Store(_)
            => "store", ClassificationStoreError::Store(payload_a()), &[NEEDLE_A];
    }

    closed_table! {
        expected_manifest_plain / manifest_plain_rows: ManifestStoreRefusal,
        "Absent" | ManifestStoreRefusal::Absent => "Absent", ManifestStoreRefusal::Absent, &[];
        "CrossWorkspace" | ManifestStoreRefusal::CrossWorkspace
            => "CrossWorkspace", ManifestStoreRefusal::CrossWorkspace, &[];
        "BytesDisagree" | ManifestStoreRefusal::BytesDisagree
            => "BytesDisagree", ManifestStoreRefusal::BytesDisagree, &[];
        "ColumnDisagrees" | ManifestStoreRefusal::ColumnDisagrees { .. }
            => "ColumnDisagrees",
            ManifestStoreRefusal::ColumnDisagrees { column: STATIC_NEEDLE_A }, &[NEEDLE_A];
        "ManifestTooLarge" | ManifestStoreRefusal::ManifestTooLarge { .. }
            => "ManifestTooLarge",
            ManifestStoreRefusal::ManifestTooLarge { configured: CEILING_CONFIGURED, observed: CEILING_OBSERVED }, &[];
        "Malformed" | ManifestStoreRefusal::Malformed(_)
            => "Malformed", ManifestStoreRefusal::Malformed(MalformedOutcome::Magic), &[];
        "Irreconcilable" | ManifestStoreRefusal::Irreconcilable { .. }
            => "Irreconcilable",
            ManifestStoreRefusal::Irreconcilable { detail: payload_a() }, &[NEEDLE_A];
        "CandidateNotAccounted" | ManifestStoreRefusal::CandidateNotAccounted { .. }
            => "CandidateNotAccounted",
            ManifestStoreRefusal::CandidateNotAccounted { reason: STATIC_NEEDLE_A }, &[NEEDLE_A];
        "Store" | ManifestStoreRefusal::Store(_)
            => "Store", ManifestStoreRefusal::Store(payload_a()), &[NEEDLE_A];
    }

    closed_table! {
        expected_publish_plain / publish_plain_rows: PublishRefusal,
        "Seam" | PublishRefusal::Seam(_)
            => "Seam", PublishRefusal::Seam(PublicationSeamRefusal::EvaluationAbsent), &[];
        "NoManifestForCandidate" | PublishRefusal::NoManifestForCandidate
            => "NoManifestForCandidate", PublishRefusal::NoManifestForCandidate, &[];
        "Manifest" | PublishRefusal::Manifest(_)
            => "Manifest", PublishRefusal::Manifest(ManifestStoreRefusal::Absent), &[];
        "ManifestMissionDiffers" | PublishRefusal::ManifestMissionDiffers { .. }
            => "ManifestMissionDiffers",
            PublishRefusal::ManifestMissionDiffers { manifest: first_uuid(), requested: second_uuid() }, &[];
        "Completion" | PublishRefusal::Completion(_)
            => "Completion", PublishRefusal::Completion(payload_a()), &[NEEDLE_A];
        "AppendixRequired" | PublishRefusal::AppendixRequired
            => "AppendixRequired", PublishRefusal::AppendixRequired, &[];
        "VersionGap" | PublishRefusal::VersionGap { .. }
            => "VersionGap", PublishRefusal::VersionGap { expected: GAP_EXPECTED, found: GAP_FOUND }, &[];
        "AlreadyPublished" | PublishRefusal::AlreadyPublished
            => "AlreadyPublished", PublishRefusal::AlreadyPublished, &[];
        "HistoryNotContiguous" | PublishRefusal::HistoryNotContiguous { .. }
            => "HistoryNotContiguous",
            PublishRefusal::HistoryNotContiguous { expected: HISTORY_EXPECTED, found: HISTORY_FOUND }, &[];
        "QualifyingVerdictNotMeets" | PublishRefusal::QualifyingVerdictNotMeets
            => "QualifyingVerdictNotMeets", PublishRefusal::QualifyingVerdictNotMeets, &[];
        "MissionBindingBroken" | PublishRefusal::MissionBindingBroken { .. }
            => "MissionBindingBroken",
            PublishRefusal::MissionBindingBroken { what: STATIC_NEEDLE_A, named: first_uuid(), relation: second_uuid() }, &[NEEDLE_A];
        "CandidateIsNotMissionScoped" | PublishRefusal::CandidateIsNotMissionScoped { .. }
            => "CandidateIsNotMissionScoped",
            PublishRefusal::CandidateIsNotMissionScoped { relation: second_uuid() }, &[];
        "Pdf" | PublishRefusal::Pdf(_)
            => "Pdf", PublishRefusal::Pdf(PdfRefusal::Compile(payload_a())), &[NEEDLE_A];
        "Absent" | PublishRefusal::Absent => "Absent", PublishRefusal::Absent, &[];
        "Account" | PublishRefusal::Account(_)
            => "Account", PublishRefusal::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))), &[NEEDLE_A];
        "Unverified" | PublishRefusal::Unverified(_)
            => "Unverified", PublishRefusal::Unverified(payload_a()), &[NEEDLE_A];
        "Store" | PublishRefusal::Store(_)
            => "Store", PublishRefusal::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
    }

    // ---- the 72 Supervisor roots, as a transport caller sees them ---------------------------
    //
    // Every fixture carries arbitrary-but-valid keys, because the whole point of the caller tag is
    // that none of them appears. Each parent that carries a child is exercised with ONE
    // representative child here; the full child-vocabulary walk is `r42_supervisor_projection_is_closed`
    // in lib.rs, where startup's own line is checked too.
    const SEQUENCE: u64 = 4_242;
    const STORED_VERSION: i64 = 4_294_967_296;
    const KNOWN_TOTALS_LABEL: &str = "project_creation_snapshots";

    fn digest_a() -> String {
        "a".repeat(64)
    }

    fn digest_b() -> String {
        "b".repeat(64)
    }

    closed_table! {
        expected_supervisor / supervisor_rows: crate::SupervisorError,
        "EmptyWorkspaceName" | SupervisorError::EmptyWorkspaceName
            => "EmptyWorkspaceName",
            SupervisorError::EmptyWorkspaceName, &[];
        "WorkspaceNameTooLong" | SupervisorError::WorkspaceNameTooLong
            => "WorkspaceNameTooLong",
            SupervisorError::WorkspaceNameTooLong, &[];
        "WorkspaceNameContainsControl" | SupervisorError::WorkspaceNameContainsControl
            => "WorkspaceNameContainsControl",
            SupervisorError::WorkspaceNameContainsControl, &[];
        "ClockBeforeUnixEpoch" | SupervisorError::ClockBeforeUnixEpoch
            => "ClockBeforeUnixEpoch",
            SupervisorError::ClockBeforeUnixEpoch, &[];
        "Io" | SupervisorError::Io(..)
            => "Io",
            SupervisorError::Io(io_error(NEEDLE_A)), &[NEEDLE_A];
        "Store" | SupervisorError::Store(..)
            => "Store",
            SupervisorError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "Json" | SupervisorError::Json(..)
            => "Json",
            SupervisorError::Json(json_error(NEEDLE_A)), &[NEEDLE_A];
        "InvalidWorkspaceId" | SupervisorError::InvalidWorkspaceId(..)
            => "InvalidWorkspaceId",
            SupervisorError::InvalidWorkspaceId(digest_a()), &[];
        "InvalidEventKind" | SupervisorError::InvalidEventKind(..)
            => "InvalidEventKind",
            SupervisorError::InvalidEventKind(digest_a()), &[];
        "EventPayloadScopeMismatch" | SupervisorError::EventPayloadScopeMismatch(..)
            => "EventPayloadScopeMismatch",
            SupervisorError::EventPayloadScopeMismatch(STORED_VERSION), &[];
        "EventChainBroken" | SupervisorError::EventChainBroken(..)
            => "EventChainBroken",
            SupervisorError::EventChainBroken(
                crate::event_chain::ChainFault::LinkDigestMismatch { sequence: STORED_VERSION },
            ), &[];
        "PauseStateUnsupported" | SupervisorError::PauseStateUnsupported { .. }
            => "PauseStateUnsupported",
            SupervisorError::PauseStateUnsupported {
                changed_at_ms: STORED_VERSION,
                terminal_sequence: None,
            }, &[];
        "StarterOrganizationEventMismatch" | SupervisorError::StarterOrganizationEventMismatch(..)
            => "StarterOrganizationEventMismatch",
            SupervisorError::StarterOrganizationEventMismatch(STORED_VERSION), &[];
        "StarterOrganizationStateMismatch" | SupervisorError::StarterOrganizationStateMismatch { cause, .. }
            => format!("StarterOrganizationStateMismatch.{}", expected_starter(cause).0),
            SupervisorError::StarterOrganizationStateMismatch { workspace_id: first_uuid(), cause: StarterOrganizationError::EmptyRoster }, &[];
        "ProjectEventMismatch" | SupervisorError::ProjectEventMismatch(..)
            => "ProjectEventMismatch",
            SupervisorError::ProjectEventMismatch(STORED_VERSION), &[];
        "ProjectStateMismatch" | SupervisorError::ProjectStateMismatch { cause, .. }
            => format!("ProjectStateMismatch.{}", expected_project(cause).0),
            SupervisorError::ProjectStateMismatch { workspace_id: first_uuid(), project_id: first_uuid(), cause: ProjectError::NotFound }, &[];
        "ProjectTotalsMismatch" | SupervisorError::ProjectTotalsMismatch(..)
            => "ProjectTotalsMismatch",
            SupervisorError::ProjectTotalsMismatch(TotalsMismatch { table: KNOWN_TOTALS_LABEL, expected: 7, observed: 11 }), &[];
        "LocalModelActivationEventMismatch" | SupervisorError::LocalModelActivationEventMismatch(..)
            => "LocalModelActivationEventMismatch",
            SupervisorError::LocalModelActivationEventMismatch(STORED_VERSION), &[];
        "LocalModelActivationStateMismatch" | SupervisorError::LocalModelActivationStateMismatch { cause, .. }
            => format!("LocalModelActivationStateMismatch.{}", expected_activation(cause).0),
            SupervisorError::LocalModelActivationStateMismatch { workspace_id: first_uuid(), activation_id: first_uuid(), cause: LocalModelActivationError::ModelNotFound }, &[];
        "LocalModelActivationRowUnreadable" | SupervisorError::LocalModelActivationRowUnreadable(cause)
            => format!("LocalModelActivationRowUnreadable.{}", expected_activation(cause).0),
            SupervisorError::LocalModelActivationRowUnreadable(LocalModelActivationError::ModelNotFound), &[];
        "ResidentLifecycleEventMismatch" | SupervisorError::ResidentLifecycleEventMismatch(..)
            => "ResidentLifecycleEventMismatch",
            SupervisorError::ResidentLifecycleEventMismatch(STORED_VERSION), &[];
        "ResidentLifecycleStateMismatch" | SupervisorError::ResidentLifecycleStateMismatch { cause, .. }
            => format!("ResidentLifecycleStateMismatch.{}", expected_lifecycle(cause).0),
            SupervisorError::ResidentLifecycleStateMismatch { workspace_id: first_uuid(), resident_id: first_uuid(), cause: ResidentLifecycleError::NotFound }, &[];
        "ResidentLifecycleTotalsMismatch" | SupervisorError::ResidentLifecycleTotalsMismatch(..)
            => "ResidentLifecycleTotalsMismatch",
            SupervisorError::ResidentLifecycleTotalsMismatch(TotalsMismatch { table: KNOWN_TOTALS_LABEL, expected: 7, observed: 11 }), &[];
        "ResidentRowUnreadable" | SupervisorError::ResidentRowUnreadable(cause)
            => format!("ResidentRowUnreadable.{}", expected_lifecycle(cause).0),
            SupervisorError::ResidentRowUnreadable(ResidentLifecycleError::NotFound), &[];
        "ResidentConfigurationEventMismatch" | SupervisorError::ResidentConfigurationEventMismatch(..)
            => "ResidentConfigurationEventMismatch",
            SupervisorError::ResidentConfigurationEventMismatch(STORED_VERSION), &[];
        "ResidentConfigurationStateMismatch" | SupervisorError::ResidentConfigurationStateMismatch { cause, .. }
            => format!("ResidentConfigurationStateMismatch.{}", expected_configuration(cause).0),
            SupervisorError::ResidentConfigurationStateMismatch { workspace_id: first_uuid(), decision_id: digest_a(), cause: ResidentConfigurationError::ResidentNotFound }, &[];
        "TeamEventMismatch" | SupervisorError::TeamEventMismatch(..)
            => "TeamEventMismatch",
            SupervisorError::TeamEventMismatch(STORED_VERSION), &[];
        "TeamStateMismatch" | SupervisorError::TeamStateMismatch { cause, .. }
            => format!("TeamStateMismatch.{}", expected_team(cause).0),
            SupervisorError::TeamStateMismatch { workspace_id: first_uuid(), team_id: first_uuid(), cause: TeamError::TeamNotFound }, &[];
        "TeamTotalsMismatch" | SupervisorError::TeamTotalsMismatch(..)
            => "TeamTotalsMismatch",
            SupervisorError::TeamTotalsMismatch(TotalsMismatch { table: KNOWN_TOTALS_LABEL, expected: 7, observed: 11 }), &[];
        "MissionEventMismatch" | SupervisorError::MissionEventMismatch(..)
            => "MissionEventMismatch",
            SupervisorError::MissionEventMismatch(STORED_VERSION), &[];
        "MissionStateMismatch" | SupervisorError::MissionStateMismatch { cause, .. }
            => format!("MissionStateMismatch.{}", expected_mission(cause).0),
            SupervisorError::MissionStateMismatch { workspace_id: first_uuid(), mission_id: first_uuid(), cause: MissionError::MissionNotFound }, &[];
        "MissionTotalsMismatch" | SupervisorError::MissionTotalsMismatch(..)
            => "MissionTotalsMismatch",
            SupervisorError::MissionTotalsMismatch(TotalsMismatch { table: KNOWN_TOTALS_LABEL, expected: 7, observed: 11 }), &[];
        "ProjectPolicyEventMismatch" | SupervisorError::ProjectPolicyEventMismatch(..)
            => "ProjectPolicyEventMismatch",
            SupervisorError::ProjectPolicyEventMismatch(STORED_VERSION), &[];
        "ProjectPolicyStateMismatch" | SupervisorError::ProjectPolicyStateMismatch { cause, .. }
            => format!("ProjectPolicyStateMismatch.{}", expected_policy(cause).0),
            SupervisorError::ProjectPolicyStateMismatch { workspace_id: first_uuid(), project_id: first_uuid(), cause: ProjectPolicyError::ProjectNotFound }, &[];
        "ProjectRepositoryEventMismatch" | SupervisorError::ProjectRepositoryEventMismatch(..)
            => "ProjectRepositoryEventMismatch",
            SupervisorError::ProjectRepositoryEventMismatch(STORED_VERSION), &[];
        "ProjectRepositoryStateMismatch" | SupervisorError::ProjectRepositoryStateMismatch { cause, .. }
            => format!("ProjectRepositoryStateMismatch.{}", expected_repository(cause).0),
            SupervisorError::ProjectRepositoryStateMismatch { workspace_id: first_uuid(), project_id: first_uuid(), cause: ProjectRepositoryError::ProjectNotFound }, &[];
        "SampleProjectEventMismatch" | SupervisorError::SampleProjectEventMismatch(..)
            => "SampleProjectEventMismatch",
            SupervisorError::SampleProjectEventMismatch(STORED_VERSION), &[];
        "SampleProjectStateMismatch" | SupervisorError::SampleProjectStateMismatch { cause, .. }
            => format!("SampleProjectStateMismatch.{}", expected_sample(cause).0),
            SupervisorError::SampleProjectStateMismatch { workspace_id: first_uuid(), cause: SampleProjectError::NotInstalled }, &[];
        "SampleProjectTotalsMismatch" | SupervisorError::SampleProjectTotalsMismatch(..)
            => "SampleProjectTotalsMismatch",
            SupervisorError::SampleProjectTotalsMismatch(TotalsMismatch { table: KNOWN_TOTALS_LABEL, expected: 7, observed: 11 }), &[];
        "RunEventMismatch" | SupervisorError::RunEventMismatch(..)
            => "RunEventMismatch",
            SupervisorError::RunEventMismatch(STORED_VERSION), &[];
        "RunStateMismatch" | SupervisorError::RunStateMismatch { cause, .. }
            => format!("RunStateMismatch.{}", expected_preflight(cause).0),
            SupervisorError::RunStateMismatch { workspace_id: first_uuid(), run_id: digest_a(), cause: CheckPreflightError::ProjectNotFound }, &[];
        "RunClockBeforeEpoch" | SupervisorError::RunClockBeforeEpoch
            => "RunClockBeforeEpoch",
            SupervisorError::RunClockBeforeEpoch, &[];
        "RunEventStateMismatch" | SupervisorError::RunEventStateMismatch { .. }
            => "RunEventStateMismatch",
            SupervisorError::RunEventStateMismatch { sequence: STORED_VERSION, workspace_id: first_uuid(), run_id: first_uuid() }, &[];
        "ProposalEventMismatch" | SupervisorError::ProposalEventMismatch(..)
            => "ProposalEventMismatch",
            SupervisorError::ProposalEventMismatch(STORED_VERSION), &[];
        "ProposalStateMismatch" | SupervisorError::ProposalStateMismatch { cause, .. }
            => format!("ProposalStateMismatch.{}", expected_proposal(cause).1),
            SupervisorError::ProposalStateMismatch { workspace_id: first_uuid(), digest_value: digest_a(), cause: ProposalError::NotFound }, &[];
        "ProposalTotalsMismatch" | SupervisorError::ProposalTotalsMismatch(..)
            => "ProposalTotalsMismatch",
            SupervisorError::ProposalTotalsMismatch(TotalsMismatch { table: KNOWN_TOTALS_LABEL, expected: 7, observed: 11 }), &[];
        "RulingEventMismatch" | SupervisorError::RulingEventMismatch(..)
            => "RulingEventMismatch",
            SupervisorError::RulingEventMismatch(STORED_VERSION), &[];
        "EvaluationEventMismatch" | SupervisorError::EvaluationEventMismatch(..)
            => "EvaluationEventMismatch",
            SupervisorError::EvaluationEventMismatch(STORED_VERSION), &[];
        "MissionCompletionEventMismatch" | SupervisorError::MissionCompletionEventMismatch(..)
            => "MissionCompletionEventMismatch",
            SupervisorError::MissionCompletionEventMismatch(STORED_VERSION), &[];
        "MissionCompletionStateMismatch" | SupervisorError::MissionCompletionStateMismatch { cause, .. }
            => format!("MissionCompletionStateMismatch.{}", expected_completion(cause).1),
            SupervisorError::MissionCompletionStateMismatch { workspace_id: first_uuid(), digest_value: digest_a(), cause: CompletionReadRefusal::Absent }, &[];
        "MissionCompletionTotalsMismatch" | SupervisorError::MissionCompletionTotalsMismatch(..)
            => "MissionCompletionTotalsMismatch",
            SupervisorError::MissionCompletionTotalsMismatch(TotalsMismatch { table: KNOWN_TOTALS_LABEL, expected: 7, observed: 11 }), &[];
        "EvaluationStateMismatch" | SupervisorError::EvaluationStateMismatch { cause, .. }
            => format!("EvaluationStateMismatch.{}", expected_evaluation(cause).1),
            SupervisorError::EvaluationStateMismatch { workspace_id: first_uuid(), digest_value: digest_a(), cause: EvaluationError::NotFound }, &[];
        "EvaluationChainsMismatch" | SupervisorError::EvaluationChainsMismatch(cause)
            => format!("EvaluationChainsMismatch.{}", expected_evaluation(cause).1),
            SupervisorError::EvaluationChainsMismatch(EvaluationError::NotFound), &[];
        "ArtifactEventMismatch" | SupervisorError::ArtifactEventMismatch(..)
            => "ArtifactEventMismatch",
            SupervisorError::ArtifactEventMismatch(STORED_VERSION), &[];
        "ArtifactStateMismatch" | SupervisorError::ArtifactStateMismatch { cause, .. }
            => format!("ArtifactStateMismatch.{}", expected_artifact(cause).1),
            SupervisorError::ArtifactStateMismatch { workspace_id: first_uuid(), digest_value: digest_a(), cause: ArtifactError::NotFound }, &[];
        "ArtifactTotalsMismatch" | SupervisorError::ArtifactTotalsMismatch(..)
            => "ArtifactTotalsMismatch",
            SupervisorError::ArtifactTotalsMismatch(TotalsMismatch { table: KNOWN_TOTALS_LABEL, expected: 7, observed: 11 }), &[];
        "ArtifactRelationMismatch" | SupervisorError::ArtifactRelationMismatch(cause)
            => format!("ArtifactRelationMismatch.{}", expected_artifact(cause).1),
            SupervisorError::ArtifactRelationMismatch(ArtifactError::NotFound), &[];
        "MissionImpactClassificationStateMismatch" | SupervisorError::MissionImpactClassificationStateMismatch { cause, .. }
            => format!("MissionImpactClassificationStateMismatch.{}", expected_classification(cause).1),
            SupervisorError::MissionImpactClassificationStateMismatch { workspace_id: first_uuid(), digest: digest_a(), cause: ClassificationStoreError::ColumnsDisagree }, &[];
        "OutcomeManifestStateMismatch" | SupervisorError::OutcomeManifestStateMismatch { cause, .. }
            => format!("OutcomeManifestStateMismatch.{}", expected_manifest_plain(cause).1),
            SupervisorError::OutcomeManifestStateMismatch { workspace_id: first_uuid(), manifest_digest: digest_a(), cause: ManifestStoreRefusal::Absent }, &[];
        "OutcomePublicationStateMismatch" | SupervisorError::OutcomePublicationStateMismatch { cause, .. }
            => format!("OutcomePublicationStateMismatch.{}", expected_publish_plain(cause).1),
            SupervisorError::OutcomePublicationStateMismatch { workspace_id: first_uuid(), mission_id: first_uuid(), version: STORED_VERSION, cause: PublishRefusal::Absent }, &[];
        "RulingStateMismatch" | SupervisorError::RulingStateMismatch { cause, .. }
            => format!("RulingStateMismatch.{}", expected_ruling(cause).1),
            SupervisorError::RulingStateMismatch { workspace_id: first_uuid(), digest_value: digest_a(), cause: RulingError::NotFound }, &[];
        "RulingTotalsMismatch" | SupervisorError::RulingTotalsMismatch(..)
            => "RulingTotalsMismatch",
            SupervisorError::RulingTotalsMismatch(TotalsMismatch { table: KNOWN_TOTALS_LABEL, expected: 7, observed: 11 }), &[];
        "RulingChainsMismatch" | SupervisorError::RulingChainsMismatch(cause)
            => format!("RulingChainsMismatch.{}", expected_ruling(cause).1),
            SupervisorError::RulingChainsMismatch(RulingError::NotFound), &[];
        "CandidateEventMismatch" | SupervisorError::CandidateEventMismatch(..)
            => "CandidateEventMismatch",
            SupervisorError::CandidateEventMismatch(STORED_VERSION), &[];
        "CandidateStateMismatch" | SupervisorError::CandidateStateMismatch { cause, .. }
            => format!("CandidateStateMismatch.{}", expected_candidate(cause).1),
            SupervisorError::CandidateStateMismatch { workspace_id: first_uuid(), digest_value: digest_a(), cause: CandidateError::NotFound }, &[];
        "DispatchEventMismatch" | SupervisorError::DispatchEventMismatch(..)
            => "DispatchEventMismatch",
            SupervisorError::DispatchEventMismatch(STORED_VERSION), &[];
        "DispatchRecoveryFailed" | SupervisorError::DispatchRecoveryFailed
            => "DispatchRecoveryFailed",
            SupervisorError::DispatchRecoveryFailed, &[];
        "PauseEventMismatch" | SupervisorError::PauseEventMismatch(..)
            => "PauseEventMismatch",
            SupervisorError::PauseEventMismatch(STORED_VERSION), &[];
        "SchemaMismatch" | SupervisorError::SchemaMismatch
            => "SchemaMismatch",
            SupervisorError::SchemaMismatch, &[];
        "UnsupportedSchemaVersion" | SupervisorError::UnsupportedSchemaVersion(..)
            => "UnsupportedSchemaVersion",
            SupervisorError::UnsupportedSchemaVersion(STORED_VERSION), &[];
        "IntegrityCheckFailed" | SupervisorError::IntegrityCheckFailed(..)
            => "IntegrityCheckFailed",
            SupervisorError::IntegrityCheckFailed(digest_a()), &[];
        "StoredIdentityUnreadable" | SupervisorError::StoredIdentityUnreadable { .. }
            => "StoredIdentityUnreadable",
            SupervisorError::StoredIdentityUnreadable { what: STATIC_NEEDLE_A, raw: digest_a() }, &[NEEDLE_A];
        // R45's 73rd arm. Its transport tag drops every field, which is why a marker-bearing
        // `raw` is safe HERE -- and why its own startup line, which prints that raw, is not.
        "StoredIntegerKeyUnreadable" | SupervisorError::StoredIntegerKeyUnreadable { .. }
            => "StoredIntegerKeyUnreadable",
            SupervisorError::StoredIntegerKeyUnreadable {
                what: STATIC_NEEDLE_A,
                raw: payload_a(),
                key_name: "boundary_event_seq",
                key: 11,
            },
            &[NEEDLE_A];
        "StartupEnumerationFailed" | SupervisorError::StartupEnumerationFailed { pass, .. }
            => format!("StartupEnumerationFailed.{}", expected_startup_pass(pass).1),
            SupervisorError::StartupEnumerationFailed { pass: StartupPass::Project, source: driver_error(NEEDLE_A) }, &[NEEDLE_A];
        "ForeignKeyViolations" | SupervisorError::ForeignKeyViolations(..)
            => "ForeignKeyViolations",
            SupervisorError::ForeignKeyViolations(SEQUENCE), &[];
    }

    struct Case {
        id: String,
        value: LocalTransportError,
        markers: &'static [&'static str],
    }

    /// Expand one domain vocabulary into cases, recursing where the selector recurses.
    ///
    /// Three shapes because the special arms are not on every enum: a `match` on the row's case ID
    /// still has to typecheck for the enum it is written against.
    macro_rules! domain_cases {
        ($name:ident, $wrap:path, $rows:ident, $stem:literal, $account:path) => {
            fn $name(cases: &mut Vec<Case>) {
                for (case, value, markers) in $rows() {
                    if case == "Account" {
                        for (child, account, child_markers) in account_rows() {
                            cases.push(Case {
                                id: format!("{}.Account.{}", $stem, child),
                                value: $wrap($account(account)),
                                markers: child_markers,
                            });
                        }
                    } else {
                        cases.push(Case {
                            id: format!("{}.{}", $stem, case),
                            value: $wrap(value),
                            markers,
                        });
                    }
                }
            }
        };
        ($name:ident, $wrap:path, $rows:ident, $stem:literal, $account:path, $stale:expr) => {
            fn $name(cases: &mut Vec<Case>) {
                for (case, value, markers) in $rows() {
                    match case {
                        "Account" => {
                            for (child, account, child_markers) in account_rows() {
                                cases.push(Case {
                                    id: format!("{}.Account.{}", $stem, child),
                                    value: $wrap($account(account)),
                                    markers: child_markers,
                                });
                            }
                        }
                        // Both approved numeric pairs, so a selector printing a constant -- or
                        // swapping the two -- cannot pass.
                        "StaleCharterRevision" => {
                            cases.push(Case {
                                id: format!("{}.StaleCharterRevision", $stem),
                                value: $wrap(($stale)(FIRST_EXPECTED, FIRST_CURRENT)),
                                markers: &[],
                            });
                            cases.push(Case {
                                id: format!("{}.StaleCharterRevision.SECOND", $stem),
                                value: $wrap(($stale)(SECOND_EXPECTED, SECOND_CURRENT)),
                                markers: &[],
                            });
                        }
                        other => cases.push(Case {
                            id: format!("{}.{}", $stem, other),
                            value: $wrap(value),
                            markers,
                        }),
                    }
                }
            }
        };
        ($name:ident, $wrap:path, $rows:ident, $stem:literal, $account:path, $stale:expr, $draft:path) => {
            fn $name(cases: &mut Vec<Case>) {
                for (case, value, markers) in $rows() {
                    match case {
                        "Account" => {
                            for (child, account, child_markers) in account_rows() {
                                cases.push(Case {
                                    id: format!("{}.Account.{}", $stem, child),
                                    value: $wrap($account(account)),
                                    markers: child_markers,
                                });
                            }
                        }
                        "InvalidDraft" => {
                            for (child, reason, _) in draft_rows() {
                                cases.push(Case {
                                    id: format!("{}.InvalidDraft.{}", $stem, child),
                                    value: $wrap($draft(reason)),
                                    markers: &[],
                                });
                            }
                        }
                        "StaleCharterRevision" => {
                            cases.push(Case {
                                id: format!("{}.StaleCharterRevision", $stem),
                                value: $wrap(($stale)(FIRST_EXPECTED, FIRST_CURRENT)),
                                markers: &[],
                            });
                            cases.push(Case {
                                id: format!("{}.StaleCharterRevision.SECOND", $stem),
                                value: $wrap(($stale)(SECOND_EXPECTED, SECOND_CURRENT)),
                                markers: &[],
                            });
                        }
                        other => cases.push(Case {
                            id: format!("{}.{}", $stem, other),
                            value: $wrap(value),
                            markers,
                        }),
                    }
                }
            }
        };
    }

    domain_cases!(
        starter_cases,
        LocalTransportError::StarterOrganization,
        starter_rows,
        "R42S2.TRANSPORT.StarterOrganization",
        StarterOrganizationError::Account
    );
    domain_cases!(
        lifecycle_cases,
        LocalTransportError::ResidentLifecycle,
        lifecycle_rows,
        "R42S2.TRANSPORT.ResidentLifecycle",
        ResidentLifecycleError::Account
    );
    domain_cases!(
        configuration_cases,
        LocalTransportError::ResidentConfiguration,
        configuration_rows,
        "R42S2.TRANSPORT.ResidentConfiguration",
        ResidentConfigurationError::Account
    );
    domain_cases!(
        project_cases,
        LocalTransportError::Project,
        project_rows,
        "R42S2.TRANSPORT.Project",
        ProjectError::Account,
        |expected, current| ProjectError::StaleCharterRevision { expected, current }
    );
    domain_cases!(
        policy_cases,
        LocalTransportError::ProjectPolicy,
        policy_rows,
        "R42S2.TRANSPORT.ProjectPolicy",
        ProjectPolicyError::Account,
        |expected, current| ProjectPolicyError::StaleCharterRevision { expected, current }
    );
    domain_cases!(
        repository_cases,
        LocalTransportError::ProjectRepository,
        repository_rows,
        "R42S2.TRANSPORT.ProjectRepository",
        ProjectRepositoryError::Account,
        |expected, current| ProjectRepositoryError::StaleCharterRevision { expected, current },
        ProjectRepositoryError::InvalidDraft
    );
    domain_cases!(
        preflight_cases,
        LocalTransportError::CheckPreflight,
        preflight_rows,
        "R42S2.TRANSPORT.CheckPreflight",
        CheckPreflightError::Account
    );
    domain_cases!(
        receipt_cases,
        LocalTransportError::RecentReceipt,
        receipt_rows,
        "R42S2.TRANSPORT.RecentReceipt",
        crate::RecentReceiptError::Account
    );
    domain_cases!(
        sample_cases,
        LocalTransportError::SampleProject,
        sample_rows,
        "R42S2.TRANSPORT.SampleProject",
        SampleProjectError::Account
    );
    domain_cases!(
        team_cases,
        LocalTransportError::Team,
        team_rows,
        "R42S2.TRANSPORT.Team",
        TeamError::Account
    );
    domain_cases!(
        mission_cases,
        LocalTransportError::Mission,
        mission_rows,
        "R42S2.TRANSPORT.Mission",
        MissionError::Account,
        |expected, current| MissionError::StaleCharterRevision { expected, current }
    );
    domain_cases!(
        discovery_cases,
        LocalTransportError::ResourceDiscovery,
        discovery_rows,
        "R42S2.TRANSPORT.ResourceDiscovery",
        ResourceDiscoveryError::Account
    );
    domain_cases!(
        activation_cases,
        LocalTransportError::LocalModelActivation,
        activation_rows,
        "R42S2.TRANSPORT.LocalModelActivation",
        crate::LocalModelActivationError::Account
    );

    /// Fold (astra read), item 1: every child of every cause-bearing Supervisor parent, through
    /// `LocalTransportError::Supervisor`.
    ///
    /// The committed witness constructed ONE representative child per parent, so the new
    /// selector's own vocabulary was never walked -- astra's forwarding mutant in
    /// `classification_tag` survived both S and T. Supervisor child tags are the PLAIN root NAME
    /// (no Account recursion, no charter numbers), which is why the expected tag here is the
    /// child row's own case ID; classification is the exception the spec names, and uses its
    /// lower-case token table with the predecessor digest dropped.
    fn supervisor_child_cases(cases: &mut Vec<Case>) {
        macro_rules! supervisor_child {
            ($parent:literal, $rows:ident, $build:expr) => {
                for (case, child, markers) in $rows() {
                    cases.push(Case {
                        id: format!("R42S2.TRANSPORT.Supervisor.{}.{}", $parent, case),
                        value: LocalTransportError::Supervisor(($build)(child)),
                        markers,
                    });
                }
            };
            ($parent:literal, $rows:ident, $expected:ident, $build:expr) => {
                for (case, child, markers) in $rows() {
                    cases.push(Case {
                        id: format!("R42S2.TRANSPORT.Supervisor.{}.{}", $parent, case),
                        value: LocalTransportError::Supervisor(($build)(child)),
                        markers,
                    });
                }
            };
        }

        supervisor_child!("StarterOrganizationStateMismatch", starter_rows, |child| {
            SupervisorError::StarterOrganizationStateMismatch {
                workspace_id: first_uuid(),
                cause: child,
            }
        });
        supervisor_child!("ProjectStateMismatch", project_rows, |child| {
            SupervisorError::ProjectStateMismatch {
                workspace_id: first_uuid(),
                project_id: first_uuid(),
                cause: child,
            }
        });
        supervisor_child!(
            "LocalModelActivationStateMismatch",
            activation_rows,
            |child| SupervisorError::LocalModelActivationStateMismatch {
                workspace_id: first_uuid(),
                activation_id: first_uuid(),
                cause: child
            }
        );
        supervisor_child!(
            "LocalModelActivationRowUnreadable",
            activation_rows,
            SupervisorError::LocalModelActivationRowUnreadable
        );
        supervisor_child!("ResidentLifecycleStateMismatch", lifecycle_rows, |child| {
            SupervisorError::ResidentLifecycleStateMismatch {
                workspace_id: first_uuid(),
                resident_id: first_uuid(),
                cause: child,
            }
        });
        supervisor_child!("ResidentRowUnreadable", lifecycle_rows, |child| {
            SupervisorError::ResidentRowUnreadable(child)
        });
        supervisor_child!(
            "ResidentConfigurationStateMismatch",
            configuration_rows,
            |child| SupervisorError::ResidentConfigurationStateMismatch {
                workspace_id: first_uuid(),
                decision_id: digest_a(),
                cause: child
            }
        );
        supervisor_child!("TeamStateMismatch", team_rows, |child| {
            SupervisorError::TeamStateMismatch {
                workspace_id: first_uuid(),
                team_id: first_uuid(),
                cause: child,
            }
        });
        supervisor_child!("MissionStateMismatch", mission_rows, |child| {
            SupervisorError::MissionStateMismatch {
                workspace_id: first_uuid(),
                mission_id: first_uuid(),
                cause: child,
            }
        });
        supervisor_child!("ProjectPolicyStateMismatch", policy_rows, |child| {
            SupervisorError::ProjectPolicyStateMismatch {
                workspace_id: first_uuid(),
                project_id: first_uuid(),
                cause: child,
            }
        });
        supervisor_child!("ProjectRepositoryStateMismatch", repository_rows, |child| {
            SupervisorError::ProjectRepositoryStateMismatch {
                workspace_id: first_uuid(),
                project_id: first_uuid(),
                cause: child,
            }
        });
        supervisor_child!("SampleProjectStateMismatch", sample_rows, |child| {
            SupervisorError::SampleProjectStateMismatch {
                workspace_id: first_uuid(),
                cause: child,
            }
        });
        supervisor_child!("RunStateMismatch", preflight_rows, |child| {
            SupervisorError::RunStateMismatch {
                workspace_id: first_uuid(),
                run_id: digest_a(),
                cause: child,
            }
        });
        supervisor_child!("ProposalStateMismatch", proposal_rows, |child| {
            SupervisorError::ProposalStateMismatch {
                workspace_id: first_uuid(),
                digest_value: digest_a(),
                cause: child,
            }
        });
        supervisor_child!("RulingStateMismatch", ruling_rows, |child| {
            SupervisorError::RulingStateMismatch {
                workspace_id: first_uuid(),
                digest_value: digest_a(),
                cause: child,
            }
        });
        supervisor_child!("RulingChainsMismatch", ruling_rows, |child| {
            SupervisorError::RulingChainsMismatch(child)
        });
        supervisor_child!("CandidateStateMismatch", candidate_rows, |child| {
            SupervisorError::CandidateStateMismatch {
                workspace_id: first_uuid(),
                digest_value: digest_a(),
                cause: child,
            }
        });
        supervisor_child!("EvaluationStateMismatch", evaluation_rows, |child| {
            SupervisorError::EvaluationStateMismatch {
                workspace_id: first_uuid(),
                digest_value: digest_a(),
                cause: child,
            }
        });
        supervisor_child!("EvaluationChainsMismatch", evaluation_rows, |child| {
            SupervisorError::EvaluationChainsMismatch(child)
        });
        supervisor_child!("MissionCompletionStateMismatch", completion_rows, |child| {
            SupervisorError::MissionCompletionStateMismatch {
                workspace_id: first_uuid(),
                digest_value: digest_a(),
                cause: child,
            }
        });
        supervisor_child!("ArtifactStateMismatch", artifact_rows, |child| {
            SupervisorError::ArtifactStateMismatch {
                workspace_id: first_uuid(),
                digest_value: digest_a(),
                cause: child,
            }
        });
        supervisor_child!("ArtifactRelationMismatch", artifact_rows, |child| {
            SupervisorError::ArtifactRelationMismatch(child)
        });
        supervisor_child!(
            "MissionImpactClassificationStateMismatch",
            classification_rows,
            expected_classification,
            |child| SupervisorError::MissionImpactClassificationStateMismatch {
                workspace_id: first_uuid(),
                digest: digest_a(),
                cause: child
            }
        );
        supervisor_child!("OutcomeManifestStateMismatch", manifest_rows, |child| {
            SupervisorError::OutcomeManifestStateMismatch {
                workspace_id: first_uuid(),
                manifest_digest: digest_a(),
                cause: child,
            }
        });
        supervisor_child!("OutcomePublicationStateMismatch", publish_rows, |child| {
            SupervisorError::OutcomePublicationStateMismatch {
                workspace_id: first_uuid(),
                mission_id: first_uuid(),
                version: STORED_VERSION,
                cause: child,
            }
        });
        supervisor_child!("StartupEnumerationFailed", startup_pass_rows, |child| {
            SupervisorError::StartupEnumerationFailed {
                source: driver_error(NEEDLE_A),
                pass: child,
            }
        });

        // The two nested classification vocabularies, walked whole. `Predecessor` carries a
        // digest the transport tag must drop -- the survivor astra found -- so both digests are
        // used and neither may appear.
        for (case, reason, markers) in malformed_classification_rows() {
            cases.push(Case {
                id: format!(
                    "R42S2.TRANSPORT.Supervisor.MissionImpactClassificationStateMismatch.Malformed.{case}"
                ),
                value: LocalTransportError::Supervisor(
                    SupervisorError::MissionImpactClassificationStateMismatch {
                        workspace_id: first_uuid(),
                        digest: digest_a(),
                        cause: ClassificationStoreError::Malformed(reason),
                    },
                ),
                markers,
            });
        }
        for (case, fault, markers) in predecessor_fault_rows() {
            cases.push(Case {
                id: format!(
                    "R42S2.TRANSPORT.Supervisor.MissionImpactClassificationStateMismatch.Predecessor.{case}"
                ),
                value: LocalTransportError::Supervisor(
                    SupervisorError::MissionImpactClassificationStateMismatch {
                        workspace_id: first_uuid(),
                        digest: digest_a(),
                        cause: ClassificationStoreError::Predecessor {
                            digest: digest_b(),
                            fault,
                        },
                    },
                ),
                markers,
            });
        }
    }

    /// The declaration-order lead cases, so a mutant's FIRST failing label is predictable.
    const TRANSPORT_LEADS: [&str; 6] = [
        "R42S2.TRANSPORT.Io",
        "R42S2.TRANSPORT.Json",
        "R42S2.TRANSPORT.Account.Store",
        "R42S2.TRANSPORT.Project.Account.Store",
        "R42S2.TRANSPORT.ProjectRepository.InvalidDraft.Source",
        // The Supervisor sub-tree's own lead: payload-free after commit 1's startup fix, so a
        // forwarding mutant fails on the TEXT rather than on a needle.
        "R42S2.TRANSPORT.Supervisor.Io",
    ];

    fn transport_cases() -> Vec<Case> {
        let mut cases: Vec<Case> = Vec::new();
        for (stem, value, markers) in transport_rows() {
            match stem {
                "R42S2.TRANSPORT.Remote" => {
                    for (child, code, _) in code_rows() {
                        cases.push(Case {
                            id: format!("{stem}.{child}"),
                            value: LocalTransportError::Remote {
                                code,
                                retry_after_ms: None,
                            },
                            markers: &[],
                        });
                    }
                }
                "R42S2.TRANSPORT.Account" => {
                    for (child, account, child_markers) in account_rows() {
                        cases.push(Case {
                            id: format!("{stem}.{child}"),
                            value: LocalTransportError::Account(account),
                            markers: child_markers,
                        });
                    }
                }
                "R42S2.TRANSPORT.StarterOrganization" => starter_cases(&mut cases),
                "R42S2.TRANSPORT.ResidentLifecycle" => lifecycle_cases(&mut cases),
                "R42S2.TRANSPORT.ResidentConfiguration" => configuration_cases(&mut cases),
                "R42S2.TRANSPORT.Project" => project_cases(&mut cases),
                "R42S2.TRANSPORT.ProjectPolicy" => policy_cases(&mut cases),
                "R42S2.TRANSPORT.ProjectRepository" => repository_cases(&mut cases),
                "R42S2.TRANSPORT.CheckPreflight" => preflight_cases(&mut cases),
                "R42S2.TRANSPORT.RecentReceipt" => receipt_cases(&mut cases),
                "R42S2.TRANSPORT.SampleProject" => sample_cases(&mut cases),
                "R42S2.TRANSPORT.Team" => team_cases(&mut cases),
                "R42S2.TRANSPORT.Mission" => mission_cases(&mut cases),
                "R42S2.TRANSPORT.ResourceDiscovery" => discovery_cases(&mut cases),
                "R42S2.TRANSPORT.LocalModelActivation" => activation_cases(&mut cases),
                "R42S2.TRANSPORT.Supervisor" => {
                    for (child, fault, child_markers) in supervisor_rows() {
                        cases.push(Case {
                            id: format!("{stem}.{child}"),
                            value: LocalTransportError::Supervisor(fault),
                            markers: child_markers,
                        });
                    }
                    supervisor_child_cases(&mut cases);
                }
                "R42S2.TRANSPORT.Outcome" => {
                    for (child, refusal, child_markers) in publish_rows() {
                        match child {
                            "R42S2.OUTCOME.Seam" => {
                                for (leaf, seam, leaf_markers) in seam_rows() {
                                    cases.push(Case {
                                        id: format!("{stem}.Seam.{leaf}"),
                                        value: LocalTransportError::Outcome(PublishRefusal::Seam(
                                            seam,
                                        )),
                                        markers: leaf_markers,
                                    });
                                }
                            }
                            "R42S2.OUTCOME.Manifest" => {
                                for (leaf, manifest, leaf_markers) in manifest_rows() {
                                    if leaf == "Malformed" {
                                        for (reason_case, reason, reason_markers) in
                                            malformed_outcome_rows()
                                        {
                                            cases.push(Case {
                                                id: format!(
                                                    "{stem}.Manifest.Malformed.{reason_case}"
                                                ),
                                                value: LocalTransportError::Outcome(
                                                    PublishRefusal::Manifest(
                                                        ManifestStoreRefusal::Malformed(reason),
                                                    ),
                                                ),
                                                markers: reason_markers,
                                            });
                                        }
                                    } else {
                                        cases.push(Case {
                                            id: format!("{stem}.Manifest.{leaf}"),
                                            value: LocalTransportError::Outcome(
                                                PublishRefusal::Manifest(manifest),
                                            ),
                                            markers: leaf_markers,
                                        });
                                    }
                                }
                            }
                            "R42S2.OUTCOME.Pdf" => {
                                for (leaf, pdf, leaf_markers) in pdf_rows() {
                                    if leaf == "Translate" {
                                        for (typst_case, typst, typst_markers) in typst_rows() {
                                            cases.push(Case {
                                                id: format!("{stem}.Pdf.Translate.{typst_case}"),
                                                value: LocalTransportError::Outcome(
                                                    PublishRefusal::Pdf(PdfRefusal::Translate(
                                                        typst,
                                                    )),
                                                ),
                                                markers: typst_markers,
                                            });
                                        }
                                    } else {
                                        cases.push(Case {
                                            id: format!("{stem}.Pdf.{leaf}"),
                                            value: LocalTransportError::Outcome(
                                                PublishRefusal::Pdf(pdf),
                                            ),
                                            markers: leaf_markers,
                                        });
                                    }
                                }
                            }
                            "R42S2.OUTCOME.Account" => {
                                for (leaf, account, leaf_markers) in account_rows() {
                                    cases.push(Case {
                                        id: format!("{stem}.Account.{leaf}"),
                                        value: LocalTransportError::Outcome(
                                            PublishRefusal::Account(account),
                                        ),
                                        markers: leaf_markers,
                                    });
                                }
                            }
                            leaf => cases.push(Case {
                                id: format!("{stem}.{}", leaf.trim_start_matches("R42S2.OUTCOME.")),
                                value: LocalTransportError::Outcome(refusal),
                                markers: child_markers,
                            }),
                        }
                    }
                }
                leaf => cases.push(Case {
                    id: leaf.to_owned(),
                    value,
                    markers,
                }),
            }
        }
        cases.sort_by_key(|case| {
            TRANSPORT_LEADS
                .iter()
                .position(|lead| *lead == case.id)
                .unwrap_or(usize::MAX)
        });
        cases
    }

    /// R42 slice 2: every `LocalTransportError` line a caller reads is the closed projection, and
    /// every value it was built from is still in `Debug`.
    ///
    /// Seventeen arms interpolated a child's own `Display` -- written for whoever owns that row,
    /// which a transport caller may not be. Sixteen are closed here; `Supervisor` is closed in
    /// commit 1 with its lib.rs selector and its row below still records the OPEN line.
    #[test]
    fn r42_transport_projection_is_closed() {
        let cases = transport_cases();

        // Fixture premises first: a broken fixture is not a mutant kill.
        assert!(
            format!("{:?}", driver_error(NEEDLE_A)).contains(NEEDLE_A),
            "R42S2.FIXTURE.Driver.NEEDLE"
        );
        assert!(
            format!("{:?}", io_error(NEEDLE_A)).contains(NEEDLE_A),
            "R42S2.FIXTURE.Io.NEEDLE"
        );
        assert!(
            format!("{:?}", json_error(NEEDLE_A)).contains(NEEDLE_A),
            "R42S2.FIXTURE.Json.NEEDLE"
        );
        assert!(
            payload_a().contains(NEEDLE_A),
            "R42S2.FIXTURE.PayloadA.NEEDLE"
        );
        assert!(
            payload_b().contains(NEEDLE_B),
            "R42S2.FIXTURE.PayloadB.NEEDLE"
        );

        assert_eq!(
            account_rows().len(),
            18,
            "R42S2.TRANSPORT.CARDINALITY.Account"
        );
        assert_eq!(
            draft_rows().len(),
            3,
            "R42S2.TRANSPORT.CARDINALITY.InvalidDraft"
        );
        assert_eq!(
            code_rows().len(),
            63,
            "R42S2.TRANSPORT.CARDINALITY.LocalErrorCode"
        );
        assert_eq!(
            transport_rows().len(),
            23,
            "R42S2.TRANSPORT.CARDINALITY.ROOT"
        );
        // 22 roots expanded: 62 Remote codes, 72 Supervisor roots PLUS every child of all 26
        // cause-bearing Supervisor parents (including the two nested classification
        // vocabularies), 18 account faults at each of 14 wrappers, the three draft reasons, both
        // charter-revision pairs, and slice 1's 83-case Outcome tree.
        assert_eq!(cases.len(), 878, "R42S2.TRANSPORT.CARDINALITY.CASES");
        assert_eq!(
            supervisor_rows().len(),
            75,
            "R42S2.TRANSPORT.CARDINALITY.Supervisor"
        );
        let ids: std::collections::BTreeSet<&str> =
            cases.iter().map(|case| case.id.as_str()).collect();
        assert_eq!(ids.len(), cases.len(), "R42S2.TRANSPORT.CASE_IDS_UNIQUE");
        for lead in TRANSPORT_LEADS {
            assert!(ids.contains(lead), "R42S2.TRANSPORT.LEAD_PRESENT: {lead}");
        }

        for case in &cases {
            let Case { id, value, markers } = case;
            let (stem, expected) = expected_transport(value);
            assert!(
                id == stem || id.starts_with(&format!("{stem}.")),
                "R42S2.FIXTURE.{id}.PATTERN: built as {stem}"
            );

            let line = value.to_string();
            for marker in *markers {
                assert!(!line.contains(marker), "{id}.NO_NEEDLE: {line}");
            }
            assert!(
                !line.contains('\n') && !line.contains('\r'),
                "{id}.ONE_LINE: {line:?}"
            );

            let debug = format!("{value:?}");
            for marker in *markers {
                assert!(debug.contains(marker), "{id}.DEBUG_RETAINS: {debug}");
            }

            assert_eq!(line, expected, "{id}.PROJECTION");
        }

        // The predecessor digest is a record identity startup may name and a caller may not.
        // Asserted on every fault, with a digest distinct from the classification's own, so a
        // selector that forwarded the child Display would print one of them.
        for (case, fault, _) in predecessor_fault_rows() {
            let value = LocalTransportError::Supervisor(
                SupervisorError::MissionImpactClassificationStateMismatch {
                    workspace_id: first_uuid(),
                    digest: digest_a(),
                    cause: ClassificationStoreError::Predecessor {
                        digest: digest_b(),
                        fault,
                    },
                },
            );
            let line = value.to_string();
            for key in [digest_a(), digest_b(), first_uuid().to_string()] {
                assert!(
                    !line.contains(&key),
                    "R42S2.TRANSPORT.Supervisor.MissionImpactClassificationStateMismatch.Predecessor.{case}.KEYS: {line}"
                );
            }
            assert!(
                format!("{value:?}").contains(&digest_b()),
                "R42S2.TRANSPORT.Supervisor.MissionImpactClassificationStateMismatch.Predecessor.{case}.DEBUG_RETAINS"
            );
        }

        // The same arms with the OTHER marker: identical outward text, its own marker in Debug.
        for (id, first, second) in [
            (
                "R42S2.TRANSPORT.Io",
                LocalTransportError::Io(io_error(NEEDLE_A)),
                LocalTransportError::Io(io_error(NEEDLE_B)),
            ),
            (
                "R42S2.TRANSPORT.Json",
                LocalTransportError::Json(json_error(NEEDLE_A)),
                LocalTransportError::Json(json_error(NEEDLE_B)),
            ),
            (
                "R42S2.TRANSPORT.Account.Store",
                LocalTransportError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))),
                LocalTransportError::Account(OperatorAccountError::Store(driver_error(NEEDLE_B))),
            ),
            (
                "R42S2.TRANSPORT.Project.Account.Store",
                LocalTransportError::Project(ProjectError::Account(OperatorAccountError::Store(
                    driver_error(NEEDLE_A),
                ))),
                LocalTransportError::Project(ProjectError::Account(OperatorAccountError::Store(
                    driver_error(NEEDLE_B),
                ))),
            ),
        ] {
            assert_eq!(
                first.to_string(),
                second.to_string(),
                "{id}.HIDDEN_CHANGE_SAME_LINE"
            );
            assert!(
                !second.to_string().contains(NEEDLE_B),
                "{id}.HIDDEN_CHANGE_NO_NEEDLE"
            );
            assert!(
                format!("{second:?}").contains(NEEDLE_B),
                "{id}.HIDDEN_CHANGE_DEBUG_RETAINS"
            );
        }

        // A production conversion, not a hand-built fixture: closing the line must not have been
        // bought by erasing the cause on the way in.
        let original = io_error(NEEDLE_A);
        assert!(
            format!("{original:?}").contains(NEEDLE_A),
            "R42S2.FIXTURE.FromIo.NEEDLE"
        );
        let converted = LocalTransportError::from(original);
        assert!(
            format!("{converted:?}").contains(NEEDLE_A),
            "R42S2.TRANSPORT.FromIo.DEBUG_RETAINS"
        );
        assert_eq!(
            converted.to_string(),
            "local transport I/O failed: Io",
            "R42S2.TRANSPORT.FromIo.PROJECTION"
        );
    }
    /// The spec's 153 immediate domain mappings, transcribed. `Account` means "recurse on
    /// the account payload", exactly as the production mapper does.
    const WIRE_MAP: [(&str, WireExpectation); 164] = [
        (
            "Account.PasswordTooShort",
            WireExpectation::Code("password_too_short"),
        ),
        (
            "Account.PasswordTooLong",
            WireExpectation::Code("password_too_long"),
        ),
        (
            "Account.PasswordConfirmationMismatch",
            WireExpectation::Code("password_confirmation_mismatch"),
        ),
        (
            "Account.RecoveryNotConfirmed",
            WireExpectation::Code("recovery_not_confirmed"),
        ),
        (
            "Account.SetupNotStarted",
            WireExpectation::Code("setup_not_started"),
        ),
        (
            "Account.AccountAlreadyExists",
            WireExpectation::Code("account_already_exists"),
        ),
        (
            "Account.AccountMissing",
            WireExpectation::Code("account_missing"),
        ),
        (
            "Account.AuthenticationFailed",
            WireExpectation::Code("authentication_failed"),
        ),
        (
            "Account.AuthenticationThrottled",
            WireExpectation::Code("authentication_throttled"),
        ),
        (
            "Account.CredentialDisabled",
            WireExpectation::Code("credential_disabled"),
        ),
        (
            "Account.RemotePasswordNotAllowed",
            WireExpectation::Code("remote_password_not_allowed"),
        ),
        (
            "Account.NotAuthenticated",
            WireExpectation::Code("not_authenticated"),
        ),
        (
            "Account.InvalidSessionTimeout",
            WireExpectation::Code("invalid_session_timeout"),
        ),
        (
            "Account.CredentialProcessing",
            WireExpectation::Code("account_service_unavailable"),
        ),
        (
            "Account.InvalidStore",
            WireExpectation::Code("account_service_unavailable"),
        ),
        (
            "Account.ClockBeforeUnixEpoch",
            WireExpectation::Code("account_service_unavailable"),
        ),
        (
            "Account.Store",
            WireExpectation::Code("account_service_unavailable"),
        ),
        (
            "Account.StateUnavailable",
            WireExpectation::Code("account_service_unavailable"),
        ),
        ("StarterOrganization.Account", WireExpectation::Account),
        (
            "StarterOrganization.UnsupportedProposalVersion",
            WireExpectation::Code("unsupported_starter_proposal"),
        ),
        (
            "StarterOrganization.EmptyRoster",
            WireExpectation::Code("invalid_starter_roster"),
        ),
        (
            "StarterOrganization.DuplicateRole",
            WireExpectation::Code("invalid_starter_roster"),
        ),
        (
            "StarterOrganization.InvalidDisplayName",
            WireExpectation::Code("invalid_starter_roster"),
        ),
        (
            "StarterOrganization.DecisionAlreadyRecorded",
            WireExpectation::Code("starter_decision_recorded"),
        ),
        (
            "StarterOrganization.InvalidStore",
            WireExpectation::Code("starter_organization_unavailable"),
        ),
        (
            "StarterOrganization.Store",
            WireExpectation::Code("starter_organization_unavailable"),
        ),
        (
            "StarterOrganization.Json",
            WireExpectation::Code("starter_organization_unavailable"),
        ),
        ("ResidentLifecycle.Account", WireExpectation::Account),
        (
            "ResidentLifecycle.NotFound",
            WireExpectation::Code("resident_not_found"),
        ),
        (
            "ResidentLifecycle.InvalidTransition",
            WireExpectation::Code("invalid_resident_transition"),
        ),
        (
            "ResidentLifecycle.StaleDecision",
            WireExpectation::Code("stale_resident_decision"),
        ),
        (
            "ResidentLifecycle.InvalidStore",
            WireExpectation::Code("resident_lifecycle_unavailable"),
        ),
        (
            "ResidentLifecycle.Store",
            WireExpectation::Code("resident_lifecycle_unavailable"),
        ),
        (
            "ResidentLifecycle.Json",
            WireExpectation::Code("resident_lifecycle_unavailable"),
        ),
        ("ResidentConfiguration.Account", WireExpectation::Account),
        (
            "ResidentConfiguration.ResidentNotFound",
            WireExpectation::Code("resident_not_found"),
        ),
        (
            "ResidentConfiguration.ResidentRetired",
            WireExpectation::Code("invalid_resident_configuration"),
        ),
        (
            "ResidentConfiguration.ModelActivationUnavailable",
            WireExpectation::Code("invalid_resident_configuration"),
        ),
        (
            "ResidentConfiguration.ModelMissingCompletion",
            WireExpectation::Code("invalid_resident_configuration"),
        ),
        (
            "ResidentConfiguration.InvalidCapabilities",
            WireExpectation::Code("invalid_resident_configuration"),
        ),
        (
            "ResidentConfiguration.AlreadyCleared",
            WireExpectation::Code("invalid_resident_configuration"),
        ),
        (
            "ResidentConfiguration.StaleDecision",
            WireExpectation::Code("stale_resident_configuration"),
        ),
        (
            "ResidentConfiguration.InvalidStore",
            WireExpectation::Code("resident_configuration_unavailable"),
        ),
        (
            "ResidentConfiguration.Store",
            WireExpectation::Code("resident_configuration_unavailable"),
        ),
        (
            "ResidentConfiguration.Json",
            WireExpectation::Code("resident_configuration_unavailable"),
        ),
        ("Project.Account", WireExpectation::Account),
        (
            "Project.InvalidName",
            WireExpectation::Code("invalid_project"),
        ),
        (
            "Project.InvalidPurpose",
            WireExpectation::Code("invalid_project"),
        ),
        (
            "Project.InvalidDesiredOutcomes",
            WireExpectation::Code("invalid_project"),
        ),
        (
            "Project.InvalidConstraint",
            WireExpectation::Code("invalid_project"),
        ),
        (
            "Project.InvalidProhibition",
            WireExpectation::Code("invalid_project"),
        ),
        (
            "Project.DuplicateCharterEntry",
            WireExpectation::Code("invalid_project"),
        ),
        (
            "Project.StaleCharterRevision",
            WireExpectation::Code("stale_charter_revision"),
        ),
        (
            "Project.UnchangedCharter",
            WireExpectation::Code("unchanged_charter"),
        ),
        (
            "Project.NotFound",
            WireExpectation::Code("project_not_found"),
        ),
        (
            "Project.InvalidStore",
            WireExpectation::Code("project_unavailable"),
        ),
        (
            "Project.Store",
            WireExpectation::Code("project_unavailable"),
        ),
        ("Project.Json", WireExpectation::Code("project_unavailable")),
        ("ProjectPolicy.Account", WireExpectation::Account),
        (
            "ProjectPolicy.ProjectNotFound",
            WireExpectation::Code("project_not_found"),
        ),
        (
            "ProjectPolicy.StaleCharterRevision",
            WireExpectation::Code("stale_charter_revision"),
        ),
        (
            "ProjectPolicy.StalePolicyRevision",
            WireExpectation::Code("stale_project_policy"),
        ),
        (
            "ProjectPolicy.InvalidRequiredCapabilities",
            WireExpectation::Code("invalid_project_policy"),
        ),
        (
            "ProjectPolicy.InvalidTrackRecordRequirements",
            WireExpectation::Code("invalid_project_policy"),
        ),
        (
            "ProjectPolicy.UnchangedPolicy",
            WireExpectation::Code("unchanged_project_policy"),
        ),
        (
            "ProjectPolicy.InvalidStore",
            WireExpectation::Code("project_policy_unavailable"),
        ),
        (
            "ProjectPolicy.Store",
            WireExpectation::Code("project_policy_unavailable"),
        ),
        (
            "ProjectPolicy.Json",
            WireExpectation::Code("project_policy_unavailable"),
        ),
        ("ProjectRepository.Account", WireExpectation::Account),
        (
            "ProjectRepository.ProjectNotFound",
            WireExpectation::Code("project_not_found"),
        ),
        (
            "ProjectRepository.StaleCharterRevision",
            WireExpectation::Code("stale_charter_revision"),
        ),
        (
            "ProjectRepository.StaleRepositoryRevision",
            WireExpectation::Code("stale_project_repository"),
        ),
        (
            "ProjectRepository.InvalidDraft",
            WireExpectation::Code("invalid_project_repository"),
        ),
        (
            "ProjectRepository.UnchangedRepository",
            WireExpectation::Code("unchanged_project_repository"),
        ),
        (
            "ProjectRepository.InvalidStore",
            WireExpectation::Code("project_repository_unavailable"),
        ),
        (
            "ProjectRepository.Store",
            WireExpectation::Code("project_repository_unavailable"),
        ),
        (
            "ProjectRepository.Json",
            WireExpectation::Code("project_repository_unavailable"),
        ),
        ("CheckPreflight.Account", WireExpectation::Account),
        (
            "CheckPreflight.ProjectNotFound",
            WireExpectation::Code("project_not_found"),
        ),
        (
            "CheckPreflight.RepositoryNotBound",
            WireExpectation::Code("repository_not_bound"),
        ),
        (
            "CheckPreflight.InvalidStore",
            WireExpectation::Code("preflight_unavailable"),
        ),
        (
            "CheckPreflight.Store",
            WireExpectation::Code("preflight_unavailable"),
        ),
        (
            "CheckPreflight.Json",
            WireExpectation::Code("preflight_unavailable"),
        ),
        ("RecentReceipt.Account", WireExpectation::Account),
        (
            "RecentReceipt.InvalidStore",
            WireExpectation::Code("preflight_unavailable"),
        ),
        (
            "RecentReceipt.Store",
            WireExpectation::Code("preflight_unavailable"),
        ),
        ("SampleProject.Account", WireExpectation::Account),
        (
            "SampleProject.NotInstalled",
            WireExpectation::Code("sample_project_unavailable"),
        ),
        (
            "SampleProject.UnsafeDestination",
            WireExpectation::Code("sample_project_unavailable"),
        ),
        (
            "SampleProject.BundleMismatch",
            WireExpectation::Code("sample_project_unavailable"),
        ),
        (
            "SampleProject.MaterializationFailed",
            WireExpectation::Code("sample_project_unavailable"),
        ),
        (
            "SampleProject.TamperedInstallation",
            WireExpectation::Code("sample_project_unavailable"),
        ),
        (
            "SampleProject.InvalidStore",
            WireExpectation::Code("sample_project_unavailable"),
        ),
        (
            "SampleProject.Store",
            WireExpectation::Code("sample_project_unavailable"),
        ),
        (
            "SampleProject.Io",
            WireExpectation::Code("sample_project_unavailable"),
        ),
        (
            "SampleProject.Json",
            WireExpectation::Code("sample_project_unavailable"),
        ),
        ("Team.Account", WireExpectation::Account),
        (
            "Team.ProjectNotFound",
            WireExpectation::Code("project_not_found"),
        ),
        ("Team.TeamNotFound", WireExpectation::Code("team_not_found")),
        ("Team.InvalidName", WireExpectation::Code("invalid_team")),
        ("Team.InvalidMembers", WireExpectation::Code("invalid_team")),
        (
            "Team.ResidentConfigurationUnavailable",
            WireExpectation::Code("invalid_team"),
        ),
        (
            "Team.StaleResidentConfiguration",
            WireExpectation::Code("stale_team_member_configuration"),
        ),
        (
            "Team.ResidentIneligible",
            WireExpectation::Code("team_member_ineligible"),
        ),
        (
            "Team.InvalidStore",
            WireExpectation::Code("team_unavailable"),
        ),
        ("Team.Store", WireExpectation::Code("team_unavailable")),
        ("Team.Json", WireExpectation::Code("team_unavailable")),
        ("Mission.Account", WireExpectation::Account),
        (
            "Mission.ProjectNotFound",
            WireExpectation::Code("project_not_found"),
        ),
        (
            "Mission.TeamNotFound",
            WireExpectation::Code("team_not_found"),
        ),
        (
            "Mission.MissionNotFound",
            WireExpectation::Code("mission_not_found"),
        ),
        (
            "Mission.StaleCharterRevision",
            WireExpectation::Code("stale_charter_revision"),
        ),
        (
            "Mission.InvalidDesiredOutcome",
            WireExpectation::Code("invalid_mission"),
        ),
        (
            "Mission.InvalidSuccessConditions",
            WireExpectation::Code("invalid_mission"),
        ),
        (
            "Mission.InvalidConstraint",
            WireExpectation::Code("invalid_mission"),
        ),
        (
            "Mission.InvalidProhibition",
            WireExpectation::Code("invalid_mission"),
        ),
        (
            "Mission.DuplicateMissionEntry",
            WireExpectation::Code("invalid_mission"),
        ),
        (
            "Mission.InvalidMembers",
            WireExpectation::Code("invalid_mission"),
        ),
        (
            "Mission.StaleTeamEligibility",
            WireExpectation::Code("stale_mission_team"),
        ),
        (
            "Mission.ResidentIneligible",
            WireExpectation::Code("mission_member_ineligible"),
        ),
        (
            "Mission.InvalidStore",
            WireExpectation::Code("mission_unavailable"),
        ),
        (
            "Mission.Store",
            WireExpectation::Code("mission_unavailable"),
        ),
        ("Mission.Json", WireExpectation::Code("mission_unavailable")),
        ("ResourceDiscovery.Account", WireExpectation::Account),
        (
            "ResourceDiscovery.EndpointNotLoopback",
            WireExpectation::Code("protocol_error"),
        ),
        ("LocalModelActivation.Account", WireExpectation::Account),
        (
            "LocalModelActivation.ResourceUnavailable",
            WireExpectation::Code("local_model_unavailable"),
        ),
        (
            "LocalModelActivation.InvalidProviderResponse",
            WireExpectation::Code("local_model_unavailable"),
        ),
        (
            "LocalModelActivation.ModelNotFound",
            WireExpectation::Code("local_model_not_found"),
        ),
        (
            "LocalModelActivation.ModelDrift",
            WireExpectation::Code("local_model_changed"),
        ),
        (
            "LocalModelActivation.InvalidApproval",
            WireExpectation::Code("invalid_local_model_approval"),
        ),
        (
            "LocalModelActivation.AlreadyActive",
            WireExpectation::Code("local_model_already_active"),
        ),
        (
            "LocalModelActivation.StaleDecision",
            WireExpectation::Code("stale_local_model_decision"),
        ),
        (
            "LocalModelActivation.InvalidStore",
            WireExpectation::Code("local_model_activation_unavailable"),
        ),
        (
            "LocalModelActivation.Store",
            WireExpectation::Code("local_model_activation_unavailable"),
        ),
        (
            "LocalModelActivation.Json",
            WireExpectation::Code("local_model_activation_unavailable"),
        ),
        ("Outcome.Seam", WireExpectation::Code("outcome_unavailable")),
        (
            "Outcome.NoManifestForCandidate",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.Manifest",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.ManifestMissionDiffers",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.Completion",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.AppendixRequired",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.VersionGap",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.AlreadyPublished",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.HistoryNotContiguous",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.QualifyingVerdictNotMeets",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.MissionBindingBroken",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.CandidateIsNotMissionScoped",
            WireExpectation::Code("outcome_unavailable"),
        ),
        ("Outcome.Pdf", WireExpectation::Code("outcome_unavailable")),
        (
            "Outcome.Unverified",
            WireExpectation::Code("outcome_unavailable"),
        ),
        (
            "Outcome.Store",
            WireExpectation::Code("outcome_unavailable"),
        ),
        ("Outcome.Absent", WireExpectation::Code("outcome_absent")),
        ("Outcome.Account", WireExpectation::Account),
        // The completion COMMAND family. Transcribed from the contract, one row per owner arm:
        // malformed grammar and duplicated set entries are `completion_invalid_details`, never
        // "text over limit", and an unverifiable completion reuses C3's existing code.
        (
            "MissionCompletionCommand.Account",
            WireExpectation::Account,
        ),
        (
            "MissionCompletionCommand.IntentChanged",
            WireExpectation::Code("completion_intent_changed"),
        ),
        (
            "MissionCompletionCommand.AlreadyRecorded",
            WireExpectation::Code("completion_already_recorded"),
        ),
        (
            "MissionCompletionCommand.ResidentNotInAdmissionRoster",
            WireExpectation::Code("completion_resident_not_in_admission_roster"),
        ),
        (
            "MissionCompletionCommand.ConditionCoverageIncomplete",
            WireExpectation::Code("completion_condition_coverage_incomplete"),
        ),
        (
            "MissionCompletionCommand.SupportOverLimit",
            WireExpectation::Code("completion_support_over_limit"),
        ),
        (
            "MissionCompletionCommand.TextOverLimit",
            WireExpectation::Code("completion_text_over_limit"),
        ),
        (
            "MissionCompletionCommand.SpendReasonRequired",
            WireExpectation::Code("completion_spend_reason_required"),
        ),
        (
            "MissionCompletionCommand.MalformedRequest",
            WireExpectation::Code("completion_invalid_details"),
        ),
        (
            "MissionCompletionCommand.DuplicateSetEntry",
            WireExpectation::Code("completion_invalid_details"),
        ),
        (
            "MissionCompletionCommand.Unavailable",
            WireExpectation::Code("mission_completion_unavailable"),
        ),
    ];

    /// The 70 code spellings: wire literal and reviewed Remote NAME, both independent.
    ///
    /// The eight completion rows are transcribed from `C4-interface.md`'s superseding section, not
    /// read off the enum: a spelling that agreed with the code because it was generated from it
    /// would witness nothing. (`MissionCompletionUnavailable` has no row here -- C3 added the code
    /// without a spelling; that gap is C3's to close, not this slice's to edit.)
    const CODE_SPELLINGS: [(&str, &str, &str); 70] = [
        ("PasswordTooShort", "password_too_short", "PasswordTooShort"),
        ("PasswordTooLong", "password_too_long", "PasswordTooLong"),
        (
            "PasswordConfirmationMismatch",
            "password_confirmation_mismatch",
            "PasswordConfirmationMismatch",
        ),
        (
            "RecoveryNotConfirmed",
            "recovery_not_confirmed",
            "RecoveryNotConfirmed",
        ),
        ("SetupNotStarted", "setup_not_started", "SetupNotStarted"),
        (
            "AccountAlreadyExists",
            "account_already_exists",
            "AccountAlreadyExists",
        ),
        ("AccountMissing", "account_missing", "AccountMissing"),
        (
            "AuthenticationFailed",
            "authentication_failed",
            "AuthenticationFailed",
        ),
        (
            "AuthenticationThrottled",
            "authentication_throttled",
            "AuthenticationThrottled",
        ),
        (
            "CredentialDisabled",
            "credential_disabled",
            "CredentialDisabled",
        ),
        (
            "RemotePasswordNotAllowed",
            "remote_password_not_allowed",
            "RemotePasswordNotAllowed",
        ),
        ("NotAuthenticated", "not_authenticated", "NotAuthenticated"),
        (
            "InvalidSessionTimeout",
            "invalid_session_timeout",
            "InvalidSessionTimeout",
        ),
        (
            "UnsupportedStarterProposal",
            "unsupported_starter_proposal",
            "UnsupportedStarterProposal",
        ),
        (
            "InvalidStarterRoster",
            "invalid_starter_roster",
            "InvalidStarterRoster",
        ),
        (
            "StarterDecisionRecorded",
            "starter_decision_recorded",
            "StarterDecisionRecorded",
        ),
        (
            "StarterOrganizationUnavailable",
            "starter_organization_unavailable",
            "StarterOrganizationUnavailable",
        ),
        ("ResidentNotFound", "resident_not_found", "ResidentNotFound"),
        (
            "InvalidResidentTransition",
            "invalid_resident_transition",
            "InvalidResidentTransition",
        ),
        (
            "StaleResidentDecision",
            "stale_resident_decision",
            "StaleResidentDecision",
        ),
        (
            "ResidentLifecycleUnavailable",
            "resident_lifecycle_unavailable",
            "ResidentLifecycleUnavailable",
        ),
        (
            "InvalidResidentConfiguration",
            "invalid_resident_configuration",
            "InvalidResidentConfiguration",
        ),
        (
            "StaleResidentConfiguration",
            "stale_resident_configuration",
            "StaleResidentConfiguration",
        ),
        (
            "ResidentConfigurationUnavailable",
            "resident_configuration_unavailable",
            "ResidentConfigurationUnavailable",
        ),
        ("InvalidProject", "invalid_project", "InvalidProject"),
        ("ProjectNotFound", "project_not_found", "ProjectNotFound"),
        (
            "StaleCharterRevision",
            "stale_charter_revision",
            "StaleCharterRevision",
        ),
        ("UnchangedCharter", "unchanged_charter", "UnchangedCharter"),
        (
            "ProjectUnavailable",
            "project_unavailable",
            "ProjectUnavailable",
        ),
        (
            "InvalidProjectPolicy",
            "invalid_project_policy",
            "InvalidProjectPolicy",
        ),
        (
            "StaleProjectPolicy",
            "stale_project_policy",
            "StaleProjectPolicy",
        ),
        (
            "UnchangedProjectPolicy",
            "unchanged_project_policy",
            "UnchangedProjectPolicy",
        ),
        (
            "ProjectPolicyUnavailable",
            "project_policy_unavailable",
            "ProjectPolicyUnavailable",
        ),
        (
            "InvalidProjectRepository",
            "invalid_project_repository",
            "InvalidProjectRepository",
        ),
        (
            "StaleProjectRepository",
            "stale_project_repository",
            "StaleProjectRepository",
        ),
        (
            "UnchangedProjectRepository",
            "unchanged_project_repository",
            "UnchangedProjectRepository",
        ),
        (
            "ProjectRepositoryUnavailable",
            "project_repository_unavailable",
            "ProjectRepositoryUnavailable",
        ),
        (
            "RepositoryNotBound",
            "repository_not_bound",
            "RepositoryNotBound",
        ),
        (
            "PreflightUnavailable",
            "preflight_unavailable",
            "PreflightUnavailable",
        ),
        (
            "SampleProjectUnavailable",
            "sample_project_unavailable",
            "SampleProjectUnavailable",
        ),
        ("InvalidTeam", "invalid_team", "InvalidTeam"),
        ("TeamNotFound", "team_not_found", "TeamNotFound"),
        (
            "StaleTeamMemberConfiguration",
            "stale_team_member_configuration",
            "StaleTeamMemberConfiguration",
        ),
        (
            "TeamMemberIneligible",
            "team_member_ineligible",
            "TeamMemberIneligible",
        ),
        ("TeamUnavailable", "team_unavailable", "TeamUnavailable"),
        ("InvalidMission", "invalid_mission", "InvalidMission"),
        ("MissionNotFound", "mission_not_found", "MissionNotFound"),
        ("StaleMissionTeam", "stale_mission_team", "StaleMissionTeam"),
        (
            "MissionMemberIneligible",
            "mission_member_ineligible",
            "MissionMemberIneligible",
        ),
        (
            "MissionUnavailable",
            "mission_unavailable",
            "MissionUnavailable",
        ),
        ("OutcomeAbsent", "outcome_absent", "OutcomeAbsent"),
        (
            "OutcomeUnavailable",
            "outcome_unavailable",
            "OutcomeUnavailable",
        ),
        (
            "LocalModelUnavailable",
            "local_model_unavailable",
            "LocalModelUnavailable",
        ),
        (
            "LocalModelNotFound",
            "local_model_not_found",
            "LocalModelNotFound",
        ),
        (
            "LocalModelChanged",
            "local_model_changed",
            "LocalModelChanged",
        ),
        (
            "InvalidLocalModelApproval",
            "invalid_local_model_approval",
            "InvalidLocalModelApproval",
        ),
        (
            "LocalModelAlreadyActive",
            "local_model_already_active",
            "LocalModelAlreadyActive",
        ),
        (
            "StaleLocalModelDecision",
            "stale_local_model_decision",
            "StaleLocalModelDecision",
        ),
        (
            "LocalModelActivationUnavailable",
            "local_model_activation_unavailable",
            "LocalModelActivationUnavailable",
        ),
        (
            "AccountServiceUnavailable",
            "account_service_unavailable",
            "AccountServiceUnavailable",
        ),
        (
            "UnauthorizedClient",
            "unauthorized_client",
            "UnauthorizedClient",
        ),
        ("ProtocolError", "protocol_error", "ProtocolError"),
        (
            "CompletionIntentChanged",
            "completion_intent_changed",
            "CompletionIntentChanged",
        ),
        (
            "CompletionAlreadyRecorded",
            "completion_already_recorded",
            "CompletionAlreadyRecorded",
        ),
        (
            "CompletionResidentNotInAdmissionRoster",
            "completion_resident_not_in_admission_roster",
            "CompletionResidentNotInAdmissionRoster",
        ),
        (
            "CompletionConditionCoverageIncomplete",
            "completion_condition_coverage_incomplete",
            "CompletionConditionCoverageIncomplete",
        ),
        (
            "CompletionSupportOverLimit",
            "completion_support_over_limit",
            "CompletionSupportOverLimit",
        ),
        (
            "CompletionTextOverLimit",
            "completion_text_over_limit",
            "CompletionTextOverLimit",
        ),
        (
            "CompletionSpendReasonRequired",
            "completion_spend_reason_required",
            "CompletionSpendReasonRequired",
        ),
        (
            "CompletionInvalidDetails",
            "completion_invalid_details",
            "CompletionInvalidDetails",
        ),
    ];

    // -------------------------------------------------------------------------------------
    // R42 slice 2, commit 3: the wire contract, pinned so a refactor cannot move it quietly.
    // -------------------------------------------------------------------------------------

    #[derive(Clone, Copy)]
    enum WireExpectation {
        /// A fixed code with no retry.
        Code(&'static str),
        /// "Recurse on the account payload" -- the same thing the production mapper does, but
        /// resolved here through this table's own account rows, never by calling the mapper.
        Account,
    }

    /// Look the expected code up in the transcribed table rather than deriving it.
    fn wire_expectation(key: &str) -> WireExpectation {
        *WIRE_MAP
            .iter()
            .find_map(|(k, v)| (*k == key).then_some(v))
            .unwrap_or_else(|| panic!("R42S2.FIXTURE.WIRE.{key}.MAPPED: no row in WIRE_MAP"))
    }

    /// The whole envelope, built from independent literals -- not from the value under test.
    fn expected_envelope(code: &str, retry: Option<u64>) -> serde_json::Value {
        let retry = match retry {
            Some(ms) => serde_json::Value::from(ms),
            None => serde_json::Value::Null,
        };
        serde_json::json!({
            "protocol": 1,
            "result": serde_json::Value::Null,
            "error": { "code": code, "retry_after_ms": retry },
        })
    }

    fn envelope_of(error: LocalTransportError) -> serde_json::Value {
        serde_json::to_value(response_from_transport_error(error))
            .expect("the response envelope serializes")
    }

    /// The account code for one account variant, from this table's own rows.
    fn account_expectation(variant: &str) -> (&'static str, Option<u64>) {
        match wire_expectation(&format!("Account.{variant}")) {
            WireExpectation::Code(code) => (
                code,
                (variant == "AuthenticationThrottled").then_some(THROTTLE_MS),
            ),
            WireExpectation::Account => {
                unreachable!("the account table never recurses into itself")
            }
        }
    }

    /// R42 slice 2: the wire's closed code table is unchanged, and both former catch-alls now
    /// name their inputs.
    ///
    /// This commit closes no disclosure site. It pins semantics at the next interface, so that
    /// replacing two `_` arms with explicit groups is provably a no-op -- and so a later refactor
    /// cannot change a code or a retry without a named failure here.
    #[test]
    fn r42_wire_projection_preserves_the_closed_contract() {
        assert_eq!(WIRE_MAP.len(), 164, "R42S2.WIRE.CARDINALITY.MAP");
        assert_eq!(CODE_SPELLINGS.len(), 70, "R42S2.WIRE.CARDINALITY.CODES");

        // Lead cases, in the spec's order, so a mutant's first RED is predictable.
        let leads: Vec<(String, LocalTransportError, &'static str, Option<u64>)> = vec![
            (
                "R42S2.WIRE.Account.Store".to_owned(),
                LocalTransportError::Account(OperatorAccountError::Store(driver_error(NEEDLE_A))),
                account_expectation("Store").0,
                None,
            ),
            (
                "R42S2.WIRE.Account.NotAuthenticated".to_owned(),
                LocalTransportError::Account(OperatorAccountError::NotAuthenticated),
                account_expectation("NotAuthenticated").0,
                None,
            ),
            (
                "R42S2.WIRE.Account.AuthenticationThrottled".to_owned(),
                LocalTransportError::Account(OperatorAccountError::AuthenticationThrottled {
                    retry_after_ms: THROTTLE_MS,
                }),
                account_expectation("AuthenticationThrottled").0,
                Some(THROTTLE_MS),
            ),
            (
                "R42S2.WIRE.Io".to_owned(),
                LocalTransportError::Io(io_error(NEEDLE_A)),
                "protocol_error",
                None,
            ),
            (
                "R42S2.WIRE.Project.NotFound".to_owned(),
                LocalTransportError::Project(ProjectError::NotFound),
                match wire_expectation("Project.NotFound") {
                    WireExpectation::Code(code) => code,
                    WireExpectation::Account => unreachable!(),
                },
                None,
            ),
            (
                "R42S2.WIRE.Outcome.Absent".to_owned(),
                LocalTransportError::Outcome(PublishRefusal::Absent),
                match wire_expectation("Outcome.Absent") {
                    WireExpectation::Code(code) => code,
                    WireExpectation::Account => unreachable!(),
                },
                None,
            ),
        ];
        for (id, error, code, retry) in leads {
            let envelope = envelope_of(error);
            assert!(
                !serde_json::to_string(&envelope)
                    .expect("the envelope stringifies")
                    .contains(NEEDLE_A),
                "{id}.NO_NEEDLE: {envelope}"
            );
            assert_eq!(envelope, expected_envelope(code, retry), "{id}.ENVELOPE");
        }

        // The five root groups the first catch-all used to swallow, now named.
        for (id, error) in [
            (
                "R42S2.WIRE.Json",
                LocalTransportError::Json(json_error(NEEDLE_A)),
            ),
            (
                "R42S2.WIRE.InvalidTokenFile",
                LocalTransportError::InvalidTokenFile,
            ),
            (
                "R42S2.WIRE.InsecurePermissions",
                LocalTransportError::InsecurePermissions,
            ),
            ("R42S2.WIRE.Protocol", LocalTransportError::Protocol),
            (
                "R42S2.WIRE.Supervisor",
                LocalTransportError::Supervisor(crate::SupervisorError::SchemaMismatch),
            ),
        ] {
            assert_eq!(
                envelope_of(error),
                expected_envelope("protocol_error", None),
                "{id}.ENVELOPE"
            );
        }

        // Every immediate domain arm, through its own wrapper, against the transcribed table.
        let mut checked = 0_usize;
        for (stem, wrap) in domain_wire_cases() {
            for (variant, error) in wrap {
                let key = format!("{stem}.{variant}");
                let (code, retry) = match wire_expectation(&key) {
                    WireExpectation::Code(code) => (code, None),
                    // A nested Account resolves through the account rows, which is the recursion
                    // the production mapper performs -- checked here, not assumed.
                    WireExpectation::Account => account_expectation("Store"),
                };
                let envelope = envelope_of(error);
                assert!(
                    !serde_json::to_string(&envelope)
                        .expect("the envelope stringifies")
                        .contains(NEEDLE_A),
                    "R42S2.WIRE.{key}.NO_NEEDLE: {envelope}"
                );
                assert_eq!(
                    envelope,
                    expected_envelope(code, retry),
                    "R42S2.WIRE.{key}.ENVELOPE"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 135, "R42S2.WIRE.CARDINALITY.DOMAIN_ARMS");

        // All 18 account faults at the root...
        for (variant, account, _) in account_rows() {
            let (code, retry) = account_expectation(variant);
            assert_eq!(
                envelope_of(LocalTransportError::Account(account)),
                expected_envelope(code, retry),
                "R42S2.WIRE.Account.{variant}.ENVELOPE"
            );
        }

        // ...and NESTED, at every carrier that recurses into them. The committed version wrapped
        // only the root Account while its comment claimed every carrier (astra's read, item 3):
        // a mapper that stopped recursing at one wrapper would have passed.
        macro_rules! nested_account {
            ($stem:literal, $wrap:path, $account:path) => {
                for (variant, account, _) in account_rows() {
                    let (code, retry) = account_expectation(variant);
                    assert_eq!(
                        envelope_of($wrap($account(account))),
                        expected_envelope(code, retry),
                        "R42S2.WIRE.{}.Account.{variant}.ENVELOPE",
                        $stem
                    );
                }
            };
        }
        nested_account!(
            "StarterOrganization",
            LocalTransportError::StarterOrganization,
            StarterOrganizationError::Account
        );
        nested_account!(
            "ResidentLifecycle",
            LocalTransportError::ResidentLifecycle,
            ResidentLifecycleError::Account
        );
        nested_account!(
            "ResidentConfiguration",
            LocalTransportError::ResidentConfiguration,
            ResidentConfigurationError::Account
        );
        nested_account!(
            "Project",
            LocalTransportError::Project,
            ProjectError::Account
        );
        nested_account!(
            "ProjectPolicy",
            LocalTransportError::ProjectPolicy,
            ProjectPolicyError::Account
        );
        nested_account!(
            "ProjectRepository",
            LocalTransportError::ProjectRepository,
            ProjectRepositoryError::Account
        );
        nested_account!(
            "CheckPreflight",
            LocalTransportError::CheckPreflight,
            CheckPreflightError::Account
        );
        nested_account!(
            "RecentReceipt",
            LocalTransportError::RecentReceipt,
            crate::RecentReceiptError::Account
        );
        nested_account!(
            "SampleProject",
            LocalTransportError::SampleProject,
            SampleProjectError::Account
        );
        nested_account!("Team", LocalTransportError::Team, TeamError::Account);
        nested_account!(
            "Mission",
            LocalTransportError::Mission,
            MissionError::Account
        );
        nested_account!(
            "ResourceDiscovery",
            LocalTransportError::ResourceDiscovery,
            ResourceDiscoveryError::Account
        );
        nested_account!(
            "LocalModelActivation",
            LocalTransportError::LocalModelActivation,
            crate::LocalModelActivationError::Account
        );
        nested_account!(
            "Outcome",
            LocalTransportError::Outcome,
            PublishRefusal::Account
        );

        // The Supervisor carrier: every one of the 72 roots is ProtocolError/None regardless of
        // its caller-safe tag, including the ones that carry an account fault of their own.
        for (variant, fault, _) in supervisor_rows() {
            assert_eq!(
                envelope_of(LocalTransportError::Supervisor(fault)),
                expected_envelope("protocol_error", None),
                "R42S2.WIRE.Supervisor.{variant}.ENVELOPE"
            );
        }

        // All 70 Remote codes relay BOTH fields unchanged, including a retry on a code that does
        // not otherwise carry one. Existing behaviour, pinned -- not a new promise.
        for (variant, literal, _) in CODE_SPELLINGS {
            let code = code_rows()
                .into_iter()
                .find_map(|(name, code, _)| (name == variant).then_some(code))
                .expect("every spelling row has a constructor row");
            assert_eq!(
                envelope_of(LocalTransportError::Remote {
                    code,
                    retry_after_ms: Some(THROTTLE_MS),
                }),
                expected_envelope(literal, Some(THROTTLE_MS)),
                "R42S2.WIRE.Remote.{variant}.ENVELOPE"
            );
        }
    }

    /// R42 slice 2: the wire schema and all 70 code spellings are closed.
    #[test]
    fn r42_wire_schema_and_code_spellings_are_closed() {
        for (variant, literal, name) in CODE_SPELLINGS {
            let code = code_rows()
                .into_iter()
                .find_map(|(row, code, _)| (row == variant).then_some(code))
                .expect("every spelling row has a constructor row");
            let value = serde_json::to_value(code).expect("a code serializes");
            assert_eq!(
                value,
                serde_json::Value::from(literal),
                "R42S2.WIRE.CODE.{variant}.SERIALIZED"
            );
            let back: LocalErrorCode = serde_json::from_value(value).expect("a code round trips");
            assert_eq!(
                serde_json::to_value(back).expect("the round trip serializes"),
                serde_json::Value::from(literal),
                "R42S2.WIRE.CODE.{variant}.ROUND_TRIP"
            );
            assert_eq!(
                format!("{code:?}"),
                name,
                "R42S2.WIRE.CODE.{variant}.REMOTE_NAME"
            );
        }

        // The decoder refuses what the schema does not declare. `is_err()` only: a driver
        // sentence is not a contract.
        assert!(
            serde_json::from_str::<WireError>(
                r#"{"code":"protocol_error","retry_after_ms":null,"message":"extra"}"#
            )
            .is_err(),
            "R42S2.WIRE.SCHEMA.WireErrorExtraField.REFUSED"
        );
        assert!(
            serde_json::from_str::<ResponseEnvelope>(
                r#"{"protocol":1,"result":null,"error":null,"message":"extra"}"#
            )
            .is_err(),
            "R42S2.WIRE.SCHEMA.EnvelopeExtraField.REFUSED"
        );
        assert!(
            serde_json::from_str::<WireError>(
                r#"{"code":"a_code_this_protocol_does_not_have","retry_after_ms":null}"#
            )
            .is_err(),
            "R42S2.WIRE.SCHEMA.UnknownCode.REFUSED"
        );
        // The shape that must still parse, so the three refusals above are not vacuous.
        assert!(
            serde_json::from_str::<WireError>(r#"{"code":"protocol_error","retry_after_ms":null}"#)
                .is_ok(),
            "R42S2.WIRE.SCHEMA.WellFormed.ACCEPTED"
        );
    }

    /// Each domain's immediate arms, wrapped in its transport root. The variant name is the row's
    /// own case ID, so the WIRE_MAP key is built from the table rather than from the value.
    #[allow(clippy::type_complexity)]
    fn domain_wire_cases() -> Vec<(&'static str, Vec<(&'static str, LocalTransportError)>)> {
        macro_rules! domain {
            ($stem:literal, $wrap:path, $rows:ident) => {
                (
                    $stem,
                    $rows()
                        .into_iter()
                        .map(|(case, value, _)| (case, $wrap(value)))
                        .collect::<Vec<_>>(),
                )
            };
        }
        vec![
            domain!(
                "StarterOrganization",
                LocalTransportError::StarterOrganization,
                starter_rows
            ),
            domain!(
                "ResidentLifecycle",
                LocalTransportError::ResidentLifecycle,
                lifecycle_rows
            ),
            domain!(
                "ResidentConfiguration",
                LocalTransportError::ResidentConfiguration,
                configuration_rows
            ),
            domain!("Project", LocalTransportError::Project, project_rows),
            domain!(
                "ProjectPolicy",
                LocalTransportError::ProjectPolicy,
                policy_rows
            ),
            domain!(
                "ProjectRepository",
                LocalTransportError::ProjectRepository,
                repository_rows
            ),
            domain!(
                "CheckPreflight",
                LocalTransportError::CheckPreflight,
                preflight_rows
            ),
            domain!(
                "RecentReceipt",
                LocalTransportError::RecentReceipt,
                receipt_rows
            ),
            domain!(
                "SampleProject",
                LocalTransportError::SampleProject,
                sample_rows
            ),
            domain!("Team", LocalTransportError::Team, team_rows),
            domain!("Mission", LocalTransportError::Mission, mission_rows),
            domain!(
                "MissionCompletionCommand",
                LocalTransportError::MissionCompletionCommand,
                completion_command_rows
            ),
            domain!(
                "ResourceDiscovery",
                LocalTransportError::ResourceDiscovery,
                discovery_rows
            ),
            domain!(
                "LocalModelActivation",
                LocalTransportError::LocalModelActivation,
                activation_rows
            ),
            (
                "Outcome",
                publish_rows()
                    .into_iter()
                    .map(|(case, value, _)| {
                        (
                            case.trim_start_matches("R42S2.OUTCOME."),
                            LocalTransportError::Outcome(value),
                        )
                    })
                    .collect::<Vec<_>>(),
            ),
        ]
    }

    // ------------------------------------------------------------------------------------------
    // C4 -- the completion family's parser ceilings, refusal ORDER and closed vocabulary.
    //
    // FOR A COLD READER: written while this seat had no Cargo grant (Opus held the sole grant for
    // the owner build). Nothing below had been compiled or run when it was committed. It is source.
    // ------------------------------------------------------------------------------------------

    struct CompletionScratch(std::path::PathBuf);

    impl Drop for CompletionScratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A bound server plus its paths. The scratch must stay alive for the test's duration.
    fn completion_fixture(
        tag: &str,
    ) -> (
        LocalSupervisorServer,
        std::path::PathBuf,
        std::path::PathBuf,
        CompletionScratch,
    ) {
        let root = std::env::temp_dir().join(format!("beacond-c4-{tag}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let scratch = CompletionScratch(root.clone());
        let socket_path = root.join("beacond.sock");
        let token_path = root.join("beacond.token");
        initialize_local_auth_token(&token_path).unwrap();
        let server = LocalSupervisorServer::bind(LocalServerConfig {
            store_path: root.join("beacon.sqlite3"),
            initial_workspace_name: "Personal".to_owned(),
            socket_path: socket_path.clone(),
            auth_token_path: token_path.clone(),
            lock_path: root.join("beacond.lock"),
            local_model_endpoint: LocalModelEndpoint::ollama_default(),
        })
        .unwrap();
        (server, socket_path, token_path, scratch)
    }

    /// Send BYTES, not a typed command: a size test that goes through the client would be testing
    /// the client's own serializer instead of what the server accepts.
    fn raw_exchange(socket_path: &std::path::Path, body: &[u8]) -> ResponseEnvelope {
        use std::io::{Read as _, Write as _};

        let mut stream = std::os::unix::net::UnixStream::connect(socket_path).unwrap();
        stream.write_all(body).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).unwrap();
        serde_json::from_slice(&answer).expect("the supervisor answers with an envelope")
    }

    /// One valid OLD-family request, padded to `total` bytes through a legal string field.
    fn padded_old_command(token: &str, total: usize) -> Vec<u8> {
        let build = |padding: usize| {
            serde_json::to_vec(&serde_json::json!({
                "protocol": 1,
                "token": token,
                "command": {
                    "name": "inspect_local_model",
                    "model_name": "m".repeat(padding),
                    "expected_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                },
            }))
            .unwrap()
        };
        let empty = build(0).len();
        assert!(total > empty, "the padding target is below the empty body");
        let body = build(total - empty);
        assert_eq!(body.len(), total, "C4.FIXTURE.PADDED_TO_EXACT_LENGTH");
        body
    }

    #[test]
    fn the_larger_request_domain_is_exactly_preview_and_record() {
        assert!(
            accepts_completion_sized_request(&LocalCommand::PreviewMissionCompletion {
                request: completion_request_fixture(),
            }),
            "C4.FAMILY.PREVIEW_IS_IN"
        );
        assert!(
            accepts_completion_sized_request(&LocalCommand::RecordMissionCompletion {
                request: completion_request_fixture(),
                reviewed_intent_sha256: "a".repeat(64),
            }),
            "C4.FAMILY.RECORD_IS_IN"
        );
        // One UUID in, one token or null out: it can never need the larger domain.
        assert!(
            !accepts_completion_sized_request(&LocalCommand::GetMissionCompletionIntent {
                mission_id: Uuid::new_v4(),
            }),
            "C4.FAMILY.GET_INTENT_IS_OUT"
        );
        assert!(
            !accepts_completion_sized_request(&LocalCommand::MissionCompletion {
                mission_id: Uuid::new_v4(),
            }),
            "C4.FAMILY.C3_READ_IS_OUT"
        );
        assert!(
            !accepts_completion_sized_request(&LocalCommand::Status),
            "C4.FAMILY.OLD_COMMAND_IS_OUT"
        );
    }

    #[test]
    fn only_the_preview_response_leaves_the_old_domain() {
        assert_eq!(
            response_max(&LocalCommand::PreviewMissionCompletion {
                request: completion_request_fixture(),
            }),
            MAX_COMPLETION_PREVIEW_RESPONSE_BYTES,
            "C4.RESPONSE.PREVIEW_IS_LARGER"
        );
        assert_eq!(
            response_max(&LocalCommand::RecordMissionCompletion {
                request: completion_request_fixture(),
                reviewed_intent_sha256: "a".repeat(64),
            }),
            MAX_MESSAGE_BYTES,
            "C4.RESPONSE.RECORD_STAYS_OLD"
        );
        assert_eq!(
            response_max(&LocalCommand::GetMissionCompletionIntent {
                mission_id: Uuid::new_v4(),
            }),
            MAX_MESSAGE_BYTES,
            "C4.RESPONSE.GET_INTENT_STAYS_OLD"
        );
        assert_eq!(
            response_max(&LocalCommand::Status),
            MAX_MESSAGE_BYTES,
            "C4.RESPONSE.OLD_COMMAND_STAYS_OLD"
        );
    }

    /// Every wire command states its own response domain, derived from the source rather than from a
    /// hand list -- a hand list would need editing exactly when nobody edits it.
    ///
    /// The needles are assembled with `concat!` so this parser does not match its own source text.
    #[test]
    fn every_command_states_its_response_domain() {
        let source = include_str!("local_transport.rs");
        let arms: Vec<String> = source
            .split(concat!("enum ", "LocalCommand {"))
            .nth(1)
            .expect("the wire vocabulary is an enum in this file")
            .split("\n}")
            .next()
            .expect("the enum closes")
            .lines()
            .map(str::trim)
            .filter(|line| line.ends_with(',') || line.ends_with(" {"))
            .filter(|line| !line.starts_with("//") && !line.starts_with("///"))
            .map(|line| line.trim_end_matches(" {").trim_end_matches(',').to_owned())
            .filter(|name| {
                name.chars().next().is_some_and(char::is_uppercase)
                    && name.chars().all(char::is_alphanumeric)
            })
            .collect();
        assert!(arms.len() >= 55, "C4.RESPONSE.TABLE.ARMS_FOUND: {}", arms.len());

        let table = source
            .split(concat!("fn response", "_max(command: &LocalCommand) -> u64 {"))
            .nth(1)
            .expect("the response table is a function in this file")
            .split("\n}")
            .next()
            .expect("the function closes");
        for arm in &arms {
            assert!(
                table.contains(&format!("LocalCommand::{arm}")),
                "C4.RESPONSE.TABLE.STATES_EVERY_COMMAND: {arm}"
            );
        }
        // Exactly one command may leave the old domain, and it is named here independently.
        let larger = table
            .split(concat!("MAX_COMPLETION_PREVIEW", "_RESPONSE_BYTES"))
            .next()
            .expect("the larger ceiling appears once");
        assert!(
            larger.contains("LocalCommand::PreviewMissionCompletion"),
            "C4.RESPONSE.TABLE.ONLY_PREVIEW_IS_LARGER"
        );
        assert_eq!(
            table.matches(concat!("MAX_COMPLETION_PREVIEW", "_RESPONSE_BYTES")).count(),
            1,
            "C4.RESPONSE.TABLE.LARGER_CEILING_APPEARS_ONCE"
        );
    }

    /// The ceilings cover the grammar, by a conservative bound rather than by an example.
    ///
    /// `J <= 2C + K`, where `C` is the owner's canonical size and `K` is the maximum of `J - 2C` over
    /// the grammar. Adding `b` UTF-8 bytes of text adds at most `2b` to `J` (escaping at most doubles
    /// a scalar) and exactly `8 + b` to `C` (the codec's `u64` length prefix), so `d(J - 2C) <= -16`:
    /// the maximum is reached with EMPTY text placeholders, which exist for the algebra only and are
    /// not a legal record.
    ///
    /// **What this bounds, exactly.** It bounds every OWNER-VALID request. It does NOT bound what
    /// serde would parse: a `Vec` or a `String` has no length limit at the parser, so parser-valid
    /// input is not finitely bounded by the owner's caps at all, and no ceiling derived from them
    /// could claim to be a maximum parser domain. The ceiling is a finite parser ceiling chosen to
    /// COVER the owner-valid domain; a larger body is refused by size, which is the intended answer.
    ///
    /// The literals are measured, not derived here: `tools/c4/opus2/transport/verify-bound.py` and
    /// `bound-certificate.json`. They use `u32::MAX` for every ordinal, which is a deliberate
    /// CONSERVATIVE SUPERSET of the owner-valid input -- the owner requires dense ordinals, so a
    /// legal record's ordinals are at most two digits. Blue's independently proved owner-valid bounds
    /// are 147,219 and 171,539; the 702-byte gap in both totals is exactly that ordinal digit width,
    /// and the wider figure is used here on purpose.
    #[test]
    fn the_completion_ceilings_cover_the_conservative_grammar_bound() {
        // Transcribed from `mission_completion.rs`, not read from it: that constant is private, and
        // a bound that imported its own input would witness nothing.
        const OWNER_CANONICAL_CAP: u64 = 65_536;
        const K_MAX_RESIDUAL: u64 = 16_622;
        const REQUEST_ENVELOPE_OVERHEAD: u64 = 227;
        const PREVIEW_RESPONSE_EXTRA: u64 = 24_462;
        const PREVIEW_RESPONSE_ENVELOPE_OVERHEAD: u64 = 85;

        let request_upper = 2 * OWNER_CANONICAL_CAP + K_MAX_RESIDUAL + REQUEST_ENVELOPE_OVERHEAD;
        assert_eq!(request_upper, 147_921, "C4.BOUND.REQUEST_UPPER_IS_UNCHANGED");
        assert!(
            request_upper <= MAX_COMPLETION_REQUEST_BYTES,
            "C4.BOUND.REQUEST_CEILING_COVERS_THE_GRAMMAR"
        );

        let preview_upper = 2 * OWNER_CANONICAL_CAP
            + K_MAX_RESIDUAL
            + PREVIEW_RESPONSE_EXTRA
            + PREVIEW_RESPONSE_ENVELOPE_OVERHEAD;
        assert_eq!(preview_upper, 172_241, "C4.BOUND.PREVIEW_UPPER_IS_UNCHANGED");
        assert!(
            preview_upper <= MAX_COMPLETION_PREVIEW_RESPONSE_BYTES,
            "C4.BOUND.PREVIEW_CEILING_COVERS_THE_GRAMMAR"
        );

        // The old domain is untouched, and the new ones are strictly above it.
        assert_eq!(MAX_MESSAGE_BYTES, 64 * 1024, "C4.BOUND.OLD_DOMAIN_UNCHANGED");
        assert!(
            MAX_COMPLETION_REQUEST_BYTES > MAX_MESSAGE_BYTES
                && MAX_COMPLETION_PREVIEW_RESPONSE_BYTES > MAX_COMPLETION_REQUEST_BYTES,
            "C4.BOUND.CEILINGS_ARE_ORDERED"
        );
    }

    /// An old command over the old ceiling is refused for its SIZE, before authentication -- which is
    /// where it was refused before this change. The token below is wrong on purpose: if the size
    /// check had moved after the token check, the answer would be `unauthorized_client`.
    #[test]
    fn an_old_command_over_the_old_ceiling_is_refused_before_authentication() {
        let (server, socket_path, _token_path, _scratch) = completion_fixture("old-over");
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let server_thread = thread::spawn(move || server.serve_until(thread_running).unwrap());

        let over = raw_exchange(
            &socket_path,
            &padded_old_command("not-the-real-token", (MAX_MESSAGE_BYTES + 1) as usize),
        );
        assert_eq!(
            over.error.map(|error| error.code),
            Some(LocalErrorCode::ProtocolError),
            "C4.OLD_DOMAIN.OVER_LIMIT_IS_PROTOCOL_BEFORE_AUTH"
        );

        // ... and one byte under it still reaches authentication, so the boundary is the boundary.
        let under = raw_exchange(
            &socket_path,
            &padded_old_command("not-the-real-token", MAX_MESSAGE_BYTES as usize),
        );
        assert_eq!(
            under.error.map(|error| error.code),
            Some(LocalErrorCode::UnauthorizedClient),
            "C4.OLD_DOMAIN.AT_LIMIT_STILL_REACHES_AUTH"
        );

        running.store(false, Ordering::SeqCst);
        let _ = std::os::unix::net::UnixStream::connect(&socket_path);
        server_thread.join().unwrap();
    }

    /// A body over the COMPLETION ceiling is refused by size, whatever it claims to be.
    #[test]
    fn a_body_over_the_completion_ceiling_is_refused_by_size() {
        let (server, socket_path, _token_path, _scratch) = completion_fixture("c4-over");
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let server_thread = thread::spawn(move || server.serve_until(thread_running).unwrap());

        let body = padded_old_command(
            "not-the-real-token",
            (MAX_COMPLETION_REQUEST_BYTES + 1) as usize,
        );
        let answer = raw_exchange(&socket_path, &body);
        assert_eq!(
            answer.error.map(|error| error.code),
            Some(LocalErrorCode::ProtocolError),
            "C4.CEILING.OVER_THE_COMPLETION_CEILING_IS_PROTOCOL"
        );

        running.store(false, Ordering::SeqCst);
        let _ = std::os::unix::net::UnixStream::connect(&socket_path);
        server_thread.join().unwrap();
    }

    /// A malformed body keeps the refusal it had: `Json` under the old ceiling, `Protocol` above it.
    ///
    /// Read through `read_request` directly, because both variants project to the same wire code --
    /// the distinction is internal, and an outward-only test could not see it.
    #[test]
    fn a_malformed_body_keeps_its_refusal_on_each_side_of_the_old_ceiling() {
        use std::io::Write as _;

        let (server, _socket_path, _token_path, _scratch) = completion_fixture("malformed");

        let mut refuse = |length: usize| {
            let (mut ours, mut theirs) = std::os::unix::net::UnixStream::pair().unwrap();
            let body = vec![b'{'; length];
            thread::spawn(move || {
                let _ = theirs.write_all(&body);
                let _ = theirs.shutdown(std::net::Shutdown::Write);
            });
            server.read_request(&mut ours)
        };

        assert!(
            matches!(refuse(64), Err(LocalTransportError::Json(_))),
            "C4.PARSE.UNDER_THE_OLD_CEILING_IS_STILL_JSON"
        );
        assert!(
            matches!(
                refuse((MAX_MESSAGE_BYTES + 1) as usize),
                Err(LocalTransportError::Protocol)
            ),
            "C4.PARSE.OVER_THE_OLD_CEILING_IS_STILL_PROTOCOL"
        );
        assert!(
            matches!(
                refuse((MAX_COMPLETION_REQUEST_BYTES + 1) as usize),
                Err(LocalTransportError::Protocol)
            ),
            "C4.PARSE.OVER_THE_COMPLETION_CEILING_IS_PROTOCOL"
        );
    }

    /// The three commands are registered, reach the owner, and are authenticated like every other
    /// command. This is NOT a claim that a completion round trips: it is a claim that an
    /// unauthenticated caller is refused by the account, not by a missing route.
    #[test]
    fn the_completion_commands_are_registered_and_authenticated() {
        let (server, socket_path, token_path, _scratch) = completion_fixture("auth");
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let server_thread = thread::spawn(move || server.serve_until(thread_running).unwrap());
        let client = LocalSupervisorClient::connect(&socket_path, &token_path).unwrap();

        assert!(
            matches!(
                client.preview_mission_completion(completion_request_fixture()),
                Err(LocalTransportError::Remote {
                    code: LocalErrorCode::NotAuthenticated,
                    ..
                })
            ),
            "C4.ROUTE.PREVIEW_IS_AUTHENTICATED"
        );
        assert!(
            matches!(
                client.record_mission_completion(completion_request_fixture(), "a".repeat(64)),
                Err(LocalTransportError::Remote {
                    code: LocalErrorCode::NotAuthenticated,
                    ..
                })
            ),
            "C4.ROUTE.RECORD_IS_AUTHENTICATED"
        );
        assert!(
            matches!(
                client.get_mission_completion_intent(Uuid::new_v4()),
                Err(LocalTransportError::Remote {
                    code: LocalErrorCode::NotAuthenticated,
                    ..
                })
            ),
            "C4.ROUTE.GET_INTENT_IS_AUTHENTICATED"
        );

        running.store(false, Ordering::SeqCst);
        let _ = std::os::unix::net::UnixStream::connect(&socket_path);
        server_thread.join().unwrap();
    }

    /// The smallest legal request: one condition, nothing optional cited. Used where the CONTENT does
    /// not matter, only that a real DTO crosses the boundary.
    fn completion_request_fixture() -> crate::CompletionIntentRequest {
        crate::CompletionIntentRequest {
            mission_id: Uuid::new_v4(),
            accountable_resident_id: Uuid::new_v4(),
            disposition: "concluded".to_owned(),
            condition_results: vec![crate::ConditionResultDto {
                ordinal: 0,
                outcome: "met".to_owned(),
                support: Vec::new(),
                note: None,
            }],
            delivered_artifacts: Vec::new(),
            unfinished_work: Vec::new(),
            uncertainty: Vec::new(),
            spend: crate::SpendDto::NotMeasured("no meter on this Mission".to_owned()),
            summary: "Shipped it.".to_owned(),
        }
    }

    /// The wire vocabulary is strict in both directions: every field required, `null` explicit, no
    /// unknown field, no unknown command.
    #[test]
    fn the_completion_wire_vocabulary_is_strict() {
        let with_command = |command: serde_json::Value| {
            serde_json::from_value::<LocalCommand>(command)
        };

        let mut request = serde_json::json!({
            "missionId": "11111111-1111-4111-8111-111111111111",
            "accountableResidentId": "22222222-2222-4222-8222-222222222222",
            "disposition": "concluded",
            "conditionResults": [{
                "ordinal": 0,
                "outcome": "met",
                "support": [],
                "note": null,
            }],
            "deliveredArtifacts": [],
            "unfinishedWork": [],
            "uncertainty": [],
            "spend": {"notMeasured": "no meter on this Mission"},
            "summary": "Shipped it.",
        });
        assert!(
            with_command(serde_json::json!({
                "name": "preview_mission_completion",
                "request": request.clone(),
            }))
            .is_ok(),
            "C4.WIRE.A_COMPLETE_REQUEST_IS_ACCEPTED"
        );

        // An ABSENT note is not the same fact as an explicit null, and the owner is told which.
        let mut absent = request.clone();
        absent["conditionResults"][0]
            .as_object_mut()
            .expect("the condition is an object")
            .remove("note");
        assert!(
            with_command(serde_json::json!({
                "name": "preview_mission_completion",
                "request": absent,
            }))
            .is_err(),
            "C4.WIRE.AN_ABSENT_NOTE_IS_REFUSED"
        );

        request["conditionResults"][0]["unexpected"] = serde_json::Value::from(1);
        assert!(
            with_command(serde_json::json!({
                "name": "preview_mission_completion",
                "request": request,
            }))
            .is_err(),
            "C4.WIRE.AN_UNKNOWN_FIELD_IS_REFUSED"
        );

        assert!(
            with_command(serde_json::json!({
                "name": "preview_mission_completion_v2",
                "request": serde_json::Value::Null,
            }))
            .is_err(),
            "C4.WIRE.AN_UNKNOWN_COMMAND_IS_REFUSED"
        );

        // The record leg carries the REVIEWED token, spelled as the contract spells it.
        assert!(
            with_command(serde_json::json!({
                "name": "record_mission_completion",
                "request": serde_json::json!({
                    "missionId": "11111111-1111-4111-8111-111111111111",
                    "accountableResidentId": "22222222-2222-4222-8222-222222222222",
                    "disposition": "concluded",
                    "conditionResults": [],
                    "deliveredArtifacts": [],
                    "unfinishedWork": [],
                    "uncertainty": [],
                    "spend": {"notMeasured": "x"},
                    "summary": "y",
                }),
                "reviewed_intent_sha256": "a".repeat(64),
            }))
            .is_ok(),
            "C4.WIRE.THE_RECORD_LEG_CARRIES_THE_REVIEWED_TOKEN"
        );
        assert!(
            with_command(serde_json::json!({
                "name": "record_mission_completion",
                "request": serde_json::json!({
                    "missionId": "11111111-1111-4111-8111-111111111111",
                    "accountableResidentId": "22222222-2222-4222-8222-222222222222",
                    "disposition": "concluded",
                    "conditionResults": [],
                    "deliveredArtifacts": [],
                    "unfinishedWork": [],
                    "uncertainty": [],
                    "spend": {"notMeasured": "x"},
                    "summary": "y",
                }),
            }))
            .is_err(),
            "C4.WIRE.A_RECORD_WITHOUT_THE_REVIEWED_TOKEN_IS_REFUSED"
        );
    }

    /// The slow deadline still belongs to exactly one command, and the completion family does not
    /// take it. Source-derived, for the same reason as the response table.
    #[test]
    fn only_the_install_command_uses_the_slow_deadline() {
        let source = include_str!("local_transport.rs");
        let needle = concat!("CLIENT_SLOW", "_COMMAND_TIMEOUT");
        let call_sites: Vec<&str> = source
            .lines()
            .filter(|line| line.contains(needle))
            .filter(|line| !line.trim_start().starts_with("//") && !line.trim_start().starts_with("///"))
            .collect();
        // One definition, one call site. Anything else means a second command took the slow budget.
        assert_eq!(
            call_sites.len(),
            2,
            "C4.DEADLINE.SLOW_BUDGET_HAS_ONE_CALLER: {call_sites:?}"
        );
        assert!(
            source
                .split(concat!("fn install_sample", "_project"))
                .nth(1)
                .expect("the install command is in this file")
                .split("\n    }")
                .next()
                .expect("the method closes")
                .contains(needle),
            "C4.DEADLINE.SLOW_BUDGET_BELONGS_TO_INSTALL"
        );
    }
}
