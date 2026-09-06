use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::starter_organization::normalize_display_name;
use crate::{OperatorAccountError, OperatorAccountService, StarterRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidentLifecycle {
    Inactive,
    Active,
    Suspended,
    Retired,
}

impl ResidentLifecycle {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Retired => "retired",
        }
    }

    fn parse(value: &str) -> Result<Self, ResidentLifecycleError> {
        match value {
            "inactive" => Ok(Self::Inactive),
            "active" => Ok(Self::Active),
            "suspended" => Ok(Self::Suspended),
            "retired" => Ok(Self::Retired),
            _ => Err(ResidentLifecycleError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidentLifecycleActor {
    Operator,
}

impl ResidentLifecycleActor {
    fn parse(value: &str) -> Result<Self, ResidentLifecycleError> {
        match value {
            "operator" => Ok(Self::Operator),
            _ => Err(ResidentLifecycleError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResidentLifecycleDecision {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub resident_id: Uuid,
    pub from_lifecycle: ResidentLifecycle,
    pub to_lifecycle: ResidentLifecycle,
    pub predecessor_id: Option<Uuid>,
    pub decided_by: ResidentLifecycleActor,
    pub decided_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResidentAgentView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub role: StarterRole,
    pub display_name: Option<String>,
    pub lifecycle: ResidentLifecycle,
    pub latest_decision_id: Option<Uuid>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Which fault the ResidentLifecycle startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum ResidentLifecycleStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One ResidentLifecycle did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    Row {
        workspace_id: Uuid,
        resident_id: Uuid,
        cause: ResidentLifecycleError,
    },
    /// A COUNT across the store that does not match the log: numbers, never an invented row.
    Totals(crate::TotalsMismatch),
    /// A row that could not be READ at all, so it cannot be named: the identity columns are part
    /// of what failed. Distinct from `Row`, which names a row that read and did not re-prove.
    Unreadable(ResidentLifecycleError),
}

impl ResidentLifecycleError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::NotFound => "NotFound",
            Self::InvalidTransition => "InvalidTransition",
            Self::StaleDecision => "StaleDecision",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for ResidentLifecycleStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the ResidentLifecycles could not be read"),
            Self::Row {
                workspace_id,
                resident_id,
                cause,
            } => write!(
                formatter,
                "ResidentLifecycle {resident_id} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Totals(totals) => write!(
                formatter,
                "the Resident lifecycle decisions do not agree with the store -- {totals}"
            ),
            Self::Unreadable(cause) => write!(
                formatter,
                "a Resident row could not be read: {}",
                cause.name()
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for ResidentLifecycleStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Totals(totals) => formatter.debug_tuple("Totals").field(totals).finish(),
            Self::Unreadable(cause) => formatter.debug_tuple("Unreadable").field(cause).finish(),
            Self::Row {
                workspace_id,
                resident_id,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("resident_id", resident_id)
                .field("cause", cause)
                .finish(),
        }
    }
}

impl std::error::Error for ResidentLifecycleStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Unreadable(cause) | Self::Row { cause, .. } => Some(cause),
            // A `TotalsMismatch` is DATA, not a nested error.
            Self::Totals(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum ResidentLifecycleError {
    Account(OperatorAccountError),
    NotFound,
    InvalidTransition,
    StaleDecision,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ResidentLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::NotFound => write!(formatter, "Resident was not found"),
            Self::InvalidTransition => {
                write!(formatter, "Resident lifecycle transition is invalid")
            }
            Self::StaleDecision => write!(formatter, "Resident lifecycle decision is stale"),
            Self::InvalidStore => write!(formatter, "Resident lifecycle state is invalid"),
            Self::Store(error) => write!(formatter, "Resident lifecycle store failed: {error}"),
            Self::Json(error) => write!(formatter, "Resident lifecycle Event is invalid: {error}"),
        }
    }
}

impl std::error::Error for ResidentLifecycleError {}

impl From<OperatorAccountError> for ResidentLifecycleError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for ResidentLifecycleError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for ResidentLifecycleError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Clone, Copy)]
enum LifecycleCommand {
    Activate,
    Suspend,
    Reactivate,
    Retire,
}

impl LifecycleCommand {
    fn transition(
        self,
        current: ResidentLifecycle,
    ) -> Result<(ResidentLifecycle, &'static str), ResidentLifecycleError> {
        match (self, current) {
            (Self::Activate, ResidentLifecycle::Inactive) => {
                Ok((ResidentLifecycle::Active, "resident.activated"))
            }
            (Self::Suspend, ResidentLifecycle::Active) => {
                Ok((ResidentLifecycle::Suspended, "resident.suspended"))
            }
            (Self::Reactivate, ResidentLifecycle::Suspended) => {
                Ok((ResidentLifecycle::Active, "resident.reactivated"))
            }
            (
                Self::Retire,
                ResidentLifecycle::Inactive
                | ResidentLifecycle::Active
                | ResidentLifecycle::Suspended,
            ) => Ok((ResidentLifecycle::Retired, "resident.retired")),
            _ => Err(ResidentLifecycleError::InvalidTransition),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResidentLifecycleDecisionPayload {
    decision: ResidentLifecycleDecision,
}

/// Owns authenticated Operator-only availability decisions for persistent Residents.
pub struct ResidentLifecycleService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl ResidentLifecycleService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    pub fn list(&self) -> Result<Vec<ResidentAgentView>, ResidentLifecycleError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            list_residents(connection, self.workspace_id)
        })
    }

    pub fn activate(
        &self,
        resident_id: Uuid,
        expected_decision_id: Option<Uuid>,
    ) -> Result<ResidentAgentView, ResidentLifecycleError> {
        self.apply(
            resident_id,
            expected_decision_id,
            LifecycleCommand::Activate,
        )
    }

    pub fn suspend(
        &self,
        resident_id: Uuid,
        expected_decision_id: Option<Uuid>,
    ) -> Result<ResidentAgentView, ResidentLifecycleError> {
        self.apply(resident_id, expected_decision_id, LifecycleCommand::Suspend)
    }

    pub fn reactivate(
        &self,
        resident_id: Uuid,
        expected_decision_id: Option<Uuid>,
    ) -> Result<ResidentAgentView, ResidentLifecycleError> {
        self.apply(
            resident_id,
            expected_decision_id,
            LifecycleCommand::Reactivate,
        )
    }

    pub fn retire(
        &self,
        resident_id: Uuid,
        expected_decision_id: Option<Uuid>,
    ) -> Result<ResidentAgentView, ResidentLifecycleError> {
        self.apply(resident_id, expected_decision_id, LifecycleCommand::Retire)
    }

    fn apply(
        &self,
        resident_id: Uuid,
        expected_decision_id: Option<Uuid>,
        command: LifecycleCommand,
    ) -> Result<ResidentAgentView, ResidentLifecycleError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = load_resident(&transaction, self.workspace_id, resident_id)?
                .ok_or(ResidentLifecycleError::NotFound)?;
            if current.latest_decision_id != expected_decision_id {
                return Err(ResidentLifecycleError::StaleDecision);
            }
            let (to_lifecycle, event_kind) = command.transition(current.lifecycle)?;
            let now = u64::try_from(now).map_err(|_| ResidentLifecycleError::InvalidStore)?;
            let decided_at_ms = now.max(current.updated_at_ms);
            let decision = ResidentLifecycleDecision {
                id: Uuid::new_v4(),
                workspace_id: self.workspace_id,
                resident_id,
                from_lifecycle: current.lifecycle,
                to_lifecycle,
                predecessor_id: current.latest_decision_id,
                decided_by: ResidentLifecycleActor::Operator,
                decided_at_ms,
            };
            insert_decision(&transaction, &decision, event_kind)?;
            let changed = transaction.execute(
                "UPDATE agents
                 SET lifecycle = ?1, updated_at_ms = ?2
                 WHERE workspace_id = ?3 AND id = ?4 AND kind = 'resident'
                   AND lifecycle = ?5 AND updated_at_ms = ?6",
                params![
                    to_lifecycle.as_str(),
                    i64::try_from(decided_at_ms)
                        .map_err(|_| ResidentLifecycleError::InvalidStore)?,
                    self.workspace_id.to_string(),
                    resident_id.to_string(),
                    current.lifecycle.as_str(),
                    i64::try_from(current.updated_at_ms)
                        .map_err(|_| ResidentLifecycleError::InvalidStore)?,
                ],
            )?;
            if changed != 1 {
                return Err(ResidentLifecycleError::StaleDecision);
            }
            transaction.commit()?;
            load_resident(connection, self.workspace_id, resident_id)?
                .ok_or(ResidentLifecycleError::InvalidStore)
        })
    }
}

