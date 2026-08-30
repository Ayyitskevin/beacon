use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project::current_charter_revision;
use crate::{OperatorAccountError, OperatorAccountService, ProjectError};

const MAX_SOURCE_CHARS: usize = 2000;
const MAX_CHECK_NAME_CHARS: usize = 64;
const BASE_REVISION_CHARS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositorySourceKind {
    LocalPath,
    GitUrl,
}

impl RepositorySourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::LocalPath => "local_path",
            Self::GitUrl => "git_url",
        }
    }

    fn parse(value: &str) -> Result<Self, ProjectRepositoryError> {
        match value {
            "local_path" => Ok(Self::LocalPath),
            "git_url" => Ok(Self::GitUrl),
            _ => Err(ProjectRepositoryError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAuthor {
    Operator,
}

impl RepositoryAuthor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
        }
    }

    fn parse(value: &str) -> Result<Self, ProjectRepositoryError> {
        match value {
            "operator" => Ok(Self::Operator),
            _ => Err(ProjectRepositoryError::InvalidStore),
        }
    }
}

/// A deterministic command the Repository Binding declares. Beacon never runs it here.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeclaredCheck {
    pub name: String,
    pub argv: Vec<String>,
    pub working_directory: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryDraft {
    pub source_kind: RepositorySourceKind,
    pub source: String,
    pub base_revision: String,
    pub declared_checks: Vec<DeclaredCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectRepository {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub revision: u32,
    pub charter_revision: u32,
    pub source_kind: RepositorySourceKind,
    pub source: String,
    pub base_revision: String,
    pub declared_checks: Vec<DeclaredCheck>,
    pub author_kind: RepositoryAuthor,
    pub authored_at_ms: u64,
    pub applies_to_current_charter: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredProjectRepository {
    revision: u32,
    charter_revision: u32,
    source_kind: RepositorySourceKind,
    source: String,
    base_revision: String,
    declared_checks: Vec<DeclaredCheck>,
    author_kind: RepositoryAuthor,
    authored_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BoundProjectReference {
    id: Uuid,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectRepositoryBoundPayload {
    project: BoundProjectReference,
    repository: StoredProjectRepository,
}

/// Why a Repository draft was refused. The wire collapses these to one code; the reason is copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidRepositoryDraft {
    Source,
    BaseRevision,
    DeclaredChecks,
}

impl InvalidRepositoryDraft {
    fn reason(self) -> &'static str {
        match self {
            Self::Source => {
                "a local path source must be absolute without a trailing slash, and a Git URL \
                 source must be an https, ssh, or scp-style Git address"
            }
            Self::BaseRevision => {
                "the pinned base revision must be a lowercase 40-character hexadecimal commit id"
            }
            Self::DeclaredChecks => {
                "declared checks must name distinct commands with a non-empty argument vector and \
                 a repository-relative working directory"
            }
        }
    }
}

#[derive(Debug)]
pub enum ProjectRepositoryError {
    Account(OperatorAccountError),
    ProjectNotFound,
    StaleCharterRevision { expected: u32, current: u32 },
    StaleRepositoryRevision,
    InvalidDraft(InvalidRepositoryDraft),
    UnchangedRepository,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ProjectRepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::ProjectNotFound => write!(formatter, "Project was not found in this Workspace"),
            Self::StaleCharterRevision { expected, current } => write!(
                formatter,
                "Project Charter changed from revision {expected} to {current} before Repository binding"
            ),
            Self::StaleRepositoryRevision => {
                write!(formatter, "Project Repository revision is stale")
            }
            Self::InvalidDraft(reason) => write!(
                formatter,
                "Project Repository binding is invalid: {}",
                reason.reason()
            ),
            Self::UnchangedRepository => {
                write!(formatter, "Project Repository binding is unchanged")
            }
            Self::InvalidStore => write!(formatter, "Project Repository state is invalid"),
            Self::Store(error) => write!(formatter, "Project Repository store failed: {error}"),
            Self::Json(error) => write!(formatter, "Project Repository event is invalid: {error}"),
        }
    }
}

impl std::error::Error for ProjectRepositoryError {}

