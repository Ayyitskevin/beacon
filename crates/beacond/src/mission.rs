use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project::current_charter_revision;
use crate::team::load_team_for_mission;
use crate::{OperatorAccountError, OperatorAccountService, ProjectError, TeamError};

const MAX_DESIRED_OUTCOME_CHARS: usize = 2_000;
const MAX_MISSION_ITEMS: usize = 20;
const MAX_MISSION_ITEM_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionAdmissionActor {
    Operator,
}

impl MissionAdmissionActor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
        }
    }

    fn parse(value: &str) -> Result<Self, MissionError> {
        match value {
            "operator" => Ok(Self::Operator),
            _ => Err(MissionError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionLifecycle {
    Admitted,
}

impl MissionLifecycle {
    fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
        }
    }

    fn parse(value: &str) -> Result<Self, MissionError> {
        match value {
            "admitted" => Ok(Self::Admitted),
            _ => Err(MissionError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissionMemberDraft {
    pub resident_id: Uuid,
    pub expected_configuration_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissionDraft {
    pub project_id: Uuid,
    pub expected_charter_revision: u32,
    pub team_id: Uuid,
    pub desired_outcome: String,
    pub success_conditions: Vec<String>,
    pub constraints: Vec<String>,
    pub prohibitions: Vec<String>,
    pub members: Vec<MissionMemberDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissionMemberView {
    pub resident_id: Uuid,
    pub admission_configuration_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissionView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub charter_revision: u32,
    pub team_id: Uuid,
    pub desired_outcome: String,
    pub success_conditions: Vec<String>,
    pub constraints: Vec<String>,
    pub prohibitions: Vec<String>,
    pub lifecycle: MissionLifecycle,
    pub admitted_by: MissionAdmissionActor,
    pub admitted_at_ms: u64,
    pub members: Vec<MissionMemberView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredMissionAdmission {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    charter_revision: u32,
    team_id: Uuid,
    desired_outcome: String,
    success_conditions: Vec<String>,
    constraints: Vec<String>,
    prohibitions: Vec<String>,
    lifecycle: MissionLifecycle,
    admitted_by: MissionAdmissionActor,
    admitted_at_ms: u64,
    members: Vec<MissionMemberView>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MissionAdmittedPayload {
    mission: StoredMissionAdmission,
}

/// Which fault the Mission startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum MissionStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One Mission did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    ///
    /// MEASURED LIMIT, recorded because it bounds what this arm proves: every column of `missions`
    /// is compared against the admission event's payload by `verify_mission_event`, which runs
    /// earlier, so a tampered column reopens as `MissionEventMismatch` and never reaches here. The
    /// faults this pass can uniquely reach -- a wrong admitted-event count, an orphaned condition
    /// row, a condition projection that disagrees with the Mission's own bytes -- are ALL
    /// `MissionError::InvalidStore`. The row identity is therefore the whole of what widening buys
    /// for this pass today, and a "cause flattened" mutant cannot be killed here.
    Row {
        workspace_id: Uuid,
        mission_id: Uuid,
        cause: MissionError,
    },
    /// A check that is about the STORE rather than one Mission: an orphaned condition row, or an
    /// admitted-event count that does not match the number of Missions. There is no row to name,
    /// and naming one with a nil UUID would be the invented identity this lane refuses everywhere
    /// else.
    Totals(crate::TotalsMismatch),
    /// A row whose identity columns are not UUIDs. Unreachable while the foreign-key check and the
    /// Workspace pass run first.
    Identity {
        workspace_id: String,
        mission_id: String,
    },
}

impl MissionError {
    /// The variant's NAME, for R28.4's one-line startup message. `Store`, `Json` and
    /// `ResidentIneligible` all carry values that do not belong in a refusal line.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::ProjectNotFound => "ProjectNotFound",
            Self::TeamNotFound => "TeamNotFound",
            Self::MissionNotFound => "MissionNotFound",
            Self::StaleCharterRevision { .. } => "StaleCharterRevision",
            Self::InvalidDesiredOutcome => "InvalidDesiredOutcome",
            Self::InvalidSuccessConditions => "InvalidSuccessConditions",
            Self::InvalidConstraint => "InvalidConstraint",
            Self::InvalidProhibition => "InvalidProhibition",
            Self::DuplicateMissionEntry => "DuplicateMissionEntry",
            Self::InvalidMembers => "InvalidMembers",
            Self::StaleTeamEligibility => "StaleTeamEligibility",
            Self::ResidentIneligible(_) => "ResidentIneligible",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for MissionStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the Missions could not be read"),
            Self::Totals(totals) => write!(
                formatter,
                "the Missions do not agree with the store as a whole -- {totals}"
            ),
            Self::Row {
                workspace_id,
                mission_id,
                cause,
            } => write!(
                formatter,
                "Mission {mission_id} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Identity {
                workspace_id,
                mission_id,
            } => write!(
                formatter,
                "a Mission row's identity is not readable: Workspace {workspace_id}, Mission \
                 {mission_id}"
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for MissionStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Totals(totals) => formatter.debug_tuple("Totals").field(totals).finish(),
            Self::Row {
                workspace_id,
                mission_id,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("mission_id", mission_id)
                .field("cause", cause)
                .finish(),
            Self::Identity {
                workspace_id,
                mission_id,
            } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .field("mission_id", mission_id)
                .finish(),
        }
    }
}

impl std::error::Error for MissionStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            // A `TotalsMismatch` is DATA, not a nested error: it has no source of its own.
            Self::Totals(_) => None,
            Self::Identity { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum MissionError {
    Account(OperatorAccountError),
    ProjectNotFound,
    TeamNotFound,
    MissionNotFound,
    StaleCharterRevision { expected: u32, current: u32 },
    InvalidDesiredOutcome,
    InvalidSuccessConditions,
    InvalidConstraint,
    InvalidProhibition,
    DuplicateMissionEntry,
    InvalidMembers,
    StaleTeamEligibility,
    ResidentIneligible(Uuid),
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for MissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::ProjectNotFound => write!(formatter, "Project was not found in this Workspace"),
            Self::TeamNotFound => write!(formatter, "Team was not found in this Project"),
            Self::MissionNotFound => write!(formatter, "Mission was not found in this Workspace"),
            Self::StaleCharterRevision { expected, current } => write!(
                formatter,
                "Project Charter changed from revision {expected} to {current} before Mission admission"
            ),
            Self::InvalidDesiredOutcome => write!(formatter, "Mission desired outcome is invalid"),
            Self::InvalidSuccessConditions => write!(
                formatter,
                "Mission needs between 1 and 20 success conditions"
            ),
            Self::InvalidConstraint => write!(formatter, "Mission constraint is invalid"),
            Self::InvalidProhibition => write!(formatter, "Mission prohibition is invalid"),
            Self::DuplicateMissionEntry => {
                write!(
                    formatter,
                    "Mission entries must be distinct within each section"
                )
            }
            Self::InvalidMembers => write!(
                formatter,
                "Mission members must exactly match the Team roster in formation order"
            ),
            Self::StaleTeamEligibility => write!(
                formatter,
                "Team membership or Resident configuration changed before Mission admission"
            ),
            Self::ResidentIneligible(resident_id) => write!(
                formatter,
                "Resident {resident_id} is not currently eligible for Mission admission"
            ),
            Self::InvalidStore => write!(formatter, "Mission state is invalid"),
            Self::Store(error) => write!(formatter, "Mission store failed: {error}"),
            Self::Json(error) => write!(formatter, "Mission event is invalid: {error}"),
        }
    }
}

impl std::error::Error for MissionError {}

impl From<OperatorAccountError> for MissionError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for MissionError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for MissionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Owns authenticated, Project-scoped Mission admission and immutable admission queries.
pub struct MissionService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl MissionService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    pub fn list(&self, project_id: Uuid) -> Result<Vec<MissionView>, MissionError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            require_project(connection, self.workspace_id, project_id)?;
            list_missions(connection, self.workspace_id, project_id)
        })
    }

    pub fn get(&self, mission_id: Uuid) -> Result<MissionView, MissionError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            // The returned admission and its owner Event must come from one snapshot, including
            // the verifier's re-read of the Mission and ordered roster.
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let mission = verified_mission_admission(&transaction, self.workspace_id, mission_id)?
                .ok_or(MissionError::MissionNotFound)?;
            transaction.commit()?;
            Ok(mission)
        })
    }

    pub fn admit(&self, mut draft: MissionDraft) -> Result<MissionView, MissionError> {
        self.accounts.with_authenticated_store(|connection, now| {
            normalize_draft(&mut draft)?;
            validate_member_draft(&draft.members)?;
            let admitted_at_ms = u64::try_from(now).map_err(|_| MissionError::InvalidStore)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

            let current_revision =
                current_charter_revision(&transaction, self.workspace_id, draft.project_id)
                    .map_err(map_project_error)?
                    .ok_or(MissionError::ProjectNotFound)?;
            if current_revision != draft.expected_charter_revision {
                return Err(MissionError::StaleCharterRevision {
                    expected: draft.expected_charter_revision,
                    current: current_revision,
                });
            }

            let team = load_team_for_mission(
                &transaction,
                self.workspace_id,
                draft.project_id,
                draft.team_id,
            )
            .map_err(map_team_error)?
            .ok_or(MissionError::TeamNotFound)?;
            if team.members.len() != draft.members.len()
                || team
                    .members
                    .iter()
                    .zip(&draft.members)
                    .any(|(actual, expected)| {
                        actual.resident_id != expected.resident_id
                            || actual.current_configuration_id != expected.expected_configuration_id
                    })
            {
                return Err(MissionError::StaleTeamEligibility);
            }
            if let Some(member) = team.members.iter().find(|member| !member.eligible_now) {
                return Err(MissionError::ResidentIneligible(member.resident_id));
            }

            let mission = StoredMissionAdmission {
                id: Uuid::new_v4(),
                workspace_id: self.workspace_id,
                project_id: draft.project_id,
                charter_revision: draft.expected_charter_revision,
                team_id: draft.team_id,
                desired_outcome: draft.desired_outcome,
                success_conditions: draft.success_conditions,
                constraints: draft.constraints,
                prohibitions: draft.prohibitions,
                lifecycle: MissionLifecycle::Admitted,
                admitted_by: MissionAdmissionActor::Operator,
                admitted_at_ms,
                members: draft
                    .members
                    .into_iter()
                    .map(|member| MissionMemberView {
                        resident_id: member.resident_id,
                        admission_configuration_id: member.expected_configuration_id,
                    })
                    .collect(),
            };
            insert_admission(&transaction, &mission)?;
            let view = render_mission(mission);
            transaction.commit()?;
            Ok(view)
        })
    }
}