fn insert_decision(
    connection: &Connection,
    decision: &ResidentLifecycleDecision,
    event_kind: &str,
) -> Result<(), ResidentLifecycleError> {
    connection.execute(
        "INSERT INTO resident_lifecycle_decisions (
            id, workspace_id, resident_id, from_lifecycle, to_lifecycle,
            predecessor_id, decided_by, decided_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'operator', ?7)",
        params![
            decision.id.to_string(),
            decision.workspace_id.to_string(),
            decision.resident_id.to_string(),
            decision.from_lifecycle.as_str(),
            decision.to_lifecycle.as_str(),
            decision.predecessor_id.map(|id| id.to_string()),
            i64::try_from(decision.decided_at_ms)
                .map_err(|_| ResidentLifecycleError::InvalidStore)?,
        ],
    )?;
    let payload = serde_json::to_string(&ResidentLifecycleDecisionPayload {
        decision: decision.clone(),
    })?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            decision.workspace_id.to_string(),
            event_kind,
            payload,
            i64::try_from(decision.decided_at_ms)
                .map_err(|_| ResidentLifecycleError::InvalidStore)?,
        ],
    )?;
    Ok(())
}

fn list_residents(
    connection: &Connection,
    workspace_id: Uuid,
) -> Result<Vec<ResidentAgentView>, ResidentLifecycleError> {
    let mut statement = connection.prepare(
        "SELECT agent.id, agent.workspace_id, agent.role, agent.display_name_state,
                agent.display_name, agent.lifecycle, latest.id,
                agent.created_at_ms, agent.updated_at_ms
         FROM agents AS agent
         LEFT JOIN resident_lifecycle_decisions AS latest
           ON latest.workspace_id = agent.workspace_id
          AND latest.resident_id = agent.id
          AND NOT EXISTS (
              SELECT 1 FROM resident_lifecycle_decisions AS newer
              WHERE newer.workspace_id = latest.workspace_id
                AND newer.resident_id = latest.resident_id
                AND newer.sequence > latest.sequence
          )
         WHERE agent.workspace_id = ?1 AND agent.kind = 'resident'
         ORDER BY CASE agent.role
             WHEN 'lead' THEN 0
             WHEN 'builder' THEN 1
             WHEN 'reviewer' THEN 2
             WHEN 'researcher' THEN 3
             ELSE 4
         END, agent.id",
    )?;
    let mut rows = statement.query([workspace_id.to_string()])?;
    let mut residents = Vec::new();
    while let Some(row) = rows.next()? {
        residents.push(read_resident_row(row)?);
    }
    Ok(residents)
}

