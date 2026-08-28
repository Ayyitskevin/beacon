use std::fmt;
use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod local_model_activation;
mod operator_account;
mod platform_session;
mod project;
mod resource_discovery;
mod starter_organization;

#[cfg(unix)]
mod local_transport;

#[cfg(unix)]
pub use local_transport::{
    LocalErrorCode, LocalServerConfig, LocalSupervisorClient, LocalSupervisorServer,
    LocalTransportError, initialize_local_auth_token,
};

pub use local_model_activation::{
    LocalModelActivation, LocalModelActivationError, LocalModelActivationReview,
    LocalModelActivationService, LocalModelActivationState, LocalModelBinding, ProviderFeature,
};
pub use operator_account::{
    AccessOrigin, OperatorAccountError, OperatorAccountService, OperatorSessionPhase,
    OperatorSessionPolicy, OperatorSessionStatus, PlatformSessionEvent, RecoveryKey, SecretString,
};
pub use platform_session::{
    PlatformMonitorState, PlatformSessionMonitor, PlatformSessionMonitorStatus,
};
pub use project::{
    AutonomyProfile, BudgetBoundary, ExternalEffects, ProjectCharter, ProjectCharterDraft,
    ProjectDraft, ProjectError, ProjectService, ProjectSummary, ProjectView,
};
pub use resource_discovery::{
    DiscoveredLocalModel, DiscoveryStatus, LocalModelDetails, LocalModelDiscoveryReport,
    LocalModelEndpoint, ModelActivation, ModelProvider, ResourceDiscoveryError,
    ResourceDiscoveryService,
};
pub use starter_organization::{
    ResidentAgentSummary, ResidentLifecycle, StarterOrganizationDecision, StarterOrganizationError,
    StarterOrganizationService, StarterOrganizationView, StarterResidentDraft, StarterRole,
    StarterRoleTemplate,
};

const SCHEMA_VERSION: i64 = 7;
const MAX_WORKSPACE_NAME_CHARS: usize = 120;

const SCHEMA_V1: &str = include_str!("../migrations/0001_initial.sql");
const MIGRATION_V2: &str = include_str!("../migrations/0002_operator_account.sql");
const MIGRATION_V3: &str = include_str!("../migrations/0003_session_policy.sql");
const MIGRATION_V4: &str = include_str!("../migrations/0004_starter_organization.sql");
const MIGRATION_V5: &str = include_str!("../migrations/0005_projects.sql");
const MIGRATION_V6: &str = include_str!("../migrations/0006_project_charter_revisions.sql");
const MIGRATION_V7: &str = include_str!("../migrations/0007_local_model_activations.sql");

#[derive(Debug, Clone)]
pub struct StartupConfig {
    pub store_path: PathBuf,
    pub initial_workspace_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
    pub created_at_ms: u64,
    pub created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStatus {
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreDiagnostics {
    pub status: DiagnosticStatus,
    pub schema_version: i64,
    pub integrity_check: String,
    pub foreign_key_violations: u64,
    pub workspace_count: u64,
    pub persisted_event_count: u64,
    pub security_event_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartupEvent {
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartupReport {
    pub store_path: String,
    pub workspace: WorkspaceSummary,
    pub diagnostics: StoreDiagnostics,
    pub events: Vec<StartupEvent>,
}

#[derive(Debug)]
pub enum SupervisorError {
    EmptyWorkspaceName,
    WorkspaceNameTooLong,
    WorkspaceNameContainsControl,
    ClockBeforeUnixEpoch,
    Io(std::io::Error),
    Store(rusqlite::Error),
    Json(serde_json::Error),
    InvalidWorkspaceId(String),
    InvalidEventKind(String),
    EventPayloadScopeMismatch(i64),
    StarterOrganizationEventMismatch(i64),
    StarterOrganizationStateMismatch,
    ProjectEventMismatch(i64),
    ProjectStateMismatch,
    LocalModelActivationEventMismatch(i64),
    LocalModelActivationStateMismatch,
    SchemaMismatch,
    UnsupportedSchemaVersion(i64),
    IntegrityCheckFailed(String),
    ForeignKeyViolations(u64),
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyWorkspaceName => write!(formatter, "workspace name cannot be empty"),
            Self::WorkspaceNameTooLong => write!(
                formatter,
                "workspace name cannot exceed {MAX_WORKSPACE_NAME_CHARS} characters"
            ),
            Self::WorkspaceNameContainsControl => {
                write!(
                    formatter,
                    "workspace name cannot contain control characters"
                )
            }
            Self::ClockBeforeUnixEpoch => {
                write!(formatter, "system clock is before the Unix epoch")
            }
            Self::Io(error) => write!(formatter, "cannot prepare the store directory: {error}"),
            Self::Store(error) => write!(formatter, "store operation failed: {error}"),
            Self::Json(error) => write!(formatter, "cannot encode an event payload: {error}"),
            Self::InvalidWorkspaceId(value) => {
                write!(formatter, "store contains an invalid Workspace id: {value}")
            }
            Self::InvalidEventKind(value) => {
                write!(formatter, "store contains an unknown Event kind: {value}")
            }
            Self::EventPayloadScopeMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} payload does not match its Workspace scope"
            ),
            Self::StarterOrganizationEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Starter Organization state"
            ),
            Self::StarterOrganizationStateMismatch => {
                write!(
                    formatter,
                    "Starter Organization state has no exact audit Event"
                )
            }
            Self::ProjectEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Project creation state"
            ),
            Self::ProjectStateMismatch => {
                write!(formatter, "Project state has no exact creation audit Event")
            }
            Self::LocalModelActivationEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match local-model activation state"
            ),
            Self::LocalModelActivationStateMismatch => write!(
                formatter,
                "local-model activation state has no exact audit Event"
            ),
            Self::SchemaMismatch => {
                write!(
                    formatter,
                    "store schema does not match its declared version"
                )
            }
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "store schema version {version} is unsupported")
            }
            Self::IntegrityCheckFailed(result) => {
                write!(formatter, "SQLite integrity check failed: {result}")
            }
            Self::ForeignKeyViolations(count) => {
                write!(formatter, "store contains {count} foreign-key violation(s)")
            }
        }
    }
}

