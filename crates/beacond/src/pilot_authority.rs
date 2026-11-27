//! Inert pilot grammars. Decoding a declaration never yields execution authority.
use std::sync::Arc;

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{AutonomyProfile, OperatorAccountError, OperatorAccountService};

pub const MAX_RECORD_BYTES: usize = 65_536;
pub const WORKSPACE_CANONICALIZER: &str = "beacon.pilot.workspace.v1";
pub const AUTHORITY_CANONICALIZER: &str = "beacon.pilot.authority.v1";
pub const OPERATION_CANONICALIZER: &str = "beacon.pilot.operation.v1";

#[derive(Debug)]
pub enum PilotError {
    Account(OperatorAccountError),
    InvalidBytes,
    InvalidScope,
    InvalidBudget,
    InvalidStore,
    MissingAuthority,
    AuthorityDrift,
    Expired,
    ClockReversed,
    UnresolvedContinuation,
    UnsupportedDescriptor,
    Paused,
    StaleGeneration,
    AlreadyClaimed,
    OutstandingCustody,
    Deadline,
    InvalidTransition,
    UnknownCustody,
    FalseRefund,
    FixtureSchemaUnavailable,
    Store(rusqlite::Error),
}

impl std::fmt::Display for PilotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pilot refusal: {self:?}")
    }
}
impl std::error::Error for PilotError {}
impl From<OperatorAccountError> for PilotError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}
impl From<rusqlite::Error> for PilotError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

/// Twelve non-fungible dimensions in the documented order; index 9 is paid USD micros.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Units(pub [u64; 12]);