pub(crate) fn load_resident(
    connection: &Connection,
    workspace_id: Uuid,
    resident_id: Uuid,
) -> Result<Option<ResidentAgentView>, ResidentLifecycleError> {
    let mut statement = connection.prepare(
        "SELECT agent.id, agent.workspace_id, agent.role, agent.display_name_state,
                    agent.display_name, agent.lifecycle, latest.id,
                    agent.created_at_ms, agent.updated_at_ms
             FROM agents AS agent
             LEFT JOIN resident_lifecycle_decisions AS latest
               ON latest.workspace_id = agent.workspace_id
              AND latest.resident_id = agent.id
              AND NOT EXISTS (
                  SELECT 1 FROM resident_lifecycle_decisions AS newer
                  WHERE newer.workspace_id = latest.workspace_id
                    AND newer.resident_id = latest.resident_id
                    AND newer.sequence > latest.sequence
              )
             WHERE agent.workspace_id = ?1 AND agent.id = ?2 AND agent.kind = 'resident'",
    )?;
    let mut rows = statement.query(params![workspace_id.to_string(), resident_id.to_string()])?;
    rows.next()?.map(read_resident_row).transpose()
}

fn read_resident_row(row: &rusqlite::Row<'_>) -> Result<ResidentAgentView, ResidentLifecycleError> {
    let id = parse_uuid(row.get(0)?)?;
    let workspace_id = parse_uuid(row.get(1)?)?;
    let role = StarterRole::parse(&row.get::<_, String>(2)?)
        .map_err(|_| ResidentLifecycleError::InvalidStore)?;
    let display_name_state = row.get::<_, String>(3)?;
    let display_name_value = row.get::<_, String>(4)?;
    let display_name = match (display_name_state.as_str(), display_name_value.as_str()) {
        ("absent", "") => None,
        ("set", value) => {
            let normalized = normalize_display_name(Some(value.to_owned()))
                .map_err(|_| ResidentLifecycleError::InvalidStore)?
                .ok_or(ResidentLifecycleError::InvalidStore)?;
            if normalized != value {
                return Err(ResidentLifecycleError::InvalidStore);
            }
            Some(normalized)
        }
        _ => return Err(ResidentLifecycleError::InvalidStore),
    };
    let lifecycle = ResidentLifecycle::parse(&row.get::<_, String>(5)?)?;
    let latest_decision_id = row
        .get::<_, Option<String>>(6)?
        .map(parse_uuid)
        .transpose()?;
    let created_at_ms = parse_time(row.get(7)?)?;
    let updated_at_ms = parse_time(row.get(8)?)?;
    if updated_at_ms < created_at_ms {
        return Err(ResidentLifecycleError::InvalidStore);
    }
    Ok(ResidentAgentView {
        id,
        workspace_id,
        role,
        display_name,
        lifecycle,
        latest_decision_id,
        created_at_ms,
        updated_at_ms,
    })
}