impl std::error::Error for SupervisorError {}

impl From<std::io::Error> for SupervisorError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for SupervisorError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for SupervisorError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Owns store startup, first-Workspace creation, and the diagnostic report behind one interface.
pub struct Supervisor;

impl Supervisor {
    pub fn bootstrap(config: StartupConfig) -> Result<StartupReport, SupervisorError> {
        let workspace_name = validate_workspace_name(&config.initial_workspace_name)?;

        prepare_parent(&config.store_path)?;
        prepare_store_file(&config.store_path)?;

        let mut connection = Connection::open(&config.store_path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;

        // The immediate transaction prevents two starting supervisors from both creating the
        // first Workspace. Source:
        // https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html#method.transaction_with_behavior
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if has_user_schema(&transaction)? {
            match declared_schema_version(&transaction)? {
                1 => {
                    verify_schema(&transaction, 1)?;
                    transaction.execute_batch(MIGRATION_V2)?;
                    transaction.execute_batch(MIGRATION_V3)?;
                    transaction.execute_batch(MIGRATION_V4)?;
                    transaction.execute_batch(MIGRATION_V5)?;
                    transaction.execute_batch(MIGRATION_V6)?;
                    transaction.execute_batch(MIGRATION_V7)?;
                    verify_schema(&transaction, SCHEMA_VERSION)?;
                }
                2 => {
                    verify_schema(&transaction, 2)?;
                    transaction.execute_batch(MIGRATION_V3)?;
                    transaction.execute_batch(MIGRATION_V4)?;
                    transaction.execute_batch(MIGRATION_V5)?;
                    transaction.execute_batch(MIGRATION_V6)?;
                    transaction.execute_batch(MIGRATION_V7)?;
                    verify_schema(&transaction, SCHEMA_VERSION)?;
                }
                3 => {
                    verify_schema(&transaction, 3)?;
                    transaction.execute_batch(MIGRATION_V4)?;
                    transaction.execute_batch(MIGRATION_V5)?;
                    transaction.execute_batch(MIGRATION_V6)?;
                    transaction.execute_batch(MIGRATION_V7)?;
                    verify_schema(&transaction, SCHEMA_VERSION)?;
                }
                4 => {
                    verify_schema(&transaction, 4)?;
                    transaction.execute_batch(MIGRATION_V5)?;
                    transaction.execute_batch(MIGRATION_V6)?;
                    transaction.execute_batch(MIGRATION_V7)?;
                    verify_schema(&transaction, SCHEMA_VERSION)?;
                }
                5 => {
                    verify_schema(&transaction, 5)?;
                    transaction.execute_batch(MIGRATION_V6)?;
                    transaction.execute_batch(MIGRATION_V7)?;
                    verify_schema(&transaction, SCHEMA_VERSION)?;
                }
                6 => {
                    verify_schema(&transaction, 6)?;
                    transaction.execute_batch(MIGRATION_V7)?;
                    verify_schema(&transaction, SCHEMA_VERSION)?;
                }
                SCHEMA_VERSION => verify_schema(&transaction, SCHEMA_VERSION)?,
                version => return Err(SupervisorError::UnsupportedSchemaVersion(version)),
            }
        } else {
            transaction.execute_batch(SCHEMA_V1)?;
            transaction.execute_batch(MIGRATION_V2)?;
            transaction.execute_batch(MIGRATION_V3)?;
            transaction.execute_batch(MIGRATION_V4)?;
            transaction.execute_batch(MIGRATION_V5)?;
            transaction.execute_batch(MIGRATION_V6)?;
            transaction.execute_batch(MIGRATION_V7)?;
            verify_schema(&transaction, SCHEMA_VERSION)?;
        }

        let existing = transaction
            .query_row(
                "SELECT id, name, created_at_ms
                 FROM workspaces
                 ORDER BY created_at_ms, id
                 LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;

        let (workspace_id, stored_name, created_at_ms, created) = match existing {
            Some((id, name, created_at_ms)) => (
                parse_workspace_id(id)?,
                name,
                nonnegative_u64(created_at_ms)?,
                false,
            ),
            None => {
                let id = Uuid::new_v4();
                let occurred_at_ms = unix_time_ms()?;
                transaction.execute(
                    "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, ?2, ?3)",
                    params![id.to_string(), workspace_name, occurred_at_ms],
                )?;
                let payload = serde_json::to_string(&WorkspaceCreatedPayload {
                    workspace_id: id,
                    name: workspace_name.to_owned(),
                })?;
                transaction.execute(
                    "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        id.to_string(),
                        PersistedEventKind::WorkspaceCreated.as_str(),
                        payload,
                        occurred_at_ms
                    ],
                )?;
                (
                    id,
                    workspace_name.to_owned(),
                    nonnegative_u64(occurred_at_ms)?,
                    true,
                )
            }
        };

        transaction.commit()?;

        let diagnostics = inspect_store(&connection)?;
        let workspace = WorkspaceSummary {
            id: workspace_id,
            name: stored_name,
            created_at_ms,
            created,
        };
        let state = if created { "created" } else { "reused" };
        let events = vec![
            StartupEvent {
                kind: "store.opened".to_owned(),
                detail: config.store_path.display().to_string(),
            },
            StartupEvent {
                kind: format!("workspace.{state}"),
                detail: workspace.id.to_string(),
            },
            StartupEvent {
                kind: "diagnostic.completed".to_owned(),
                detail: "store is ready".to_owned(),
            },
        ];

        Ok(StartupReport {
            store_path: config.store_path.display().to_string(),
            workspace,
            diagnostics,
            events,
        })
    }
}

fn validate_workspace_name(value: &str) -> Result<&str, SupervisorError> {
    if value.chars().any(char::is_control) {
        return Err(SupervisorError::WorkspaceNameContainsControl);
    }
    let value = value.trim();
    if value.is_empty() {
        return Err(SupervisorError::EmptyWorkspaceName);
    }
    if value.chars().count() > MAX_WORKSPACE_NAME_CHARS {
        return Err(SupervisorError::WorkspaceNameTooLong);
    }
    Ok(value)
}

fn prepare_parent(store_path: &Path) -> Result<(), std::io::Error> {
    if let Some(parent) = store_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_store_file(store_path: &Path) -> Result<(), std::io::Error> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options.open(store_path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("store path is not a regular file"));
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn prepare_store_file(_store_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

fn parse_workspace_id(value: String) -> Result<Uuid, SupervisorError> {
    Uuid::parse_str(&value).map_err(|_| SupervisorError::InvalidWorkspaceId(value))
}

#[derive(Debug, PartialEq, Eq)]
struct SchemaObject {
    object_type: String,
    name: String,
    table_name: String,
    sql: Option<String>,
}

fn has_user_schema(connection: &Connection) -> Result<bool, SupervisorError> {
    let exists: bool = connection.query_row(
        "
        SELECT EXISTS (
            SELECT 1
            FROM sqlite_schema
            WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
        )
        ",
        [],
        |row| row.get(0),
    )?;
    Ok(exists)
}

fn declared_schema_version(connection: &Connection) -> Result<i64, SupervisorError> {
    connection
        .query_row(
            "SELECT version FROM schema_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(SupervisorError::Store)
}

fn verify_schema(connection: &Connection, version: i64) -> Result<(), SupervisorError> {
    let expected_connection = Connection::open_in_memory()?;
    expected_connection.execute_batch(SCHEMA_V1)?;
    if version >= 2 {
        expected_connection.execute_batch(MIGRATION_V2)?;
    }
    if version >= 3 {
        expected_connection.execute_batch(MIGRATION_V3)?;
    }
    if version >= 4 {
        expected_connection.execute_batch(MIGRATION_V4)?;
    }
    if version >= 5 {
        expected_connection.execute_batch(MIGRATION_V5)?;
    }
    if version >= 6 {
        expected_connection.execute_batch(MIGRATION_V6)?;
    }
    if version >= 7 {
        expected_connection.execute_batch(MIGRATION_V7)?;
    }
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Err(SupervisorError::UnsupportedSchemaVersion(version));
    }
    let expected = schema_signature(&expected_connection)?;
    let actual = schema_signature(connection)?;
    if actual != expected {
        return Err(SupervisorError::SchemaMismatch);
    }
    Ok(())
}

fn schema_signature(connection: &Connection) -> Result<Vec<SchemaObject>, SupervisorError> {
    let mut statement = connection.prepare(
        "
        SELECT type, name, tbl_name, sql
        FROM sqlite_schema
        WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
        ORDER BY type, name
        ",
    )?;
    let objects = statement
        .query_map([], |row| {
            Ok(SchemaObject {
                object_type: row.get(0)?,
                name: row.get(1)?,
                table_name: row.get(2)?,
                sql: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(objects)
}

fn unix_time_ms() -> Result<i64, SupervisorError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SupervisorError::ClockBeforeUnixEpoch)?;
    i64::try_from(duration.as_millis()).map_err(|_| SupervisorError::ClockBeforeUnixEpoch)
}

fn nonnegative_u64(value: i64) -> Result<u64, SupervisorError> {
    u64::try_from(value)
        .map_err(|_| SupervisorError::Store(rusqlite::Error::IntegralValueOutOfRange(0, value)))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceCreatedPayload {
    workspace_id: Uuid,
    name: String,
}

enum PersistedEventKind {
    WorkspaceCreated,
    StarterOrganizationAccepted,
    StarterOrganizationDiscarded,
    ProjectCreated,
    ProjectCharterRevised,
    LocalModelActivated,
    LocalModelDeactivated,
}

impl PersistedEventKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::WorkspaceCreated => "workspace.created",
            Self::StarterOrganizationAccepted => "starter_organization.accepted",
            Self::StarterOrganizationDiscarded => "starter_organization.discarded",
            Self::ProjectCreated => "project.created",
            Self::ProjectCharterRevised => "project.charter_revised",
            Self::LocalModelActivated => "local_model.activated",
            Self::LocalModelDeactivated => "local_model.deactivated",
        }
    }

    fn parse(value: String) -> Result<Self, SupervisorError> {
        match value.as_str() {
            "workspace.created" => Ok(Self::WorkspaceCreated),
            "starter_organization.accepted" => Ok(Self::StarterOrganizationAccepted),
            "starter_organization.discarded" => Ok(Self::StarterOrganizationDiscarded),
            "project.created" => Ok(Self::ProjectCreated),
            "project.charter_revised" => Ok(Self::ProjectCharterRevised),
            "local_model.activated" => Ok(Self::LocalModelActivated),
            "local_model.deactivated" => Ok(Self::LocalModelDeactivated),
            _ => Err(SupervisorError::InvalidEventKind(value)),
        }
    }
}

fn inspect_store(connection: &Connection) -> Result<StoreDiagnostics, SupervisorError> {
    let schema_version = connection.query_row(
        "SELECT version FROM schema_meta WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if schema_version != SCHEMA_VERSION {
        return Err(SupervisorError::UnsupportedSchemaVersion(schema_version));
    }

    let integrity_check: String =
        connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity_check != "ok" {
        return Err(SupervisorError::IntegrityCheckFailed(integrity_check));
    }

    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut foreign_key_rows = statement.query([])?;
    let mut foreign_key_violations = 0_u64;
    while foreign_key_rows.next()?.is_some() {
        foreign_key_violations += 1;
    }
    if foreign_key_violations > 0 {
        return Err(SupervisorError::ForeignKeyViolations(
            foreign_key_violations,
        ));
    }

    let persisted_event_count = verify_persisted_events(connection)?;
    starter_organization::verify_starter_state(connection)
        .map_err(|_| SupervisorError::StarterOrganizationStateMismatch)?;
    project::verify_project_state(connection).map_err(|_| SupervisorError::ProjectStateMismatch)?;
    local_model_activation::verify_activation_state(connection)
        .map_err(|_| SupervisorError::LocalModelActivationStateMismatch)?;

    Ok(StoreDiagnostics {
        status: DiagnosticStatus::Ready,
        schema_version,
        integrity_check,
        foreign_key_violations,
        workspace_count: count_rows(connection, "workspaces")?,
        persisted_event_count,
        security_event_count: count_rows(connection, "security_events")?,
    })
}

fn verify_persisted_events(connection: &Connection) -> Result<u64, SupervisorError> {
    let mut statement = connection.prepare(
        "
        SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
        FROM events
        ORDER BY sequence
        ",
    )?;
    let mut rows = statement.query([])?;
    let mut count = 0_u64;
    while let Some(row) = rows.next()? {
        let sequence = row.get::<_, i64>(0)?;
        let workspace_id = parse_workspace_id(row.get::<_, String>(1)?)?;
        let kind = PersistedEventKind::parse(row.get::<_, String>(2)?)?;
        let payload_json = row.get::<_, String>(3)?;
        let occurred_at_ms = nonnegative_u64(row.get::<_, i64>(4)?)?;

        match kind {
            PersistedEventKind::WorkspaceCreated => {
                let payload: WorkspaceCreatedPayload = serde_json::from_str(&payload_json)?;
                if payload.workspace_id != workspace_id {
                    return Err(SupervisorError::EventPayloadScopeMismatch(sequence));
                }
                validate_workspace_name(&payload.name)?;
            }
            PersistedEventKind::StarterOrganizationAccepted => {
                starter_organization::verify_starter_event(
                    connection,
                    workspace_id,
                    true,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::StarterOrganizationEventMismatch(sequence))?;
            }
            PersistedEventKind::StarterOrganizationDiscarded => {
                starter_organization::verify_starter_event(
                    connection,
                    workspace_id,
                    false,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::StarterOrganizationEventMismatch(sequence))?;
            }
            PersistedEventKind::ProjectCreated => {
                project::verify_project_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ProjectEventMismatch(sequence))?;
            }
            PersistedEventKind::ProjectCharterRevised => {
                project::verify_project_charter_revised_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ProjectEventMismatch(sequence))?;
            }
            PersistedEventKind::LocalModelActivated => {
                local_model_activation::verify_activation_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    LocalModelActivationState::Active,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::LocalModelActivationEventMismatch(sequence))?;
            }
            PersistedEventKind::LocalModelDeactivated => {
                local_model_activation::verify_activation_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    LocalModelActivationState::Inactive,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::LocalModelActivationEventMismatch(sequence))?;
            }
        }
        count += 1;
    }
    Ok(count)
}

