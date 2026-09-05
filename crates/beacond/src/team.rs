use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::resident_configuration::current_configuration;
use crate::{
    OperatorAccountError, OperatorAccountService, ResidentConfigurationError,
    TeamIneligibilityReason,
};

const MAX_TEAM_NAME_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamFormationActor {
    Operator,
}

impl TeamFormationActor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
        }
    }

    fn parse(value: &str) -> Result<Self, TeamError> {
        match value {
            "operator" => Ok(Self::Operator),
            _ => Err(TeamError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamMemberDraft {
    pub resident_id: Uuid,
    pub expected_configuration_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamDraft {
    pub project_id: Uuid,
    pub name: String,
    pub members: Vec<TeamMemberDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamMemberView {
    pub resident_id: Uuid,
    pub formation_configuration_id: Uuid,
    pub current_configuration_id: Uuid,
    pub eligible_now: bool,
    pub ineligibility_reasons: Vec<TeamIneligibilityReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub formed_by: TeamFormationActor,
    pub formed_at_ms: u64,
    pub members: Vec<TeamMemberView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredTeamFormation {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    name: String,
    formed_by: TeamFormationActor,
    formed_at_ms: u64,
    members: Vec<StoredTeamMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredTeamMember {
    resident_id: Uuid,
    formation_configuration_id: Uuid,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TeamFormedPayload {
    team: StoredTeamFormation,
}

/// Which fault the Team startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum TeamStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One Team did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    Row {
        workspace_id: Uuid,
        team_id: Uuid,
        cause: TeamError,
    },
    /// A COUNT across the store that does not match the log: numbers, never an invented row.
    Totals(crate::TotalsMismatch),
    /// A row whose Workspace column is not a UUID. Unreachable while the foreign-key check and
    /// the Workspace pass run first.
    Identity {
        workspace_id: String,
        team_id: String,
    },
}

impl TeamError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::ProjectNotFound => "ProjectNotFound",
            Self::TeamNotFound => "TeamNotFound",
            Self::InvalidName => "InvalidName",
            Self::InvalidMembers => "InvalidMembers",
            Self::ResidentConfigurationUnavailable => "ResidentConfigurationUnavailable",
            Self::StaleResidentConfiguration => "StaleResidentConfiguration",
            Self::ResidentIneligible(_) => "ResidentIneligible",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for TeamStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the Teams could not be read"),
            Self::Row {
                workspace_id,
                team_id,
                cause,
            } => write!(
                formatter,
                "Team {team_id} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Totals(totals) => write!(
                formatter,
                "the Teams do not agree with the log as a whole -- {totals}"
            ),
            Self::Identity {
                workspace_id,
                team_id,
            } => write!(
                formatter,
                "a Team row's Workspace is not readable: {workspace_id}, Team \
                 {team_id}"
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for TeamStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Totals(totals) => formatter.debug_tuple("Totals").field(totals).finish(),
            Self::Row {
                workspace_id,
                team_id,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("team_id", team_id)
                .field("cause", cause)
                .finish(),
            Self::Identity {
                workspace_id,
                team_id,
            } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .field("team_id", team_id)
                .finish(),
        }
    }
}

impl std::error::Error for TeamStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            // A `TotalsMismatch` is DATA, not a nested error.
            Self::Totals(_) | Self::Identity { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum TeamError {
    Account(OperatorAccountError),
    ProjectNotFound,
    TeamNotFound,
    InvalidName,
    InvalidMembers,
    ResidentConfigurationUnavailable,
    StaleResidentConfiguration,
    ResidentIneligible(Uuid),
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for TeamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::ProjectNotFound => write!(formatter, "Project was not found in this Workspace"),
            Self::TeamNotFound => write!(formatter, "Team was not found in this Workspace"),
            Self::InvalidName => write!(formatter, "Team name is invalid"),
            Self::InvalidMembers => {
                write!(formatter, "Team members must be non-empty and distinct")
            }
            Self::ResidentConfigurationUnavailable => {
                write!(formatter, "Resident has no current configuration")
            }
            Self::StaleResidentConfiguration => {
                write!(
                    formatter,
                    "Resident configuration changed before Team formation"
                )
            }
            Self::ResidentIneligible(resident_id) => {
                write!(
                    formatter,
                    "Resident {resident_id} is not currently eligible for a Team"
                )
            }
            Self::InvalidStore => write!(formatter, "Team state is invalid"),
            Self::Store(error) => write!(formatter, "Team store failed: {error}"),
            Self::Json(error) => write!(formatter, "Team event is invalid: {error}"),
        }
    }
}

impl std::error::Error for TeamError {}

impl From<OperatorAccountError> for TeamError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for TeamError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for TeamError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Owns authenticated, Project-scoped Team formation and Team queries.
pub struct TeamService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl TeamService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    pub fn list(&self, project_id: Uuid) -> Result<Vec<TeamView>, TeamError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            require_project(connection, self.workspace_id, project_id)?;
            list_teams(connection, self.workspace_id, project_id)
        })
    }

    pub fn get(&self, team_id: Uuid) -> Result<TeamView, TeamError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let formation = load_stored_team(connection, self.workspace_id, team_id)?
                .ok_or(TeamError::TeamNotFound)?;
            render_team(connection, formation)
        })
    }

    pub fn form(&self, mut draft: TeamDraft) -> Result<TeamView, TeamError> {
        self.accounts.with_authenticated_store(|connection, now| {
            draft.name = normalize_name(draft.name)?;
            validate_members(&draft.members)?;
            let formed_at_ms = u64::try_from(now).map_err(|_| TeamError::InvalidStore)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            require_project(&transaction, self.workspace_id, draft.project_id)?;

            for member in &draft.members {
                let configuration =
                    current_configuration(&transaction, self.workspace_id, member.resident_id)
                        .map_err(map_configuration_error)?
                        .ok_or(TeamError::ResidentConfigurationUnavailable)?;
                if configuration.id != member.expected_configuration_id {
                    return Err(TeamError::StaleResidentConfiguration);
                }
                if !configuration.eligible_for_team {
                    return Err(TeamError::ResidentIneligible(member.resident_id));
                }
            }

            let formation = StoredTeamFormation {
                id: Uuid::new_v4(),
                workspace_id: self.workspace_id,
                project_id: draft.project_id,
                name: draft.name,
                formed_by: TeamFormationActor::Operator,
                formed_at_ms,
                members: draft
                    .members
                    .into_iter()
                    .map(|member| StoredTeamMember {
                        resident_id: member.resident_id,
                        formation_configuration_id: member.expected_configuration_id,
                    })
                    .collect(),
            };
            insert_formation(&transaction, &formation)?;
            let view = render_team(&transaction, formation)?;
            transaction.commit()?;
            Ok(view)
        })
    }
}