impl From<OperatorAccountError> for ProjectRepositoryError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for ProjectRepositoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for ProjectRepositoryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Owns authenticated, immutable Project Repository binding and history queries.
///
/// A binding is a declaration. This service never opens, clones, fetches, or reads the named
/// repository, and never runs a declared check.
pub struct ProjectRepositoryService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl ProjectRepositoryService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    pub fn current(
        &self,
        project_id: Uuid,
    ) -> Result<Option<ProjectRepository>, ProjectRepositoryError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let current_charter = require_project(connection, self.workspace_id, project_id)?;
            latest_stored_repository(connection, self.workspace_id, project_id)?
                .map(|repository| {
                    render_repository(
                        repository,
                        self.workspace_id,
                        project_id,
                        current_charter,
                        true,
                    )
                })
                .transpose()
        })
    }

    pub fn history(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<ProjectRepository>, ProjectRepositoryError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let current_charter = require_project(connection, self.workspace_id, project_id)?;
            let repositories = load_repository_history(connection, self.workspace_id, project_id)?;
            let latest_revision = repositories.last().map(|repository| repository.revision);
            repositories
                .into_iter()
                .map(|repository| {
                    let is_latest = Some(repository.revision) == latest_revision;
                    render_repository(
                        repository,
                        self.workspace_id,
                        project_id,
                        current_charter,
                        is_latest,
                    )
                })
                .collect()
        })
    }

    pub fn bind(
        &self,
        project_id: Uuid,
        expected_charter_revision: u32,
        expected_prior_repository_revision: Option<u32>,
        mut draft: RepositoryDraft,
    ) -> Result<ProjectRepository, ProjectRepositoryError> {
        self.accounts.with_authenticated_store(|connection, now| {
            normalize_draft(&mut draft)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current_charter = require_project(&transaction, self.workspace_id, project_id)?;
            if current_charter != expected_charter_revision {
                return Err(ProjectRepositoryError::StaleCharterRevision {
                    expected: expected_charter_revision,
                    current: current_charter,
                });
            }
            let previous = latest_stored_repository(&transaction, self.workspace_id, project_id)?;
            if previous.as_ref().map(|repository| repository.revision)
                != expected_prior_repository_revision
            {
                return Err(ProjectRepositoryError::StaleRepositoryRevision);
            }
            if previous.as_ref().is_some_and(|repository| {
                repository.charter_revision == current_charter
                    && repository.source_kind == draft.source_kind
                    && repository.source == draft.source
                    && repository.base_revision == draft.base_revision
                    && repository.declared_checks == draft.declared_checks
            }) {
                return Err(ProjectRepositoryError::UnchangedRepository);
            }
            let revision = match previous.as_ref() {
                Some(repository) => repository
                    .revision
                    .checked_add(1)
                    .ok_or(ProjectRepositoryError::InvalidStore)?,
                None => 1,
            };
            let authored_at_ms = monotonic_time(now, previous.as_ref())?;
            let repository = StoredProjectRepository {
                revision,
                charter_revision: current_charter,
                source_kind: draft.source_kind,
                source: draft.source,
                base_revision: draft.base_revision,
                declared_checks: draft.declared_checks,
                author_kind: RepositoryAuthor::Operator,
                authored_at_ms,
            };
            insert_repository(&transaction, self.workspace_id, project_id, &repository)?;
            let view = render_repository(
                repository,
                self.workspace_id,
                project_id,
                current_charter,
                true,
            )?;
            transaction.commit()?;
            Ok(view)
        })
    }
}

fn normalize_draft(draft: &mut RepositoryDraft) -> Result<(), ProjectRepositoryError> {
    validate_source(draft.source_kind, &draft.source)?;
    validate_base_revision(&draft.base_revision)?;
    normalize_declared_checks(&mut draft.declared_checks)
}

fn validate_source(
    source_kind: RepositorySourceKind,
    source: &str,
) -> Result<(), ProjectRepositoryError> {
    let length = source.chars().count();
    if length == 0 || length > MAX_SOURCE_CHARS || source.trim() != source {
        return Err(ProjectRepositoryError::InvalidDraft(
            InvalidRepositoryDraft::Source,
        ));
    }
    let accepted = match source_kind {
        RepositorySourceKind::LocalPath => {
            source.starts_with('/') && !source.ends_with('/') && !source.contains('\0')
        }
        RepositorySourceKind::GitUrl => is_git_url(source),
    };
    if accepted {
        Ok(())
    } else {
        Err(ProjectRepositoryError::InvalidDraft(
            InvalidRepositoryDraft::Source,
        ))
    }
}