fn read_decision_row(
    row: &rusqlite::Row<'_>,
) -> Result<ResidentLifecycleDecision, ResidentLifecycleError> {
    Ok(ResidentLifecycleDecision {
        id: parse_uuid(row.get(0)?)?,
        workspace_id: parse_uuid(row.get(1)?)?,
        resident_id: parse_uuid(row.get(2)?)?,
        from_lifecycle: ResidentLifecycle::parse(&row.get::<_, String>(3)?)?,
        to_lifecycle: ResidentLifecycle::parse(&row.get::<_, String>(4)?)?,
        predecessor_id: row
            .get::<_, Option<String>>(5)?
            .map(parse_uuid)
            .transpose()?,
        decided_by: ResidentLifecycleActor::parse(&row.get::<_, String>(6)?)?,
        decided_at_ms: parse_time(row.get(7)?)?,
    })
}

fn decision_by_id(
    connection: &Connection,
    workspace_id: Uuid,
    decision_id: Uuid,
) -> Result<Option<ResidentLifecycleDecision>, ResidentLifecycleError> {
    let mut statement = connection.prepare(
        "SELECT id, workspace_id, resident_id, from_lifecycle, to_lifecycle,
                    predecessor_id, decided_by, decided_at_ms
             FROM resident_lifecycle_decisions
             WHERE workspace_id = ?1 AND id = ?2",
    )?;
    let mut rows = statement.query(params![workspace_id.to_string(), decision_id.to_string()])?;
    rows.next()?.map(read_decision_row).transpose()
}

fn parse_uuid(value: String) -> Result<Uuid, ResidentLifecycleError> {
    Uuid::parse_str(&value).map_err(|_| ResidentLifecycleError::InvalidStore)
}

