# Phase 0 Project Policy

This checkpoint adds Beacon's first durable Project Policy declaration without claiming candidate
evaluation, Team Lead selection, or autonomous Agent work. An authenticated Operator can publish
immutable Team Lead selection criteria inside one Project.

## Closed declaration boundary

The public command accepts a Project ID, the exact current Charter revision, the expected prior
Policy revision, and a closed Team Lead selection draft. The draft must contain distinct required
Capabilities including `work.coordinate_mission` and at least one distinct named Track Record
evidence kind with a positive minimum count. Beacon canonicalizes both lists before persistence.

Availability is fixed to current Team eligibility. A declared leadership conflict disqualifies a
candidate, and the Operator must adjudicate whenever eligible candidates remain ambiguous. These
fixed safeguards cannot be widened through the command payload.

The persisted author is `operator`, matching the command subject that exists. Publication records
criteria only. It creates no candidate evaluation, Team Lead, Assignment, Route, Invocation, Run,
budget, credential, network scope, external effect, aggregate Team authority, or execution
authority. Track Record evidence, selection, replacement, and Resident-issued commands remain
later slices.

## Persistence and reconciliation

Schema version 12 adds immutable `project_policy_revisions`. Every row belongs to one Project and
has a composite foreign key to the exact Charter revision it interprets. Revision numbers are
contiguous per Project, author times are monotonic, and update/delete triggers preserve published
history. Publication checks the Charter and expected prior Policy revision inside one immediate
transaction, then writes the Policy and one `project.policy_published` Event atomically.

Startup compares every Policy Event to its exact stored revision, requires one Event per revision,
and refuses missing, malformed, widened, noncanonical, or mismatched state. The v11→v12 migration
rebuilds the closed Event-kind table transactionally while preserving earlier Event sequence, JSON
bytes, timestamps, and the v11 Mission roster guard.

The latest Policy reports whether it still applies to the current Charter. A Charter revision makes
that answer false without changing history. Republishing against the new Charter creates the next
Policy revision; earlier revisions never become current again.

## Interfaces and presentation

The same types cross the Rust service, authenticated Unix socket, Tauri commands and capability
allowlist, and strict TypeScript adapter. Signed-out sessions cannot list, inspect, or publish
Policies. The Projects application presents the declaration beside the owning Charter, preserves
revision history, warns when the Charter has made the Policy stale, and makes the no-selection and
no-authority boundary explicit before publication.

## Verification

From the repository root:

```sh
cargo test -p beacond --test project_policy
cargo test -p beacond --test local_transport authenticated_client_publishes_project_policy_over_the_real_socket -- --exact
cargo test -p beacond --lib version_eleven_store_preserves_event_bytes_when_adding_empty_project_policy_state
cargo test -p beacon-desktop command_adapter_publishes_and_reads_project_policy_without_selecting_a_lead
cd apps/desktop && npm test && npm run build
scripts/gate all
```

The measured cases are recorded in
[`docs/evals/2026-08-29-project-policy.md`](../evals/2026-08-29-project-policy.md).