fn normalize_draft(draft: &mut MissionDraft) -> Result<(), MissionError> {
    draft.desired_outcome = normalize_desired_outcome(std::mem::take(&mut draft.desired_outcome))?;
    draft.success_conditions = normalize_items(
        std::mem::take(&mut draft.success_conditions),
        false,
        MissionError::InvalidSuccessConditions,
    )?;
    draft.constraints = normalize_items(
        std::mem::take(&mut draft.constraints),
        true,
        MissionError::InvalidConstraint,
    )?;
    draft.prohibitions = normalize_items(
        std::mem::take(&mut draft.prohibitions),
        true,
        MissionError::InvalidProhibition,
    )?;
    Ok(())
}

fn normalize_desired_outcome(value: String) -> Result<String, MissionError> {
    if value
        .chars()
        .any(|character| character.is_control() && character != '\n')
    {
        return Err(MissionError::InvalidDesiredOutcome);
    }
    let normalized = value.trim();
    if normalized.is_empty() || normalized.chars().count() > MAX_DESIRED_OUTCOME_CHARS {
        return Err(MissionError::InvalidDesiredOutcome);
    }
    Ok(normalized.to_owned())
}

fn normalize_text(
    value: String,
    max_chars: usize,
    invalid: MissionError,
) -> Result<String, MissionError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.chars().count() > max_chars
        || normalized.chars().any(char::is_control)
    {
        return Err(invalid);
    }
    Ok(normalized.to_owned())
}

fn normalize_items(
    values: Vec<String>,
    allow_empty: bool,
    invalid: MissionError,
) -> Result<Vec<String>, MissionError> {
    if values.len() > MAX_MISSION_ITEMS || (!allow_empty && values.is_empty()) {
        return Err(invalid);
    }
    let normalized = values
        .into_iter()
        .map(|value| normalize_text(value, MAX_MISSION_ITEM_CHARS, MissionError::InvalidStore))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid)?;
    let mut distinct = HashSet::with_capacity(normalized.len());
    if normalized.iter().any(|value| !distinct.insert(value)) {
        return Err(MissionError::DuplicateMissionEntry);
    }
    Ok(normalized)
}

fn validate_member_draft(members: &[MissionMemberDraft]) -> Result<(), MissionError> {
    let mut residents = HashSet::with_capacity(members.len());
    if members.is_empty()
        || members
            .iter()
            .any(|member| !residents.insert(member.resident_id))
    {
        return Err(MissionError::InvalidMembers);
    }
    Ok(())
}

fn require_project(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<(), MissionError> {
    if current_charter_revision(connection, workspace_id, project_id)
        .map_err(map_project_error)?
        .is_none()
    {
        return Err(MissionError::ProjectNotFound);
    }
    Ok(())
}

fn insert_admission(
    connection: &Connection,
    mission: &StoredMissionAdmission,
) -> Result<(), MissionError> {
    connection.execute(
        "INSERT INTO missions (
             id, workspace_id, project_id, charter_revision, team_id, desired_outcome,
             success_conditions_json, constraints_json, prohibitions_json, lifecycle,
             admitted_by, admitted_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            mission.id.to_string(),
            mission.workspace_id.to_string(),
            mission.project_id.to_string(),
            i64::from(mission.charter_revision),
            mission.team_id.to_string(),
            mission.desired_outcome,
            serde_json::to_string(&mission.success_conditions)?,
            serde_json::to_string(&mission.constraints)?,
            serde_json::to_string(&mission.prohibitions)?,
            mission.lifecycle.as_str(),
            mission.admitted_by.as_str(),
            i64::try_from(mission.admitted_at_ms).map_err(|_| MissionError::InvalidStore)?,
        ],
    )?;
    for (position, member) in mission.members.iter().enumerate() {
        connection.execute(
            "INSERT INTO mission_admission_members (
                 workspace_id, mission_id, team_id, position, resident_id, configuration_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                mission.workspace_id.to_string(),
                mission.id.to_string(),
                mission.team_id.to_string(),
                i64::try_from(position).map_err(|_| MissionError::InvalidStore)?,
                member.resident_id.to_string(),
                member.admission_configuration_id.to_string(),
            ],
        )?;
    }
    // The declared conditions, given rows in the same transaction that writes the Mission (ADR
    // 0041). The migration's backfill covers the Missions that existed before this table; this
    // covers every Mission after it, and without it the first admission after the migration fails
    // startup reconciliation -- which is how the omission was found, by an existing test, one build
    // after the design's slice-1 list did not mention it.
    for (ordinal, text) in mission.success_conditions.iter().enumerate() {
        connection.execute(
            "INSERT INTO mission_success_conditions (workspace_id, mission_id, ordinal, text)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                mission.workspace_id.to_string(),
                mission.id.to_string(),
                i64::try_from(ordinal).map_err(|_| MissionError::InvalidStore)?,
                text,
            ],
        )?;
    }
    let payload = serde_json::to_string(&MissionAdmittedPayload {
        mission: mission.clone(),
    })?;
    crate::event_chain::append_event(
        connection,
        &mission.workspace_id.to_string(),
        "mission.admitted",
        &payload,
        i64::try_from(mission.admitted_at_ms).map_err(|_| MissionError::InvalidStore)?,
    )?;
    Ok(())
}

