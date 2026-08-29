---
status: accepted
---

# Separate Resident availability from execution authority

An admitted Resident has one Workspace-scoped lifecycle: `inactive`, `active`, `suspended`, or
`retired`. Only the authenticated Operator may change it. Activation makes the identity eligible
for later assignment; suspension temporarily removes that eligibility; reactivation restores it;
retirement permanently closes it while preserving its identity, history, and audit record.

Lifecycle is not authority. None of these decisions binds a model, Capability, Route, Worker,
credential, Policy, Team, Mission, budget, data scope, network permission, Run, or execution
grant. Those remain separate future decisions through their own typed interfaces. In particular,
an active Resident still cannot perform work in the current Phase 0 product.

## Consequences

Every accepted lifecycle change appends an Operator-attributed decision and one matching typed
Event in the same immediate transaction. The mutable `agents.lifecycle` value is a current-state
projection, not the audit record. Commands name the latest decision they observed; a concurrent or
stale command is refused instead of overwriting newer intent.

The only legal transitions are:

- inactive → active;
- active → suspended;
- suspended → active;
- inactive, active, or suspended → retired.

Retirement has no outgoing transition. The desktop requires explicit confirmation because it is
irreversible. Startup reconciles every decision, predecessor, Event, and current-state projection
before reporting the store ready. Starter Organization acceptance remains an immutable snapshot of
the identities as admitted—always inactive—even after their current lifecycle changes.

Accepted 2026-08-29 by the Product Steward. The reviewed branch merged to `main` as `dba5e01` on 2026-08-28; the Steward ruled on 2026-08-29 that every ADR merged under this convention is accepted.