fn normalize_name(name: String) -> Result<String, TeamError> {
    let normalized = name.trim();
    if normalized.is_empty()
        || normalized.chars().count() > MAX_TEAM_NAME_CHARS
        || normalized.chars().any(char::is_control)
    {
        return Err(TeamError::InvalidName);
    }
    Ok(normalized.to_owned())
}

fn validate_members(members: &[TeamMemberDraft]) -> Result<(), TeamError> {
    let mut resident_ids = HashSet::with_capacity(members.len());
    if members.is_empty()
        || members
            .iter()
            .any(|member| !resident_ids.insert(member.resident_id))
    {
        return Err(TeamError::InvalidMembers);
    }
    Ok(())
}

fn require_project(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<(), TeamError> {
    let exists = connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM projects WHERE workspace_id = ?1 AND id = ?2
         )",
        params![workspace_id.to_string(), project_id.to_string()],
        |row| row.get::<_, bool>(0),
    )?;
    if !exists {
        return Err(TeamError::ProjectNotFound);
    }
    Ok(())
}

fn insert_formation(
    connection: &Connection,
    formation: &StoredTeamFormation,
) -> Result<(), TeamError> {
    connection.execute(
        "INSERT INTO teams (id, workspace_id, project_id, name, formed_by, formed_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            formation.id.to_string(),
            formation.workspace_id.to_string(),
            formation.project_id.to_string(),
            formation.name,
            formation.formed_by.as_str(),
            i64::try_from(formation.formed_at_ms).map_err(|_| TeamError::InvalidStore)?,
        ],
    )?;
    for (position, member) in formation.members.iter().enumerate() {
        connection.execute(
            "INSERT INTO team_formation_members (
                 workspace_id, team_id, position, resident_id, configuration_id
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                formation.workspace_id.to_string(),
                formation.id.to_string(),
                i64::try_from(position).map_err(|_| TeamError::InvalidStore)?,
                member.resident_id.to_string(),
                member.formation_configuration_id.to_string(),
            ],
        )?;
    }
    let payload = serde_json::to_string(&TeamFormedPayload {
        team: formation.clone(),
    })?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'team.formed', ?2, ?3)",
        params![
            formation.workspace_id.to_string(),
            payload,
            i64::try_from(formation.formed_at_ms).map_err(|_| TeamError::InvalidStore)?,
        ],
    )?;
    Ok(())
}

