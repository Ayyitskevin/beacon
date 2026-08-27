use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 1;
const MAX_WORKSPACE_NAME_CHARS: usize = 120;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");

#[derive(Debug, Clone)]
pub struct StartupConfig {
    pub store_path: PathBuf,
    pub initial_workspace_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
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
    pub project_count: u64,
    pub persisted_event_count: u64,
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
    UnsupportedSchemaVersion(i64),
    IntegrityCheckFailed(String),
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
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "store schema version {version} is unsupported")
            }
            Self::IntegrityCheckFailed(result) => {
                write!(formatter, "SQLite integrity check failed: {result}")
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

        let mut connection = Connection::open(&config.store_path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;

        // The immediate transaction prevents two starting supervisors from both creating the
        // first Workspace. Source:
        // https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html#method.transaction_with_behavior
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute_batch(SCHEMA)?;

        let schema_version: i64 = transaction.query_row(
            "SELECT version FROM schema_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if schema_version != SCHEMA_VERSION {
            return Err(SupervisorError::UnsupportedSchemaVersion(schema_version));
        }

        let existing = transaction
            .query_row(
                "SELECT id, name FROM workspaces ORDER BY created_at_ms, id LIMIT 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        let (workspace_id, stored_name, created) = match existing {
            Some((id, name)) => (parse_workspace_id(id)?, name, false),
            None => {
                let id = Uuid::new_v4();
                let occurred_at_ms = unix_time_ms()?;
                transaction.execute(
                    "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, ?2, ?3)",
                    params![id.to_string(), workspace_name, occurred_at_ms],
                )?;
                let payload = serde_json::to_string(&serde_json::json!({
                    "workspace_id": id,
                    "name": workspace_name,
                }))?;
                transaction.execute(
                    "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                     VALUES (?1, 'workspace.created', ?2, ?3)",
                    params![id.to_string(), payload, occurred_at_ms],
                )?;
                (id, workspace_name.to_owned(), true)
            }
        };

        transaction.commit()?;

        let diagnostics = inspect_store(&connection)?;
        let workspace = WorkspaceSummary {
            id: workspace_id,
            name: stored_name,
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
    let value = value.trim();
    if value.is_empty() {
        return Err(SupervisorError::EmptyWorkspaceName);
    }
    if value.chars().count() > MAX_WORKSPACE_NAME_CHARS {
        return Err(SupervisorError::WorkspaceNameTooLong);
    }
    if value.chars().any(char::is_control) {
        return Err(SupervisorError::WorkspaceNameContainsControl);
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

fn parse_workspace_id(value: String) -> Result<Uuid, SupervisorError> {
    Uuid::parse_str(&value).map_err(|_| SupervisorError::InvalidWorkspaceId(value))
}

fn unix_time_ms() -> Result<i64, SupervisorError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SupervisorError::ClockBeforeUnixEpoch)?;
    i64::try_from(duration.as_millis()).map_err(|_| SupervisorError::ClockBeforeUnixEpoch)
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

    Ok(StoreDiagnostics {
        status: DiagnosticStatus::Ready,
        schema_version,
        integrity_check,
        foreign_key_violations,
        workspace_count: count_rows(connection, "workspaces")?,
        project_count: count_rows(connection, "projects")?,
        persisted_event_count: count_rows(connection, "events")?,
    })
}

fn count_rows(connection: &Connection, table: &str) -> Result<u64, SupervisorError> {
    let sql = match table {
        "workspaces" => "SELECT count(*) FROM workspaces",
        "projects" => "SELECT count(*) FROM projects",
        "events" => "SELECT count(*) FROM events",
        _ => unreachable!("count_rows is restricted to known tables"),
    };
    let count: i64 = connection.query_row(sql, [], |row| row.get(0))?;
    u64::try_from(count)
        .map_err(|_| SupervisorError::Store(rusqlite::Error::IntegralValueOutOfRange(0, count)))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use rusqlite::Connection;
    use uuid::Uuid;

    use super::{DiagnosticStatus, StartupConfig, Supervisor, SupervisorError};

    struct TestStore {
        path: PathBuf,
    }

    impl TestStore {
        fn new(label: &str) -> Self {
            Self {
                path: std::env::temp_dir()
                    .join(format!("mosaicd-{label}-{}.sqlite3", Uuid::new_v4())),
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
        assert_eq!(report.diagnostics.schema_version, 1);
        assert_eq!(report.diagnostics.integrity_check, "ok");
        assert_eq!(report.diagnostics.foreign_key_violations, 0);
        assert_eq!(report.diagnostics.workspace_count, 1);
        assert_eq!(report.diagnostics.project_count, 0);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        assert_eq!(
            report
                .events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["store.opened", "workspace.created", "diagnostic.completed"]
        );
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
    }

    #[test]
    fn injection_shaped_workspace_name_is_stored_as_data() {
        let store = TestStore::new("parameterized-name");
        let name = "Personal'); DROP TABLE projects; --";

        let report = Supervisor::bootstrap(config(&store.path, name)).unwrap();
        let connection = Connection::open(&store.path).unwrap();
        let project_count: i64 = connection
            .query_row("SELECT count(*) FROM projects", [], |row| row.get(0))
            .unwrap();

        assert_eq!(report.workspace.name, name);
        assert_eq!(project_count, 0);
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
}