fn list_missions(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<MissionView>, MissionError> {
    let mut statement = connection.prepare(
        "SELECT id FROM missions
         WHERE workspace_id = ?1 AND project_id = ?2
         ORDER BY admitted_at_ms, id",
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
            load_stored_mission(connection, workspace_id, id)?
                .map(render_mission)
                .ok_or(MissionError::InvalidStore)
        })
        .collect()
}

fn load_stored_mission(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Option<StoredMissionAdmission>, MissionError> {
    let row = connection
        .query_row(
            "SELECT id, workspace_id, project_id, charter_revision, team_id,
                    desired_outcome, success_conditions_json, constraints_json,
                    prohibitions_json, lifecycle, admitted_by, admitted_at_ms
             FROM missions WHERE workspace_id = ?1 AND id = ?2",
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            },
        )
        .optional()?;
    let Some((
        id,
        stored_workspace_id,
        project_id,
        charter_revision,
        team_id,
        desired_outcome,
        success_conditions_json,
        constraints_json,
        prohibitions_json,
        lifecycle,
        admitted_by,
        admitted_at_ms,
    )) = row
    else {
        return Ok(None);
    };
    let id = parse_uuid(id)?;
    let stored_workspace_id = parse_uuid(stored_workspace_id)?;
    let mut statement = connection.prepare(
        "SELECT position, resident_id, configuration_id
         FROM mission_admission_members
         WHERE workspace_id = ?1 AND mission_id = ?2
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
        return Err(MissionError::InvalidStore);
    }
    let members = rows
        .into_iter()
        .map(|(_, resident_id, configuration_id)| {
            Ok(MissionMemberView {
                resident_id: parse_uuid(resident_id)?,
                admission_configuration_id: parse_uuid(configuration_id)?,
            })
        })
        .collect::<Result<Vec<_>, MissionError>>()?;
    let mut mission = StoredMissionAdmission {
        id,
        workspace_id: stored_workspace_id,
        project_id: parse_uuid(project_id)?,
        charter_revision: u32::try_from(charter_revision)
            .map_err(|_| MissionError::InvalidStore)?,
        team_id: parse_uuid(team_id)?,
        desired_outcome,
        success_conditions: serde_json::from_str(&success_conditions_json)?,
        constraints: serde_json::from_str(&constraints_json)?,
        prohibitions: serde_json::from_str(&prohibitions_json)?,
        lifecycle: MissionLifecycle::parse(&lifecycle)?,
        admitted_by: MissionAdmissionActor::parse(&admitted_by)?,
        admitted_at_ms: u64::try_from(admitted_at_ms).map_err(|_| MissionError::InvalidStore)?,
        members,
    };
    let before = mission.clone();
    normalize_stored_mission(&mut mission)?;
    if mission != before {
        return Err(MissionError::InvalidStore);
    }
    Ok(Some(mission))
}

fn normalize_stored_mission(mission: &mut StoredMissionAdmission) -> Result<(), MissionError> {
    let mut draft = MissionDraft {
        project_id: mission.project_id,
        expected_charter_revision: mission.charter_revision,
        team_id: mission.team_id,
        desired_outcome: std::mem::take(&mut mission.desired_outcome),
        success_conditions: std::mem::take(&mut mission.success_conditions),
        constraints: std::mem::take(&mut mission.constraints),
        prohibitions: std::mem::take(&mut mission.prohibitions),
        members: mission
            .members
            .iter()
            .map(|member| MissionMemberDraft {
                resident_id: member.resident_id,
                expected_configuration_id: member.admission_configuration_id,
            })
            .collect(),
    };
    normalize_draft(&mut draft)?;
    validate_member_draft(&draft.members)?;
    mission.desired_outcome = draft.desired_outcome;
    mission.success_conditions = draft.success_conditions;
    mission.constraints = draft.constraints;
    mission.prohibitions = draft.prohibitions;
    Ok(())
}

fn render_mission(mission: StoredMissionAdmission) -> MissionView {
    MissionView {
        id: mission.id,
        workspace_id: mission.workspace_id,
        project_id: mission.project_id,
        charter_revision: mission.charter_revision,
        team_id: mission.team_id,
        desired_outcome: mission.desired_outcome,
        success_conditions: mission.success_conditions,
        constraints: mission.constraints,
        prohibitions: mission.prohibitions,
        lifecycle: mission.lifecycle,
        admitted_by: mission.admitted_by,
        admitted_at_ms: mission.admitted_at_ms,
        members: mission.members,
    }
}

fn parse_uuid(value: String) -> Result<Uuid, MissionError> {
    Uuid::parse_str(&value).map_err(|_| MissionError::InvalidStore)
}

fn map_project_error(error: ProjectError) -> MissionError {
    match error {
        ProjectError::Store(error) => MissionError::Store(error),
        ProjectError::Json(error) => MissionError::Json(error),
        _ => MissionError::InvalidStore,
    }
}

fn map_team_error(error: TeamError) -> MissionError {
    match error {
        TeamError::Store(error) => MissionError::Store(error),
        TeamError::Json(error) => MissionError::Json(error),
        _ => MissionError::InvalidStore,
    }
}

/// What one admitted Mission tells a caller that must bind to it: the Project it
/// belongs to, and the immutable admission Configuration recorded for one Resident —
/// `None` when that Resident is not on this Mission's admission roster.
///
/// A Proposal names Project, Mission, Resident, and Configuration redundantly so a
/// cross-Project or cross-Resident substitution is detectable without following one
/// identity to discover the others. This is one bounded read rather than four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MissionAdmissionFacts {
    pub(crate) project_id: Uuid,
    pub(crate) member_configuration_id: Option<Uuid>,
}