fn list_teams(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<TeamView>, TeamError> {
    let mut statement = connection.prepare(
        "SELECT id FROM teams
         WHERE workspace_id = ?1 AND project_id = ?2
         ORDER BY formed_at_ms, id",
    )?;
    let ids = statement
        .query_map(
            params![workspace_id.to_string(), project_id.to_string()],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| {
            let id = parse_uuid(id)?;
            let formation =
                load_stored_team(connection, workspace_id, id)?.ok_or(TeamError::InvalidStore)?;
            render_team(connection, formation)
        })
        .collect()
}

fn load_stored_team(
    connection: &Connection,
    workspace_id: Uuid,
    team_id: Uuid,
) -> Result<Option<StoredTeamFormation>, TeamError> {
    let row = connection
        .query_row(
            "SELECT id, workspace_id, project_id, name, formed_by, formed_at_ms
             FROM teams WHERE workspace_id = ?1 AND id = ?2",
            params![workspace_id.to_string(), team_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((id, stored_workspace_id, project_id, name, formed_by, formed_at_ms)) = row else {
        return Ok(None);
    };
    let id = parse_uuid(id)?;
    let stored_workspace_id = parse_uuid(stored_workspace_id)?;
    let mut statement = connection.prepare(
        "SELECT position, resident_id, configuration_id
         FROM team_formation_members
         WHERE workspace_id = ?1 AND team_id = ?2
         ORDER BY position",
    )?;
    let rows = statement
        .query_map(
            params![stored_workspace_id.to_string(), id.to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.is_empty()
        || rows
            .iter()
            .enumerate()
            .any(|(index, (position, _, _))| usize::try_from(*position) != Ok(index))
    {
        return Err(TeamError::InvalidStore);
    }
    let members = rows
        .into_iter()
        .map(|(_, resident_id, configuration_id)| {
            Ok(StoredTeamMember {
                resident_id: parse_uuid(resident_id)?,
                formation_configuration_id: parse_uuid(configuration_id)?,
            })
        })
        .collect::<Result<Vec<_>, TeamError>>()?;
    Ok(Some(StoredTeamFormation {
        id,
        workspace_id: stored_workspace_id,
        project_id: parse_uuid(project_id)?,
        name: normalize_name(name)?,
        formed_by: TeamFormationActor::parse(&formed_by)?,
        formed_at_ms: u64::try_from(formed_at_ms).map_err(|_| TeamError::InvalidStore)?,
        members,
    }))
}

fn render_team(
    connection: &Connection,
    formation: StoredTeamFormation,
) -> Result<TeamView, TeamError> {
    let members = formation
        .members
        .iter()
        .map(|member| {
            let current =
                current_configuration(connection, formation.workspace_id, member.resident_id)
                    .map_err(map_configuration_error)?
                    .ok_or(TeamError::InvalidStore)?;
            Ok(TeamMemberView {
                resident_id: member.resident_id,
                formation_configuration_id: member.formation_configuration_id,
                current_configuration_id: current.id,
                eligible_now: current.eligible_for_team,
                ineligibility_reasons: current.ineligibility_reasons,
            })
        })
        .collect::<Result<Vec<_>, TeamError>>()?;
    Ok(TeamView {
        id: formation.id,
        workspace_id: formation.workspace_id,
        project_id: formation.project_id,
        name: formation.name,
        formed_by: formation.formed_by,
        formed_at_ms: formation.formed_at_ms,
        members,
    })
}

pub(crate) fn load_team_for_mission(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
    team_id: Uuid,
) -> Result<Option<TeamView>, TeamError> {
    let Some(formation) = load_stored_team(connection, workspace_id, team_id)? else {
        return Ok(None);
    };
    if formation.project_id != project_id {
        return Ok(None);
    }
    render_team(connection, formation).map(Some)
}

fn parse_uuid(value: String) -> Result<Uuid, TeamError> {
    Uuid::parse_str(&value).map_err(|_| TeamError::InvalidStore)
}

fn map_configuration_error(error: ResidentConfigurationError) -> TeamError {
    match error {
        ResidentConfigurationError::Store(error) => TeamError::Store(error),
        ResidentConfigurationError::Json(error) => TeamError::Json(error),
        _ => TeamError::InvalidStore,
    }
}

pub(crate) fn verify_team_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), TeamError> {
    let payload: TeamFormedPayload = serde_json::from_str(payload_json)?;
    if payload.team.workspace_id != workspace_id
        || payload.team.formed_at_ms != occurred_at_ms
        || load_stored_team(connection, workspace_id, payload.team.id)? != Some(payload.team)
    {
        return Err(TeamError::InvalidStore);
    }
    Ok(())
}

pub(crate) fn verify_team_state(connection: &Connection) -> Result<(), TeamStateFault> {
    let mut statement = connection
        .prepare("SELECT workspace_id, id FROM teams ORDER BY workspace_id, id")
        .map_err(TeamStateFault::Store)?;
    let ids = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(TeamStateFault::Store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(TeamStateFault::Store)?;
    for (workspace_id, team_id) in &ids {
        let identity = || TeamStateFault::Identity {
            workspace_id: workspace_id.clone(),
            team_id: team_id.clone(),
        };
        let parsed_workspace = Uuid::parse_str(workspace_id).map_err(|_| identity())?;
        let parsed_team = Uuid::parse_str(team_id).map_err(|_| identity())?;
        verify_one_team(connection, parsed_workspace, parsed_team).map_err(|cause| {
            TeamStateFault::Row {
                workspace_id: parsed_workspace,
                team_id: parsed_team,
                cause,
            }
        })?;
    }
    verify_team_totals(connection, ids.len())?;
    Ok(())
}

/// One stored Team, re-proved. Unchanged from what the loop held inline.
fn verify_one_team(
    connection: &Connection,
    workspace_id: Uuid,
    team_id: Uuid,
) -> Result<(), TeamError> {
    {
        let formation =
            load_stored_team(connection, workspace_id, team_id)?.ok_or(TeamError::InvalidStore)?;
        let event_count = connection.query_row(
            "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = 'team.formed'
               AND json_extract(payload_json, '$.team.id') = ?2",
            params![workspace_id.to_string(), formation.id.to_string()],
            |row| row.get::<_, i64>(0),
        )?;
        if event_count != 1 {
            return Err(TeamError::InvalidStore);
        }
    }
    Ok(())
}

/// The count across the store: no row to name.
fn verify_team_totals(connection: &Connection, team_count: usize) -> Result<(), TeamStateFault> {
    let event_count = connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'team.formed'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(TeamStateFault::Store)?;
    if usize::try_from(event_count) != Ok(team_count) {
        return Err(TeamStateFault::Totals(crate::TotalsMismatch {
            table: "events (team.formed)",
            expected: team_count as u64,
            observed: event_count.unsigned_abs(),
        }));
    }
    Ok(())
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()` (the fold at 30d2a20): an exhaustive `name()` catches an
    /// added arm, only a second table catches a wrong or duplicated name.
    fn expected_name(error: &TeamError) -> &'static str {
        match error {
            TeamError::Account(_) => "Account",
            TeamError::ProjectNotFound => "ProjectNotFound",
            TeamError::TeamNotFound => "TeamNotFound",
            TeamError::InvalidName => "InvalidName",
            TeamError::InvalidMembers => "InvalidMembers",
            TeamError::ResidentConfigurationUnavailable => "ResidentConfigurationUnavailable",
            TeamError::StaleResidentConfiguration => "StaleResidentConfiguration",
            TeamError::ResidentIneligible(_) => "ResidentIneligible",
            TeamError::InvalidStore => "InvalidStore",
            TeamError::Store(_) => "Store",
            TeamError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_team_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            TeamError::Account(crate::operator_account::OperatorAccountError::PasswordTooShort),
            TeamError::ProjectNotFound,
            TeamError::TeamNotFound,
            TeamError::InvalidName,
            TeamError::InvalidMembers,
            TeamError::ResidentConfigurationUnavailable,
            TeamError::StaleResidentConfiguration,
            TeamError::ResidentIneligible(Uuid::nil()),
            TeamError::InvalidStore,
            TeamError::Store(rusqlite::Error::QueryReturnedNoRows),
            TeamError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            11,
            "an arm was added to `TeamError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(TeamError::name).collect();
        assert_eq!(
            names.len(),
            11,
            "two arms share a startup name, so a swap between them is invisible: {names:?}"
        );
        for name in names {
            assert!(
                !name.contains(' ') && !name.contains('{'),
                "a name is a name, not a rendering: {name}"
            );
        }
    }
}