fn parse_time(value: i64) -> Result<u64, ResidentLifecycleError> {
    u64::try_from(value).map_err(|_| ResidentLifecycleError::InvalidStore)
}

pub(crate) fn verify_lifecycle_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    kind: &str,
    payload_json: &str,
) -> Result<(), ResidentLifecycleError> {
    let payload: ResidentLifecycleDecisionPayload = serde_json::from_str(payload_json)?;
    let decision = decision_by_id(connection, workspace_id, payload.decision.id)?
        .ok_or(ResidentLifecycleError::InvalidStore)?;
    let expected_kind = event_kind(&decision)?;
    if decision != payload.decision
        || decision.workspace_id != workspace_id
        || decision.decided_at_ms != occurred_at_ms
        || expected_kind != kind
    {
        return Err(ResidentLifecycleError::InvalidStore);
    }
    Ok(())
}

pub(crate) fn verify_lifecycle_state(
    connection: &Connection,
) -> Result<(), ResidentLifecycleStateFault> {
    let mut statement = connection
        .prepare(
            "SELECT agent.id, agent.workspace_id, agent.role, agent.display_name_state,
                agent.display_name, agent.lifecycle, latest.id,
                agent.created_at_ms, agent.updated_at_ms
         FROM agents AS agent
         LEFT JOIN resident_lifecycle_decisions AS latest
           ON latest.workspace_id = agent.workspace_id
          AND latest.resident_id = agent.id
          AND NOT EXISTS (
              SELECT 1 FROM resident_lifecycle_decisions AS newer
              WHERE newer.workspace_id = latest.workspace_id
                AND newer.resident_id = latest.resident_id
                AND newer.sequence > latest.sequence
          )
         WHERE agent.kind = 'resident'
         ORDER BY agent.workspace_id, agent.id",
        )
        .map_err(ResidentLifecycleStateFault::Store)?;
    let mut resident_rows = statement
        .query([])
        .map_err(ResidentLifecycleStateFault::Store)?;
    let mut residents = Vec::new();
    while let Some(row) = resident_rows
        .next()
        .map_err(ResidentLifecycleStateFault::Store)?
    {
        residents.push(read_resident_row(row).map_err(ResidentLifecycleStateFault::Unreadable)?);
    }
    let mut verified_decisions = 0_i64;
    for resident in residents {
        verified_decisions +=
            verify_one_resident_lifecycle(connection, &resident).map_err(|cause| {
                ResidentLifecycleStateFault::Row {
                    workspace_id: resident.workspace_id,
                    resident_id: resident.id,
                    cause,
                }
            })?;
    }
    let stored_decisions: i64 = connection
        .query_row(
            "SELECT count(*) FROM resident_lifecycle_decisions",
            [],
            |row| row.get(0),
        )
        .map_err(ResidentLifecycleStateFault::Store)?;
    if stored_decisions != verified_decisions {
        return Err(ResidentLifecycleStateFault::Totals(crate::TotalsMismatch {
            table: "resident_lifecycle_decisions",
            expected: verified_decisions.unsigned_abs(),
            observed: stored_decisions.unsigned_abs(),
        }));
    }
    Ok(())
}