pub(crate) fn mission_admission_facts(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
    resident_id: Uuid,
) -> Result<Option<MissionAdmissionFacts>, MissionError> {
    let Some(project_id) = connection
        .query_row(
            "SELECT project_id FROM missions WHERE workspace_id = ?1 AND id = ?2",
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let member_configuration_id = connection
        .query_row(
            "SELECT configuration_id FROM mission_admission_members
             WHERE workspace_id = ?1 AND mission_id = ?2 AND resident_id = ?3",
            params![
                workspace_id.to_string(),
                mission_id.to_string(),
                resident_id.to_string()
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(parse_uuid)
        .transpose()?;
    Ok(Some(MissionAdmissionFacts {
        project_id: parse_uuid(project_id)?,
        member_configuration_id,
    }))
}

/// The Project a Mission belongs to, proved rather than read off a column.
///
/// Startup already re-proves every Mission against its own `mission.admitted` Event; this is the
/// same proof, on demand, for a reader that must not take `missions.project_id` on trust. It finds
/// the one Event that admitted this Mission, runs the same whole-payload comparison
/// `verify_mission_event` runs, and only then returns the Project the verified record names.
///
/// `Ok(None)` means this Workspace admitted no such Mission -- a different fact from a Mission whose
/// record and Event disagree, which is `InvalidStore`.
///
/// Added for sol1's #5: `mission_completions` has no composite key binding a Mission to a Project,
/// so a completion could name Mission A and Project B and satisfy the schema. The completion reader
/// asks this instead of joining.
pub(crate) fn verified_mission_project(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Option<Uuid>, MissionError> {
    Ok(
        verified_mission_declaration(connection, workspace_id, mission_id)?
            .map(|declaration| declaration.project_id),
    )
}

/// What a Mission declared, proved against its own admission Event.
///
/// The Outcome Document prints the Mission's title and each declared condition's statement, and
/// gpt2 §2 requires that text to come through the owner's verified reader rather than off a column.
/// `missions.desired_outcome` and `missions.success_conditions_json` are columns; this is the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MissionDeclaration {
    pub(crate) project_id: Uuid,
    pub(crate) desired_outcome: String,
    pub(crate) success_conditions: Vec<String>,
}

/// The Mission's declaration, proved the way startup proves it: the stored row must equal the
/// `mission.admitted` Event payload that recorded it, whole. `Ok(None)` means this Workspace admitted
/// no such Mission; a row and Event that disagree are `InvalidStore`.
pub(crate) fn verified_mission_declaration(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Option<MissionDeclaration>, MissionError> {
    // The projection, of the one proof. The whole-payload comparison lives in
    // `verified_mission_admission` and is not repeated here: two copies of an owner validator are
    // two places to be wrong about what a Mission declared, and the second copy is the one nobody
    // updates.
    Ok(
        verified_mission_admission(connection, workspace_id, mission_id)?.map(|mission| {
            MissionDeclaration {
                project_id: mission.project_id,
                desired_outcome: mission.desired_outcome,
                success_conditions: mission.success_conditions,
            }
        }),
    )
}

/// The COMPLETE admitted Mission, proved the way startup proves it.
///
/// The stored row must equal the one `mission.admitted` Event payload that recorded it, whole --
/// identities, Project and charter revision, Team, desired outcome, success conditions, constraints,
/// prohibitions, lifecycle, admitting actor and time, and the ordered admission roster with each
/// member's admission Configuration. Callers that must bind to what a Mission ADMITTED read this
/// rather than the columns.
///
/// `Ok(None)` means this Workspace admitted no such Mission -- a different fact from a Mission whose
/// row and Event disagree, which is `InvalidStore`. Exactly one Event must match: none is a Mission
/// the log does not account for, and two would make "when was this Mission admitted" a question with
/// two answers.
///
/// **What this proves, and what it does not.** It proves what was admitted. It says nothing about
/// whether the named Residents are still active, whether their Configurations are still current,
/// whether the activation behind them is still live, or whether anything is authorized to run a
/// model. Those are separate owners, and a caller that needs them must ask them; deriving a current
/// value from this admitted one would be rewriting history into eligibility.
///
/// **Snapshot expectations.** It reads the Mission row and its admission roster, then the matching
/// `mission.admitted` Event, then re-reads the Mission through `verify_mission_event` -- several
/// SQL statements, not two -- on the `connection` it is handed, and opens no transaction of its
/// own. A caller inside a transaction sees that transaction's snapshot; a caller outside one gets
/// no atomicity guarantee across those statements beyond what its connection already provides. It
/// is deliberately not nested inside the existing Outcome and Completion callers' transactions in
/// any new way.
pub(crate) fn verified_mission_admission(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Option<MissionView>, MissionError> {
    let Some(stored) = load_stored_mission(connection, workspace_id, mission_id)? else {
        return Ok(None);
    };
    let mut statement = connection.prepare(
        "SELECT payload_json, occurred_at_ms FROM events
         WHERE workspace_id = ?1 AND kind = 'mission.admitted'
           AND json_extract(payload_json, '$.mission.id') = ?2",
    )?;
    let events: Vec<(String, i64)> = statement
        .query_map(
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    // Exactly one. None is a Mission the log does not account for; two would make "when was this
    // Mission admitted" a question with two answers.
    let [(payload_json, occurred_at_ms)] = events.as_slice() else {
        return Err(MissionError::InvalidStore);
    };
    let occurred_at_ms = u64::try_from(*occurred_at_ms).map_err(|_| MissionError::InvalidStore)?;
    verify_mission_event(connection, workspace_id, occurred_at_ms, payload_json)?;
    Ok(Some(render_mission(stored)))
}

pub(crate) fn verify_mission_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), MissionError> {
    let payload: MissionAdmittedPayload = serde_json::from_str(payload_json)?;
    if payload.mission.workspace_id != workspace_id
        || payload.mission.admitted_at_ms != occurred_at_ms
        || load_stored_mission(connection, workspace_id, payload.mission.id)?
            != Some(payload.mission)
    {
        return Err(MissionError::InvalidStore);
    }
    Ok(())
}

/// Re-derive `mission_success_conditions` from the Mission's own bytes, and refuse on disagreement.
///
/// The table is a projection of `missions.success_conditions_json`, and a projection that can drift
/// from its source is a second place to be wrong about what a Mission declared. There is no store
/// trigger comparing an inserted row against the JSON, deliberately: it would make a disagreeing row
/// unrepresentable and this check dead code, and a check that cannot fail is a sentence rather than
/// a guard. So the guard is here, and its tests write the bad rows directly.
///
/// **Two checks, not one.** The count and the text answer different questions and are separated so
/// neither can hide the other: a row whose text was rewritten leaves the count correct, and a row
/// appended past the end of the array leaves every text correct. Deriving both from one comparison
/// -- rebuild the expected list, compare the whole set -- would pass both scenarios through one
/// assertion, and a single mutant would kill a test that is supposed to be discriminating.
fn verify_mission_success_conditions(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
    declared: &[String],
) -> Result<(), MissionError> {
    // 1. The count, against the array's length. Catches a row the Mission never declared, which is
    //    the one shape a per-ordinal walk of the array cannot see.
    let stored: i64 = connection.query_row(
        "SELECT count(*) FROM mission_success_conditions
         WHERE workspace_id = ?1 AND mission_id = ?2",
        params![workspace_id.to_string(), mission_id.to_string()],
        |row| row.get(0),
    )?;
    if usize::try_from(stored) != Ok(declared.len()) {
        return Err(MissionError::InvalidStore);
    }
    // 2. The text, per ordinal, against the Mission's own bytes. Catches a row rewritten in place --
    //    which the count cannot see, because rewriting one is not adding one.
    for (ordinal, text) in declared.iter().enumerate() {
        let ordinal = i64::try_from(ordinal).map_err(|_| MissionError::InvalidStore)?;
        let row: Option<String> = connection
            .query_row(
                "SELECT text FROM mission_success_conditions
                 WHERE workspace_id = ?1 AND mission_id = ?2 AND ordinal = ?3",
                params![workspace_id.to_string(), mission_id.to_string(), ordinal],
                |row| row.get(0),
            )
            .optional()?;
        if row.as_deref() != Some(text.as_str()) {
            return Err(MissionError::InvalidStore);
        }
    }
    Ok(())
}

/// The rows `verify_mission_state` enumerates, in the order it enumerates them.
///
/// EXTRACTED SO A BAR CAN CALL IT (R44.6): a placement assertion that COPIES this query agrees
/// with itself, so flipping the pass's own `ORDER BY` leaves it decorative. One function, called
/// by both, moves the pass and the bar's view of it together.
pub fn enumerate_missions(connection: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement =
        connection.prepare("SELECT workspace_id, id FROM missions ORDER BY workspace_id, id")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn verify_mission_state(connection: &Connection) -> Result<(), MissionStateFault> {
    let ids = enumerate_missions(connection).map_err(MissionStateFault::Store)?;
    for (workspace_id, mission_id) in &ids {
        let identity = || MissionStateFault::Identity {
            workspace_id: workspace_id.clone(),
            mission_id: mission_id.clone(),
        };
        let parsed_workspace = Uuid::parse_str(workspace_id).map_err(|_| identity())?;
        let parsed_mission = Uuid::parse_str(mission_id).map_err(|_| identity())?;
        verify_one_mission(connection, parsed_workspace, parsed_mission).map_err(|cause| {
            MissionStateFault::Row {
                workspace_id: parsed_workspace,
                mission_id: parsed_mission,
                cause,
            }
        })?;
    }
    // Whole-store checks: no row to name, so they are reported as the store fault they are.
    verify_mission_totals(connection, ids.len())?;
    Ok(())
}

/// One stored Mission, re-proved. Unchanged from what the loop held inline.
fn verify_one_mission(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<(), MissionError> {
    {
        let mission = load_stored_mission(connection, workspace_id, mission_id)?
            .ok_or(MissionError::InvalidStore)?;
        let event_count = connection.query_row(
            "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = 'mission.admitted'
               AND json_extract(payload_json, '$.mission.id') = ?2",
            params![workspace_id.to_string(), mission.id.to_string()],
            |row| row.get::<_, i64>(0),
        )?;
        if event_count != 1 {
            return Err(MissionError::InvalidStore);
        }
        verify_mission_success_conditions(
            connection,
            workspace_id,
            mission_id,
            &mission.success_conditions,
        )?;
    }
    Ok(())
}

/// The two checks that are about the STORE rather than about one Mission, so they have no row to
/// name and stay whole-store faults.
fn verify_mission_totals(
    connection: &Connection,
    mission_count: usize,
) -> Result<(), MissionStateFault> {
    // A condition row for a Mission that does not exist cannot be stored -- the composite foreign
    // key forbids it -- so walking the Missions reaches every row there is.
    let orphan_conditions: i64 = connection
        .query_row(
            "SELECT count(*) FROM mission_success_conditions AS conditions
         WHERE NOT EXISTS (
             SELECT 1 FROM missions
             WHERE missions.workspace_id = conditions.workspace_id
               AND missions.id = conditions.mission_id
         )",
            [],
            |row| row.get(0),
        )
        .map_err(MissionStateFault::Store)?;
    if orphan_conditions != 0 {
        return Err(MissionStateFault::Totals(crate::TotalsMismatch {
            table: "mission_success_conditions (rows whose Mission does not exist)",
            expected: 0,
            observed: orphan_conditions.unsigned_abs(),
        }));
    }
    let event_count = connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'mission.admitted'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(MissionStateFault::Store)?;
    if usize::try_from(event_count) != Ok(mission_count) {
        return Err(MissionStateFault::Totals(crate::TotalsMismatch {
            table: "events (mission.admitted)",
            expected: mission_count as u64,
            observed: event_count.unsigned_abs(),
        }));
    }
    Ok(())
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()`; see the note in `outcome_publication`'s twin.
    fn expected_name(error: &MissionError) -> &'static str {
        match error {
            MissionError::Account(_) => "Account",
            MissionError::ProjectNotFound => "ProjectNotFound",
            MissionError::TeamNotFound => "TeamNotFound",
            MissionError::MissionNotFound => "MissionNotFound",
            MissionError::StaleCharterRevision { .. } => "StaleCharterRevision",
            MissionError::InvalidDesiredOutcome => "InvalidDesiredOutcome",
            MissionError::InvalidSuccessConditions => "InvalidSuccessConditions",
            MissionError::InvalidConstraint => "InvalidConstraint",
            MissionError::InvalidProhibition => "InvalidProhibition",
            MissionError::DuplicateMissionEntry => "DuplicateMissionEntry",
            MissionError::InvalidMembers => "InvalidMembers",
            MissionError::StaleTeamEligibility => "StaleTeamEligibility",
            MissionError::ResidentIneligible(_) => "ResidentIneligible",
            MissionError::InvalidStore => "InvalidStore",
            MissionError::Store(_) => "Store",
            MissionError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_mission_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            MissionError::Account(OperatorAccountError::PasswordTooShort),
            MissionError::ProjectNotFound,
            MissionError::TeamNotFound,
            MissionError::MissionNotFound,
            MissionError::StaleCharterRevision {
                expected: 1,
                current: 2,
            },
            MissionError::InvalidDesiredOutcome,
            MissionError::InvalidSuccessConditions,
            MissionError::InvalidConstraint,
            MissionError::InvalidProhibition,
            MissionError::DuplicateMissionEntry,
            MissionError::InvalidMembers,
            MissionError::StaleTeamEligibility,
            MissionError::ResidentIneligible(Uuid::nil()),
            MissionError::InvalidStore,
            MissionError::Store(rusqlite::Error::QueryReturnedNoRows),
            MissionError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            16,
            "an arm was added to `MissionError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(MissionError::name).collect();
        assert_eq!(
            names.len(),
            16,
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

#[cfg(test)]
mod verified_admission_reader {
    //! The complete owner-verified Mission admission (N37).
    //!
    //! These bars build a real store through the real services -- Owner, Workspace, Project,
    //! Policy, Residents, activation, Configurations, Team, Mission -- because the thing under test
    //! is a proof against a real `mission.admitted` Event, and a hand-planted row would be proving
    //! the fixture rather than the owner. **Raw SQL appears only to corrupt a store the owner has
    //! already minted, and every such site says so.**
    //!
    //! Expected values are held here. Nothing asserts a field by reading it back out of the value
    //! under test; the one clock-derived field, `admitted_at_ms`, is compared against the
    //! `MissionService::admit` return, which is a different producer than the reader.
    //!
    //! **R58 door census, measured.** `event_chain::tests::no_product_code_writes_an_event_around_the_door`
    //! exempts "everything from the file's own test module onward", but it finds that module by the
    //! literal `"#[cfg(test)]\nmod tests"`. `mission.rs`'s test modules are named `name_witness` and
    //! this one, so the census scans this whole file as product. It flagged an insert into the
    //! Events table written here for a corruption fixture -- and then flagged the comment that
    //! explained it, because the match is a plain substring over every line, comments included. The
    //! corruption below therefore retags an existing Event instead, and no line here spells the
    //! matched phrase. The census's narrow match is a real gap -- every in-crate test module not
    //! literally named `tests` is scanned as product -- but `event_chain.rs` is outside this task's
    //! fence, so it is reported rather than edited.

    use std::fs;
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};

    use rusqlite::Connection;
    use serde_json::json;

    use super::*;
    use crate::local_model_activation::LocalModelActivationService;
    use crate::operator_account::OperatorAccountService;
    use crate::project::{AutonomyProfile, ProjectService, ProjectView};
    use crate::resident_configuration::{ResidentConfiguration, ResidentConfigurationService};
    use crate::resident_lifecycle::ResidentLifecycleService;
    use crate::resource_discovery::LocalModelEndpoint;
    use crate::starter_organization::{
        StarterOrganizationService, StarterResidentDraft, StarterRole,
    };
    use crate::team::{TeamDraft, TeamMemberDraft, TeamService, TeamView};
    use crate::{
        BeaconCapability, ProjectCharterDraft, ProjectDraft, ProviderFeature, SecretString,
        StartupConfig, Supervisor,
    };

    const MODEL_NAME: &str = "qwen3.8:27b";
    const MODEL_DIGEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

    // Held expectations. Every one is written here and read by the assertions; none is derived from
    // the value under test.
    const DESIRED_OUTCOME: &str = "Ship one reviewed Beacon feature.";
    const CONDITION_ONE: &str = "Relevant gates pass";
    const CONDITION_TWO: &str = "One non-author seat reviews the change";
    const CONSTRAINT_ONE: &str = "Keep the change reversible";
    const CONSTRAINT_TWO: &str = "Touch only the files the task names";
    const PROHIBITION_ONE: &str = "Do not deploy";
    const PROHIBITION_TWO: &str = "Do not rewrite another lane's tree";

    struct TestStore {
        path: PathBuf,
        workspace_id: Uuid,
    }

    impl TestStore {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("beacond-n37-{label}-{}.sqlite3", Uuid::new_v4()));
            let report = Supervisor::bootstrap(StartupConfig {
                store_path: path.clone(),
                initial_workspace_name: "Personal".to_owned(),
            })
            .unwrap();
            Self {
                path,
                workspace_id: report.workspace.id,
            }
        }

        fn open(&self) -> Connection {
            Connection::open(&self.path).unwrap()
        }
    }

    impl Drop for TestStore {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = fs::remove_file(format!("{}{suffix}", self.path.display()));
            }
        }
    }

    fn enrolled_accounts(store: &TestStore) -> Arc<OperatorAccountService> {
        let accounts = Arc::new(OperatorAccountService::open(&store.path).unwrap());
        accounts
            .begin_password_setup(
                SecretString::new("a long local passphrase".to_owned()),
                SecretString::new("a long local passphrase".to_owned()),
            )
            .unwrap();
        accounts.complete_operator_setup(true).unwrap();
        accounts
    }

    fn tags_response() -> String {
        json!({
            "models": [{
                "name": MODEL_NAME,
                "model": MODEL_NAME,
                "modified_at": "2026-08-14T14:38:20.13373148-04:00",
                "size": 17_741_872_154_u64,
                "digest": MODEL_DIGEST,
                "details": {
                    "format": "gguf",
                    "family": "qwen35",
                    "families": ["qwen35"],
                    "parameter_size": "27.3B",
                    "quantization_level": "Q4_K_M"
                }
            }]
        })
        .to_string()
    }

    /// The crate's existing stub reader, carried across verbatim from `tests/mission.rs:120-143`
    /// rather than rewritten: my own byte-at-a-time version dropped a request whose header arrived
    /// split across reads, which killed the stub thread and surfaced as `InvalidProviderResponse`
    /// in one run of six.
    fn read_http_request(stream: &mut impl Read) -> String {
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let size = stream.read(&mut chunk).unwrap();
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                return String::from_utf8(request).unwrap();
            }
        }
    }

    /// A local stand-in for the model host's inspection endpoints. It is not a provider and no
    /// model is called: the Mission fixture cannot mint a Configuration without an activation, and
    /// an activation is minted from one inspection.
    fn serve_ollama_inspections(count: usize) -> (SocketAddr, JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for _ in 0..count {
                for expected in ["GET /api/tags", "POST /api/show", "GET /api/tags"] {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_http_request(&mut stream);
                    assert!(
                        request.starts_with(expected),
                        "unexpected request: {request}"
                    );
                    let body = if expected.starts_with("POST") {
                        json!({ "capabilities": ["completion", "tools"] }).to_string()
                    } else {
                        tags_response()
                    };
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .unwrap();
                }
            }
        });
        (address, handle)
    }

    struct PreparedTeam {
        project: ProjectView,
        team: TeamView,
        configurations: Vec<ResidentConfiguration>,
    }

    fn prepare_team(store: &TestStore, accounts: &Arc<OperatorAccountService>) -> PreparedTeam {
        let project = ProjectService::new(Arc::clone(accounts), store.workspace_id)
            .create(ProjectDraft {
                name: "Beacon Alpha".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Prove the local agent office.".to_owned(),
                    desired_outcomes: vec!["Complete one attributable Mission".to_owned()],
                    constraints: vec!["Keep authority explicit".to_owned()],
                    prohibitions: vec!["Do not deploy".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .unwrap();
        let residents = StarterOrganizationService::new(Arc::clone(accounts), store.workspace_id)
            .accept(
                1,
                vec![
                    StarterResidentDraft {
                        role: StarterRole::Lead,
                        display_name: None,
                    },
                    StarterResidentDraft {
                        role: StarterRole::Builder,
                        display_name: None,
                    },
                ],
            )
            .unwrap()
            .residents;
        let lifecycle = ResidentLifecycleService::new(Arc::clone(accounts), store.workspace_id);
        for resident in &residents {
            lifecycle.activate(resident.id, None).unwrap();
        }
        let (address, ollama) = serve_ollama_inspections(2);
        let models = LocalModelActivationService::new(
            Arc::clone(accounts),
            store.workspace_id,
            LocalModelEndpoint::new(address).unwrap(),
        );
        let review = models.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
        let activation = models
            .activate(review, vec![ProviderFeature::Completion])
            .unwrap();
        ollama.join().unwrap();
        let configurations = residents
            .iter()
            .map(|resident| {
                ResidentConfigurationService::new(Arc::clone(accounts), store.workspace_id)
                    .configure(
                        resident.id,
                        None,
                        activation.id,
                        vec![BeaconCapability::WorkCreateMission],
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let team = TeamService::new(Arc::clone(accounts), store.workspace_id)
            .form(TeamDraft {
                project_id: project.id,
                name: "Mission Team".to_owned(),
                members: configurations
                    .iter()
                    .map(|configuration| TeamMemberDraft {
                        resident_id: configuration.resident_id,
                        expected_configuration_id: configuration.id,
                    })
                    .collect(),
            })
            .unwrap();
        PreparedTeam {
            project,
            team,
            configurations,
        }
    }

    /// One admitted Mission with two ordered members carrying distinct Configurations, and two
    /// distinct entries in each declared list.
    fn admitted_mission(label: &str) -> (TestStore, PreparedTeam, MissionView) {
        let store = TestStore::new(label);
        let accounts = enrolled_accounts(&store);
        let prepared = prepare_team(&store, &accounts);
        let admitted = MissionService::new(Arc::clone(&accounts), store.workspace_id)
            .admit(MissionDraft {
                project_id: prepared.project.id,
                expected_charter_revision: prepared.project.charter.revision,
                team_id: prepared.team.id,
                desired_outcome: DESIRED_OUTCOME.to_owned(),
                success_conditions: vec![CONDITION_ONE.to_owned(), CONDITION_TWO.to_owned()],
                constraints: vec![CONSTRAINT_ONE.to_owned(), CONSTRAINT_TWO.to_owned()],
                prohibitions: vec![PROHIBITION_ONE.to_owned(), PROHIBITION_TWO.to_owned()],
                members: prepared
                    .configurations
                    .iter()
                    .map(|configuration| MissionMemberDraft {
                        resident_id: configuration.resident_id,
                        expected_configuration_id: configuration.id,
                    })
                    .collect(),
            })
            .unwrap();
        assert_ne!(
            prepared.configurations[0].id, prepared.configurations[1].id,
            "the two members must carry distinct Configurations for the roster bars to discriminate"
        );
        (store, prepared, admitted)
    }

    #[test]
    fn the_verified_reader_returns_every_admitted_field_including_the_ordered_roster() {
        let (store, prepared, admitted) = admitted_mission("complete");
        let connection = store.open();

        let mission = verified_mission_admission(&connection, store.workspace_id, admitted.id)
            .unwrap()
            .expect("the Mission was admitted in this Workspace");

        assert_eq!(mission.id, admitted.id);
        assert_eq!(mission.workspace_id, store.workspace_id);
        assert_eq!(mission.project_id, prepared.project.id);
        assert_eq!(mission.charter_revision, prepared.project.charter.revision);
        assert_eq!(mission.team_id, prepared.team.id);
        assert_eq!(mission.desired_outcome, DESIRED_OUTCOME);
        assert_eq!(
            mission.success_conditions,
            vec![CONDITION_ONE.to_owned(), CONDITION_TWO.to_owned()]
        );
        assert_eq!(
            mission.constraints,
            vec![CONSTRAINT_ONE.to_owned(), CONSTRAINT_TWO.to_owned()]
        );
        assert_eq!(
            mission.prohibitions,
            vec![PROHIBITION_ONE.to_owned(), PROHIBITION_TWO.to_owned()]
        );
        assert_eq!(mission.lifecycle, MissionLifecycle::Admitted);
        assert_eq!(mission.admitted_by, MissionAdmissionActor::Operator);
        // The one clock-derived field, compared against a DIFFERENT producer: the admit call's own
        // return, never the reader's.
        assert_eq!(mission.admitted_at_ms, admitted.admitted_at_ms);

        assert_eq!(
            mission.members.len(),
            2,
            "two ordered members were admitted"
        );
        assert_eq!(
            mission.members[0],
            MissionMemberView {
                resident_id: prepared.configurations[0].resident_id,
                admission_configuration_id: prepared.configurations[0].id,
            }
        );
        assert_eq!(
            mission.members[1],
            MissionMemberView {
                resident_id: prepared.configurations[1].resident_id,
                admission_configuration_id: prepared.configurations[1].id,
            }
        );
    }

    #[test]
    fn the_declaration_projection_keeps_its_shape_over_the_shared_proof() {
        let (store, prepared, admitted) = admitted_mission("declaration");
        let connection = store.open();

        let declaration =
            verified_mission_declaration(&connection, store.workspace_id, admitted.id)
                .unwrap()
                .expect("the Mission was admitted in this Workspace");
        assert_eq!(
            declaration,
            MissionDeclaration {
                project_id: prepared.project.id,
                desired_outcome: DESIRED_OUTCOME.to_owned(),
                success_conditions: vec![CONDITION_ONE.to_owned(), CONDITION_TWO.to_owned()],
            }
        );

        let project = verified_mission_project(&connection, store.workspace_id, admitted.id)
            .unwrap()
            .expect("the Mission was admitted in this Workspace");
        assert_eq!(project, prepared.project.id);
    }

    #[test]
    fn a_missing_mission_and_another_workspaces_mission_are_both_absent() {
        let (store, _prepared, admitted) = admitted_mission("absent");
        let connection = store.open();

        assert_eq!(
            verified_mission_admission(&connection, store.workspace_id, Uuid::new_v4()).unwrap(),
            None,
            "a Mission this Workspace never admitted is absent"
        );
        assert_eq!(
            verified_mission_admission(&connection, Uuid::new_v4(), admitted.id).unwrap(),
            None,
            "another Workspace's Mission is absent, not broken"
        );
    }

    #[test]
    fn a_mission_whose_admission_event_was_removed_refuses_as_invalid_store() {
        let (store, _prepared, admitted) = admitted_mission("ownerless");
        let connection = store.open();
        // INTENTIONAL CORRUPTION, after the owner minted this Mission: drop its one Event.
        connection
            .execute(
                "DELETE FROM events WHERE kind = 'mission.admitted'
                 AND json_extract(payload_json, '$.mission.id') = ?1",
                rusqlite::params![admitted.id.to_string()],
            )
            .unwrap();

        let refusal = verified_mission_admission(&connection, store.workspace_id, admitted.id)
            .expect_err("a Mission the log does not account for is not absent");
        assert_eq!(refusal.name(), "InvalidStore");
    }

    #[test]
    fn a_mission_with_two_admission_events_refuses_as_invalid_store() {
        let (store, _prepared, admitted) = admitted_mission("duplicate");
        let connection = store.open();
        // INTENTIONAL CORRUPTION, after the owner minted this Mission: retag the store's first
        // Event so a SECOND row claims this admission, and "when was this Mission admitted" has two
        // answers.
        //
        // BOTH the payload AND the timestamp are copied. Root's N37 review measured why: copying
        // the payload alone left the retagged row's own `occurred_at_ms` in place, so
        // `verify_mission_event` could refuse it on the timestamp -- and an `accept the first of
        // several` mutant then stayed GREEN, because the witness was reading a timestamp fault as
        // if it were a cardinality fault. Two individually VALID Events are what makes the count
        // the only thing wrong.
        //
        // Retagging rather than inserting keeps the chain's sequence contiguous. It does NOT keep
        // the chain's hashes valid -- the retagged row's link digest no longer describes its bytes
        // -- and nothing here claims otherwise; this bar calls the reader directly and never runs
        // the chain verifier. It also keeps the phrase R58's door census matches out of a source
        // file it scans as product (see the note at the top of this module).
        let changed = connection
            .execute(
                "UPDATE events
                 SET kind = 'mission.admitted',
                     payload_json = (
                         SELECT payload_json FROM events
                         WHERE kind = 'mission.admitted'
                           AND json_extract(payload_json, '$.mission.id') = ?1),
                     occurred_at_ms = (
                         SELECT occurred_at_ms FROM events
                         WHERE kind = 'mission.admitted'
                           AND json_extract(payload_json, '$.mission.id') = ?1)
                 WHERE sequence = (SELECT min(sequence) FROM events)",
                rusqlite::params![admitted.id.to_string()],
            )
            .unwrap();
        assert_eq!(changed, 1, "exactly one earlier Event was retagged");

        // The fixture proves its own premise before the reader is asked anything, so a timestamp,
        // Workspace or payload fault cannot masquerade as a cardinality fault.
        let candidates = scoped_admission_events(&connection, store.workspace_id, admitted.id);
        assert_eq!(
            candidates.len(),
            2,
            "N37.DUPLICATE_CARDINALITY.FIXTURE_HAS_TWO_CANDIDATES"
        );
        for (index, (payload_json, occurred_at_ms)) in candidates.iter().enumerate() {
            verify_mission_event(
                &connection,
                store.workspace_id,
                u64::try_from(*occurred_at_ms).unwrap(),
                payload_json,
            )
            .unwrap_or_else(|error| {
                panic!(
                    "N37.DUPLICATE_CARDINALITY.EACH_CANDIDATE_IS_INDIVIDUALLY_VALID: \
                     candidate {index} refused as {}",
                    error.name()
                )
            });
        }

        let refusal = verified_mission_admission(&connection, store.workspace_id, admitted.id)
            .expect_err("N37.DUPLICATE_CARDINALITY.EXACTLY_ONE_EVENT_REQUIRED");
        assert_eq!(
            refusal.name(),
            "InvalidStore",
            "N37.DUPLICATE_CARDINALITY.EXACTLY_ONE_EVENT_REQUIRED"
        );
    }

    /// Every `mission.admitted` Event this Workspace holds for this Mission, in the reader's own
    /// order. Written here rather than reusing the reader's query, so the fixture's count is an
    /// independent observation of the store rather than a restatement of the code under test.
    fn scoped_admission_events(
        connection: &Connection,
        workspace_id: Uuid,
        mission_id: Uuid,
    ) -> Vec<(String, i64)> {
        let mut statement = connection
            .prepare(
                "SELECT payload_json, occurred_at_ms FROM events
                 WHERE workspace_id = ?1 AND kind = 'mission.admitted'
                   AND json_extract(payload_json, '$.mission.id') = ?2",
            )
            .unwrap();
        statement
            .query_map(
                rusqlite::params![workspace_id.to_string(), mission_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn a_second_roster_member_that_disagrees_with_the_event_refuses_as_invalid_store() {
        let (store, _prepared, admitted) = admitted_mission("roster");
        let connection = store.open();
        // INTENTIONAL CORRUPTION, after the owner minted this Mission: rewrite the SECOND member's
        // admission Configuration in the Event payload only. The row still says what it always
        // said, so nothing but a whole-roster comparison can see this.
        let rewritten = Uuid::new_v4().to_string();
        let changed = connection
            .execute(
                "UPDATE events
                 SET payload_json = json_set(
                         payload_json,
                         '$.mission.members[1].admissionConfigurationId',
                         ?2)
                 WHERE kind = 'mission.admitted'
                   AND json_extract(payload_json, '$.mission.id') = ?1",
                rusqlite::params![admitted.id.to_string(), rewritten],
            )
            .unwrap();
        assert_eq!(changed, 1, "exactly one admission Event was rewritten");

        let refusal = verified_mission_admission(&connection, store.workspace_id, admitted.id)
            .expect_err("the roster the Event names is not the roster the store holds");
        assert_eq!(refusal.name(), "InvalidStore");
    }

    #[test]
    fn public_get_keeps_one_snapshot_when_an_admission_event_changes_between_queries() {
        use rusqlite::ffi;
        use std::ffi::{CStr, c_int, c_uint, c_void};

        struct Rewrite {
            writer: Connection,
            mission_id: String,
            attempted: bool,
            changed: Option<Result<usize, String>>,
            panicked: bool,
        }

        // This is SQLite's existing trace API, installed only on this test's account connection.
        // No product hook, sleep, worker, or scheduling assumption is involved. The separate WAL
        // connection commits after the Mission row/roster reads and before the Event read begins.
        // https://www.sqlite.org/c3ref/c_trace.html
        unsafe extern "C" fn before_statement(
            kind: c_uint,
            context: *mut c_void,
            _statement: *mut c_void,
            sql: *mut c_void,
        ) -> c_int {
            if kind != ffi::SQLITE_TRACE_STMT as c_uint || sql.is_null() {
                return 0;
            }
            // SAFETY: SQLite passes the registered live Box and a NUL-terminated SQL string for
            // SQLITE_TRACE_STMT. The callback runs synchronously on this test's calling thread.
            let state = unsafe { &mut *context.cast::<Rewrite>() };
            let sql = unsafe { CStr::from_ptr(sql.cast()) }.to_bytes();
            if state.attempted || !sql.windows(11).any(|part| part == b"FROM events") {
                return 0;
            }
            state.attempted = true;
            // A Rust panic must never unwind through SQLite's C callback.
            let changed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                state
                    .writer
                    .execute(
                        "UPDATE events SET payload_json = json_set(
                         payload_json, '$.mission.constraints[0]', 'Changed after snapshot')
                     WHERE kind = 'mission.admitted'
                       AND json_extract(payload_json, '$.mission.id') = ?1",
                        [&state.mission_id],
                    )
                    .map_err(|error| error.to_string())
            }));
            match changed {
                Ok(result) => state.changed = Some(result),
                Err(_) => state.panicked = true,
            }
            0
        }

        struct TraceGuard {
            handle: *mut ffi::sqlite3,
            // Keeps the connection alive until Drop unregisters the callback, including unwinds.
            _accounts: Arc<OperatorAccountService>,
            state: Box<Rewrite>,
        }
        impl Drop for TraceGuard {
            fn drop(&mut self) {
                // SAFETY: the Arc keeps this handle alive, the test shares it with no worker,
                // and unregistering precedes dropping the callback context even after an assertion.
                unsafe {
                    ffi::sqlite3_trace_v2(self.handle, 0, None, std::ptr::null_mut());
                }
            }
        }

        let (store, _prepared, admitted) = admitted_mission("public-snapshot");
        let accounts = Arc::new(OperatorAccountService::open(&store.path).unwrap());
        accounts
            .authenticate_password(
                SecretString::new("a long local passphrase".to_owned()),
                crate::AccessOrigin::SameMachine,
            )
            .unwrap();
        let missions = MissionService::new(Arc::clone(&accounts), store.workspace_id);
        let writer = store.open();
        let mode: String = writer
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal", "B11.GET.SNAPSHOT_WAL_PREMISE");
        let mut guard = accounts
            .with_authenticated_store(|connection, _now| {
                let mut guard = TraceGuard {
                    // SAFETY: the retained Arc owns this connection for the guard's whole lifetime.
                    handle: unsafe { connection.handle() },
                    _accounts: Arc::clone(&accounts),
                    state: Box::new(Rewrite {
                        writer,
                        mission_id: admitted.id.to_string(),
                        attempted: false,
                        changed: None,
                        panicked: false,
                    }),
                };
                // SAFETY: Box address is stable, the function signature matches SQLite, and Drop
                // removes the callback before the Box or account connection can be destroyed.
                let code = unsafe {
                    ffi::sqlite3_trace_v2(
                        guard.handle,
                        ffi::SQLITE_TRACE_STMT as c_uint,
                        Some(before_statement),
                        (&mut *guard.state as *mut Rewrite).cast(),
                    )
                };
                assert_eq!(code, ffi::SQLITE_OK, "B11.GET.TRACE_INSTALLED");
                Ok::<_, MissionError>(guard)
            })
            .unwrap();

        let read = missions.get(admitted.id);
        let attempted = guard.state.attempted;
        let panicked = guard.state.panicked;
        let changed = guard.state.changed.take();
        drop(guard);
        assert!(
            attempted && !panicked,
            "B11.GET.BETWEEN_QUERY_WRITE_REACHED"
        );
        assert_eq!(
            changed,
            Some(Ok(1)),
            "B11.GET.BETWEEN_QUERY_WRITE_COMMITTED"
        );
        assert_eq!(
            read.expect("B11.GET.ONE_SNAPSHOT"),
            admitted,
            "B11.GET.ONE_SNAPSHOT"
        );
        assert!(
            matches!(missions.get(admitted.id), Err(MissionError::InvalidStore)),
            "B11.GET.NEXT_READ_SEES_COMMITTED_CHANGE"
        );
    }
}
