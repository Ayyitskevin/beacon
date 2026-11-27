//! Inert serial admission/settlement model. No provider, process or filesystem effect exists.
//! CurrentSnapshot has no public constructor: production continuation resolution is UNBOUND.
use std::collections::BTreeMap;

use uuid::Uuid;

use crate::pilot_authority::{
    ActorBinding, OperationIntent, OperationScope, PilotAuthority, PilotError, Units,
    WorkspaceAllocation, identity,
};

/// Owner-resolved facts, not caller assertions. Only a cfg(test) fixture currently constructs it.
#[derive(Clone, Debug)]
pub struct CurrentSnapshot {
    owner_authority_verified: bool,
    operator_command_authenticated: bool,
    authority: Option<PilotAuthority>,
    parent: Option<WorkspaceAllocation>,
    actor: Option<ActorBinding>,
    assignment: Option<Uuid>,
    parent_invocation: Option<[u8; 32]>,
    descriptors_resolved: bool,
    paused: bool,
    generation: u64,
    wall_ms: u64,
    epoch: Uuid,
    monotonic_ms: u64,
}

#[cfg(test)]
impl CurrentSnapshot {
    pub(crate) fn fixture(
        authority: PilotAuthority,
        parent: WorkspaceAllocation,
        intent: &OperationIntent,
        pause: (bool, u64),
        time: (u64, Uuid, u64),
    ) -> Self {
        Self {
            owner_authority_verified: true,
            operator_command_authenticated: true,
            authority: Some(authority),
            parent: Some(parent),
            actor: Some(intent.actor),
            assignment: intent.assignment,
            parent_invocation: Some(intent.parent_invocation),
            descriptors_resolved: true,
            paused: pause.0,
            generation: pause.1,
            wall_ms: time.0,
            epoch: time.1,
            monotonic_ms: time.2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    ClaimedUnknown,
    Sent,
    NotSentProven,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationState {
    pub intent: OperationIntent,
    pub delivery: Delivery,
    pub management_terminal: bool,
    pub physical_complete: bool,
    pub capture_complete: bool,
    pub measured: Option<Units>,
    pub uncertainty_reviewed: bool,
    pub stopping: bool,
    pub closed: bool,
    epoch: Uuid,
    admitted_at_monotonic_ms: u64,
    useful_deadline_ms: u64,
    final_deadline_ms: u64,
    next_stop_stage: usize,
}

/// A receipt is a set of independent observations, never a manager's single success boolean.
#[derive(Clone, Copy, Debug)]
pub struct Settlement {
    pub management_terminal: bool,
    pub physical_complete: bool,
    pub capture_complete: bool,
    pub measured: Option<Units>,
}

#[derive(Clone, Debug, Default)]
pub struct AdmissionBook {
    operations: BTreeMap<Uuid, OperationState>,
    last_wall_ms: Option<u64>,
}

impl AdmissionBook {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn operation(&self, id: Uuid) -> Option<&OperationState> {
        self.operations.get(&id)
    }
    pub fn outstanding(&self) -> Vec<Uuid> {
        self.operations
            .iter()
            .filter_map(|(id, s)| (!s.closed).then_some(*id))
            .collect()
    }
    pub fn claim(
        &mut self,
        intent: &OperationIntent,
        current: &CurrentSnapshot,
    ) -> Result<(), PilotError> {
        intent.canonical_bytes()?;
        check_current(intent, current)?;
        if self.last_wall_ms.is_some_and(|old| current.wall_ms < old) {
            return Err(PilotError::ClockReversed);
        }
        if self.operations.contains_key(&intent.operation) {
            return Err(PilotError::AlreadyClaimed);
        }
        if !self.outstanding().is_empty() {
            return Err(PilotError::OutstandingCustody);
        }
        self.check_budget(intent, current)?;
        let final_deadline = current
            .monotonic_ms
            .checked_add(intent.reservation.0[8])
            .ok_or(PilotError::Deadline)?;
        let useful_deadline = final_deadline
            .checked_sub(intent.stop.total()?)
            .ok_or(PilotError::Deadline)?;
        if useful_deadline <= current.monotonic_ms {
            return Err(PilotError::Deadline);
        }
        self.operations.insert(
            intent.operation,
            OperationState {
                intent: intent.clone(),
                delivery: Delivery::ClaimedUnknown,
                management_terminal: false,
                physical_complete: false,
                capture_complete: false,
                measured: None,
                uncertainty_reviewed: false,
                stopping: false,
                closed: false,
                epoch: current.epoch,
                admitted_at_monotonic_ms: current.monotonic_ms,
                useful_deadline_ms: useful_deadline,
                final_deadline_ms: final_deadline,
                next_stop_stage: 0,
            },
        );
        self.last_wall_ms = Some(current.wall_ms);
        Ok(())
    }
    fn check_budget(
        &self,
        intent: &OperationIntent,
        current: &CurrentSnapshot,
    ) -> Result<(), PilotError> {
        let authority = current
            .authority
            .as_ref()
            .ok_or(PilotError::MissingAuthority)?;
        let parent = current
            .parent
            .as_ref()
            .ok_or(PilotError::MissingAuthority)?;
        self.verify_declared_budget(intent, authority, parent)
    }
    pub(crate) fn verify_declared_budget(
        &self,
        intent: &OperationIntent,
        authority: &PilotAuthority,
        parent: &WorkspaceAllocation,
    ) -> Result<(), PilotError> {
        if !authority.ceiling.fits(parent.ceiling) {
            return Err(PilotError::InvalidBudget);
        }
        let mut workspace = intent.reservation;
        let mut project = intent.reservation;
        let mut scope = intent.reservation;
        let mut resident = intent.reservation;
        for old in self.operations.values() {
            let use_ = old
                .measured
                .filter(|_| old.closed)
                .unwrap_or(old.intent.reservation);
            if old.intent.workspace == intent.workspace {
                workspace = workspace.checked_add(use_)?;
            }
            if old.intent.project == intent.project {
                project = project.checked_add(use_)?;
                if old.intent.scope == intent.scope {
                    scope = scope.checked_add(use_)?;
                    if old.intent.actor.resident == intent.actor.resident {
                        resident = resident.checked_add(use_)?;
                    }
                }
            }
        }
        if !workspace.fits(parent.ceiling) || !project.fits(authority.ceiling) {
            return Err(PilotError::InvalidBudget);
        }
        let (scope_cap, resident_cap) = match intent.scope {
            OperationScope::ProjectPreparation => (authority.preparation, authority.preparation),
            OperationScope::Mission(id) => {
                let mission = authority
                    .missions
                    .iter()
                    .find(|m| m.mission == id)
                    .ok_or(PilotError::InvalidScope)?;
                (mission.ceiling, mission.residents[intent.role.index()])
            }
        };
        if !scope.fits(scope_cap) || !resident.fits(resident_cap) {
            return Err(PilotError::InvalidBudget);
        }
        Ok(())
    }
    /// Rechecking at this seam is mandatory; successful claim is not a cached launch permit.
    pub fn launch(&mut self, operation: Uuid, current: &CurrentSnapshot) -> Result<(), PilotError> {
        let state = self
            .operations
            .get(&operation)
            .ok_or(PilotError::InvalidTransition)?;
        check_current(&state.intent, current)?;
        if state.stopping {
            return Err(PilotError::Paused);
        }
        if state.epoch != current.epoch
            || current.monotonic_ms < state.admitted_at_monotonic_ms
            || current.monotonic_ms >= state.useful_deadline_ms
        {
            return Err(PilotError::Deadline);
        }
        if state.closed || state.delivery != Delivery::ClaimedUnknown {
            return Err(PilotError::AlreadyClaimed);
        }
        self.operations
            .get_mut(&operation)
            .ok_or(PilotError::InvalidTransition)?
            .delivery = Delivery::Sent;
        Ok(())
    }
    /// No new authority is required for pre-authorized bounded stopping. No physical action here.
    pub fn stop_stage(
        &mut self,
        operation: Uuid,
        epoch: Uuid,
        now: u64,
        stage: usize,
        cap: u64,
    ) -> Result<(), PilotError> {
        let state = self
            .operations
            .get_mut(&operation)
            .ok_or(PilotError::InvalidTransition)?;
        if state.closed
            || stage != state.next_stop_stage
            || stage >= 4
            || cap == 0
            || state.epoch != epoch
            || now < state.admitted_at_monotonic_ms
        {
            return Err(PilotError::InvalidTransition);
        }
        let later = state.intent.stop.0[stage + 1..]
            .iter()
            .try_fold(0_u64, |sum, n| {
                sum.checked_add(*n).ok_or(PilotError::Deadline)
            })?;
        let available = state
            .final_deadline_ms
            .checked_sub(now)
            .and_then(|n| n.checked_sub(later))
            .ok_or(PilotError::Deadline)?;
        if cap > state.intent.stop.0[stage] || cap > available {
            return Err(PilotError::Deadline);
        }
        state.next_stop_stage += 1;
        Ok(())
    }
    pub fn settle(&mut self, operation: Uuid, observation: Settlement) -> Result<(), PilotError> {
        let state = self
            .operations
            .get_mut(&operation)
            .ok_or(PilotError::InvalidTransition)?;
        if state.closed {
            return Err(PilotError::InvalidTransition);
        }
        if let Some(use_) = observation.measured {
            // A claim permanently consumes its one attempt. A new UUID never refunds that attempt.
            if use_.0[0] != 1 || !use_.fits(state.intent.reservation) {
                return Err(PilotError::FalseRefund);
            }
            if state.measured.is_some_and(|old| !old.fits(use_)) {
                return Err(PilotError::FalseRefund);
            }
        }
        state.management_terminal |= observation.management_terminal;
        state.physical_complete |= observation.physical_complete;
        state.capture_complete |= observation.capture_complete;
        if observation.measured.is_some() {
            state.measured = observation.measured;
        }
        state.closed =
            state.physical_complete && state.capture_complete && state.measured.is_some();
        if !state.closed {
            return Err(PilotError::UnknownCustody);
        }
        Ok(())
    }
    /// Records human review only. It cannot settle, refund or erase a shared resource fence.
    pub fn review_uncertainty(
        &mut self,
        operation: Uuid,
        authenticated: &CurrentSnapshot,
    ) -> Result<(), PilotError> {
        if !authenticated.operator_command_authenticated {
            return Err(PilotError::MissingAuthority);
        }
        self.operations
            .get_mut(&operation)
            .ok_or(PilotError::InvalidTransition)?
            .uncertainty_reviewed = true;
        Ok(())
    }
    pub fn resume_eligible(&self, authenticated: &CurrentSnapshot) -> Result<(), PilotError> {
        if !authenticated.operator_command_authenticated {
            return Err(PilotError::MissingAuthority);
        }
        if !self.outstanding().is_empty() {
            return Err(PilotError::OutstandingCustody);
        }
        Ok(())
    }
    /// A reopened process does not inherit a usable monotonic epoch and never resends.
    pub fn after_restart(&mut self) {
        for state in self.operations.values_mut().filter(|state| !state.closed) {
            state.epoch = Uuid::nil();
        }
    }
    pub(crate) fn begin_stopping(&mut self, operation: Uuid) -> Result<(), PilotError> {
        self.operations
            .get_mut(&operation)
            .ok_or(PilotError::InvalidTransition)?
            .stopping = true;
        Ok(())
    }
    pub(crate) fn restore_claim(&mut self, intent: OperationIntent) -> Result<(), PilotError> {
        intent.canonical_bytes()?;
        if self.operations.contains_key(&intent.operation) || !self.outstanding().is_empty() {
            return Err(PilotError::InvalidStore);
        }
        self.operations.insert(
            intent.operation,
            OperationState {
                intent,
                delivery: Delivery::ClaimedUnknown,
                management_terminal: false,
                physical_complete: false,
                capture_complete: false,
                measured: None,
                uncertainty_reviewed: false,
                stopping: false,
                closed: false,
                epoch: Uuid::nil(),
                admitted_at_monotonic_ms: 0,
                useful_deadline_ms: 0,
                final_deadline_ms: 0,
                next_stop_stage: 0,
            },
        );
        Ok(())
    }
    pub(crate) fn restore_review(&mut self, operation: Uuid) -> Result<(), PilotError> {
        self.operations
            .get_mut(&operation)
            .ok_or(PilotError::InvalidStore)?
            .uncertainty_reviewed = true;
        Ok(())
    }
    pub(crate) fn observe_retained_wall(&mut self, wall_ms: u64) -> Result<(), PilotError> {
        if self.last_wall_ms.is_some_and(|old| wall_ms < old) {
            return Err(PilotError::ClockReversed);
        }
        self.last_wall_ms = Some(wall_ms);
        Ok(())
    }
}

fn check_current(intent: &OperationIntent, current: &CurrentSnapshot) -> Result<(), PilotError> {
    // Session unlock is not a Resident continuation or a Pause generation. This private
    // snapshot must come from the relevant owners; production resolution remains unavailable.
    if !current.owner_authority_verified {
        return Err(PilotError::MissingAuthority);
    }
    let authority = current
        .authority
        .as_ref()
        .ok_or(PilotError::MissingAuthority)?;
    let parent = current
        .parent
        .as_ref()
        .ok_or(PilotError::MissingAuthority)?;
    if intent.workspace != authority.workspace
        || intent.project != authority.project
        || parent.workspace != authority.workspace
    {
        return Err(PilotError::InvalidScope);
    }
    if intent.authority != identity(&authority.canonical_bytes()?)
        || authority.workspace_allocation != identity(&parent.canonical_bytes()?)
    {
        return Err(PilotError::AuthorityDrift);
    }
    if current.wall_ms < authority.issued_at_ms || current.wall_ms < parent.issued_at_ms {
        return Err(PilotError::ClockReversed);
    }
    if current.wall_ms >= authority.expires_at_ms || current.wall_ms >= parent.expires_at_ms {
        return Err(PilotError::Expired);
    }
    if current.actor != Some(intent.actor)
        || intent.actor != authority.actors[intent.role.index()]
        || current.assignment != intent.assignment
        || current.parent_invocation != Some(intent.parent_invocation)
    {
        return Err(PilotError::UnresolvedContinuation);
    }
    if !current.descriptors_resolved
        || authority
            .descriptors
            .iter()
            .any(|d| matches!(d, crate::pilot_authority::Descriptor::NotDeclared))
    {
        return Err(PilotError::UnsupportedDescriptor);
    }
    if current.paused {
        return Err(PilotError::Paused);
    }
    if current.generation != authority.pause_generation {
        return Err(PilotError::StaleGeneration);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pilot_authority::{Descriptor, Purpose, Role, StopPlan, test_vectors};

    pub(crate) fn fixture() -> (OperationIntent, CurrentSnapshot) {
        let mut authority = test_vectors::authority();
        authority.preparation = Units([3, 30, 30, 30, 30, 30, 30, 30, 300, 0, 30, 30]);
        let parent = WorkspaceAllocation {
            workspace: authority.workspace,
            predecessor: None,
            issued_at_ms: 1,
            expires_at_ms: 100,
            ceiling: Units([10, 100, 100, 100, 100, 100, 100, 100, 1000, 0, 100, 100]),
        };
        authority.workspace_allocation = identity(&parent.canonical_bytes().unwrap());
        authority.descriptors = std::array::from_fn(|_| Descriptor::Bound {
            kind: "fixture-only".into(),
            version: 1,
            canonicalizer: "fixture.v1".into(),
            digest: [7; 32],
        });
        let mut intent = test_vectors::operation();
        intent.authority = identity(&authority.canonical_bytes().unwrap());
        let current = CurrentSnapshot {
            owner_authority_verified: true,
            operator_command_authenticated: true,
            authority: Some(authority),
            parent: Some(parent),
            actor: Some(intent.actor),
            assignment: None,
            parent_invocation: Some(intent.parent_invocation),
            descriptors_resolved: true,
            paused: false,
            generation: 0,
            wall_ms: 2,
            epoch: Uuid::from_bytes([9; 16]),
            monotonic_ms: 10,
        };
        (intent, current)
    }
    fn refusal(
        intent: OperationIntent,
        current: CurrentSnapshot,
        expected: fn(&PilotError) -> bool,
    ) {
        let mut book = AdmissionBook::new();
        let result = book.claim(&intent, &current);
        assert!(
            result.as_ref().is_err_and(expected),
            "unexpected result: {result:?}"
        );
        assert!(book.outstanding().is_empty());
    }
    #[test]
    fn pilot_positive_claim_launch_settlement() {
        let (intent, current) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&intent, &current).unwrap();
        book.launch(intent.operation, &current).unwrap();
        assert_eq!(
            book.operation(intent.operation).unwrap().delivery,
            Delivery::Sent
        );
        book.settle(
            intent.operation,
            Settlement {
                management_terminal: false,
                physical_complete: true,
                capture_complete: true,
                measured: Some(intent.reservation),
            },
        )
        .unwrap();
        assert!(book.outstanding().is_empty());
        book.resume_eligible(&current).unwrap();
    }
    #[test]
    fn pilot_negative_unauthorized() {
        let (o, mut c) = fixture();
        c.owner_authority_verified = false;
        refusal(o, c, |e| matches!(e, PilotError::MissingAuthority));
    }
    #[test]
    fn pilot_negative_missing_authority() {
        let (o, mut c) = fixture();
        c.authority = None;
        refusal(o, c, |e| matches!(e, PilotError::MissingAuthority));
    }
    #[test]
    fn pilot_current_missing_guard_before_budget_fallback() {
        let (o, mut c) = fixture();
        c.authority = None;
        assert!(matches!(
            check_current(&o, &c),
            Err(PilotError::MissingAuthority)
        ));
    }
    #[test]
    fn pilot_negative_drifted_authority() {
        let (o, mut c) = fixture();
        c.authority.as_mut().unwrap().policy_revision += 1;
        refusal(o, c, |e| matches!(e, PilotError::AuthorityDrift));
    }
    #[test]
    fn pilot_negative_cross_scope() {
        let (mut o, c) = fixture();
        o.project = Uuid::from_bytes([1; 16]);
        refusal(o, c, |e| matches!(e, PilotError::InvalidScope));
    }
    #[test]
    fn pilot_negative_expired() {
        let (o, mut c) = fixture();
        c.wall_ms = 100;
        refusal(o, c, |e| matches!(e, PilotError::Expired));
    }
    #[test]
    fn pilot_negative_clock_reverse() {
        let (o, mut c) = fixture();
        c.wall_ms = 0;
        refusal(o, c, |e| matches!(e, PilotError::ClockReversed));
    }
    #[test]
    fn pilot_negative_role_spoof() {
        let (o, mut c) = fixture();
        c.actor.as_mut().unwrap().resident = Uuid::from_bytes([2; 16]);
        refusal(o, c, |e| matches!(e, PilotError::UnresolvedContinuation));
    }
    #[test]
    fn pilot_negative_parent_invocation_spoof() {
        let (o, mut c) = fixture();
        c.parent_invocation = Some([8; 32]);
        refusal(o, c, |e| matches!(e, PilotError::UnresolvedContinuation));
    }
    #[test]
    fn pilot_negative_descriptor_unresolved() {
        let (o, mut c) = fixture();
        c.descriptors_resolved = false;
        refusal(o, c, |e| matches!(e, PilotError::UnsupportedDescriptor));
    }
    #[test]
    fn pilot_negative_generation() {
        let (o, mut c) = fixture();
        c.generation = 1;
        refusal(o, c, |e| matches!(e, PilotError::StaleGeneration));
    }
    #[test]
    fn pilot_negative_pause_wins() {
        let (o, mut c) = fixture();
        c.paused = true;
        refusal(o, c, |e| matches!(e, PilotError::Paused));
    }
    #[test]
    fn pilot_negative_claim_wins_then_pause_cached_launch() {
        let (o, mut c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        c.paused = true;
        assert!(matches!(
            book.launch(o.operation, &c),
            Err(PilotError::Paused)
        ));
        assert_eq!(book.outstanding(), vec![o.operation]);
        assert_eq!(
            book.operation(o.operation).unwrap().delivery,
            Delivery::ClaimedUnknown
        );
    }
    #[test]
    fn pilot_negative_nested_budget() {
        let (mut o, c) = fixture();
        o.reservation.0[1] = 31;
        refusal(o, c, |e| matches!(e, PilotError::InvalidBudget));
    }
    #[test]
    fn pilot_negative_request_byte_budget() {
        let (mut o, c) = fixture();
        o.reservation.0[10] = 31;
        refusal(o, c, |e| matches!(e, PilotError::InvalidBudget));
    }
    #[test]
    fn pilot_negative_response_byte_budget() {
        let (mut o, c) = fixture();
        o.reservation.0[11] = 31;
        refusal(o, c, |e| matches!(e, PilotError::InvalidBudget));
    }
    #[test]
    fn pilot_negative_duplicate_and_distinct_id_shared_custody() {
        let (o, c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        assert!(matches!(
            book.claim(&o, &c),
            Err(PilotError::AlreadyClaimed)
        ));
        let mut next = o.clone();
        next.operation = Uuid::from_bytes([3; 16]);
        assert!(matches!(
            book.claim(&next, &c),
            Err(PilotError::OutstandingCustody)
        ));
    }
    #[test]
    fn pilot_unknown_exit_restart_false_refund_and_late_settlement() {
        let (o, c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        assert!(matches!(
            book.settle(
                o.operation,
                Settlement {
                    management_terminal: true,
                    physical_complete: false,
                    capture_complete: true,
                    measured: None
                }
            ),
            Err(PilotError::UnknownCustody)
        ));
        assert!(book.operation(o.operation).unwrap().management_terminal);
        book.review_uncertainty(o.operation, &c).unwrap();
        assert!(matches!(
            book.resume_eligible(&c),
            Err(PilotError::OutstandingCustody)
        ));
        book.after_restart();
        assert!(matches!(
            book.launch(o.operation, &c),
            Err(PilotError::Deadline)
        ));
        assert!(matches!(
            book.settle(
                o.operation,
                Settlement {
                    management_terminal: true,
                    physical_complete: true,
                    capture_complete: true,
                    measured: Some(Units::ZERO)
                }
            ),
            Err(PilotError::FalseRefund)
        ));
        assert_eq!(book.outstanding(), vec![o.operation]);
        book.settle(
            o.operation,
            Settlement {
                management_terminal: true,
                physical_complete: true,
                capture_complete: true,
                measured: Some(o.reservation),
            },
        )
        .unwrap();
        assert!(matches!(
            book.claim(&o, &c),
            Err(PilotError::AlreadyClaimed)
        ));
    }
    #[test]
    fn pilot_stage_budget_remaining_reserves_and_expiry_stop_obligation() {
        let (o, mut c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        c.monotonic_ms = 100;
        assert!(matches!(
            book.launch(o.operation, &c),
            Err(PilotError::Deadline)
        ));
        assert!(matches!(
            book.stop_stage(o.operation, c.epoch, 100, 0, 2),
            Err(PilotError::Deadline)
        ));
        book.stop_stage(o.operation, c.epoch, 100, 0, 1).unwrap();
        assert!(matches!(
            book.stop_stage(o.operation, c.epoch, 109, 1, 2),
            Err(PilotError::Deadline)
        ));
        assert_eq!(book.outstanding(), vec![o.operation]);
    }
    #[test]
    fn pilot_role_purpose_is_not_freeform() {
        let (mut o, _) = fixture();
        o.role = Role::Reviewer;
        o.purpose = Purpose::LeadBootstrap;
        assert!(matches!(o.canonical_bytes(), Err(PilotError::InvalidScope)));
        o.stop = StopPlan([0, 1, 1, 1]);
        assert!(o.canonical_bytes().is_err());
    }
    #[test]
    fn pilot_cumulative_new_id_and_successor_never_reset_usage() {
        let (mut first, mut current) = fixture();
        current.authority.as_mut().unwrap().preparation.0[0] = 1;
        first.authority = identity(
            &current
                .authority
                .as_ref()
                .unwrap()
                .canonical_bytes()
                .unwrap(),
        );
        let mut book = AdmissionBook::new();
        book.claim(&first, &current).unwrap();
        book.settle(
            first.operation,
            Settlement {
                management_terminal: true,
                physical_complete: true,
                capture_complete: true,
                measured: Some(first.reservation),
            },
        )
        .unwrap();
        let mut next = first.clone();
        next.operation = Uuid::from_bytes([0x71; 16]);
        assert!(matches!(
            book.claim(&next, &current),
            Err(PilotError::InvalidBudget)
        ));
        let previous = next.authority;
        let authority = current.authority.as_mut().unwrap();
        authority.predecessor = Some(previous);
        authority.policy_revision += 1;
        next.authority = identity(&authority.canonical_bytes().unwrap());
        assert!(matches!(
            book.claim(&next, &current),
            Err(PilotError::InvalidBudget)
        ));
        assert_eq!(book.operations.len(), 1);
    }
    #[test]
    fn pilot_bootstrap_charge_rolls_up_once_not_again_into_mission() {
        let (mut first, mut current) = fixture();
        let mission_id = Uuid::from_bytes([0x73; 16]);
        let one = first.reservation;
        let two = one.checked_add(one).unwrap();
        let parent = current.parent.as_mut().unwrap();
        parent.ceiling = two;
        let parent_digest = identity(&parent.canonical_bytes().unwrap());
        let authority = current.authority.as_mut().unwrap();
        authority.workspace_allocation = parent_digest;
        authority.ceiling = two;
        authority.preparation = one;
        authority.missions = vec![crate::pilot_authority::MissionAllocation {
            mission: mission_id,
            ceiling: one,
            residents: [Units::ZERO, one, Units::ZERO],
        }];
        first.authority = identity(&authority.canonical_bytes().unwrap());
        let mut book = AdmissionBook::new();
        book.claim(&first, &current).unwrap();
        book.settle(
            first.operation,
            Settlement {
                management_terminal: false,
                physical_complete: true,
                capture_complete: true,
                measured: Some(one),
            },
        )
        .unwrap();
        let mut second = first.clone();
        second.operation = Uuid::from_bytes([0x74; 16]);
        second.scope = OperationScope::Mission(mission_id);
        second.role = Role::Builder;
        second.actor = current.authority.as_ref().unwrap().actors[1];
        second.assignment = Some(Uuid::from_bytes([0x75; 16]));
        second.purpose = Purpose::BuilderCandidate;
        current.actor = Some(second.actor);
        current.assignment = second.assignment;
        book.claim(&second, &current).unwrap();
        book.settle(
            second.operation,
            Settlement {
                management_terminal: true,
                physical_complete: true,
                capture_complete: true,
                measured: Some(one),
            },
        )
        .unwrap();
        assert_eq!(book.operations.len(), 2);
        assert_eq!(book.operation(first.operation).unwrap().measured, Some(one));
        assert_eq!(
            book.operation(second.operation).unwrap().measured,
            Some(one)
        );
    }
    #[test]
    fn pilot_duplicate_claim_after_positive_close_is_not_resendable() {
        let (o, c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        book.settle(
            o.operation,
            Settlement {
                management_terminal: true,
                physical_complete: true,
                capture_complete: true,
                measured: Some(o.reservation),
            },
        )
        .unwrap();
        assert!(matches!(
            book.claim(&o, &c),
            Err(PilotError::AlreadyClaimed)
        ));
    }
    #[test]
    fn pilot_session_lock_is_not_pause_or_owner_resume_authority() {
        let (o, mut c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        c.operator_command_authenticated = false;
        book.launch(o.operation, &c).unwrap();
        assert!(matches!(
            book.review_uncertainty(o.operation, &c),
            Err(PilotError::MissingAuthority)
        ));
        assert!(matches!(
            book.resume_eligible(&c),
            Err(PilotError::MissingAuthority)
        ));
        assert_eq!(
            book.operation(o.operation).unwrap().delivery,
            Delivery::Sent
        );
    }

    #[test]
    fn pilot_guard_false_refund_independent_of_unknown_close_guard() {
        let (o, c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        assert!(matches!(
            book.settle(
                o.operation,
                Settlement {
                    management_terminal: true,
                    physical_complete: true,
                    capture_complete: true,
                    measured: Some(Units::ZERO),
                }
            ),
            Err(PilotError::FalseRefund)
        ));
        assert_eq!(book.outstanding(), vec![o.operation]);
    }

    #[test]
    fn pilot_guard_resume_retains_raw_outstanding_claim() {
        let (o, c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        assert!(matches!(
            book.resume_eligible(&c),
            Err(PilotError::OutstandingCustody)
        ));
    }
    #[test]
    fn pilot_guard_new_id_cannot_bypass_outstanding_custody() {
        let (o, c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        let mut next = o.clone();
        next.operation = Uuid::from_bytes([0x76; 16]);
        assert!(matches!(
            book.claim(&next, &c),
            Err(PilotError::OutstandingCustody)
        ));
    }
    fn one_missing_observation_keeps_custody(observation: Settlement) {
        let (o, c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        assert!(matches!(
            book.settle(o.operation, observation),
            Err(PilotError::UnknownCustody)
        ));
        assert_eq!(book.outstanding(), vec![o.operation]);
    }
    #[test]
    fn pilot_guard_physical_completion_is_independent() {
        one_missing_observation_keeps_custody(Settlement {
            management_terminal: true,
            physical_complete: false,
            capture_complete: true,
            measured: Some(fixture().0.reservation),
        });
    }
    #[test]
    fn pilot_guard_capture_completion_is_independent() {
        one_missing_observation_keeps_custody(Settlement {
            management_terminal: true,
            physical_complete: true,
            capture_complete: false,
            measured: Some(fixture().0.reservation),
        });
    }
    #[test]
    fn pilot_guard_measured_usage_is_independent() {
        one_missing_observation_keeps_custody(Settlement {
            management_terminal: true,
            physical_complete: true,
            capture_complete: true,
            measured: None,
        });
    }

    #[test]
    fn pilot_guard_restart_invalidates_cached_monotonic_epoch() {
        let (o, c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        book.after_restart();
        assert!(matches!(
            book.launch(o.operation, &c),
            Err(PilotError::Deadline)
        ));
        assert_eq!(
            book.operation(o.operation).unwrap().delivery,
            Delivery::ClaimedUnknown
        );
    }
    #[test]
    fn pilot_retained_wall_refuses_backward_claim_after_restart() {
        let (mut o, mut c) = fixture();
        let mut book = AdmissionBook::new();
        book.observe_retained_wall(20).unwrap();
        book.after_restart();
        c.wall_ms = 19;
        o.operation = Uuid::from_bytes([0x77; 16]);
        assert!(matches!(book.claim(&o, &c), Err(PilotError::ClockReversed)));
        assert!(book.outstanding().is_empty());
        assert!(matches!(
            book.observe_retained_wall(19),
            Err(PilotError::ClockReversed)
        ));
    }

    #[test]
    fn pilot_guard_remaining_time_excludes_later_stop_reserves() {
        let (o, c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        assert!(matches!(
            book.stop_stage(o.operation, c.epoch, 101, 0, 1),
            Err(PilotError::Deadline)
        ));
        book.stop_stage(o.operation, c.epoch, 100, 0, 1).unwrap();
    }
    #[test]
    fn pilot_guard_each_stop_stage_respects_its_allocation() {
        let (o, c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        assert!(matches!(
            book.stop_stage(o.operation, c.epoch, 10, 0, o.stop.0[0] + 1),
            Err(PilotError::Deadline)
        ));
        book.stop_stage(o.operation, c.epoch, 10, 0, o.stop.0[0])
            .unwrap();
    }

    #[test]
    fn pilot_expiry_refuses_useful_work_but_keeps_bounded_stop_obligation() {
        let (o, mut c) = fixture();
        let mut book = AdmissionBook::new();
        book.claim(&o, &c).unwrap();
        c.wall_ms = 100;
        assert!(matches!(
            book.launch(o.operation, &c),
            Err(PilotError::Expired)
        ));
        book.stop_stage(o.operation, c.epoch, 100, 0, 1).unwrap();
        assert_eq!(book.outstanding(), vec![o.operation]);
    }
}