/// `https://host/path`, `ssh://host/path`, or the scp-style `git@host:path`.
fn is_git_url(source: &str) -> bool {
    if source.contains('\0') || source.contains(char::is_whitespace) {
        return false;
    }
    if let Some(rest) = source.strip_prefix("https://") {
        return !rest.is_empty();
    }
    if let Some(rest) = source.strip_prefix("ssh://") {
        return !rest.is_empty();
    }
    let Some((user_and_host, path)) = source.split_once(':') else {
        return false;
    };
    let Some((user, host)) = user_and_host.split_once('@') else {
        return false;
    };
    !user.is_empty() && !host.is_empty() && !host.contains('/') && !path.is_empty()
}

fn validate_base_revision(base_revision: &str) -> Result<(), ProjectRepositoryError> {
    if base_revision.len() == BASE_REVISION_CHARS
        && base_revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(ProjectRepositoryError::InvalidDraft(
            InvalidRepositoryDraft::BaseRevision,
        ))
    }
}

/// Validates every declared check and sorts them into the canonical order: by name.
fn normalize_declared_checks(
    declared_checks: &mut [DeclaredCheck],
) -> Result<(), ProjectRepositoryError> {
    if declared_checks.is_empty() {
        return Err(ProjectRepositoryError::InvalidDraft(
            InvalidRepositoryDraft::DeclaredChecks,
        ));
    }
    let mut names = HashSet::with_capacity(declared_checks.len());
    for check in declared_checks.iter() {
        if !is_check_name(&check.name)
            || !names.insert(check.name.clone())
            || check.argv.is_empty()
            || check
                .argv
                .iter()
                .any(|argument| argument.is_empty() || argument.contains('\0'))
            || !is_repository_relative(&check.working_directory)
        {
            return Err(ProjectRepositoryError::InvalidDraft(
                InvalidRepositoryDraft::DeclaredChecks,
            ));
        }
    }
    declared_checks.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(())
}

fn is_check_name(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    name.chars().count() <= MAX_CHECK_NAME_CHARS
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '.'
                || character == '_'
                || character == '-'
        })
}

/// `.` or a POSIX path inside the repository: never absolute, never escaping with `..`.
fn is_repository_relative(working_directory: &str) -> bool {
    if working_directory == "." {
        return true;
    }
    if working_directory.is_empty()
        || working_directory.starts_with('/')
        || working_directory.contains('\0')
        || working_directory.contains('\\')
    {
        return false;
    }
    working_directory
        .split('/')
        .all(|component| !component.is_empty() && component != "." && component != "..")
}

/// The parts of a Repository Binding that Check Preflight verifies.
///
/// A narrow projection rather than the full [`ProjectRepository`] view: preflight
/// needs the declaration, not its presentation, and comparing this type is how a
/// Run detects that its binding moved underneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightBinding {
    pub revision: u32,
    pub charter_revision: u32,
    pub source_kind: RepositorySourceKind,
    pub source: String,
    pub base_revision: String,
    pub declared_checks: Vec<DeclaredCheck>,
    pub applies_to_current_charter: bool,
}

impl PreflightBinding {
    fn from_stored(stored: StoredProjectRepository, current_charter: u32) -> Self {
        Self {
            revision: stored.revision,
            charter_revision: stored.charter_revision,
            source_kind: stored.source_kind,
            source: stored.source,
            base_revision: stored.base_revision,
            declared_checks: stored.declared_checks,
            applies_to_current_charter: stored.charter_revision == current_charter,
        }
    }
}

