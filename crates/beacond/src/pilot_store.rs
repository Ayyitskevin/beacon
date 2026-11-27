//! Inert transactional storage. The schema installer is test-only; production startup is unchanged.
use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pilot_admission::{AdmissionBook, Settlement};
use crate::pilot_authority::{
    MAX_RECORD_BYTES, OperationIntent, PilotAuthority, PilotError, Units, WorkspaceAllocation,
    identity,
};

const MAX_RECORDS: usize = 256;
const MAX_EVENT_BYTES: i64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredRecord {
    pub digest: [u8; 32],
    pub kind: String,
    pub workspace: Uuid,
    pub project: Option<Uuid>,
    pub canonical: Vec<u8>,
    pub event_sequence: i64,
    pub occurred_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordEvent {
    version: u32,
    owner_basis: String,
    kind: String,
    digest_algorithm: String,
    digest_canonicalizer: String,
    digest: [u8; 32],
    byte_length: u64,
    scope: RecordScope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum RecordScope {
    Workspace,
    Project { project: Uuid },
}
impl RecordScope {
    fn from_project(project: Option<Uuid>) -> Self {
        match project {
            None => Self::Workspace,
            Some(project) => Self::Project { project },
        }
    }
    fn project(&self) -> Option<Uuid> {
        match self {
            Self::Workspace => None,
            Self::Project { project } => Some(*project),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Usage {
    Unknown,
    Measured { units: [u64; 12] },
}
impl Usage {
    fn measured(self) -> Option<[u64; 12]> {
        match self {
            Self::Unknown => None,
            Self::Measured { units } => Some(units),
        }
    }
    #[cfg(test)]
    fn from_measured(value: Option<Units>) -> Self {
        match value {
            None => Self::Unknown,
            Some(units) => Self::Measured { units: units.0 },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Revocation {
    pub canonicalizer: String,
    pub version: u32,
    pub authority: [u8; 32],
    pub reason: String,
    pub stopping: Vec<Uuid>,
}

/// Fixed-field serde JSON, exact re-serialization required; a domain observation, not a permit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Observation {
    pub canonicalizer: String,
    pub version: u32,
    pub operation: Uuid,
    pub ordinal: u32,
    pub management_terminal: bool,
    pub physical_complete: bool,
    pub capture_complete: bool,
    pub usage: Usage,
}
impl Observation {
    fn decode(bytes: &[u8]) -> Result<Self, PilotError> {
        let o: Self = parse_json(bytes)?;
        if o.canonicalizer != "beacon.pilot.observation.v1"
            || o.version != 1
            || o.ordinal == 0
            || o.usage.measured().is_some_and(|u| u[0] != 1 || u[9] != 0)
        {
            return Err(PilotError::InvalidStore);
        }
        Ok(o)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UncertaintyReview {
    pub canonicalizer: String,
    pub version: u32,
    pub operation: Uuid,
    pub reason: String,
}

fn event_kind(kind: &str) -> Result<&'static str, PilotError> {
    match kind {
        "workspace" => Ok("pilot.workspace_published"),
        "authority" => Ok("pilot.authority_published"),
        "revocation" => Ok("pilot.authority_revoked"),
        "claim" => Ok("pilot.operation_claimed"),
        "observation" => Ok("pilot.operation_observed"),
        "review" => Ok("pilot.uncertainty_reviewed"),
        _ => Err(PilotError::InvalidStore),
    }
}
fn canonicalizer(kind: &str) -> Result<&'static str, PilotError> {
    match kind {
        "workspace" => Ok(crate::pilot_authority::WORKSPACE_CANONICALIZER),
        "authority" => Ok(crate::pilot_authority::AUTHORITY_CANONICALIZER),
        "claim" => Ok(crate::pilot_authority::OPERATION_CANONICALIZER),
        "revocation" => Ok("beacon.pilot.revocation.v1"),
        "observation" => Ok("beacon.pilot.observation.v1"),
        "review" => Ok("beacon.pilot.uncertainty.v1"),
        _ => Err(PilotError::InvalidStore),
    }
}
fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, PilotError> {
    let bytes = serde_json::to_vec(value).map_err(|_| PilotError::InvalidBytes)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(PilotError::InvalidBytes);
    }
    Ok(bytes)
}
fn parse_json<T: for<'de> Deserialize<'de> + Serialize>(bytes: &[u8]) -> Result<T, PilotError> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(PilotError::InvalidStore);
    }
    let value: T = serde_json::from_slice(bytes).map_err(|_| PilotError::InvalidStore)?;
    if json_bytes(&value)? != bytes {
        return Err(PilotError::InvalidStore);
    }
    Ok(value)
}
fn reason_valid(reason: &str) -> bool {
    !reason.trim().is_empty()
        && reason.len() <= 1024
        && reason.bytes().all(|b| (0x20..=0x7e).contains(&b))
}
fn table_present(connection: &Connection) -> Result<(), PilotError> {
    let count: i64 = connection.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('pilot_records','pilot_heads')", [], |r|r.get(0))?;
    if count != 2 {
        return Err(PilotError::FixtureSchemaUnavailable);
    }
    Ok(())
}
fn head(connection: &Connection, scope: &str) -> Result<Option<[u8; 32]>, PilotError> {
    let value: Option<Vec<u8>> = connection
        .query_row(
            "SELECT digest FROM pilot_heads WHERE scope=?1",
            [scope],
            |r| r.get(0),
        )
        .optional()?;
    value
        .map(|v| v.try_into().map_err(|_| PilotError::InvalidStore))
        .transpose()
}
fn scope(workspace: Uuid, project: Option<Uuid>) -> String {
    project.map_or_else(
        || format!("workspace:{workspace}"),
        |p| format!("project:{workspace}:{p}"),
    )
}

/// Bounded reconciliation is observational. The caller supplies a read or write transaction.
pub fn reconcile(connection: &Connection) -> Result<Vec<StoredRecord>, PilotError> {
    table_present(connection)?;
    if !crate::operator_account::owner_account_exists(connection)? {
        return Err(PilotError::InvalidStore);
    }
    let (count, bytes, largest): (i64,i64,i64) = connection.query_row(
        "SELECT count(*),coalesce(sum(length(canonical_bytes)),0),coalesce(max(length(canonical_bytes)),0) FROM pilot_records", [], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if count > MAX_RECORDS as i64
        || bytes > (MAX_RECORDS * MAX_RECORD_BYTES) as i64
        || largest > MAX_RECORD_BYTES as i64
    {
        return Err(PilotError::InvalidStore);
    }
    let oversized:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM pilot_records WHERE length(kind)>32 OR length(workspace_id)>36 OR length(scope_json)>128 OR length(owner_basis)>16 OR length(digest)!=32 OR length(digest_algorithm)>16 OR length(digest_canonicalizer)>64)",[],|r|r.get(0))?;
    let bad_heads:bool=connection.query_row("SELECT count(*)>256 OR coalesce(max(length(scope)>128 OR length(digest)!=32),0) FROM pilot_heads",[],|r|r.get(0))?;
    if oversized || bad_heads {
        return Err(PilotError::InvalidStore);
    }
    let (events, payload_bytes, largest_event): (i64,i64,i64) = connection.query_row(
        "SELECT count(*),coalesce(sum(length(CAST(payload_json AS BLOB))),0),coalesce(max(length(CAST(payload_json AS BLOB))),0) FROM events", [], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if events > 10_000 || payload_bytes > MAX_EVENT_BYTES || largest_event > MAX_RECORD_BYTES as i64
    {
        return Err(PilotError::InvalidStore);
    }
    if crate::event_chain::verify_chain(connection)?.is_some()
        || crate::dispatch::reconcile_pause_state(connection)?.is_some()
    {
        return Err(PilotError::InvalidStore);
    }
    let mut statement = connection.prepare("SELECT digest,kind,workspace_id,scope_json,canonical_bytes,event_sequence,occurred_at_ms,owner_basis,digest_algorithm,digest_canonicalizer FROM pilot_records ORDER BY event_sequence")?;
    let mut rows = statement.query([])?;
    let mut records = Vec::new();
    let mut heads: BTreeMap<String, [u8; 32]> = BTreeMap::new();
    let mut revoked = BTreeSet::new();
    let mut claims: BTreeMap<Uuid, OperationIntent> = BTreeMap::new();
    let mut observations: BTreeMap<Uuid, Observation> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let digest: Vec<u8> = row.get(0)?;
        let workspace: String = row.get(2)?;
        let scope_text: String = row.get(3)?;
        let record_scope: RecordScope = parse_json(scope_text.as_bytes())?;
        let record = StoredRecord {
            digest: digest.try_into().map_err(|_| PilotError::InvalidStore)?,
            kind: row.get(1)?,
            workspace: Uuid::parse_str(&workspace).map_err(|_| PilotError::InvalidStore)?,
            project: record_scope.project(),
            canonical: row.get(4)?,
            event_sequence: row.get(5)?,
            occurred_at_ms: row.get(6)?,
        };
        if row.get::<_, String>(7)? != "owner"
            || workspace != record.workspace.to_string()
            || identity(&record.canonical) != record.digest
            || row.get::<_, String>(8)? != "sha256"
            || row.get::<_, String>(9)? != canonicalizer(&record.kind)?
            || record.occurred_at_ms < 0
        {
            return Err(PilotError::InvalidStore);
        }
        let (ew, ek, ep, et): (String, String, String, i64) = connection.query_row(
            "SELECT workspace_id,kind,payload_json,occurred_at_ms FROM events WHERE sequence=?1",
            [record.event_sequence],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let expected = RecordEvent {
            version: 1,
            owner_basis: "owner".into(),
            kind: record.kind.clone(),
            digest_algorithm: "sha256".into(),
            digest_canonicalizer: canonicalizer(&record.kind)?.into(),
            digest: record.digest,
            byte_length: record.canonical.len() as u64,
            scope: record_scope,
        };
        if ew != workspace
            || ek != event_kind(&record.kind)?
            || et != record.occurred_at_ms
            || parse_json::<RecordEvent>(ep.as_bytes())? != expected
        {
            return Err(PilotError::InvalidStore);
        }
        match record.kind.as_str() {
            "workspace" => {
                let w = WorkspaceAllocation::decode(&record.canonical)?;
                if record.project.is_some()
                    || w.workspace != record.workspace
                    || w.predecessor != heads.get(&scope(w.workspace, None)).copied()
                {
                    return Err(PilotError::InvalidStore);
                }
                heads.insert(scope(w.workspace, None), record.digest);
            }
            "authority" => {
                let a = PilotAuthority::decode(&record.canonical)?;
                if record.project != Some(a.project)
                    || a.workspace != record.workspace
                    || a.predecessor != heads.get(&scope(a.workspace, Some(a.project))).copied()
                    || Some(a.workspace_allocation) != heads.get(&scope(a.workspace, None)).copied()
                {
                    return Err(PilotError::InvalidStore);
                }
                let parent = records
                    .iter()
                    .find(|r: &&StoredRecord| r.digest == a.workspace_allocation)
                    .ok_or(PilotError::InvalidStore)?;
                let p = WorkspaceAllocation::decode(&parent.canonical)?;
                let mut allocated = a.ceiling;
                for old in records.iter().filter(|r| {
                    r.kind == "authority"
                        && r.workspace == a.workspace
                        && r.project != Some(a.project)
                }) {
                    if heads.get(&scope(old.workspace, old.project)) == Some(&old.digest) {
                        allocated = allocated
                            .checked_add(PilotAuthority::decode(&old.canonical)?.ceiling)?;
                    }
                }
                if !allocated.fits(p.ceiling) {
                    return Err(PilotError::InvalidStore);
                }
                heads.insert(scope(a.workspace, Some(a.project)), record.digest);
            }
            "revocation" => {
                let r: Revocation = parse_json(&record.canonical)?;
                let stopping: Vec<Uuid> = claims
                    .iter()
                    .filter_map(|(id, o)| {
                        (o.authority == r.authority
                            && !observations.get(id).is_some_and(observation_complete))
                        .then_some(*id)
                    })
                    .collect();
                if r.canonicalizer != "beacon.pilot.revocation.v1"
                    || r.version != 1
                    || !reason_valid(&r.reason)
                    || r.stopping != stopping
                    || r.stopping.len() > 1
                    || !records.iter().any(|x| {
                        x.digest == r.authority
                            && x.kind == "authority"
                            && x.workspace == record.workspace
                            && x.project == record.project
                    })
                    || !revoked.insert(r.authority)
                {
                    return Err(PilotError::InvalidStore);
                }
            }
            "claim" => {
                let o = OperationIntent::decode(&record.canonical)?;
                let authority = PilotAuthority::decode(
                    &records
                        .iter()
                        .find(|r| r.kind == "authority" && r.digest == o.authority)
                        .ok_or(PilotError::InvalidStore)?
                        .canonical,
                )?;
                if record.workspace != o.workspace
                    || record.project != Some(o.project)
                    || revoked.contains(&o.authority)
                    || heads.get(&scope(o.workspace, Some(o.project))) != Some(&o.authority)
                    || heads.get(&scope(o.workspace, None)) != Some(&authority.workspace_allocation)
                    || claims.contains_key(&o.operation)
                    || claims
                        .keys()
                        .any(|id| !observations.get(id).is_some_and(observation_complete))
                {
                    return Err(PilotError::InvalidStore);
                }
                claims.insert(o.operation, o);
            }
            "observation" => {
                let observation = Observation::decode(&record.canonical)?;
                let old = observations.get(&observation.operation);
                let claim = claims
                    .get(&observation.operation)
                    .ok_or(PilotError::InvalidStore)?;
                if observation.canonicalizer != "beacon.pilot.observation.v1"
                    || observation.version != 1
                    || record.workspace != claim.workspace
                    || record.project != Some(claim.project)
                    || observation.ordinal != old.map_or(1, |o| o.ordinal + 1)
                    || old.is_some_and(observation_complete)
                    || old.is_some_and(|o| {
                        o.management_terminal && !observation.management_terminal
                            || o.physical_complete && !observation.physical_complete
                            || o.capture_complete && !observation.capture_complete
                    })
                    || old.is_some_and(|o| {
                        o.usage.measured().is_some_and(|u| {
                            observation
                                .usage
                                .measured()
                                .is_none_or(|new| !Units(u).fits(Units(new)))
                        })
                    })
                    || observation
                        .usage
                        .measured()
                        .is_some_and(|u| u[0] != 1 || !Units(u).fits(claim.reservation))
                {
                    return Err(PilotError::InvalidStore);
                }
                observations.insert(observation.operation, observation);
            }
            "review" => {
                let r: UncertaintyReview = parse_json(&record.canonical)?;
                let claim = claims.get(&r.operation).ok_or(PilotError::InvalidStore)?;
                if r.canonicalizer != "beacon.pilot.uncertainty.v1"
                    || r.version != 1
                    || !reason_valid(&r.reason)
                    || record.workspace != claim.workspace
                    || record.project != Some(claim.project)
                {
                    return Err(PilotError::InvalidStore);
                }
            }
            _ => return Err(PilotError::InvalidStore),
        }
        records.push(record);
    }
    let event_count: i64 = connection.query_row(
        "SELECT count(*) FROM events WHERE kind LIKE 'pilot.%'",
        [],
        |r| r.get(0),
    )?;
    if event_count != records.len() as i64 {
        return Err(PilotError::InvalidStore);
    }
    let mut stored = connection.prepare("SELECT scope,digest FROM pilot_heads ORDER BY scope")?;
    let actual = stored
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if actual.len() != heads.len()
        || actual
            .iter()
            .any(|(s, d)| heads.get(s).is_none_or(|h| h.as_slice() != d))
    {
        return Err(PilotError::InvalidStore);
    }
    replay_records(connection, &records)?;
    Ok(records)
}
fn observation_complete(o: &Observation) -> bool {
    o.physical_complete && o.capture_complete && o.usage.measured().is_some()
}

fn replay_records(
    connection: &Connection,
    records: &[StoredRecord],
) -> Result<AdmissionBook, PilotError> {
    let mut book = AdmissionBook::new();
    for record in records {
        book.observe_retained_wall(record.occurred_at_ms as u64)?;
        match record.kind.as_str() {
            "claim" => {
                let intent = OperationIntent::decode(&record.canonical)?;
                let a_record = records
                    .iter()
                    .find(|r| {
                        r.digest == intent.authority && r.event_sequence < record.event_sequence
                    })
                    .ok_or(PilotError::InvalidStore)?;
                let authority = PilotAuthority::decode(&a_record.canonical)?;
                let p_record = records
                    .iter()
                    .find(|r| {
                        r.digest == authority.workspace_allocation
                            && r.event_sequence < a_record.event_sequence
                    })
                    .ok_or(PilotError::InvalidStore)?;
                let parent = WorkspaceAllocation::decode(&p_record.canonical)?;
                // Compare the parent at this claim's event position, never today's final head.
                // Valid historical claims remain valid after a later Workspace successor.
                let current_parent = records
                    .iter()
                    .rev()
                    .find(|r| {
                        r.kind == "workspace"
                            && r.workspace == intent.workspace
                            && r.event_sequence < record.event_sequence
                    })
                    .ok_or(PilotError::InvalidStore)?;
                if current_parent.digest != authority.workspace_allocation {
                    return Err(PilotError::InvalidStore);
                }
                if intent.workspace != authority.workspace
                    || intent.project != authority.project
                    || intent.actor != authority.actors[intent.role.index()]
                    || record.occurred_at_ms as u64 >= authority.expires_at_ms
                    || (record.occurred_at_ms as u64) < authority.issued_at_ms
                    || record.occurred_at_ms as u64 >= parent.expires_at_ms
                    || (record.occurred_at_ms as u64) < parent.issued_at_ms
                {
                    return Err(PilotError::InvalidStore);
                }
                let pause:Option<(i64,String)>=connection.query_row("SELECT sequence,kind FROM events WHERE sequence<?1 AND kind IN ('installation.paused','installation.resumed') ORDER BY sequence DESC LIMIT 1",[record.event_sequence],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                if pause
                    .as_ref()
                    .is_some_and(|(_, kind)| kind == "installation.paused")
                    || pause.as_ref().map_or(0, |(seq, _)| *seq as u64)
                        != authority.pause_generation
                {
                    return Err(PilotError::InvalidStore);
                }
                book.verify_declared_budget(&intent, &authority, &parent)?;
                book.restore_claim(intent)?;
            }
            "observation" => {
                let o = Observation::decode(&record.canonical)?;
                match book.settle(
                    o.operation,
                    Settlement {
                        management_terminal: o.management_terminal,
                        physical_complete: o.physical_complete,
                        capture_complete: o.capture_complete,
                        measured: o.usage.measured().map(Units),
                    },
                ) {
                    Ok(()) | Err(PilotError::UnknownCustody) => (),
                    Err(e) => return Err(e),
                }
            }
            "review" => {
                let r: UncertaintyReview = parse_json(&record.canonical)?;
                book.restore_review(r.operation)?;
            }
            "revocation" => {
                let r: Revocation = parse_json(&record.canonical)?;
                for id in r.stopping {
                    book.begin_stopping(id)?;
                }
            }
            _ => (),
        }
    }
    // A durable Pause after a claim selects that outstanding operation even across a reopen.
    for id in book.outstanding() {
        let claim = records
            .iter()
            .find(|r| {
                r.kind == "claim"
                    && OperationIntent::decode(&r.canonical).is_ok_and(|o| o.operation == id)
            })
            .ok_or(PilotError::InvalidStore)?;
        let paused_since: i64 = connection.query_row(
            "SELECT count(*) FROM events WHERE kind='installation.paused' AND sequence>?1",
            [claim.event_sequence],
            |r| r.get(0),
        )?;
        if paused_since > 0 {
            book.begin_stopping(id)?;
        }
    }
    book.after_restart();
    Ok(book)
}
pub(crate) fn restore_book(connection: &Connection) -> Result<AdmissionBook, PilotError> {
    let records = reconcile(connection)?;
    replay_records(connection, &records)
}

fn append(
    connection: &Connection,
    workspace: Uuid,
    project: Option<Uuid>,
    kind: &str,
    canonical: &[u8],
    now: i64,
) -> Result<[u8; 32], PilotError> {
    table_present(connection)?;
    if canonical.is_empty() || canonical.len() > MAX_RECORD_BYTES || now < 0 {
        return Err(PilotError::InvalidBytes);
    }
    let digest = identity(canonical);
    let record_scope = RecordScope::from_project(project);
    let scope_json =
        String::from_utf8(json_bytes(&record_scope)?).map_err(|_| PilotError::InvalidBytes)?;
    let payload = json_bytes(&RecordEvent {
        version: 1,
        owner_basis: "owner".into(),
        kind: kind.into(),
        digest_algorithm: "sha256".into(),
        digest_canonicalizer: canonicalizer(kind)?.into(),
        digest,
        byte_length: canonical.len() as u64,
        scope: record_scope,
    })?;
    let payload = std::str::from_utf8(&payload).map_err(|_| PilotError::InvalidBytes)?;
    let sequence = crate::event_chain::append_event(
        connection,
        &workspace.to_string(),
        event_kind(kind)?,
        payload,
        now,
    )?;
    connection.execute("INSERT INTO pilot_records(digest,kind,workspace_id,scope_json,canonical_bytes,event_sequence,occurred_at_ms,owner_basis,digest_algorithm,digest_canonicalizer) VALUES(?1,?2,?3,?4,?5,?6,?7,'owner','sha256',?8)",
        params![digest.as_slice(),kind,workspace.to_string(),scope_json,canonical,sequence,now,canonicalizer(kind)?])?;
    Ok(digest)
}
pub(crate) fn publish_workspace(
    connection: &Connection,
    workspace: Uuid,
    bytes: &[u8],
    now: i64,
) -> Result<[u8; 32], PilotError> {
    let records = reconcile(connection)?;
    if !replay_records(connection, &records)?
        .outstanding()
        .is_empty()
    {
        return Err(PilotError::OutstandingCustody);
    }
    let w = WorkspaceAllocation::decode(bytes)?;
    if w.workspace != workspace || w.predecessor != head(connection, &scope(workspace, None))? {
        return Err(PilotError::AuthorityDrift);
    }
    if (now as u64) < w.issued_at_ms || (now as u64) >= w.expires_at_ms {
        return Err(PilotError::Expired);
    }
    let digest = append(connection, workspace, None, "workspace", bytes, now)?;
    let changed = match w.predecessor {
        None => connection.execute(
            "INSERT INTO pilot_heads(scope,digest) VALUES(?1,?2)",
            params![scope(workspace, None), digest.as_slice()],
        )?,
        Some(predecessor) => connection.execute(
            "UPDATE pilot_heads SET digest=?2 WHERE scope=?1 AND digest=?3",
            params![
                scope(workspace, None),
                digest.as_slice(),
                predecessor.as_slice()
            ],
        )?,
    };
    if changed != 1 {
        return Err(PilotError::AuthorityDrift);
    }
    reconcile(connection)?;
    Ok(digest)
}
pub(crate) fn publish_authority(
    connection: &Connection,
    authority: &PilotAuthority,
    now: i64,
) -> Result<[u8; 32], PilotError> {
    let records = reconcile(connection)?;
    if !replay_records(connection, &records)?
        .outstanding()
        .is_empty()
    {
        return Err(PilotError::OutstandingCustody);
    }
    if authority.predecessor
        != head(
            connection,
            &scope(authority.workspace, Some(authority.project)),
        )?
        || Some(authority.workspace_allocation)
            != head(connection, &scope(authority.workspace, None))?
    {
        return Err(PilotError::AuthorityDrift);
    }
    let parent = records
        .iter()
        .find(|r| r.digest == authority.workspace_allocation)
        .ok_or(PilotError::MissingAuthority)?;
    let parent = WorkspaceAllocation::decode(&parent.canonical)?;
    let mut total = authority.ceiling;
    for record in records.iter().filter(|r| {
        r.kind == "authority"
            && r.workspace == authority.workspace
            && r.project != Some(authority.project)
    }) {
        if head(connection, &scope(record.workspace, record.project))? == Some(record.digest) {
            total = total.checked_add(PilotAuthority::decode(&record.canonical)?.ceiling)?;
        }
    }
    if !total.fits(parent.ceiling) {
        return Err(PilotError::InvalidBudget);
    }
    let digest = append(
        connection,
        authority.workspace,
        Some(authority.project),
        "authority",
        &authority.canonical_bytes()?,
        now,
    )?;
    let changed = match authority.predecessor {
        None => connection.execute(
            "INSERT INTO pilot_heads(scope,digest) VALUES(?1,?2)",
            params![
                scope(authority.workspace, Some(authority.project)),
                digest.as_slice()
            ],
        )?,
        Some(predecessor) => connection.execute(
            "UPDATE pilot_heads SET digest=?2 WHERE scope=?1 AND digest=?3",
            params![
                scope(authority.workspace, Some(authority.project)),
                digest.as_slice(),
                predecessor.as_slice()
            ],
        )?,
    };
    if changed != 1 {
        return Err(PilotError::AuthorityDrift);
    }
    reconcile(connection)?;
    Ok(digest)
}
pub(crate) fn revoke_authority(
    connection: &Connection,
    workspace: Uuid,
    project: Uuid,
    expected: [u8; 32],
    reason: &str,
    now: i64,
) -> Result<(), PilotError> {
    let records = reconcile(connection)?;
    if head(connection, &scope(workspace, Some(project)))? != Some(expected) {
        return Err(PilotError::AuthorityDrift);
    }
    if !reason_valid(reason) {
        return Err(PilotError::InvalidBytes);
    }
    let mut stopping = Vec::new();
    for record in records.iter().filter(|r| r.kind == "claim") {
        let claim = OperationIntent::decode(&record.canonical)?;
        if claim.authority == expected {
            let latest = records
                .iter()
                .filter(|r| r.kind == "observation")
                .map(|r| parse_json::<Observation>(&r.canonical))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .rev()
                .find(|o| o.operation == claim.operation);
            if !latest.as_ref().is_some_and(observation_complete) {
                stopping.push(claim.operation);
            }
        }
    }
    stopping.sort();
    append(
        connection,
        workspace,
        Some(project),
        "revocation",
        &json_bytes(&Revocation {
            canonicalizer: "beacon.pilot.revocation.v1".into(),
            version: 1,
            authority: expected,
            reason: reason.into(),
            stopping,
        })?,
        now,
    )?;
    reconcile(connection)?;
    Ok(())
}
pub(crate) fn review_uncertainty(
    connection: &Connection,
    workspace: Uuid,
    project: Uuid,
    operation: Uuid,
    reason: &str,
    now: i64,
) -> Result<(), PilotError> {
    let records = reconcile(connection)?;
    if !reason_valid(reason)
        || !records.iter().filter(|r| r.kind == "claim").any(|r| {
            r.workspace == workspace
                && r.project == Some(project)
                && OperationIntent::decode(&r.canonical).is_ok_and(|o| o.operation == operation)
        })
    {
        return Err(PilotError::InvalidScope);
    }
    append(
        connection,
        workspace,
        Some(project),
        "review",
        &json_bytes(&UncertaintyReview {
            canonicalizer: "beacon.pilot.uncertainty.v1".into(),
            version: 1,
            operation,
            reason: reason.into(),
        })?,
        now,
    )?;
    reconcile(connection)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pilot_admission::CurrentSnapshot;
    use crate::pilot_authority::{Descriptor, test_vectors};
    use crate::{OperatorAccountService, SecretString, StartupConfig, Supervisor};
    use std::{path::PathBuf, sync::Arc};
    type HistoryRow = (i64, String, String, Option<Vec<u8>>, Option<Vec<u8>>);
    struct Fixture {
        path: PathBuf,
        accounts: Arc<OperatorAccountService>,
        workspace: Uuid,
    }
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("beacond-i1-{}.sqlite3", Uuid::new_v4()));
            let startup = Supervisor::bootstrap(StartupConfig {
                store_path: path.clone(),
                initial_workspace_name: "I1 synthetic".into(),
            })
            .unwrap();
            let accounts = Arc::new(OperatorAccountService::open(&path).unwrap());
            accounts
                .begin_password_setup(
                    SecretString::new("a long local passphrase".into()),
                    SecretString::new("a long local passphrase".into()),
                )
                .unwrap();
            accounts.complete_operator_setup(true).unwrap();
            accounts.with_authenticated_store::<_,PilotError>(|connection,_| {
                let tx=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let old:Vec<HistoryRow> =tx.prepare("SELECT sequence,kind,payload_json,predecessor_digest,link_digest FROM events ORDER BY sequence")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;
                tx.execute_batch(include_str!("../tests/fixtures/i1/schema.sql"))?;
                let retained:Vec<HistoryRow> =tx.prepare("SELECT sequence,kind,payload_json,predecessor_digest,link_digest FROM events ORDER BY sequence")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?.collect::<rusqlite::Result<_>>()?;
                assert_eq!(old,retained);
                tx.commit()?; Ok(())
            }).unwrap();
            Self {
                path,
                accounts,
                workspace: startup.workspace.id,
            }
        }
        fn seed(&self) -> ([u8; 32], PilotAuthority) {
            self.accounts
                .with_authenticated_store::<_, PilotError>(|connection, now| {
                    let tx = connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                    let w = WorkspaceAllocation {
                        workspace: self.workspace,
                        predecessor: None,
                        issued_at_ms: now as u64,
                        expires_at_ms: now as u64 + 60_000,
                        ceiling: Units([
                            100, 1000, 1000, 1000, 1000, 1000, 1000, 1000, 10_000, 0, 1000, 1000,
                        ]),
                    };
                    let parent =
                        publish_workspace(&tx, self.workspace, &w.canonical_bytes()?, now)?;
                    let mut a = test_vectors::authority();
                    a.workspace = self.workspace;
                    a.workspace_allocation = parent;
                    a.issued_at_ms = now as u64;
                    a.expires_at_ms = now as u64 + 60_000;
                    a.descriptors = std::array::from_fn(|_| Descriptor::Bound {
                        kind: "fixture-only".into(),
                        version: 1,
                        canonicalizer: "fixture.v1".into(),
                        digest: [7; 32],
                    });
                    // TEST ONLY: no claim that synthetic Resident/configuration IDs resolve production owners.
                    let digest = publish_authority(&tx, &a, now)?;
                    tx.commit()?;
                    Ok((digest, a))
                })
                .unwrap()
        }
        fn intent(&self, digest: [u8; 32]) -> OperationIntent {
            let mut intent = test_vectors::operation();
            intent.workspace = self.workspace;
            intent.authority = digest;
            intent
        }
        fn successor_workspace(&self, ceiling: Units) -> [u8; 32] {
            self.accounts
                .with_authenticated_store::<_, PilotError>(|connection, now| {
                    let tx = connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                    let records = reconcile(&tx)?;
                    let old = records
                        .iter()
                        .rev()
                        .find(|r| r.kind == "workspace")
                        .unwrap();
                    let mut parent = WorkspaceAllocation::decode(&old.canonical)?;
                    parent.predecessor = Some(old.digest);
                    parent.issued_at_ms = now as u64;
                    parent.expires_at_ms = now as u64 + 60_000;
                    parent.ceiling = ceiling;
                    let digest =
                        publish_workspace(&tx, self.workspace, &parent.canonical_bytes()?, now)?;
                    tx.commit()?;
                    Ok(digest)
                })
                .unwrap()
        }
        fn claim(&self, intent: &OperationIntent) -> Result<(), PilotError> {
            self.accounts.with_authenticated_store(|connection, now| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let records = reconcile(&tx)?;
                let authority = PilotAuthority::decode(
                    &records
                        .iter()
                        .find(|r| r.digest == intent.authority && r.kind == "authority")
                        .ok_or(PilotError::MissingAuthority)?
                        .canonical,
                )?;
                if head(&tx, &scope(intent.workspace, None))?
                    != Some(authority.workspace_allocation)
                {
                    return Err(PilotError::AuthorityDrift);
                }
                let parent = WorkspaceAllocation::decode(
                    &records
                        .iter()
                        .find(|r| r.digest == authority.workspace_allocation)
                        .ok_or(PilotError::MissingAuthority)?
                        .canonical,
                )?;
                if records
                    .iter()
                    .filter(|r| r.kind == "revocation")
                    .map(|r| parse_json::<Revocation>(&r.canonical))
                    .collect::<Result<Vec<_>, _>>()?
                    .iter()
                    .any(|r| r.authority == intent.authority)
                {
                    return Err(PilotError::AuthorityDrift);
                }
                let pause = crate::dispatch::verified_pilot_pause(&tx)
                    .map_err(|_| PilotError::InvalidStore)?;
                // This constructor and command exist ONLY in this cfg(test) module.
                let current = CurrentSnapshot::fixture(
                    authority,
                    parent,
                    intent,
                    pause,
                    (now as u64, Uuid::from_bytes([9; 16]), 10),
                );
                let mut book = replay_records(&tx, &records)?;
                book.claim(intent, &current)?;
                append(
                    &tx,
                    intent.workspace,
                    Some(intent.project),
                    "claim",
                    &intent.canonical_bytes()?,
                    now,
                )?;
                reconcile(&tx)?;
                tx.commit()?;
                Ok(())
            })
        }
        fn observe(
            &self,
            intent: &OperationIntent,
            observation: Settlement,
        ) -> Result<bool, PilotError> {
            self.accounts.with_authenticated_store(|connection, now| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let records = reconcile(&tx)?;
                let mut book = replay_records(&tx, &records)?;
                match book.settle(intent.operation, observation) {
                    Ok(()) | Err(PilotError::UnknownCustody) => (),
                    Err(e) => return Err(e),
                }
                let state = book
                    .operation(intent.operation)
                    .ok_or(PilotError::InvalidStore)?;
                let ordinal = records
                    .iter()
                    .filter(|r| r.kind == "observation")
                    .map(|r| parse_json::<Observation>(&r.canonical))
                    .collect::<Result<Vec<_>, _>>()?
                    .iter()
                    .filter(|o| o.operation == intent.operation)
                    .count() as u32
                    + 1;
                let o = Observation {
                    canonicalizer: "beacon.pilot.observation.v1".into(),
                    version: 1,
                    operation: intent.operation,
                    ordinal,
                    management_terminal: state.management_terminal,
                    physical_complete: state.physical_complete,
                    capture_complete: state.capture_complete,
                    usage: Usage::from_measured(state.measured),
                };
                append(
                    &tx,
                    intent.workspace,
                    Some(intent.project),
                    "observation",
                    &json_bytes(&o)?,
                    now,
                )?;
                reconcile(&tx)?;
                tx.commit()?;
                Ok(state.closed)
            })
        }
        fn review(&self, intent: &OperationIntent) -> Result<(), PilotError> {
            crate::pilot_authority::PilotAuthorityService::new(
                Arc::clone(&self.accounts),
                self.workspace,
            )
            .record_uncertainty_review(
                intent.project,
                intent.operation,
                "fixture reviewed, custody still unknown",
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
            }
        }
    }
    #[test]
    fn pilot_fixture_records_reopen_reconcile_without_production_startup_claim() {
        let f = Fixture::new();
        let (_digest, a) = f.seed();
        let connection = crate::open_store_connection(&f.path).unwrap();
        let records = reconcile(&connection).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(PilotAuthority::decode(&records[1].canonical).unwrap(), a);
        assert!(OperatorAccountService::open(&f.path).is_err());
    }
    #[test]
    fn pilot_fixture_missing_parent_and_transaction_rollback() {
        let f = Fixture::new();
        f.accounts
            .with_authenticated_store::<_, PilotError>(|connection, now| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let mut a = test_vectors::authority();
                a.workspace = f.workspace;
                assert!(publish_authority(&tx, &a, now).is_err());
                tx.rollback()?;
                assert!(reconcile(connection)?.is_empty());
                Ok(())
            })
            .unwrap();
    }
    #[test]
    fn pilot_fixture_tamper_columns_bytes_missing_and_orphan_events() {
        for variant in 0..4 {
            let f = Fixture::new();
            let (digest, _) = f.seed();
            let connection = crate::open_store_connection(&f.path).unwrap();
            match variant {
                0 => {
                    connection
                        .execute(
                            "UPDATE pilot_records SET scope_json=?1 WHERE digest=?2",
                            params![
                                String::from_utf8(
                                    json_bytes(&RecordScope::Project {
                                        project: Uuid::new_v4()
                                    })
                                    .unwrap()
                                )
                                .unwrap(),
                                digest.as_slice()
                            ],
                        )
                        .unwrap();
                }
                1 => {
                    connection
                        .execute(
                            "UPDATE pilot_records SET canonical_bytes=x'00' WHERE digest=?1",
                            [digest.as_slice()],
                        )
                        .unwrap();
                }
                2 => {
                    connection.execute("DELETE FROM pilot_heads", []).unwrap();
                }
                _ => {
                    connection.execute("DELETE FROM pilot_heads", []).unwrap();
                    connection.execute("DELETE FROM pilot_records", []).unwrap();
                }
            }
            assert!(reconcile(&connection).is_err(), "tamper variant {variant}");
        }
    }
    #[test]
    fn pilot_fixture_preserves_old_events_and_refuses_event_reorder() {
        let f = Fixture::new();
        let connection = crate::open_store_connection(&f.path).unwrap();
        let before: Vec<(i64, String, String)> = connection
            .prepare("SELECT sequence,kind,payload_json FROM events ORDER BY sequence")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        f.seed();
        let after:Vec<(i64,String,String)>=connection.prepare("SELECT sequence,kind,payload_json FROM events WHERE kind NOT LIKE 'pilot.%' ORDER BY sequence").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(before, after);
        connection
            .execute(
                "UPDATE pilot_records SET event_sequence=event_sequence+100",
                [],
            )
            .unwrap();
        assert!(reconcile(&connection).is_err());
    }
    #[test]
    fn pilot_fixture_claim_reservation_reopen_unknown_and_late_settlement() {
        let f = Fixture::new();
        let (digest, _) = f.seed();
        let intent = f.intent(digest);
        f.claim(&intent).unwrap();
        assert!(
            !f.observe(
                &intent,
                Settlement {
                    management_terminal: true,
                    physical_complete: false,
                    capture_complete: true,
                    measured: None
                }
            )
            .unwrap()
        );
        f.review(&intent).unwrap();
        let connection = crate::open_store_connection(&f.path).unwrap();
        let book = restore_book(&connection).unwrap();
        let state = book.operation(intent.operation).unwrap();
        assert!(state.management_terminal);
        assert!(!state.physical_complete);
        assert!(state.measured.is_none());
        assert!(state.uncertainty_reviewed);
        assert_eq!(book.outstanding(), vec![intent.operation]);
        assert!(f.claim(&intent).is_err());
        assert!(matches!(
            f.observe(
                &intent,
                Settlement {
                    management_terminal: true,
                    physical_complete: true,
                    capture_complete: true,
                    measured: Some(Units::ZERO)
                }
            ),
            Err(PilotError::FalseRefund)
        ));
        assert!(
            f.observe(
                &intent,
                Settlement {
                    management_terminal: true,
                    physical_complete: true,
                    capture_complete: true,
                    measured: Some(intent.reservation)
                }
            )
            .unwrap()
        );
        assert!(restore_book(&connection).unwrap().outstanding().is_empty());
        assert!(f.claim(&intent).is_err());
    }
    #[test]
    fn pilot_fixture_successor_workspace_authority_and_new_id_do_not_reset_use() {
        let f = Fixture::new();
        let (first_digest, mut authority) = f.seed();
        let first = f.intent(first_digest);
        f.claim(&first).unwrap();
        assert!(
            f.observe(
                &first,
                Settlement {
                    management_terminal: true,
                    physical_complete: true,
                    capture_complete: true,
                    measured: Some(first.reservation)
                }
            )
            .unwrap()
        );
        let next_digest = f
            .accounts
            .with_authenticated_store::<_, PilotError>(|connection, now| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let records = reconcile(&tx)?;
                let old_parent = records
                    .iter()
                    .find(|r| r.digest == authority.workspace_allocation)
                    .unwrap();
                let mut parent = WorkspaceAllocation::decode(&old_parent.canonical)?;
                parent.predecessor = Some(old_parent.digest);
                parent.issued_at_ms = now as u64;
                parent.expires_at_ms = now as u64 + 60_000;
                authority.workspace_allocation =
                    publish_workspace(&tx, f.workspace, &parent.canonical_bytes()?, now)?;
                authority.predecessor = Some(first_digest);
                authority.policy_revision += 1;
                let digest = publish_authority(&tx, &authority, now)?;
                tx.commit()?;
                Ok(digest)
            })
            .unwrap();
        let mut next = f.intent(next_digest);
        next.operation = Uuid::from_bytes([0x72; 16]);
        assert!(matches!(f.claim(&next), Err(PilotError::InvalidBudget)));
        let connection = crate::open_store_connection(&f.path).unwrap();
        let records = reconcile(&connection).unwrap();
        assert_eq!(records.iter().filter(|r| r.kind == "claim").count(), 1);
        assert_eq!(
            restore_book(&connection)
                .unwrap()
                .operation(first.operation)
                .unwrap()
                .measured,
            Some(first.reservation)
        );
    }
    #[test]
    fn pilot_stale_workspace_parent_refuses_new_claim() {
        let f = Fixture::new();
        let (old_digest, _) = f.seed();
        f.successor_workspace(Units::ZERO);
        let intent = f.intent(old_digest);
        assert!(matches!(f.claim(&intent), Err(PilotError::AuthorityDrift)));
        let connection = crate::open_store_connection(&f.path).unwrap();
        assert!(
            reconcile(&connection)
                .unwrap()
                .iter()
                .all(|r| r.kind != "claim")
        );
    }
    #[test]
    fn pilot_stale_workspace_parent_replay_refuses_retained_claim() {
        let f = Fixture::new();
        let (old_digest, _) = f.seed();
        f.successor_workspace(Units::ZERO);
        let intent = f.intent(old_digest);
        f.accounts
            .with_authenticated_store::<_, PilotError>(|connection, now| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                // Deliberately bypass the fixture claim command to test the persisted reader itself.
                // This is an invalid synthetic history, never a production writer or valid admission.
                append(
                    &tx,
                    f.workspace,
                    Some(intent.project),
                    "claim",
                    &intent.canonical_bytes()?,
                    now,
                )?;
                assert!(matches!(reconcile(&tx), Err(PilotError::InvalidStore)));
                tx.rollback()?;
                Ok(())
            })
            .unwrap();
    }
    #[test]
    fn pilot_workspace_successor_preserves_valid_historical_claim() {
        let f = Fixture::new();
        let (old_digest, _) = f.seed();
        let intent = f.intent(old_digest);
        f.claim(&intent).unwrap();
        assert!(
            f.observe(
                &intent,
                Settlement {
                    management_terminal: true,
                    physical_complete: true,
                    capture_complete: true,
                    measured: Some(intent.reservation),
                }
            )
            .unwrap()
        );
        f.successor_workspace(Units::ZERO);
        let connection = crate::open_store_connection(&f.path).unwrap();
        let book = restore_book(&connection).unwrap();
        assert!(book.operation(intent.operation).unwrap().closed);
        assert_eq!(
            book.operation(intent.operation).unwrap().measured,
            Some(intent.reservation)
        );
    }
    #[test]
    fn pilot_workspace_successor_republished_authority_claims_current_parent() {
        let f = Fixture::new();
        let (old_digest, mut authority) = f.seed();
        authority.workspace_allocation = f.successor_workspace(Units([
            100, 1000, 1000, 1000, 1000, 1000, 1000, 1000, 10_000, 0, 1000, 1000,
        ]));
        authority.predecessor = Some(old_digest);
        let next = f
            .accounts
            .with_authenticated_store::<_, PilotError>(|connection, now| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let digest = publish_authority(&tx, &authority, now)?;
                tx.commit()?;
                Ok(digest)
            })
            .unwrap();
        let intent = f.intent(next);
        f.claim(&intent).unwrap();
        let connection = crate::open_store_connection(&f.path).unwrap();
        assert_eq!(
            restore_book(&connection).unwrap().outstanding(),
            vec![intent.operation]
        );
    }
    fn non_ascii_event_fixture(characters: usize, count: usize) -> (Fixture, usize) {
        let f = Fixture::new();
        let payload = serde_json::to_string(&"é".repeat(characters)).unwrap();
        assert_eq!(payload.len(), characters * 2 + 2);
        f.accounts
            .with_authenticated_store::<_, PilotError>(|connection, now| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                for _ in 0..count {
                    // Synthetic chain-valid input, not a legitimate Starter decision. The intake
                    // byte bound must run before any domain parser, even for invalid retained input.
                    crate::event_chain::append_event(
                        &tx,
                        &f.workspace.to_string(),
                        "starter_organization.discarded",
                        &payload,
                        now,
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .unwrap();
        (f, payload.len() * count)
    }
    #[test]
    fn pilot_event_utf8_record_byte_limit_refuses_non_ascii_overflow() {
        let (f, bytes) = non_ascii_event_fixture(40_000, 1);
        assert_eq!(bytes, 80_002);
        assert!(bytes > MAX_RECORD_BYTES);
        let connection = crate::open_store_connection(&f.path).unwrap();
        assert!(matches!(
            reconcile(&connection),
            Err(PilotError::InvalidStore)
        ));
    }
    #[test]
    fn pilot_event_utf8_aggregate_byte_limit_refuses_non_ascii_overflow() {
        let (f, bytes) = non_ascii_event_fixture(30_000, 280);
        assert_eq!(bytes, 16_800_560);
        const _: () = assert!(60_002 <= MAX_RECORD_BYTES);
        assert!(bytes > MAX_EVENT_BYTES as usize);
        let connection = crate::open_store_connection(&f.path).unwrap();
        assert!(matches!(
            reconcile(&connection),
            Err(PilotError::InvalidStore)
        ));
    }
    #[test]
    fn pilot_event_utf8_record_under_byte_bound_is_readable() {
        let (f, bytes) = non_ascii_event_fixture(1_000, 1);
        assert_eq!(bytes, 2_002);
        let connection = crate::open_store_connection(&f.path).unwrap();
        assert!(reconcile(&connection).unwrap().is_empty());
    }
    #[test]
    fn pilot_event_utf8_aggregate_under_byte_bound_is_readable() {
        let (f, bytes) = non_ascii_event_fixture(30_000, 279);
        assert_eq!(bytes, 16_740_558);
        let connection = crate::open_store_connection(&f.path).unwrap();
        let actual: i64 = connection
            .query_row(
                "SELECT sum(length(CAST(payload_json AS BLOB))) FROM events",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(actual < MAX_EVENT_BYTES);
        assert!(reconcile(&connection).unwrap().is_empty());
    }
    fn pause_order(pause_first: bool) {
        let f = Fixture::new();
        let (digest, _) = f.seed();
        let intent = f.intent(digest);
        if !pause_first {
            f.claim(&intent).unwrap();
        }
        crate::PauseService::new(Arc::clone(&f.accounts))
            .pause()
            .unwrap();
        // Fake custodian callback begins only after PauseService's successful return/commit.
        let observer = crate::open_store_connection(&f.path).unwrap();
        let (paused, generation) = crate::dispatch::verified_pilot_pause(&observer).unwrap();
        assert!(paused);
        assert!(generation > 0);
        if pause_first {
            assert!(f.claim(&intent).is_err());
            assert!(restore_book(&observer).unwrap().outstanding().is_empty());
        } else {
            let book = restore_book(&observer).unwrap();
            assert_eq!(book.outstanding(), vec![intent.operation]);
            assert!(book.operation(intent.operation).unwrap().stopping);
        }
    }
    #[test]
    fn pilot_fixture_real_pause_wins_before_claim() {
        pause_order(true);
    }
    #[test]
    fn pilot_fixture_real_claim_wins_then_pause_selects_stop() {
        pause_order(false);
    }
    fn revoke_order(revoke_first: bool) {
        let f = Fixture::new();
        let (digest, a) = f.seed();
        let intent = f.intent(digest);
        if !revoke_first {
            f.claim(&intent).unwrap();
        }
        let service = crate::pilot_authority::PilotAuthorityService::new(
            Arc::clone(&f.accounts),
            f.workspace,
        );
        service
            .revoke_project_authority(a.project, digest, "fixture revoke")
            .unwrap();
        let observer = crate::open_store_connection(&f.path).unwrap();
        let records = reconcile(&observer).unwrap();
        let revoked: Revocation = parse_json(&records.last().unwrap().canonical).unwrap();
        assert_eq!(
            revoked.stopping,
            if revoke_first {
                vec![]
            } else {
                vec![intent.operation]
            }
        );
        assert!(f.claim(&intent).is_err());
        if !revoke_first {
            let book = restore_book(&observer).unwrap();
            let state = book.operation(intent.operation).unwrap();
            assert!(state.stopping);
            assert!(!state.closed);
            assert_eq!(state.intent.reservation, intent.reservation);
            assert!(
                f.observe(
                    &intent,
                    Settlement {
                        management_terminal: true,
                        physical_complete: true,
                        capture_complete: true,
                        measured: Some(intent.reservation)
                    }
                )
                .unwrap()
            );
        }
    }
    #[test]
    fn pilot_fixture_real_revocation_wins_before_claim() {
        revoke_order(true);
    }
    #[test]
    fn pilot_fixture_real_claim_wins_then_revocation_selects_durable_stop() {
        revoke_order(false);
    }
    #[test]
    fn pilot_json_independent_vectors() {
        for (text, expected) in [
            (
                include_str!("../tests/fixtures/i1/revocation.json"),
                "e129cd350f6a2aa89eecefcd131f0152c46374c0a1675fd92304dcb3324f2393",
            ),
            (
                include_str!("../tests/fixtures/i1/observation.json"),
                "44eae7cf94ce79f6c2a8cf3b72ca99736b0b1a7951680932db58264353a75a12",
            ),
            (
                include_str!("../tests/fixtures/i1/observation-measured.json"),
                "2aa7112072638f53558dd9dd60e393a4988e4b4662926361475e69d0669a81c4",
            ),
            (
                include_str!("../tests/fixtures/i1/uncertainty.json"),
                "1bd7ddbb9614c939a784592a839c9831bd4f84400e6568c9f7024b4788cd193b",
            ),
        ] {
            assert_eq!(
                identity(text.trim_end_matches('\n').as_bytes()).to_vec(),
                test_vectors::unhex(expected)
            );
        }
        let r: Revocation = parse_json(
            include_str!("../tests/fixtures/i1/revocation.json")
                .trim_end_matches('\n')
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(r.stopping, vec![Uuid::from_bytes([0xcc; 16])]);
        let o: Observation = parse_json(
            include_str!("../tests/fixtures/i1/observation.json")
                .trim_end_matches('\n')
                .as_bytes(),
        )
        .unwrap();
        assert!(o.management_terminal);
        assert!(!observation_complete(&o));
        let o: Observation = parse_json(
            include_str!("../tests/fixtures/i1/observation-measured.json")
                .trim_end_matches('\n')
                .as_bytes(),
        )
        .unwrap();
        assert!(observation_complete(&o));
        assert_eq!(
            o.usage,
            Usage::Measured {
                units: [1, 2, 3, 4, 5, 6, 7, 8, 100, 0, 9, 10]
            }
        );
        let r: UncertaintyReview = parse_json(
            include_str!("../tests/fixtures/i1/uncertainty.json")
                .trim_end_matches('\n')
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(r.reason, "fixture");
    }
    #[test]
    fn pilot_json_usage_rejects_null_missing_extra_and_contradictory_forms() {
        let base =
            include_str!("../tests/fixtures/i1/observation-measured.json").trim_end_matches('\n');
        let known = r#"{"state":"measured","units":[1,2,3,4,5,6,7,8,100,0,9,10]}"#;
        assert!(base.contains(known));
        for bad in [
            "null",
            r#"{}"#,
            r#"{"state":"unknown","units":[1,2,3,4,5,6,7,8,100,0,9,10]}"#,
            r#"{"state":"measured"}"#,
            r#"{"state":"unknown","extra":0}"#,
            r#"{"state":"other"}"#,
            r#"{"state":"measured","units":[1,2,3,4,5,6,7,8,100,1,9,10]}"#,
            r#"{"state":"measured","units":[0,2,3,4,5,6,7,8,100,0,9,10]}"#,
            r#"{"state":"measured","units":[1,2,3,4,5,6,7,8,100,0,9]}"#,
            r#"{"state":"measured","units":[1,2.5,3,4,5,6,7,8,100,0,9,10]}"#,
            r#"{"state":"measured","units":[1,-2,3,4,5,6,7,8,100,0,9,10]}"#,
            r#"{"state":"measured","units":[1,18446744073709551616,3,4,5,6,7,8,100,0,9,10]}"#,
        ] {
            assert!(
                Observation::decode(base.replace(known, bad).as_bytes()).is_err(),
                "accepted usage {bad}"
            );
        }
        let missing = base.replace(&format!(",\"usage\":{known}"), "");
        assert!(Observation::decode(missing.as_bytes()).is_err());
        assert!(Observation::decode(base.as_bytes()).is_ok());
        assert!(
            Observation::decode(
                include_str!("../tests/fixtures/i1/observation.json")
                    .trim_end_matches('\n')
                    .as_bytes()
            )
            .is_ok()
        );
    }
}
