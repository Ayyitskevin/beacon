# Phase 0 Project Missions

This checkpoint adds Beacon's first durable Mission admission without claiming autonomous Agent
work. An authenticated Operator can admit a finite Mission inside one Project, owned by one
currently eligible Team.

## Closed admission boundary

The public command accepts a Project ID and exact current Charter revision, a Project-scoped Team
ID, a finite desired outcome, at least one success condition, optional constraints and
prohibitions, and the Team's ordered Resident/current-configuration pairs. Validation and
persistence occur inside one immediate transaction. The Project Charter and every Team member's
configuration must still be current, the submitted roster must exactly match the Team's formation
order, and every member must still be eligible.

The persisted actor is `operator`, matching the command subject that exists. Beacon has no
authenticated Resident command path or Policy module yet. The admission therefore selects no Team
Lead: leadership remains a separate Mission-specific Policy decision that will require capability,
availability, and Track Record evidence.

Every accepted command creates a distinct Mission. Admission creates no Assignment, Route,
Invocation, Run, budget, credential, network scope, external effect, or execution authority.

## Persistence and reconciliation

Schema version 11 adds immutable `missions` and `mission_admission_members` tables. Composite
foreign keys bind a Mission to one exact Project Charter revision, one Team inside that Project,
each admitted Resident to that owning Team's formation roster, and each member's exact Resident
Configuration. Update, delete, and late-roster-insert triggers keep admission evidence
append-only. The accepted Command writes the Mission, ordered member snapshot, and one
`mission.admitted` Event atomically.

Startup compares every Mission Event to the exact stored Mission and member snapshot, requires one
Event per Mission, and refuses missing, malformed, widened, or mismatched state. The v10→v11
migration rebuilds the closed Event-kind table transactionally while preserving earlier Event
sequence, JSON bytes, timestamps, and the v10 Team roster guard.

## Interfaces and presentation

The same types cross the Rust service, authenticated Unix socket, Tauri commands and capability
allowlist, and strict TypeScript adapter. Signed-out sessions cannot list, inspect, or admit
Missions. The Projects application presents Missions beside their owning Teams. Its accessible
composer uses current eligible Team evidence and states the no-lead/no-execution boundary before
submission.

## Verification

From the repository root:

```sh
cargo test -p beacond --test mission
cargo test -p beacond --test local_transport authenticated_client_forms_a_team_and_admits_a_mission_over_the_real_socket -- --exact
cargo test -p beacond
cargo test -p beacon-desktop
cd apps/desktop && npm test && npm run build
scripts/gate all
```

The measured cases are recorded in
[`docs/evals/2026-08-29-project-mission-admission.md`](../evals/2026-08-29-project-mission-admission.md).