/// The current Charter revision and the latest Repository Binding, if one exists.
pub(crate) fn latest_repository_for_preflight(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<(u32, Option<PreflightBinding>), ProjectRepositoryError> {
    let current_charter = require_project(connection, workspace_id, project_id)?;
    let latest = latest_stored_repository(connection, workspace_id, project_id)?
        .map(|stored| PreflightBinding::from_stored(stored, current_charter));
    Ok((current_charter, latest))
}

/// One stored Repository Binding revision, as preflight compares it.
pub(crate) fn stored_repository_for_preflight(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
    revision: u32,
) -> Result<Option<PreflightBinding>, ProjectRepositoryError> {
    let current_charter = require_project(connection, workspace_id, project_id)?;
    Ok(
        load_stored_repository(connection, workspace_id, project_id, revision)?
            .map(|stored| PreflightBinding::from_stored(stored, current_charter)),
    )
}

fn require_project(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<u32, ProjectRepositoryError> {
    current_charter_revision(connection, workspace_id, project_id)
        .map_err(map_project_error)?
        .ok_or(ProjectRepositoryError::ProjectNotFound)
}

fn insert_repository(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
    repository: &StoredProjectRepository,
) -> Result<(), ProjectRepositoryError> {
    connection.execute(
        "INSERT INTO project_repository_revisions (
             workspace_id, project_id, revision, charter_revision,
             source_kind, source, base_revision, declared_checks_json,
             author_kind, authored_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            workspace_id.to_string(),
            project_id.to_string(),
            i64::from(repository.revision),
            i64::from(repository.charter_revision),
            repository.source_kind.as_str(),
            repository.source.as_str(),
            repository.base_revision.as_str(),
            serde_json::to_string(&repository.declared_checks)?,
            repository.author_kind.as_str(),
            i64::try_from(repository.authored_at_ms)
                .map_err(|_| ProjectRepositoryError::InvalidStore)?,
        ],
    )?;
    let payload = serde_json::to_string(&ProjectRepositoryBoundPayload {
        project: BoundProjectReference { id: project_id },
        repository: repository.clone(),
    })?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'project.repository_bound', ?2, ?3)",
        params![
            workspace_id.to_string(),
            payload,
            i64::try_from(repository.authored_at_ms)
                .map_err(|_| ProjectRepositoryError::InvalidStore)?,
        ],
    )?;
    Ok(())
}