/// One Resident's lifecycle chain, re-proved. Unchanged from what the loop held inline; it
/// returns the decisions it verified so the pass can compare the total.
fn verify_one_resident_lifecycle(
    connection: &Connection,
    resident: &ResidentAgentView,
) -> Result<i64, ResidentLifecycleError> {
    let mut verified_decisions = 0_i64;
    let mut decisions_statement = connection.prepare(
        "SELECT id, workspace_id, resident_id, from_lifecycle, to_lifecycle,
                    predecessor_id, decided_by, decided_at_ms
             FROM resident_lifecycle_decisions
             WHERE workspace_id = ?1 AND resident_id = ?2
             ORDER BY sequence",
    )?;
    let mut decision_rows = decisions_statement.query(params![
        resident.workspace_id.to_string(),
        resident.id.to_string()
    ])?;
    let mut decisions = Vec::new();
    while let Some(row) = decision_rows.next()? {
        decisions.push(read_decision_row(row)?);
    }
    let mut current = ResidentLifecycle::Inactive;
    let mut predecessor = None;
    let mut updated_at_ms = resident.created_at_ms;
    for decision in &decisions {
        if decision.workspace_id != resident.workspace_id
            || decision.resident_id != resident.id
            || decision.from_lifecycle != current
            || decision.predecessor_id != predecessor
            || decision.decided_by != ResidentLifecycleActor::Operator
            || decision.decided_at_ms < updated_at_ms
        {
            return Err(ResidentLifecycleError::InvalidStore);
        }
        event_kind(decision)?;
        let payload = serde_json::to_string(&ResidentLifecycleDecisionPayload {
            decision: decision.clone(),
        })?;
        let event_count: i64 = connection.query_row(
            "SELECT count(*) FROM events
                 WHERE workspace_id = ?1 AND kind = ?2 AND payload_json = ?3
                   AND occurred_at_ms = ?4",
            params![
                resident.workspace_id.to_string(),
                event_kind(decision)?,
                payload,
                i64::try_from(decision.decided_at_ms)
                    .map_err(|_| ResidentLifecycleError::InvalidStore)?,
            ],
            |row| row.get(0),
        )?;
        if event_count != 1 {
            return Err(ResidentLifecycleError::InvalidStore);
        }
        current = decision.to_lifecycle;
        predecessor = Some(decision.id);
        updated_at_ms = decision.decided_at_ms;
        verified_decisions += 1;
    }
    if current != resident.lifecycle
        || predecessor != resident.latest_decision_id
        || updated_at_ms != resident.updated_at_ms
    {
        return Err(ResidentLifecycleError::InvalidStore);
    }
    Ok(verified_decisions)
}

fn event_kind(
    decision: &ResidentLifecycleDecision,
) -> Result<&'static str, ResidentLifecycleError> {
    match (decision.from_lifecycle, decision.to_lifecycle) {
        (ResidentLifecycle::Inactive, ResidentLifecycle::Active) => Ok("resident.activated"),
        (ResidentLifecycle::Active, ResidentLifecycle::Suspended) => Ok("resident.suspended"),
        (ResidentLifecycle::Suspended, ResidentLifecycle::Active) => Ok("resident.reactivated"),
        (
            ResidentLifecycle::Inactive | ResidentLifecycle::Active | ResidentLifecycle::Suspended,
            ResidentLifecycle::Retired,
        ) => Ok("resident.retired"),
        _ => Err(ResidentLifecycleError::InvalidStore),
    }
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()` (the fold at 30d2a20): an exhaustive `name()` catches an
    /// added arm, only a second table catches a wrong or duplicated name.
    fn expected_name(error: &ResidentLifecycleError) -> &'static str {
        match error {
            ResidentLifecycleError::Account(_) => "Account",
            ResidentLifecycleError::NotFound => "NotFound",
            ResidentLifecycleError::InvalidTransition => "InvalidTransition",
            ResidentLifecycleError::StaleDecision => "StaleDecision",
            ResidentLifecycleError::InvalidStore => "InvalidStore",
            ResidentLifecycleError::Store(_) => "Store",
            ResidentLifecycleError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_resident_lifecycle_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            ResidentLifecycleError::Account(
                crate::operator_account::OperatorAccountError::PasswordTooShort,
            ),
            ResidentLifecycleError::NotFound,
            ResidentLifecycleError::InvalidTransition,
            ResidentLifecycleError::StaleDecision,
            ResidentLifecycleError::InvalidStore,
            ResidentLifecycleError::Store(rusqlite::Error::QueryReturnedNoRows),
            ResidentLifecycleError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            7,
            "an arm was added to `ResidentLifecycleError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(ResidentLifecycleError::name).collect();
        assert_eq!(
            names.len(),
            7,
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