impl Units {
    pub const ZERO: Self = Self([0; 12]);
    pub fn fits(self, parent: Self) -> bool {
        self.0.iter().zip(parent.0).all(|(a, b)| *a <= b)
    }
    pub fn checked_add(self, other: Self) -> Result<Self, PilotError> {
        let mut result = [0; 12];
        for (i, value) in result.iter_mut().enumerate() {
            *value = self.0[i]
                .checked_add(other.0[i])
                .ok_or(PilotError::InvalidBudget)?;
        }
        Ok(Self(result))
    }
    pub fn checked_sub(self, other: Self) -> Result<Self, PilotError> {
        let mut result = [0; 12];
        for (i, value) in result.iter_mut().enumerate() {
            *value = self.0[i]
                .checked_sub(other.0[i])
                .ok_or(PilotError::InvalidBudget)?;
        }
        Ok(Self(result))
    }
    fn validate(self) -> Result<(), PilotError> {
        if self.0[9] != 0 {
            return Err(PilotError::InvalidBudget);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceAllocation {
    pub workspace: Uuid,
    pub predecessor: Option<[u8; 32]>,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub ceiling: Units,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActorBinding {
    pub resident: Uuid,
    pub configuration: Uuid,
    pub activation: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionAllocation {
    pub mission: Uuid,
    pub ceiling: Units,
    pub residents: [Units; 3],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Descriptor {
    NotDeclared,
    Bound {
        kind: String,
        version: u32,
        canonicalizer: String,
        digest: [u8; 32],
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PilotAuthority {
    pub workspace: Uuid,
    pub project: Uuid,
    pub charter_revision: u32,
    pub policy_revision: u32,
    pub workspace_allocation: [u8; 32],
    pub predecessor: Option<[u8; 32]>,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub pause_generation: u64,
    pub actors: [ActorBinding; 3],
    pub ceiling: Units,
    pub preparation: Units,
    pub missions: Vec<MissionAllocation>,
    pub descriptors: [Descriptor; 8],
    pub readable_paths: Vec<String>,
    pub writable_paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Lead,
    Builder,
    Reviewer,
}
impl Role {
    pub fn index(self) -> usize {
        match self {
            Self::Lead => 0,
            Self::Builder => 1,
            Self::Reviewer => 2,
        }
    }
    fn tag(self) -> u8 {
        self.index() as u8 + 1
    }
    fn from_tag(tag: u8) -> Result<Self, PilotError> {
        match tag {
            1 => Ok(Self::Lead),
            2 => Ok(Self::Builder),
            3 => Ok(Self::Reviewer),
            _ => Err(PilotError::InvalidBytes),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationScope {
    ProjectPreparation,
    Mission(Uuid),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    LeadBootstrap,
    LeadMissionPlanning,
    BuilderCandidate,
    ReviewerEvaluation,
}
impl Purpose {
    fn tag(self) -> u8 {
        match self {
            Self::LeadBootstrap => 1,
            Self::LeadMissionPlanning => 2,
            Self::BuilderCandidate => 3,
            Self::ReviewerEvaluation => 4,
        }
    }
    fn from_tag(tag: u8) -> Result<Self, PilotError> {
        match tag {
            1 => Ok(Self::LeadBootstrap),
            2 => Ok(Self::LeadMissionPlanning),
            3 => Ok(Self::BuilderCandidate),
            4 => Ok(Self::ReviewerEvaluation),
            _ => Err(PilotError::InvalidBytes),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopPlan(pub [u64; 4]);
impl StopPlan {
    pub fn total(self) -> Result<u64, PilotError> {
        if self.0.contains(&0) {
            return Err(PilotError::InvalidBudget);
        }
        self.0.into_iter().try_fold(0_u64, |sum, n| {
            sum.checked_add(n).ok_or(PilotError::InvalidBudget)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationIntent {
    pub operation: Uuid,
    pub authority: [u8; 32],
    pub workspace: Uuid,
    pub project: Uuid,
    pub scope: OperationScope,
    pub role: Role,
    pub actor: ActorBinding,
    pub assignment: Option<Uuid>,
    pub parent_invocation: [u8; 32],
    pub purpose: Purpose,
    pub input_digest: [u8; 32],
    pub reservation: Units,
    pub stop: StopPlan,
}

pub fn identity(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn valid_uuid(id: Uuid) -> Result<(), PilotError> {
    if id.is_nil() {
        Err(PilotError::InvalidScope)
    } else {
        Ok(())
    }
}
fn valid_times(issued: u64, expires: u64) -> Result<(), PilotError> {
    if expires <= issued {
        Err(PilotError::InvalidBytes)
    } else {
        Ok(())
    }
}
fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-')
}
fn paths(values: &[String]) -> Result<(), PilotError> {
    if values.len() > 256
        || values
            .windows(2)
            .any(|p| p[0].as_bytes() >= p[1].as_bytes())
    {
        return Err(PilotError::InvalidBytes);
    }
    for value in values {
        if value.starts_with('/')
            || value.contains(['\\', '\0', '*', '?', '[', ']'])
            || value
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
        {
            return Err(PilotError::InvalidBytes);
        }
    }
    Ok(())
}

impl WorkspaceAllocation {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PilotError> {
        valid_uuid(self.workspace)?;
        valid_times(self.issued_at_ms, self.expires_at_ms)?;
        self.ceiling.validate()?;
        let mut w = Writer::new(WORKSPACE_CANONICALIZER);
        w.tag(1);
        w.uuid(self.workspace);
        w.optional_digest(self.predecessor);
        w.u64(self.issued_at_ms);
        w.u64(self.expires_at_ms);
        w.units(self.ceiling);
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, PilotError> {
        let mut r = Reader::new(bytes, WORKSPACE_CANONICALIZER)?;
        r.expect_tag(1)?;
        let result = Self {
            workspace: r.uuid()?,
            predecessor: r.optional_digest()?,
            issued_at_ms: r.u64()?,
            expires_at_ms: r.u64()?,
            ceiling: r.units()?,
        };
        r.finish()?;
        if result.canonical_bytes()? != bytes {
            return Err(PilotError::InvalidBytes);
        }
        Ok(result)
    }
}

impl PilotAuthority {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PilotError> {
        valid_uuid(self.workspace)?;
        valid_uuid(self.project)?;
        valid_times(self.issued_at_ms, self.expires_at_ms)?;
        if self.charter_revision == 0 || self.policy_revision == 0 {
            return Err(PilotError::InvalidScope);
        }
        for (i, actor) in self.actors.iter().enumerate() {
            valid_uuid(actor.resident)?;
            valid_uuid(actor.configuration)?;
            valid_uuid(actor.activation)?;
            if self.actors[..i]
                .iter()
                .any(|a| a.resident == actor.resident)
            {
                return Err(PilotError::InvalidScope);
            }
        }
        self.ceiling.validate()?;
        self.preparation.validate()?;
        let mut allocated = self.preparation;
        if self.missions.len() > 256
            || self
                .missions
                .windows(2)
                .any(|p| p[0].mission.as_bytes() >= p[1].mission.as_bytes())
        {
            return Err(PilotError::InvalidScope);
        }
        for mission in &self.missions {
            valid_uuid(mission.mission)?;
            mission.ceiling.validate()?;
            let mut resident_total = Units::ZERO;
            for resident in mission.residents {
                resident.validate()?;
                resident_total = resident_total.checked_add(resident)?;
            }
            if !resident_total.fits(mission.ceiling) {
                return Err(PilotError::InvalidBudget);
            }
            allocated = allocated.checked_add(mission.ceiling)?;
        }
        if !allocated.fits(self.ceiling) {
            return Err(PilotError::InvalidBudget);
        }
        paths(&self.readable_paths)?;
        paths(&self.writable_paths)?;
        // Bound typed input as well as the decoder before copying variable-sized strings.
        let path_bytes = self
            .readable_paths
            .iter()
            .chain(&self.writable_paths)
            .try_fold(0_usize, |total, value| {
                total
                    .checked_add(value.len())
                    .and_then(|n| n.checked_add(8))
                    .ok_or(PilotError::InvalidBytes)
            })?;
        if path_bytes > MAX_RECORD_BYTES {
            return Err(PilotError::InvalidBytes);
        }
        let mut w = Writer::new(AUTHORITY_CANONICALIZER);
        w.tag(1);
        w.uuid(self.workspace);
        w.uuid(self.project);
        w.u32(self.charter_revision);
        w.u32(self.policy_revision);
        w.digest(self.workspace_allocation);
        w.optional_digest(self.predecessor);
        w.u64(self.issued_at_ms);
        w.u64(self.expires_at_ms);
        w.u64(self.pause_generation);
        w.tag(1);
        for actor in self.actors {
            w.actor(actor);
        }
        w.units(self.ceiling);
        w.units(self.preparation);
        w.u32(self.missions.len() as u32);
        for mission in &self.missions {
            w.uuid(mission.mission);
            w.units(mission.ceiling);
            for resident in mission.residents {
                w.units(resident);
            }
        }
        for descriptor in &self.descriptors {
            match descriptor {
                Descriptor::NotDeclared => w.tag(0),
                Descriptor::Bound {
                    kind,
                    version,
                    canonicalizer,
                    digest,
                } => {
                    if !token(kind) || !token(canonicalizer) || *version == 0 {
                        return Err(PilotError::InvalidBytes);
                    }
                    w.tag(1);
                    w.string(kind);
                    w.u32(*version);
                    w.string(canonicalizer);
                    w.digest(*digest);
                }
            }
        }
        w.strings(&self.readable_paths);
        w.strings(&self.writable_paths);
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, PilotError> {
        let mut r = Reader::new(bytes, AUTHORITY_CANONICALIZER)?;
        r.expect_tag(1)?;
        let workspace = r.uuid()?;
        let project = r.uuid()?;
        let charter_revision = r.u32()?;
        let policy_revision = r.u32()?;
        let workspace_allocation = r.digest()?;
        let predecessor = r.optional_digest()?;
        let issued_at_ms = r.u64()?;
        let expires_at_ms = r.u64()?;
        let pause_generation = r.u64()?;
        r.expect_tag(1)?;
        let actors = [r.actor()?, r.actor()?, r.actor()?];
        let ceiling = r.units()?;
        let preparation = r.units()?;
        let count = r.count(400)?;
        let mut missions = Vec::new();
        for _ in 0..count {
            missions.push(MissionAllocation {
                mission: r.uuid()?,
                ceiling: r.units()?,
                residents: [r.units()?, r.units()?, r.units()?],
            });
        }
        let mut descriptors = std::array::from_fn(|_| Descriptor::NotDeclared);
        for descriptor in &mut descriptors {
            *descriptor = match r.tag()? {
                0 => Descriptor::NotDeclared,
                1 => Descriptor::Bound {
                    kind: r.string(64)?,
                    version: r.u32()?,
                    canonicalizer: r.string(64)?,
                    digest: r.digest()?,
                },
                _ => return Err(PilotError::InvalidBytes),
            };
        }
        let readable_paths = r.strings()?;
        let writable_paths = r.strings()?;
        r.finish()?;
        let result = Self {
            workspace,
            project,
            charter_revision,
            policy_revision,
            workspace_allocation,
            predecessor,
            issued_at_ms,
            expires_at_ms,
            pause_generation,
            actors,
            ceiling,
            preparation,
            missions,
            descriptors,
            readable_paths,
            writable_paths,
        };
        if result.canonical_bytes()? != bytes {
            return Err(PilotError::InvalidBytes);
        }
        Ok(result)
    }
}

impl OperationIntent {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PilotError> {
        valid_uuid(self.operation)?;
        valid_uuid(self.workspace)?;
        valid_uuid(self.project)?;
        valid_uuid(self.actor.resident)?;
        valid_uuid(self.actor.configuration)?;
        valid_uuid(self.actor.activation)?;
        self.reservation.validate()?;
        if self.reservation.0[0] != 1 || self.stop.total()? >= self.reservation.0[8] {
            return Err(PilotError::InvalidBudget);
        }
        match (self.scope, self.purpose, self.role, self.assignment) {
            (OperationScope::ProjectPreparation, Purpose::LeadBootstrap, Role::Lead, None) => (),
            (OperationScope::Mission(m), Purpose::LeadMissionPlanning, Role::Lead, Some(a))
            | (OperationScope::Mission(m), Purpose::BuilderCandidate, Role::Builder, Some(a))
            | (OperationScope::Mission(m), Purpose::ReviewerEvaluation, Role::Reviewer, Some(a)) => {
                valid_uuid(m)?;
                valid_uuid(a)?;
            }
            _ => return Err(PilotError::InvalidScope),
        }
        let mut w = Writer::new(OPERATION_CANONICALIZER);
        w.uuid(self.operation);
        w.digest(self.authority);
        w.uuid(self.workspace);
        w.uuid(self.project);
        match self.scope {
            OperationScope::ProjectPreparation => w.tag(1),
            OperationScope::Mission(m) => {
                w.tag(2);
                w.uuid(m);
            }
        }
        w.tag(self.role.tag());
        w.actor(self.actor);
        match self.assignment {
            None => w.tag(0),
            Some(a) => {
                w.tag(1);
                w.uuid(a);
            }
        }
        w.digest(self.parent_invocation);
        w.tag(self.purpose.tag());
        w.digest(self.input_digest);
        w.units(self.reservation);
        for cap in self.stop.0 {
            w.u64(cap);
        }
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, PilotError> {
        let mut r = Reader::new(bytes, OPERATION_CANONICALIZER)?;
        let operation = r.uuid()?;
        let authority = r.digest()?;
        let workspace = r.uuid()?;
        let project = r.uuid()?;
        let scope = match r.tag()? {
            1 => OperationScope::ProjectPreparation,
            2 => OperationScope::Mission(r.uuid()?),
            _ => return Err(PilotError::InvalidBytes),
        };
        let role = Role::from_tag(r.tag()?)?;
        let actor = r.actor()?;
        let assignment = match r.tag()? {
            0 => None,
            1 => Some(r.uuid()?),
            _ => return Err(PilotError::InvalidBytes),
        };
        let parent_invocation = r.digest()?;
        let purpose = Purpose::from_tag(r.tag()?)?;
        let input_digest = r.digest()?;
        let reservation = r.units()?;
        let stop = StopPlan([r.u64()?, r.u64()?, r.u64()?, r.u64()?]);
        r.finish()?;
        let result = Self {
            operation,
            authority,
            workspace,
            project,
            scope,
            role,
            actor,
            assignment,
            parent_invocation,
            purpose,
            input_digest,
            reservation,
            stop,
        };
        if result.canonical_bytes()? != bytes {
            return Err(PilotError::InvalidBytes);
        }
        Ok(result)
    }
}

struct Writer(Vec<u8>);
impl Writer {
    fn new(magic: &str) -> Self {
        let mut b = magic.as_bytes().to_vec();
        b.push(0);
        Self(b)
    }
    fn tag(&mut self, n: u8) {
        self.0.push(n);
    }
    fn u32(&mut self, n: u32) {
        self.0.extend(n.to_be_bytes());
    }
    fn u64(&mut self, n: u64) {
        self.0.extend(n.to_be_bytes());
    }
    fn uuid(&mut self, id: Uuid) {
        self.0.extend(id.as_bytes());
    }
    fn digest(&mut self, digest: [u8; 32]) {
        self.0.extend(digest);
    }
    fn optional_digest(&mut self, digest: Option<[u8; 32]>) {
        match digest {
            None => self.tag(0),
            Some(d) => {
                self.tag(1);
                self.digest(d);
            }
        }
    }
    fn units(&mut self, units: Units) {
        for n in units.0 {
            self.u64(n);
        }
    }
    fn actor(&mut self, actor: ActorBinding) {
        self.uuid(actor.resident);
        self.uuid(actor.configuration);
        self.uuid(actor.activation);
    }
    fn string(&mut self, text: &str) {
        self.u64(text.len() as u64);
        self.0.extend(text.as_bytes());
    }
    fn strings(&mut self, texts: &[String]) {
        self.u32(texts.len() as u32);
        for text in texts {
            self.string(text);
        }
    }
    fn finish(self) -> Result<Vec<u8>, PilotError> {
        if self.0.len() > MAX_RECORD_BYTES {
            Err(PilotError::InvalidBytes)
        } else {
            Ok(self.0)
        }
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], magic: &str) -> Result<Self, PilotError> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(PilotError::InvalidBytes);
        }
        let mut r = Self { bytes, offset: 0 };
        if r.take(magic.len())? != magic.as_bytes() || r.tag()? != 0 {
            return Err(PilotError::InvalidBytes);
        }
        Ok(r)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], PilotError> {
        let end = self.offset.checked_add(n).ok_or(PilotError::InvalidBytes)?;
        let result = self
            .bytes
            .get(self.offset..end)
            .ok_or(PilotError::InvalidBytes)?;
        self.offset = end;
        Ok(result)
    }
    fn tag(&mut self) -> Result<u8, PilotError> {
        Ok(self.take(1)?[0])
    }
    fn expect_tag(&mut self, tag: u8) -> Result<(), PilotError> {
        if self.tag()? != tag {
            Err(PilotError::InvalidBytes)
        } else {
            Ok(())
        }
    }
    fn u32(&mut self) -> Result<u32, PilotError> {
        Ok(u32::from_be_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| PilotError::InvalidBytes)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, PilotError> {
        Ok(u64::from_be_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| PilotError::InvalidBytes)?,
        ))
    }
    fn uuid(&mut self) -> Result<Uuid, PilotError> {
        Uuid::from_slice(self.take(16)?).map_err(|_| PilotError::InvalidBytes)
    }
    fn digest(&mut self) -> Result<[u8; 32], PilotError> {
        self.take(32)?
            .try_into()
            .map_err(|_| PilotError::InvalidBytes)
    }
    fn optional_digest(&mut self) -> Result<Option<[u8; 32]>, PilotError> {
        match self.tag()? {
            0 => Ok(None),
            1 => Ok(Some(self.digest()?)),
            _ => Err(PilotError::InvalidBytes),
        }
    }
    fn units(&mut self) -> Result<Units, PilotError> {
        let mut ns = [0; 12];
        for n in &mut ns {
            *n = self.u64()?;
        }
        Ok(Units(ns))
    }
    fn actor(&mut self) -> Result<ActorBinding, PilotError> {
        Ok(ActorBinding {
            resident: self.uuid()?,
            configuration: self.uuid()?,
            activation: self.uuid()?,
        })
    }
    fn count(&mut self, minimum: usize) -> Result<usize, PilotError> {
        let n = self.u32()? as usize;
        if n > 256 || n > (self.bytes.len() - self.offset) / minimum {
            return Err(PilotError::InvalidBytes);
        }
        Ok(n)
    }
    fn string(&mut self, max: usize) -> Result<String, PilotError> {
        let n = usize::try_from(self.u64()?).map_err(|_| PilotError::InvalidBytes)?;
        if n > max {
            return Err(PilotError::InvalidBytes);
        }
        std::str::from_utf8(self.take(n)?)
            .map(str::to_owned)
            .map_err(|_| PilotError::InvalidBytes)
    }
    fn strings(&mut self) -> Result<Vec<String>, PilotError> {
        let n = self.count(8)?;
        let mut values = Vec::new();
        for _ in 0..n {
            values.push(self.string(MAX_RECORD_BYTES)?);
        }
        Ok(values)
    }
    fn finish(self) -> Result<(), PilotError> {
        if self.offset != self.bytes.len() {
            Err(PilotError::InvalidBytes)
        } else {
            Ok(())
        }
    }
}

/// Authenticated diagnostic, never a permission or an effect adapter.
pub struct PilotAuthorityService {
    accounts: Arc<OperatorAccountService>,
    workspace: Uuid,
}
impl PilotAuthorityService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace: Uuid) -> Self {
        Self {
            accounts,
            workspace,
        }
    }
    /// Inert owner command. Missing fixture schema refuses; this installs or activates nothing.
    pub fn publish_workspace_allocation(&self, bytes: &[u8]) -> Result<[u8; 32], PilotError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let digest = crate::pilot_store::publish_workspace(&tx, self.workspace, bytes, now)?;
            tx.commit()?;
            Ok(digest)
        })
    }
    pub fn publish_project_authority(&self, bytes: &[u8]) -> Result<[u8; 32], PilotError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let authority = PilotAuthority::decode(bytes)?;
            verify_current_owners(
                &tx,
                self.workspace,
                &authority,
                u64::try_from(now).map_err(|_| PilotError::InvalidStore)?,
            )?;
            let (paused, generation) =
                crate::dispatch::verified_pilot_pause(&tx).map_err(|_| PilotError::InvalidStore)?;
            if paused {
                return Err(PilotError::Paused);
            }
            if generation != authority.pause_generation {
                return Err(PilotError::StaleGeneration);
            }
            let digest = crate::pilot_store::publish_authority(&tx, &authority, now)?;
            tx.commit()?;
            Ok(digest)
        })
    }
    pub fn revoke_project_authority(
        &self,
        project: Uuid,
        expected: [u8; 32],
        reason: &str,
    ) -> Result<(), PilotError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            crate::pilot_store::revoke_authority(
                &tx,
                self.workspace,
                project,
                expected,
                reason,
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
    }
    pub fn retained_records(&self) -> Result<Vec<crate::pilot_store::StoredRecord>, PilotError> {
        self.accounts.with_authenticated_store(|connection, _| {
            let tx = connection.transaction()?;
            let records = crate::pilot_store::reconcile(&tx)?;
            if records.iter().any(|r| r.workspace != self.workspace) {
                return Err(PilotError::InvalidScope);
            }
            tx.commit()?;
            Ok(records)
        })
    }
    pub fn retained_admissions(&self) -> Result<crate::pilot_admission::AdmissionBook, PilotError> {
        self.accounts.with_authenticated_store(|connection, _| {
            let tx = connection.transaction()?;
            if crate::pilot_store::reconcile(&tx)?
                .iter()
                .any(|r| r.workspace != self.workspace)
            {
                return Err(PilotError::InvalidScope);
            }
            let book = crate::pilot_store::restore_book(&tx)?;
            tx.commit()?;
            Ok(book)
        })
    }
    pub fn record_uncertainty_review(
        &self,
        project: Uuid,
        operation: Uuid,
        reason: &str,
    ) -> Result<(), PilotError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            crate::pilot_store::review_uncertainty(
                &tx,
                self.workspace,
                project,
                operation,
                reason,
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
    }
    pub fn inspect_current_owner_references(
        &self,
        bytes: &[u8],
    ) -> Result<PilotAuthority, PilotError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let transaction = connection.transaction()?;
            let authority = PilotAuthority::decode(bytes)?;
            verify_current_owners(
                &transaction,
                self.workspace,
                &authority,
                u64::try_from(now).map_err(|_| PilotError::InvalidStore)?,
            )?;
            transaction.commit()?;
            Ok(authority)
        })
    }
    pub fn resolve_resident_operation(&self, bytes: &[u8]) -> Result<(), PilotError> {
        self.accounts.with_authenticated_store(|_, _| {
            let _intent = OperationIntent::decode(bytes)?;
            // There is no production Assignment/Invocation continuation owner at this base.
            Err(PilotError::UnresolvedContinuation)
        })
    }
}

#[cfg(test)]
pub(crate) mod test_vectors {
    use super::*;
    pub fn unhex(text: &str) -> Vec<u8> {
        let compact: String = text.split_whitespace().collect();
        assert_eq!(compact.len() % 2, 0);
        compact
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
    pub fn authority() -> PilotAuthority {
        PilotAuthority::decode(&unhex(include_str!("../tests/fixtures/i1/authority.hex"))).unwrap()
    }
    pub fn operation() -> OperationIntent {
        OperationIntent::decode(&unhex(include_str!("../tests/fixtures/i1/operation.hex"))).unwrap()
    }
    #[test]
    fn pilot_canonical_independent_vectors_and_identities() {
        let w = unhex(include_str!("../tests/fixtures/i1/workspace.hex"));
        let a = unhex(include_str!("../tests/fixtures/i1/authority.hex"));
        let o = unhex(include_str!("../tests/fixtures/i1/operation.hex"));
        assert_eq!(w.len(), 156);
        assert_eq!(a.len(), 498);
        assert_eq!(o.len(), 350);
        for (bytes, expected) in [
            (
                &w,
                "0b2e4324709248b9505f54e5b8516638941c0e90ec0e87f5c443448466333030",
            ),
            (
                &a,
                "d9a92e8f186f4dc20ddaf2e6bd9004094fd11aecb76abbd17250e78109def591",
            ),
            (
                &o,
                "d4b7bb2993d4b09ff1b3f49e2f689d31aab1e8e9470b721bf0d74947a249a6dd",
            ),
        ] {
            assert_eq!(identity(bytes).to_vec(), unhex(expected));
        }
        let workspace = WorkspaceAllocation::decode(&w).unwrap();
        assert_eq!(workspace.workspace, Uuid::from_bytes([0x11; 16]));
        assert_eq!(
            workspace.ceiling,
            Units([1, 2, 3, 4, 5, 6, 7, 8, 100, 0, 9, 10])
        );
        assert_eq!(workspace.canonical_bytes().unwrap(), w);
        let authority = PilotAuthority::decode(&a).unwrap();
        assert_eq!(authority.project, Uuid::from_bytes([0x22; 16]));
        assert_eq!(authority.readable_paths, vec!["README.md"]);
        assert_eq!(authority.actors[2].resident, Uuid::from_bytes([0x99; 16]));
        assert_eq!(authority.canonical_bytes().unwrap(), a);
        let operation = OperationIntent::decode(&o).unwrap();
        assert_eq!(operation.stop, StopPlan([1, 2, 3, 4]));
        assert_eq!(operation.purpose, Purpose::LeadBootstrap);
        assert_eq!(operation.canonical_bytes().unwrap(), o);
    }
    #[test]
    fn pilot_grammar_refuses_truncation_trailing_unknown_and_huge() {
        let bytes = operation().canonical_bytes().unwrap();
        for end in 0..bytes.len() {
            assert!(OperationIntent::decode(&bytes[..end]).is_err());
        }
        let mut extra = bytes;
        extra.push(0);
        assert!(OperationIntent::decode(&extra).is_err());
        let mut unknown = authority().canonical_bytes().unwrap();
        unknown[26] = 9;
        assert!(PilotAuthority::decode(&unknown).is_err());
        assert!(PilotAuthority::decode(&vec![0; MAX_RECORD_BYTES + 1]).is_err());
    }
    #[test]
    fn pilot_grammar_refuses_nested_overrun_and_arithmetic_overflow() {
        let mut a = authority();
        a.preparation.0[1] = a.ceiling.0[1] + 1;
        assert!(matches!(
            a.canonical_bytes(),
            Err(PilotError::InvalidBudget)
        ));
        assert!(Units([u64::MAX; 12]).checked_add(Units([1; 12])).is_err());
        let mut a = authority();
        a.ceiling.0[9] = 1;
        assert!(a.canonical_bytes().is_err());
    }
    #[test]
    fn pilot_grammar_refuses_duplicate_roles_paths_and_fake_mission() {
        let mut a = authority();
        a.actors[1] = a.actors[0];
        assert!(a.canonical_bytes().is_err());
        let mut a = authority();
        a.readable_paths = vec!["a".into(), "a".into()];
        assert!(a.canonical_bytes().is_err());
        let mut a = authority();
        a.writable_paths = vec!["../escape".into()];
        assert!(a.canonical_bytes().is_err());
        let mut o = operation();
        o.scope = OperationScope::Mission(Uuid::from_bytes([7; 16]));
        assert!(o.canonical_bytes().is_err());
    }
    #[test]
    fn pilot_current_project_policy_owner_positive_then_revision_drift() {
        use crate::{
            BeaconCapability, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft,
            ProjectPolicyService, ProjectService, SecretString, StartupConfig, Supervisor,
            TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule,
            TeamLeadSelectionPolicy, TrackRecordEvidenceKind, TrackRecordRequirement,
        };
        struct OwnedStore(std::path::PathBuf);
        impl Drop for OwnedStore {
            fn drop(&mut self) {
                for suffix in ["", "-wal", "-shm"] {
                    let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
                }
            }
        }
        let store = OwnedStore(
            std::env::temp_dir().join(format!("beacond-i1-owner-{}.sqlite3", Uuid::new_v4())),
        );
        let startup = Supervisor::bootstrap(StartupConfig {
            store_path: store.0.clone(),
            initial_workspace_name: "I1 owners".into(),
        })
        .unwrap();
        let accounts = Arc::new(OperatorAccountService::open(&store.0).unwrap());
        accounts
            .begin_password_setup(
                SecretString::new("a long local passphrase".into()),
                SecretString::new("a long local passphrase".into()),
            )
            .unwrap();
        accounts.complete_operator_setup(true).unwrap();
        let projects = ProjectService::new(Arc::clone(&accounts), startup.workspace.id);
        let mut draft = ProjectCharterDraft {
            purpose: "Verify real owner references".into(),
            desired_outcomes: vec!["Current revision".into()],
            constraints: vec![],
            prohibitions: vec![],
            autonomy_profile: AutonomyProfile::Supervised,
        };
        let project = projects
            .create(ProjectDraft {
                name: "I1 owner positive".into(),
                charter: draft.clone(),
            })
            .unwrap();
        ProjectPolicyService::new(Arc::clone(&accounts), startup.workspace.id)
            .publish(
                project.id,
                1,
                None,
                ProjectPolicyDraft {
                    team_lead_selection: TeamLeadSelectionPolicy {
                        required_capabilities: vec![BeaconCapability::WorkCoordinateMission],
                        availability_rule: TeamLeadAvailabilityRule::CurrentTeamEligibility,
                        track_record_requirements: vec![TrackRecordRequirement {
                            evidence_kind: TrackRecordEvidenceKind::Evaluation,
                            minimum_count: 1,
                        }],
                        conflict_rule: TeamLeadConflictRule::RequireNoDeclaredConflict,
                        ambiguity_rule: TeamLeadAmbiguityRule::RequireOperatorDecision,
                    },
                },
            )
            .unwrap();
        let mut a = authority();
        a.workspace = startup.workspace.id;
        a.project = project.id;
        accounts
            .with_authenticated_store::<_, PilotError>(|connection, _| {
                verify_project_owners(connection, startup.workspace.id, &a)
            })
            .unwrap();
        draft.purpose = "Revised exact owner reference".into();
        projects.revise_charter(project.id, 1, draft).unwrap();
        assert!(matches!(
            accounts.with_authenticated_store::<_, PilotError>(|connection, _| {
                verify_project_owners(connection, startup.workspace.id, &a)
            }),
            Err(PilotError::AuthorityDrift)
        ));
        a.charter_revision = 2;
        assert!(matches!(
            accounts.with_authenticated_store::<_, PilotError>(|connection, _| {
                verify_project_owners(connection, startup.workspace.id, &a)
            }),
            Err(PilotError::AuthorityDrift)
        ));
    }
}

fn verify_current_owners(
    connection: &Connection,
    workspace: Uuid,
    authority: &PilotAuthority,
    now: u64,
) -> Result<(), PilotError> {
    if authority.workspace != workspace {
        return Err(PilotError::InvalidScope);
    }
    if now < authority.issued_at_ms {
        return Err(PilotError::ClockReversed);
    }
    if now >= authority.expires_at_ms {
        return Err(PilotError::Expired);
    }
    verify_project_owners(connection, workspace, authority)?;
    verify_owner_tail_events(connection, workspace)?;
    for actor in authority.actors {
        if !crate::resident_lifecycle::lifecycle_within_context_read_limits(
            connection,
            workspace,
            actor.resident,
        )
        .map_err(|_| PilotError::AuthorityDrift)?
            || !crate::resident_configuration::configuration_within_context_read_limits(
                connection,
                workspace,
                actor.resident,
            )
            .map_err(|_| PilotError::AuthorityDrift)?
            || !crate::local_model_activation::activation_within_context_read_limits(
                connection,
                workspace,
                actor.activation,
            )
            .map_err(|_| PilotError::AuthorityDrift)?
        {
            return Err(PilotError::AuthorityDrift);
        }
        let resident = crate::resident_lifecycle::verified_resident_lifecycle_within(
            connection,
            workspace,
            actor.resident,
        )
        .map_err(|_| PilotError::AuthorityDrift)?;
        let config = crate::resident_configuration::verified_current_configuration_within(
            connection,
            workspace,
            actor.resident,
        )
        .map_err(|_| PilotError::AuthorityDrift)?;
        let (selected, latest) = crate::local_model_activation::verified_activation_chain_within(
            connection,
            workspace,
            actor.activation,
        )
        .map_err(|_| PilotError::AuthorityDrift)?;
        if resident.lifecycle != crate::ResidentLifecycle::Active
            || config.id != actor.configuration
            || !config.eligible_for_team
            || config.binding.as_ref().map(|b| b.activation_id) != Some(actor.activation)
        {
            return Err(PilotError::AuthorityDrift);
        }
        if selected.id != latest.id || latest.state != crate::LocalModelActivationState::Active {
            return Err(PilotError::AuthorityDrift);
        }
    }
    Ok(())
}

fn verify_owner_tail_events(connection: &Connection, workspace: Uuid) -> Result<(), PilotError> {
    // Reverse the owner row -> event proof without inventing a new owner. Bound the
    // complete selected event set before transferring any payload to Rust. The caller
    // holds the same transaction for this proof and the subsequent current-row reads.
    const KINDS: &str = "'resident.configuration_set','resident.configuration_cleared',
                         'local_model.activated','local_model.deactivated'";
    let (count, bytes, largest): (i64, i64, i64) = connection.query_row(
        &format!(
            "SELECT count(*), coalesce(sum(length(CAST(payload_json AS BLOB))),0),
             coalesce(max(length(CAST(payload_json AS BLOB))),0)
             FROM events WHERE workspace_id=?1 AND kind IN ({KINDS})"
        ),
        [workspace.to_string()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if count > 10_000 || bytes > 16 * 1024 * 1024 || largest > MAX_RECORD_BYTES as i64 {
        return Err(PilotError::AuthorityDrift);
    }
    let mut statement = connection.prepare(&format!(
        "SELECT kind, occurred_at_ms, payload_json FROM events
         WHERE workspace_id=?1 AND kind IN ({KINDS}) ORDER BY sequence"
    ))?;
    let mut rows = statement.query([workspace.to_string()])?;
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let occurred: i64 = row.get(1)?;
        let occurred = u64::try_from(occurred).map_err(|_| PilotError::AuthorityDrift)?;
        let payload: String = row.get(2)?;
        // The bounded JSON value supplies only a lookup key for the existing owner's
        // allocation preflight. Its complete original bytes still go to that owner's
        // strict event verifier; this is not a substitute parser or authority decision.
        let lookup: serde_json::Value =
            serde_json::from_str(&payload).map_err(|_| PilotError::AuthorityDrift)?;
        match kind.as_str() {
            "resident.configuration_set" | "resident.configuration_cleared" => {
                let decision = lookup
                    .pointer("/decision/id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or(PilotError::AuthorityDrift)?;
                // Bound the stored decision's real Resident, not a possibly conflicting
                // Resident asserted by the payload being verified.
                let resident: String = connection
                    .query_row(
                        "SELECT resident_id FROM resident_configuration_decisions
                         WHERE workspace_id=?1 AND id=?2 AND length(CAST(resident_id AS BLOB))<=36",
                        rusqlite::params![workspace.to_string(), decision.to_string()],
                        |row| row.get(0),
                    )
                    .map_err(|_| PilotError::AuthorityDrift)?;
                let resident =
                    Uuid::parse_str(&resident).map_err(|_| PilotError::AuthorityDrift)?;
                if !crate::resident_configuration::configuration_within_context_read_limits(
                    connection, workspace, resident,
                )
                .map_err(|_| PilotError::AuthorityDrift)?
                {
                    return Err(PilotError::AuthorityDrift);
                }
                let state = if kind == "resident.configuration_set" {
                    crate::ResidentConfigurationState::Configured
                } else {
                    crate::ResidentConfigurationState::Cleared
                };
                crate::resident_configuration::verify_configuration_event(
                    connection, workspace, occurred, state, &payload,
                )
                .map_err(|_| PilotError::AuthorityDrift)?;
            }
            "local_model.activated" | "local_model.deactivated" => {
                let activation = lookup
                    .pointer("/activation/id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or(PilotError::AuthorityDrift)?;
                if !crate::local_model_activation::activation_within_context_read_limits(
                    connection, workspace, activation,
                )
                .map_err(|_| PilotError::AuthorityDrift)?
                {
                    return Err(PilotError::AuthorityDrift);
                }
                let state = if kind == "local_model.activated" {
                    crate::LocalModelActivationState::Active
                } else {
                    crate::LocalModelActivationState::Inactive
                };
                crate::local_model_activation::verify_activation_event(
                    connection, workspace, occurred, state, &payload,
                )
                .map_err(|_| PilotError::AuthorityDrift)?;
            }
            _ => return Err(PilotError::AuthorityDrift),
        }
    }
    Ok(())
}

fn verify_project_owners(
    connection: &Connection,
    workspace: Uuid,
    authority: &PilotAuthority,
) -> Result<(), PilotError> {
    if workspace != authority.workspace {
        return Err(PilotError::InvalidScope);
    }
    let charter =
        crate::project::verified_current_pilot_charter(connection, workspace, authority.project)
            .map_err(|_| PilotError::AuthorityDrift)?;
    if charter != (authority.charter_revision, AutonomyProfile::Supervised) {
        return Err(PilotError::AuthorityDrift);
    }
    let (policies,oversized):(i64,bool)=connection.query_row(
        "SELECT count(*),coalesce(max(length(CAST(required_capabilities_json AS BLOB))>65536 OR length(CAST(track_record_requirements_json AS BLOB))>65536),0)
         FROM project_policy_revisions WHERE workspace_id=?1 AND project_id=?2",
        rusqlite::params![workspace.to_string(),authority.project.to_string()],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let (events,event_oversized):(i64,bool)=connection.query_row("SELECT count(*),coalesce(max(length(CAST(payload_json AS BLOB))>65536),0) FROM events WHERE workspace_id=?1 AND kind='project.policy_published'",[workspace.to_string()],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if policies > 256 || events > 10000 || oversized || event_oversized {
        return Err(PilotError::AuthorityDrift);
    }
    let policy = crate::project_policy::verified_latest_policy_citation(
        connection,
        workspace,
        authority.project,
    )
    .map_err(|_| PilotError::AuthorityDrift)?;
    if policy != Some((authority.policy_revision, authority.charter_revision)) {
        return Err(PilotError::AuthorityDrift);
    }
    Ok(())
}

#[cfg(test)]
mod owner_tail_tests {
    use super::*;
    use crate::{
        BeaconCapability, LocalModelActivation, LocalModelActivationService,
        LocalModelActivationState, LocalModelBinding, LocalModelEndpoint, ModelProvider,
        ProviderFeature, ResidentConfigurationService, ResidentLifecycleService, SecretString,
        StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig, Supervisor,
    };

    struct OwnerFixture {
        path: std::path::PathBuf,
        accounts: Arc<OperatorAccountService>,
        workspace: Uuid,
        resident: Uuid,
        activation: Uuid,
        configuration: Uuid,
        actors: Vec<ActorBinding>,
    }
    impl Drop for OwnerFixture {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
            }
        }
    }
    impl OwnerFixture {
        fn new() -> Self {
            Self::with_roles(&[StarterRole::Lead])
        }
        fn with_roles(roles: &[StarterRole]) -> Self {
            let path =
                std::env::temp_dir().join(format!("beacond-i1-tail-{}.sqlite3", Uuid::new_v4()));
            let workspace = Supervisor::bootstrap(StartupConfig {
                store_path: path.clone(),
                initial_workspace_name: "I1 owner-tail fixture".into(),
            })
            .unwrap()
            .workspace
            .id;
            let accounts = Arc::new(OperatorAccountService::open(&path).unwrap());
            accounts
                .begin_password_setup(
                    SecretString::new("a long local passphrase".into()),
                    SecretString::new("a long local passphrase".into()),
                )
                .unwrap();
            accounts.complete_operator_setup(true).unwrap();
            let residents = StarterOrganizationService::new(Arc::clone(&accounts), workspace)
                .accept(
                    1,
                    roles
                        .iter()
                        .enumerate()
                        .map(|(i, role)| StarterResidentDraft {
                            role: *role,
                            display_name: Some(format!("I1 owner {i}")),
                        })
                        .collect(),
                )
                .unwrap()
                .residents;
            let resident = residents[0].id;
            for entry in &residents {
                ResidentLifecycleService::new(Arc::clone(&accounts), workspace)
                    .activate(entry.id, None)
                    .unwrap();
            }
            // TEST ONLY synthetic inspection/activation data. No provider is contacted or enabled.
            // The real activation reader checks this retained representation; configure/clear/deactivate
            // below use their real owners. This is not a successful provider inspection receipt.
            let activation = accounts.with_authenticated_store::<_, PilotError>(|connection, now| {
                let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let model_digest = "11".repeat(32);
                let canonical = format!("beacon.local-model-inspection.v1\nprovider=ollama\nendpoint=http://127.0.0.1:1\nmodel=i1-synthetic\nmodel_digest={model_digest}\nprovider_features=[\"completion\"]\nunrecognized_features=[]\n");
                let activation = LocalModelActivation {
                    id: Uuid::new_v4(), workspace_id: workspace,
                    binding: LocalModelBinding {
                        provider: ModelProvider::Ollama, endpoint: "http://127.0.0.1:1".into(),
                        model_name: "i1-synthetic".into(), model_digest,
                        inspection_digest: data_encoding::HEXLOWER.encode(&identity(canonical.as_bytes())),
                    },
                    provider_features: vec![ProviderFeature::Completion], unrecognized_features: vec![],
                    approved_features: vec![ProviderFeature::Completion], state: LocalModelActivationState::Active,
                    predecessor_id: None, decided_at_ms: now as u64,
                };
                tx.execute("INSERT INTO local_model_activation_decisions(id,workspace_id,provider,endpoint,model_name,model_digest,inspection_digest,provider_features_json,unrecognized_features_json,approved_features_json,state,predecessor_id,decided_at_ms) VALUES(?1,?2,'ollama',?3,?4,?5,?6,'[\"completion\"]','[]','[\"completion\"]','active',NULL,?7)",
                    rusqlite::params![activation.id.to_string(),workspace.to_string(),activation.binding.endpoint,activation.binding.model_name,activation.binding.model_digest,activation.binding.inspection_digest,now])?;
                #[derive(serde::Serialize)]
                struct Payload<'a> { activation: &'a LocalModelActivation }
                let payload = serde_json::to_string(&Payload {activation: &activation}).unwrap();
                crate::event_chain::append_event(&tx,&workspace.to_string(),"local_model.activated",&payload,now)?;
                tx.commit()?;
                Ok(activation.id)
            }).unwrap();
            let actors: Vec<ActorBinding> = residents
                .iter()
                .zip(roles)
                .map(|(entry, role)| {
                    let capability = match role {
                        StarterRole::Lead => BeaconCapability::WorkCoordinateMission,
                        StarterRole::Builder => BeaconCapability::ArtifactProduce,
                        StarterRole::Reviewer => BeaconCapability::ArtifactEvaluate,
                        StarterRole::Researcher => BeaconCapability::EvidenceDevelop,
                    };
                    let configuration =
                        ResidentConfigurationService::new(Arc::clone(&accounts), workspace)
                            .configure(entry.id, None, activation, vec![capability])
                            .unwrap()
                            .id;
                    ActorBinding {
                        resident: entry.id,
                        configuration,
                        activation,
                    }
                })
                .collect();
            let configuration = actors[0].configuration;
            Self {
                path,
                accounts,
                workspace,
                resident,
                activation,
                configuration,
                actors,
            }
        }
        fn verify_tail(&self) -> Result<(), PilotError> {
            self.accounts.with_authenticated_store(|connection, _| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
                verify_owner_tail_events(&tx, self.workspace)
            })
        }
        fn combined_authority(&self) -> PilotAuthority {
            use crate::{
                ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft, ProjectPolicyService,
                ProjectService, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
                TeamLeadConflictRule, TeamLeadSelectionPolicy, TrackRecordEvidenceKind,
                TrackRecordRequirement,
            };
            assert_eq!(self.actors.len(), 3);
            let project = ProjectService::new(Arc::clone(&self.accounts), self.workspace)
                .create(ProjectDraft {
                    name: "I1 combined owners".into(),
                    charter: ProjectCharterDraft {
                        purpose: "Exercise the complete current-owner consumer".into(),
                        desired_outcomes: vec!["No vacuous owner refusal".into()],
                        constraints: vec![],
                        prohibitions: vec![],
                        autonomy_profile: AutonomyProfile::Supervised,
                    },
                })
                .unwrap();
            ProjectPolicyService::new(Arc::clone(&self.accounts), self.workspace)
                .publish(
                    project.id,
                    1,
                    None,
                    ProjectPolicyDraft {
                        team_lead_selection: TeamLeadSelectionPolicy {
                            required_capabilities: vec![BeaconCapability::WorkCoordinateMission],
                            availability_rule: TeamLeadAvailabilityRule::CurrentTeamEligibility,
                            track_record_requirements: vec![TrackRecordRequirement {
                                evidence_kind: TrackRecordEvidenceKind::Evaluation,
                                minimum_count: 1,
                            }],
                            conflict_rule: TeamLeadConflictRule::RequireNoDeclaredConflict,
                            ambiguity_rule: TeamLeadAmbiguityRule::RequireOperatorDecision,
                        },
                    },
                )
                .unwrap();
            let now = self
                .accounts
                .with_authenticated_store::<_, PilotError>(|_, now| Ok(now as u64))
                .unwrap();
            let mut authority = test_vectors::authority();
            authority.workspace = self.workspace;
            authority.project = project.id;
            authority.issued_at_ms = now;
            authority.expires_at_ms = now + 60_000;
            authority.actors = self.actors.clone().try_into().unwrap();
            authority
        }
        fn pilot(&self) -> PilotAuthorityService {
            PilotAuthorityService::new(Arc::clone(&self.accounts), self.workspace)
        }
        fn install_fixture_schema(&self) {
            self.accounts
                .with_authenticated_store::<_, PilotError>(|connection, _| {
                    let tx = connection
                        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                    // Same explicitly admitted unnumbered schema, still cfg(test), never production startup.
                    tx.execute_batch(include_str!("../tests/fixtures/i1/schema.sql"))?;
                    tx.commit()?;
                    Ok(())
                })
                .unwrap();
        }
    }

    #[test]
    fn pilot_combined_three_resident_owner_inspection_positive() {
        let f = OwnerFixture::with_roles(&[
            StarterRole::Lead,
            StarterRole::Builder,
            StarterRole::Reviewer,
        ]);
        let authority = f.combined_authority();
        let actual = f
            .pilot()
            .inspect_current_owner_references(&authority.canonical_bytes().unwrap())
            .unwrap();
        assert_eq!(actual, authority);
        assert_eq!(
            actual
                .actors
                .iter()
                .map(|a| a.resident)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3
        );
    }
    #[test]
    fn pilot_combined_three_resident_owner_publication_positive() {
        let f = OwnerFixture::with_roles(&[
            StarterRole::Lead,
            StarterRole::Builder,
            StarterRole::Reviewer,
        ]);
        let mut authority = f.combined_authority();
        f.install_fixture_schema();
        let pilot = f.pilot();
        authority.workspace_allocation = pilot
            .publish_workspace_allocation(
                &WorkspaceAllocation {
                    workspace: f.workspace,
                    predecessor: None,
                    issued_at_ms: authority.issued_at_ms,
                    expires_at_ms: authority.expires_at_ms,
                    ceiling: authority.ceiling,
                }
                .canonical_bytes()
                .unwrap(),
            )
            .unwrap();
        let bytes = authority.canonical_bytes().unwrap();
        let digest = pilot.publish_project_authority(&bytes).unwrap();
        assert_eq!(digest, identity(&bytes));
        let records = pilot.retained_records().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].project, Some(authority.project));
        assert_eq!(records[1].canonical, bytes);
    }
    #[test]
    fn pilot_combined_owner_missing_configuration_is_reached_after_positive() {
        let f = OwnerFixture::with_roles(&[
            StarterRole::Lead,
            StarterRole::Builder,
            StarterRole::Reviewer,
        ]);
        let mut authority = f.combined_authority();
        let pilot = f.pilot();
        pilot
            .inspect_current_owner_references(&authority.canonical_bytes().unwrap())
            .unwrap();
        authority.actors[1].configuration = Uuid::new_v4();
        assert!(matches!(
            pilot.inspect_current_owner_references(&authority.canonical_bytes().unwrap()),
            Err(PilotError::AuthorityDrift)
        ));
    }
    #[test]
    fn pilot_combined_owner_activation_drift_is_reached_after_positive() {
        let f = OwnerFixture::with_roles(&[
            StarterRole::Lead,
            StarterRole::Builder,
            StarterRole::Reviewer,
        ]);
        let authority = f.combined_authority();
        let bytes = authority.canonical_bytes().unwrap();
        let pilot = f.pilot();
        pilot.inspect_current_owner_references(&bytes).unwrap();
        LocalModelActivationService::new(
            Arc::clone(&f.accounts),
            f.workspace,
            LocalModelEndpoint::new("127.0.0.1:1".parse().unwrap()).unwrap(),
        )
        .deactivate(f.activation)
        .unwrap();
        assert!(matches!(
            pilot.inspect_current_owner_references(&bytes),
            Err(PilotError::AuthorityDrift)
        ));
    }

    #[test]
    fn pilot_owner_tail_coherent_history_positive() {
        let f = OwnerFixture::new();
        f.verify_tail().unwrap();
        ResidentConfigurationService::new(Arc::clone(&f.accounts), f.workspace)
            .clear(f.resident, f.configuration)
            .unwrap();
        LocalModelActivationService::new(
            Arc::clone(&f.accounts),
            f.workspace,
            LocalModelEndpoint::new("127.0.0.1:1".parse().unwrap()).unwrap(),
        )
        .deactivate(f.activation)
        .unwrap();
        f.verify_tail().unwrap();
    }
    #[test]
    fn pilot_owner_tail_missing_configuration_clear_refuses() {
        let f = OwnerFixture::new();
        let cleared = ResidentConfigurationService::new(Arc::clone(&f.accounts), f.workspace)
            .clear(f.resident, f.configuration)
            .unwrap();
        f.verify_tail().unwrap();
        let connection = crate::open_store_connection(&f.path).unwrap();
        // Explicit corrupt-store fixture: bypass only the append-only delete trigger, not an owner command.
        connection
            .execute_batch("DROP TRIGGER resident_configuration_decisions_no_delete")
            .unwrap();
        connection
            .execute(
                "DELETE FROM resident_configuration_decisions WHERE id=?1",
                [cleared.id.to_string()],
            )
            .unwrap();
        let surviving = crate::resident_configuration::verified_current_configuration_within(
            &connection,
            f.workspace,
            f.resident,
        )
        .unwrap();
        assert_eq!(surviving.id, f.configuration);
        assert!(matches!(f.verify_tail(), Err(PilotError::AuthorityDrift)));
    }
    #[test]
    fn pilot_owner_tail_missing_deactivation_refuses() {
        let f = OwnerFixture::new();
        let inactive = LocalModelActivationService::new(
            Arc::clone(&f.accounts),
            f.workspace,
            LocalModelEndpoint::new("127.0.0.1:1".parse().unwrap()).unwrap(),
        )
        .deactivate(f.activation)
        .unwrap();
        f.verify_tail().unwrap();
        let connection = crate::open_store_connection(&f.path).unwrap();
        // Explicit corrupt-store fixture: terminal row missing, genuine owner event retained.
        connection
            .execute_batch("DROP TRIGGER local_model_activation_decisions_no_delete")
            .unwrap();
        connection
            .execute(
                "DELETE FROM local_model_activation_decisions WHERE id=?1",
                [inactive.id.to_string()],
            )
            .unwrap();
        let (_, latest) = crate::local_model_activation::verified_activation_chain_within(
            &connection,
            f.workspace,
            f.activation,
        )
        .unwrap();
        assert_eq!(latest.id, f.activation);
        assert!(matches!(f.verify_tail(), Err(PilotError::AuthorityDrift)));
    }
}