fn latest_stored_repository(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<StoredProjectRepository>, ProjectRepositoryError> {
    let revision = connection
        .query_row(
            "SELECT max(revision) FROM project_repository_revisions
             WHERE workspace_id = ?1 AND project_id = ?2",
            params![workspace_id.to_string(), project_id.to_string()],
            |row| row.get::<_, Option<i64>>(0),
        )?
        .map(positive_u32)
        .transpose()?;
    revision
        .map(|revision| load_stored_repository(connection, workspace_id, project_id, revision))
        .transpose()?
        .flatten()
        .map_or(Ok(None), |repository| Ok(Some(repository)))
}

fn load_repository_history(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<StoredProjectRepository>, ProjectRepositoryError> {
    let mut statement = connection.prepare(
        "SELECT revision FROM project_repository_revisions
         WHERE workspace_id = ?1 AND project_id = ?2 ORDER BY revision",
    )?;
    let revisions = statement
        .query_map(
            params![workspace_id.to_string(), project_id.to_string()],
            |row| row.get::<_, i64>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    revisions
        .into_iter()
        .map(|revision| {
            let revision = positive_u32(revision)?;
            load_stored_repository(connection, workspace_id, project_id, revision)?
                .ok_or(ProjectRepositoryError::InvalidStore)
        })
        .collect()
}

fn load_stored_repository(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
    revision: u32,
) -> Result<Option<StoredProjectRepository>, ProjectRepositoryError> {
    let row = connection
        .query_row(
            "SELECT revision, charter_revision, source_kind, source, base_revision,
                    declared_checks_json, author_kind, authored_at_ms
             FROM project_repository_revisions
             WHERE workspace_id = ?1 AND project_id = ?2 AND revision = ?3",
            params![
                workspace_id.to_string(),
                project_id.to_string(),
                i64::from(revision)
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((
        stored_revision,
        charter_revision,
        source_kind,
        source,
        base_revision,
        declared_checks_json,
        author_kind,
        authored_at_ms,
    )) = row
    else {
        return Ok(None);
    };
    let mut draft = RepositoryDraft {
        source_kind: RepositorySourceKind::parse(&source_kind)?,
        source,
        base_revision,
        declared_checks: serde_json::from_str(&declared_checks_json)?,
    };
    let before = draft.clone();
    normalize_draft(&mut draft).map_err(|_| ProjectRepositoryError::InvalidStore)?;
    if draft != before {
        return Err(ProjectRepositoryError::InvalidStore);
    }
    Ok(Some(StoredProjectRepository {
        revision: positive_u32(stored_revision)?,
        charter_revision: positive_u32(charter_revision)?,
        source_kind: draft.source_kind,
        source: draft.source,
        base_revision: draft.base_revision,
        declared_checks: draft.declared_checks,
        author_kind: RepositoryAuthor::parse(&author_kind)?,
        authored_at_ms: nonnegative_u64(authored_at_ms)?,
    }))
}

fn render_repository(
    repository: StoredProjectRepository,
    workspace_id: Uuid,
    project_id: Uuid,
    current_charter_revision: u32,
    is_latest: bool,
) -> Result<ProjectRepository, ProjectRepositoryError> {
    Ok(ProjectRepository {
        workspace_id,
        project_id,
        revision: repository.revision,
        charter_revision: repository.charter_revision,
        source_kind: repository.source_kind,
        source: repository.source,
        base_revision: repository.base_revision,
        declared_checks: repository.declared_checks,
        author_kind: repository.author_kind,
        authored_at_ms: repository.authored_at_ms,
        applies_to_current_charter: is_latest
            && repository.charter_revision == current_charter_revision,
    })
}

fn monotonic_time(
    now: i64,
    previous: Option<&StoredProjectRepository>,
) -> Result<u64, ProjectRepositoryError> {
    let now = nonnegative_u64(now)?;
    previous.map_or(Ok(now), |repository| {
        Ok(now.max(
            repository
                .authored_at_ms
                .checked_add(1)
                .ok_or(ProjectRepositoryError::InvalidStore)?,
        ))
    })
}

pub(crate) fn verify_project_repository_state(
    connection: &Connection,
) -> Result<(), ProjectRepositoryError> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT workspace_id, project_id FROM project_repository_revisions
         ORDER BY workspace_id, project_id",
    )?;
    let projects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (workspace_id, project_id) in projects {
        let workspace_id = parse_uuid(workspace_id)?;
        let project_id = parse_uuid(project_id)?;
        let repositories = load_repository_history(connection, workspace_id, project_id)?;
        if repositories.iter().enumerate().any(|(index, repository)| {
            usize::try_from(repository.revision) != Ok(index + 1)
                || index > 0 && repositories[index - 1].authored_at_ms > repository.authored_at_ms
        }) {
            return Err(ProjectRepositoryError::InvalidStore);
        }
        for repository in repositories {
            let event_count: i64 = connection.query_row(
                "SELECT count(*) FROM events
                 WHERE workspace_id = ?1 AND kind = 'project.repository_bound'
                   AND json_extract(payload_json, '$.project.id') = ?2
                   AND json_extract(payload_json, '$.repository.revision') = ?3",
                params![
                    workspace_id.to_string(),
                    project_id.to_string(),
                    i64::from(repository.revision)
                ],
                |row| row.get(0),
            )?;
            if event_count != 1 {
                return Err(ProjectRepositoryError::InvalidStore);
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_project_repository_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), ProjectRepositoryError> {
    let payload: ProjectRepositoryBoundPayload = serde_json::from_str(payload_json)?;
    let repository = payload.repository;
    if repository.authored_at_ms != occurred_at_ms {
        return Err(ProjectRepositoryError::InvalidStore);
    }
    let stored = load_stored_repository(
        connection,
        workspace_id,
        payload.project.id,
        repository.revision,
    )?
    .ok_or(ProjectRepositoryError::InvalidStore)?;
    if stored != repository {
        return Err(ProjectRepositoryError::InvalidStore);
    }
    Ok(())
}

fn map_project_error(error: ProjectError) -> ProjectRepositoryError {
    match error {
        ProjectError::Store(error) => ProjectRepositoryError::Store(error),
        ProjectError::Json(error) => ProjectRepositoryError::Json(error),
        _ => ProjectRepositoryError::InvalidStore,
    }
}

fn parse_uuid(value: String) -> Result<Uuid, ProjectRepositoryError> {
    Uuid::parse_str(&value).map_err(|_| ProjectRepositoryError::InvalidStore)
}

fn positive_u32(value: i64) -> Result<u32, ProjectRepositoryError> {
    let value = u32::try_from(value).map_err(|_| ProjectRepositoryError::InvalidStore)?;
    if value == 0 {
        return Err(ProjectRepositoryError::InvalidStore);
    }
    Ok(value)
}

fn nonnegative_u64(value: i64) -> Result<u64, ProjectRepositoryError> {
    u64::try_from(value).map_err(|_| ProjectRepositoryError::InvalidStore)
}
