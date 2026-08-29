# Phase 0 Project Teams

This checkpoint adds Beacon's first durable Team record without claiming autonomous Agent work.
An authenticated Operator can form a named Team inside one Project from the exact current
configurations of one or more eligible Residents.

## Closed formation boundary

The public command accepts a Project ID, a trimmed Team name, and distinct pairs of Resident ID and
expected Resident Configuration ID. Validation and persistence occur inside one immediate
transaction. The Project must belong to the current Workspace; every configuration must still be
current; and every member must still be eligible from current lifecycle, configuration, and model
activation state.

The persisted actor is `operator`, matching the command subject that actually exists. Beacon has
no Resident authentication/runtime path or Policy module yet, so this checkpoint does not claim
that Residents can issue the command. `work.form_team` remains a declared capability for that
later path and is not required of ordinary Team members.

Teams have stable UUID identity, Project scope, an immutable ordered formation roster, and no
permanent lead. Names are labels rather than uniqueness or authority keys. Every accepted command
creates a distinct Team. Formation creates no Mission, Assignment, Route, Invocation, Run, budget,
credential, network scope, external effect, or aggregated capability.

## Persistence and reconciliation

Schema version 10 adds `teams` and `team_formation_members`. Composite foreign keys keep Project,
Resident, and configuration evidence inside one Workspace. Update and delete triggers preserve
the Team and roster as formation history. The accepted Command writes the Team, every member, and
one `team.formed` Event atomically.

Startup compares every formation Event to its exact stored Team and ordered roster, requires one
Event per Team, and refuses missing, malformed, widened, or mismatched state. The v9→v10 migration
rebuilds the closed Event-kind table transactionally while preserving prior sequence, JSON bytes,
and timestamps.

Team reads retain each member's formation configuration ID and independently derive the exact
current configuration ID, current eligibility, and closed ineligibility reasons. Suspending a
Resident or clearing a configuration therefore changes current status without changing formation
history; a later valid configuration restores current eligibility without becoming the original
formation proof.

## Interfaces and presentation

The same types cross the Rust service, authenticated Unix socket, Tauri commands and capability
allowlist, and strict TypeScript adapter. Signed-out sessions cannot list, inspect, or form Teams.
The Projects application presents Teams inside their Project rather than as a competing suite app.
Its composer loads a fresh eligible-Resident snapshot, records exact configuration IDs, is
keyboard operable, and states the no-Mission/no-authority boundary before submission.

## Verification

From the repository root:

```sh
cargo test -p beacond --test team
cargo test -p beacond --test local_transport authenticated_client_forms_and_reads_a_project_team_over_the_real_socket -- --exact
cargo test -p beacond --lib version_nine_store_preserves_event_bytes_when_adding_empty_team_state -- --exact
cd apps/desktop && npm test && npm run build
scripts/gate all
```

The measured cases are recorded in
[`docs/evals/2026-08-29-project-team-formation.md`](../evals/2026-08-29-project-team-formation.md).
