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
    CheckPreflightService, MissionDraft, MissionError, MissionService, MissionView,
    OperatorAccountError, OperatorAccountService, OperatorSessionPolicy, OperatorSessionStatus,
    PlatformSessionMonitor, PlatformSessionMonitorStatus, PreflightReceipt, ProjectCharter,
    ProjectCharterDraft, ProjectDraft, ProjectError, ProjectPolicy, ProjectPolicyDraft,
    ProjectPolicyError, ProjectPolicyService, ProjectRepository, ProjectRepositoryError,
    ProjectRepositoryService, ProjectService, ProjectSummary, ProjectView, RecoveryKey,
    RepositoryDraft, ResidentAgentView, ResidentConfiguration, ResidentConfigurationError,
    ResidentConfigurationService, ResidentLifecycleError, ResidentLifecycleService,
    ResourceDiscoveryError, ResourceDiscoveryService, SampleProjectError,
    SampleProjectInstallResult, SampleProjectService, SampleProjectStatus, SecretString,
    StarterOrganizationError, StarterOrganizationService, StarterOrganizationView,
    StarterResidentDraft, StartupConfig, Supervisor, TeamDraft, TeamError, TeamService, TeamView,
};

const PROTOCOL_VERSION: u16 = 1;
const AUTH_TOKEN_BYTES: usize = 32;
const MAX_MESSAGE_BYTES: u64 = 64 * 1024;
const SERVER_REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
// Exact activation review makes three independently bounded provider reads (tags, show, tags).
// Keep transport overhead outside that six-second worst case so a provider error is returned as
// a domain result rather than being mistaken for a broken supervisor socket.
const CLIENT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(7);

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
    ResourceDiscovery(ResourceDiscoveryError),
    LocalModelActivation(crate::LocalModelActivationError),
}

impl fmt::Display for LocalTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "local transport I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "local transport message was invalid: {error}"),
            Self::InvalidTokenFile => write!(formatter, "local transport token is invalid"),
            Self::InsecurePermissions => {
                write!(
                    formatter,
                    "local transport path permissions are not private"
                )
            }
            Self::Protocol => write!(formatter, "local transport protocol mismatch"),
            Self::Remote { code, .. } => write!(formatter, "local supervisor refused: {code:?}"),
            Self::Supervisor(error) => write!(formatter, "supervisor startup failed: {error}"),
            Self::Account(error) => write!(formatter, "account service failed: {error}"),
            Self::StarterOrganization(error) => {
                write!(formatter, "starter organization failed: {error}")
            }
            Self::ResidentLifecycle(error) => {
                write!(formatter, "Resident lifecycle failed: {error}")
            }
            Self::ResidentConfiguration(error) => {
                write!(formatter, "Resident configuration failed: {error}")
            }
            Self::Project(error) => write!(formatter, "Project operation failed: {error}"),
            Self::ProjectPolicy(error) => write!(formatter, "Project Policy failed: {error}"),
            Self::ProjectRepository(error) => {
                write!(formatter, "Project Repository failed: {error}")
            }
            Self::CheckPreflight(error) => write!(formatter, "check preflight failed: {error}"),
            Self::RecentReceipt(error) => write!(formatter, "recent Receipts failed: {error}"),
            Self::SampleProject(error) => write!(formatter, "Sample Project failed: {error}"),
            Self::Team(error) => write!(formatter, "Team operation failed: {error}"),
            Self::Mission(error) => write!(formatter, "Mission operation failed: {error}"),
            Self::ResourceDiscovery(error) => {
                write!(formatter, "resource discovery failed: {error}")
            }
            Self::LocalModelActivation(error) => {
                write!(formatter, "local model activation failed: {error}")
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
    PreflightReceipts {
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
    AdmitMission {
        draft: MissionDraft,
    },
    DiscoverLocalModels,
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
    ProjectRepository {
        repository: Option<ProjectRepository>,
    },
    ProjectRepositoryHistory {
        repositories: Vec<ProjectRepository>,
    },
    PreflightReceipt {
        receipt: PreflightReceipt,
    },
    PreflightReceipts {
        receipts: Vec<PreflightReceipt>,
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
    Mission {
        mission: MissionView,
    },
    LocalModelDiscovery {
        report: crate::LocalModelDiscoveryReport,
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
    resource_discovery: ResourceDiscoveryService,
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
        let resource_discovery =
            ResourceDiscoveryService::new(Arc::clone(&accounts), config.local_model_endpoint);
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
            resource_discovery,
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

    fn read_request(&self, stream: &mut UnixStream) -> Result<LocalCommand, LocalTransportError> {
        let mut bytes = Zeroizing::new(Vec::new());
        stream.take(MAX_MESSAGE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_MESSAGE_BYTES {
            return Err(LocalTransportError::Protocol);
        }
        let request: RequestEnvelope = serde_json::from_slice(&bytes)?;
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
            LocalCommand::PreflightReceipts { project_id } => LocalResult::PreflightReceipts {
                receipts: self.check_preflights.receipts(project_id)?,
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
            LocalCommand::AdmitMission { draft } => LocalResult::Mission {
                mission: self.missions.admit(draft)?,
            },
            LocalCommand::DiscoverLocalModels => LocalResult::LocalModelDiscovery {
                report: self.resource_discovery.discover_local_models()?,
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
        expect_sample_project_install_result(self.send(LocalCommand::InstallSampleProject)?)
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

    pub fn preflight_receipts(
        &self,
        project_id: uuid::Uuid,
    ) -> Result<Vec<PreflightReceipt>, LocalTransportError> {
        expect_preflight_receipts(self.send(LocalCommand::PreflightReceipts { project_id })?)
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
        let token = load_local_auth_token(&self.token_path)?;
        let request = RequestEnvelope {
            protocol: PROTOCOL_VERSION,
            token: WireSecret(token.encode()),
            command,
        };
        let serialized = Zeroizing::new(serde_json::to_vec(&request)?);
        let mut stream = UnixStream::connect(&self.socket_path)?;
        configure_stream_deadlines(&stream, CLIENT_RESPONSE_TIMEOUT)?;
        stream.write_all(&serialized)?;
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut bytes = Zeroizing::new(Vec::new());
        stream.take(MAX_MESSAGE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_MESSAGE_BYTES {
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

fn expect_preflight_receipts(
    result: LocalResult,
) -> Result<Vec<PreflightReceipt>, LocalTransportError> {
    match result {
        LocalResult::PreflightReceipts { receipts } => Ok(receipts),
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
        LocalTransportError::ProjectRepository(error) => wire_error_from_project_repository(error),
        LocalTransportError::Team(error) => wire_error_from_team(error),
        LocalTransportError::CheckPreflight(error) => wire_error_from_check_preflight(error),
        LocalTransportError::RecentReceipt(error) => wire_error_from_recent_receipt(error),
        LocalTransportError::SampleProject(error) => wire_error_from_sample_project(error),
        LocalTransportError::Mission(error) => wire_error_from_mission(error),
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
        _ => WireError {
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
        SampleProjectError::UnsafeDestination
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
        _ => (LocalErrorCode::AccountServiceUnavailable, None),
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
        LocalErrorCode, LocalServerConfig, LocalSupervisorClient, LocalSupervisorServer,
        LocalTransportError, initialize_local_auth_token,
    };
    use crate::{
        LocalModelEndpoint, SampleProjectInstallOutcome, SampleProjectStatus, SecretString,
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
}
