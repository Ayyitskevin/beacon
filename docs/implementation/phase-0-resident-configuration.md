# Phase 0 Resident Configuration

This checkpoint lets the authenticated Operator prepare persistent Residents for later Team
formation. It deliberately stops before Teams, Missions, Routes, model invocation, or execution.

## Closed configuration boundary

One Resident Configuration binds:

- one admitted, non-retired Resident;
- one exact current and active local-model Resource Activation with `completion` approved;
- one non-empty, duplicate-free subset of Beacon's seven operation-shaped Capabilities; and
- the fixed Alpha `no_spend` budget boundary.

The Capability catalog is closed to `work.create_mission`, `work.form_team`,
`work.coordinate_mission`, `work.assign_work`, `artifact.produce`, `artifact.evaluate`, and
`evidence.develop`. These are declared fitness for later policy decisions. Configuration creates no
Route, Team, Mission, Assignment, Invocation, Run, credential, network scope, external effect, or
execution authority.

Team eligibility is derived on every read. A configured Resident is eligible only when its
lifecycle is active and the exact bound activation remains the current active decision for that
local model. Suspension, retirement, clearing, or model deactivation removes eligibility without
rewriting history. Reactivating the same model creates a new activation ID and cannot revive the
old binding.

## Persistence and reconciliation

Schema version 9 adds three append-only structures:

- `resident_configuration_decisions` stores the linear Operator decision chain;
- `resident_configuration_bindings` stores the exact activation and `no_spend` boundary; and
- `resident_configuration_capabilities` stores the closed Capability set.

Partial unique indexes prevent two initial decisions or two successors from one predecessor.
Composite foreign keys preserve Workspace and Resident scope. Triggers reject updates and deletes
from all three structures. Set or clear writes its decision, binding rows, Capability rows, and one
typed Event in the same immediate transaction.

Startup refuses to report ready unless every chain begins once, names its exact predecessor,
advances time, has the structurally correct binding for its state, references an activation with
approved completion, and has exactly one byte-equivalent `resident.configuration_set` or
`resident.configuration_cleared` Event. Foreign-key and integrity checks run before reconciliation.
The v8→v9 migration rebuilds the closed Event-kind table transactionally and preserves existing
sequence, payload, and timestamps.

## Interfaces and presentation

The public Rust service, authenticated Unix socket, Tauri commands, generated capability
allowlist, and strict TypeScript adapter use the same closed types. Signed-out sessions cannot read
the catalog or configurations. Invalid targets, model state, Capability selections, and stale
predecessors fail without a write.

Briefing loads configuration state only after the Operator asks to review it. That read uses stored
Beacon state and does not contact or invoke a provider. The editor offers role-based suggestions,
but the Operator must review and confirm every Capability and exact model. It displays the fixed
no-spend boundary, distinguishes configuration from lifecycle, and states that configuration does
not grant execution authority. Clearing requires an accessible confirmation step.

## Verification

From the repository root:

```sh
cargo test -p beacond --test resident_configuration
cargo test -p beacond --test local_transport authenticated_client_configures_and_clears_a_resident_over_the_real_socket -- --exact
cargo test -p beacond --lib version_eight_store_preserves_existing_events_when_adding_resident_configuration
cd apps/desktop && npm test && npm run build
scripts/gate all
```

The 15-case authenticated-socket sweep and additional persistence, desktop, and tamper cases are
recorded in
[`docs/evals/2026-08-29-resident-configuration.md`](../evals/2026-08-29-resident-configuration.md).