fn count_rows(connection: &Connection, table: &str) -> Result<u64, SupervisorError> {
    let sql = match table {
        "workspaces" => "SELECT count(*) FROM workspaces",
        "security_events" => "SELECT count(*) FROM security_events",
        _ => unreachable!("count_rows is restricted to known tables"),
    };
    let count: i64 = connection.query_row(sql, [], |row| row.get(0))?;
    u64::try_from(count)
        .map_err(|_| SupervisorError::Store(rusqlite::Error::IntegralValueOutOfRange(0, count)))
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use rusqlite::Connection;
    use uuid::Uuid;

    use super::{
        DiagnosticStatus, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4, MIGRATION_V5, MIGRATION_V6,
        SCHEMA_V1, StartupConfig, Supervisor, SupervisorError,
    };

    struct TestStore {
        path: PathBuf,
    }

    impl TestStore {
        fn new(label: &str) -> Self {
            Self {
                path: std::env::temp_dir()
                    .join(format!("beacond-{label}-{}.sqlite3", Uuid::new_v4())),
            }
        }
    }

    impl Drop for TestStore {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = fs::remove_file(format!("{}{suffix}", self.path.display()));
            }
        }
    }

    fn config(path: &Path, workspace_name: &str) -> StartupConfig {
        StartupConfig {
            store_path: path.to_owned(),
            initial_workspace_name: workspace_name.to_owned(),
        }
    }

    #[test]
    fn bootstrap_creates_one_empty_workspace_and_reports_a_healthy_store() {
        let store = TestStore::new("fresh");

        let report = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();

        assert!(report.workspace.created);
        assert_eq!(report.workspace.name, "Personal");
        assert_eq!(report.diagnostics.status, DiagnosticStatus::Ready);
        assert_eq!(report.diagnostics.schema_version, 7);
        assert_eq!(report.diagnostics.integrity_check, "ok");
        assert_eq!(report.diagnostics.foreign_key_violations, 0);
        assert_eq!(report.diagnostics.workspace_count, 1);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        assert_eq!(report.diagnostics.security_event_count, 0);
        assert_eq!(
            report
                .events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["store.opened", "workspace.created", "diagnostic.completed"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_makes_the_store_owner_only() {
        let root = std::env::temp_dir().join(format!("beacon-store-{}", Uuid::new_v4()));
        let store_path = root.join("beacon.sqlite3");

        Supervisor::bootstrap(StartupConfig {
            store_path: store_path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .unwrap();

        assert_eq!(
            fs::metadata(&store_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_reuses_the_workspace_without_replaying_creation() {
        let store = TestStore::new("restart");
        let first = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();

        let second = Supervisor::bootstrap(config(&store.path, "Ignored on restart")).unwrap();

        assert_eq!(second.workspace.id, first.workspace.id);
        assert_eq!(second.workspace.name, "Personal");
        assert!(!second.workspace.created);
        assert_eq!(second.diagnostics.workspace_count, 1);
        assert_eq!(second.diagnostics.persisted_event_count, 1);
        assert_eq!(second.events[1].kind, "workspace.reused");
    }

    #[test]
    fn version_one_store_migrates_transactionally_without_replacing_workspace_state() {
        let store = TestStore::new("version-one-migration");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        let workspace_id = Uuid::new_v4();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": workspace_id,
                        "name": "Existing"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.workspace.name, "Existing");
        assert!(!report.workspace.created);
        assert_eq!(report.diagnostics.schema_version, 7);
        assert_eq!(report.diagnostics.workspace_count, 1);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        assert_eq!(report.diagnostics.security_event_count, 0);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM operator_accounts", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn version_two_store_migrates_account_and_security_history_with_default_policy() {
        let store = TestStore::new("version-two-migration");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection
            .execute(
                "INSERT INTO operator_accounts (
                    singleton, role, password_hash, recovery_key_hash, created_at_ms, updated_at_ms
                 ) VALUES (1, 'owner', ?1, ?2, 7, 7)",
                rusqlite::params!["x".repeat(32), vec![1_u8; 32]],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO security_events (kind, detail_json, occurred_at_ms)
                 VALUES ('operator.enrolled', '{}', 7)",
                [],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();

        assert_eq!(report.diagnostics.schema_version, 7);
        assert_eq!(report.diagnostics.security_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT session_timeout_minutes FROM operator_accounts WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            30
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT kind FROM security_events WHERE sequence = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "operator.enrolled"
        );
    }

    #[test]
    fn version_four_store_adds_projects_without_replacing_existing_state() {
        let store = TestStore::new("version-four-migration");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        let workspace_id = Uuid::new_v4();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": workspace_id,
                        "name": "Existing"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.diagnostics.schema_version, 7);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM projects", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn version_five_store_adds_revision_events_without_replacing_project_state() {
        let store = TestStore::new("version-five-migration");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        connection.execute_batch(MIGRATION_V5).unwrap();
        let workspace_id = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": workspace_id,
                        "name": "Existing"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO projects (
                    id, workspace_id, name, current_charter_revision, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, 'Existing Project', 1, 9, 9)",
                rusqlite::params![project_id.to_string(), workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO project_charter_revisions (
                    workspace_id, project_id, revision, purpose, desired_outcomes_json,
                    constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
                    external_effects, author_kind, authored_at_ms
                 ) VALUES (?1, ?2, 1, 'Preserve this Project.', '[\"Remain intact\"]', '[]',
                           '[]', 'supervised', 'no_spend', 'prohibited', 'owner', 9)",
                rusqlite::params![workspace_id.to_string(), project_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO project_creation_snapshots (
                    project_id, workspace_id, initial_name, initial_charter_revision, created_at_ms
                 ) VALUES (?1, ?2, 'Existing Project', 1, 9)",
                rusqlite::params![project_id.to_string(), workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'project.created', ?2, 9)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "project": {
                            "id": project_id,
                            "workspaceId": workspace_id,
                            "name": "Existing Project",
                            "charter": {
                                "revision": 1,
                                "purpose": "Preserve this Project.",
                                "desiredOutcomes": ["Remain intact"],
                                "constraints": [],
                                "prohibitions": [],
                                "autonomyProfile": "supervised",
                                "budgetBoundary": "no_spend",
                                "externalEffects": "prohibited",
                                "authoredAtMs": 9
                            },
                            "createdAtMs": 9
                        }
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.diagnostics.schema_version, 7);
        assert_eq!(report.diagnostics.persisted_event_count, 2);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT name FROM projects WHERE id = ?1",
                    [project_id.to_string()],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "Existing Project"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM events WHERE kind = 'project.created'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn version_six_store_adds_the_empty_activation_ledger_without_replacing_workspace_state() {
        let store = TestStore::new("version-six-migration");
        let workspace_id = Uuid::new_v4();
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        connection.execute_batch(MIGRATION_V5).unwrap();
        connection.execute_batch(MIGRATION_V6).unwrap();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": workspace_id,
                        "name": "Existing"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.workspace.name, "Existing");
        assert_eq!(report.diagnostics.schema_version, 7);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM local_model_activation_decisions",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn empty_workspace_name_is_refused_before_a_store_is_created() {
        let store = TestStore::new("empty-name");

        let error = Supervisor::bootstrap(config(&store.path, "   ")).unwrap_err();

        assert!(matches!(error, SupervisorError::EmptyWorkspaceName));
        assert!(!store.path.exists());
    }

    #[test]
    fn oversized_or_control_character_names_are_refused_before_store_creation() {
        let oversized_store = TestStore::new("oversized-name");
        let oversized =
            Supervisor::bootstrap(config(&oversized_store.path, &"a".repeat(121))).unwrap_err();
        assert!(matches!(oversized, SupervisorError::WorkspaceNameTooLong));
        assert!(!oversized_store.path.exists());

        let control_store = TestStore::new("control-name");
        let control =
            Supervisor::bootstrap(config(&control_store.path, "Line one\nLine two")).unwrap_err();
        assert!(matches!(
            control,
            SupervisorError::WorkspaceNameContainsControl
        ));
        assert!(!control_store.path.exists());

        let leading_control_store = TestStore::new("leading-control-name");
        let leading_control =
            Supervisor::bootstrap(config(&leading_control_store.path, "\nPersonal")).unwrap_err();
        assert!(matches!(
            leading_control,
            SupervisorError::WorkspaceNameContainsControl
        ));
        assert!(!leading_control_store.path.exists());
    }

    #[test]
    fn injection_shaped_workspace_name_is_stored_as_data() {
        let store = TestStore::new("parameterized-name");
        let name = "Personal'); DROP TABLE events; --";

        let first = Supervisor::bootstrap(config(&store.path, name)).unwrap();
        let second = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(first.workspace.name, name);
        assert_eq!(second.workspace.id, first.workspace.id);
        assert_eq!(second.diagnostics.persisted_event_count, 1);
    }

    #[test]
    fn unsupported_schema_version_fails_closed() {
        let store = TestStore::new("future-schema");
        Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute(
                "UPDATE schema_meta SET version = 99 WHERE singleton = 1",
                [],
            )
            .unwrap();
        drop(connection);

        let error = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap_err();

        assert!(matches!(
            error,
            SupervisorError::UnsupportedSchemaVersion(99)
        ));
    }

    #[test]
    fn same_version_malformed_schema_is_refused_without_mutation() {
        let store = TestStore::new("malformed-schema");
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE schema_meta (
                    singleton INTEGER PRIMARY KEY,
                    version INTEGER NOT NULL
                );
                INSERT INTO schema_meta (singleton, version) VALUES (1, 1);
                CREATE TABLE projects (unexpected_column TEXT);
                ",
            )
            .unwrap();
        drop(connection);

        let result = Supervisor::bootstrap(config(&store.path, "Personal"));

        assert!(result.is_err(), "a malformed version-1 store was accepted");
        let connection = Connection::open(&store.path).unwrap();
        let mut statement = connection
            .prepare(
                "
                SELECT name
                FROM sqlite_schema
                WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                ORDER BY name
                ",
            )
            .unwrap();
        let tables = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tables, ["projects", "schema_meta"]);
    }

    #[test]
    fn unexpected_view_is_refused_without_creating_application_tables() {
        let store = TestStore::new("unexpected-view");
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute("CREATE VIEW unexpected AS SELECT 1 AS value", [])
            .unwrap();
        drop(connection);

        let result = Supervisor::bootstrap(config(&store.path, "Personal"));

        assert!(
            result.is_err(),
            "a view-only store was accepted and mutated"
        );
        let connection = Connection::open(&store.path).unwrap();
        let mut statement = connection
            .prepare(
                "
                SELECT type, name
                FROM sqlite_schema
                WHERE name NOT LIKE 'sqlite_%'
                ORDER BY type, name
                ",
            )
            .unwrap();
        let objects = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(objects, [("view".to_owned(), "unexpected".to_owned())]);
    }

    #[test]
    fn sqlite_internal_prefix_filter_treats_underscore_literally() {
        let store = TestStore::new("sqlite-prefix-wildcard");
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute("CREATE VIEW sqlitexunexpected AS SELECT 1 AS value", [])
            .unwrap();
        drop(connection);

        let result = Supervisor::bootstrap(config(&store.path, "Personal"));

        assert!(
            result.is_err(),
            "a user view matching an unescaped SQLite prefix pattern was accepted"
        );
        let connection = Connection::open(&store.path).unwrap();
        let object_count: i64 = connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM sqlite_schema
                WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
                ",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(object_count, 1, "bootstrap mutated the rejected store");
    }

    #[test]
    fn foreign_key_corruption_is_refused_instead_of_reported_ready() {
        let store = TestStore::new("foreign-key-corruption");
        Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        connection
            .execute(
                "
                INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                VALUES (?1, 'workspace.created', ?2, 0)
                ",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    serde_json::json!({
                        "workspace_id": Uuid::new_v4(),
                        "name": "Orphan"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let result = Supervisor::bootstrap(config(&store.path, "Personal"));

        assert!(
            result.is_err(),
            "a store with a foreign-key violation was reported ready"
        );
        assert!(matches!(
            result,
            Err(SupervisorError::ForeignKeyViolations(1))
        ));
    }

    #[test]
    fn store_rejects_an_event_without_workspace_scope() {
        let store = TestStore::new("unscoped-event");
        Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();

        let result = connection.execute(
            "
            INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
            VALUES (NULL, 'workspace.created', '{}', 0)
            ",
            [],
        );

        assert!(result.is_err(), "an unscoped Event was persisted");
    }

    #[test]
    fn concurrent_bootstrap_creates_exactly_one_workspace_and_event() {
        let store = TestStore::new("concurrent");
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let path = store.path.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    Supervisor::bootstrap(config(&path, "Personal")).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let reports = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            reports
                .iter()
                .filter(|report| report.workspace.created)
                .count(),
            1
        );
        assert_eq!(reports[0].workspace.id, reports[1].workspace.id);
        assert!(
            reports
                .iter()
                .all(|report| report.diagnostics.workspace_count == 1)
        );
        assert!(
            reports
                .iter()
                .all(|report| report.diagnostics.persisted_event_count == 1)
        );
    }

    #[test]
    fn sql_schema_independently_enforces_workspace_name_length() {
        let store = TestStore::new("sql-name-limit");
        Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();

        let result = connection.execute(
            "
            INSERT INTO workspaces (id, name, created_at_ms)
            VALUES (?1, ?2, 0)
            ",
            rusqlite::params![Uuid::new_v4().to_string(), "a".repeat(121)],
        );

        assert!(
            result.is_err(),
            "SQLite accepted an oversized Workspace name"
        );
    }

    #[test]
    fn event_payload_workspace_must_match_its_persisted_scope() {
        let store = TestStore::new("event-scope-mismatch");
        let report = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute(
                "
                INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                VALUES (?1, 'workspace.created', ?2, 0)
                ",
                rusqlite::params![
                    report.workspace.id.to_string(),
                    serde_json::json!({
                        "workspace_id": Uuid::new_v4(),
                        "name": "Wrong scope"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let error = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap_err();

        assert!(matches!(
            error,
            SupervisorError::EventPayloadScopeMismatch(2)
        ));
    }

    #[test]
    fn event_payload_rejects_unknown_fields() {
        let store = TestStore::new("event-unknown-field");
        let report = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute(
                "
                UPDATE events
                SET payload_json = ?1
                WHERE sequence = 1
                ",
                [serde_json::json!({
                    "workspace_id": report.workspace.id,
                    "name": report.workspace.name,
                    "unexpected": true
                })
                .to_string()],
            )
            .unwrap();
        drop(connection);

        let result = Supervisor::bootstrap(config(&store.path, "Personal"));

        assert!(
            result.is_err(),
            "an event payload with an undeclared field was accepted"
        );
    }
}
