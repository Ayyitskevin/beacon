# Project Policy evaluation — 2026-08-29

Scope: schema-v12 migration, public Project Policy service, authenticated Unix transport, Tauri
command manifest, strict TypeScript parsing, and the accessible Project-context Policy flow. No
merge, deployment, live migration, Track Record evidence, candidate evaluation, Team Lead
selection, model invocation, or external effect is part of this evaluation.

## Measured domain and persistence cases

| Case | Expected behavior | Observed |
|---|---|---|
| First publication on current Charter | Immutable revision 1 with exact Charter, criteria, author, time, and Event | Pass |
| Signed-out current/history/publish | Authentication refusal precedes project lookup or draft validation | Pass |
| Missing `work.coordinate_mission` | Refused before any Policy or Event write | Pass |
| Duplicate required Capability | Refused instead of silently widening or deduplicating intent | Pass |
| Empty Track Record requirements | Refused before persistence | Pass |
| Zero evidence threshold | Refused rather than treated as evidence | Pass |
| Duplicate evidence kind | Refused instead of combining competing thresholds | Pass |
| Missing Project | Refused inside the current Workspace with no partial state | Pass |
| Charter revision after publication | Preserved Policy reports that it no longer applies | Pass |
| Stale expected Charter revision | Refused instead of silently adopting current Project intent | Pass |
| Republishing after Charter revision | New immutable Policy revision binds the new Charter | Pass |
| Stale expected Policy revision | Refused rather than overwriting concurrent intent | Pass |
| Unchanged current Policy | Refused rather than creating meaningless history | Pass |
| Concurrent first publications | Exactly one commit and Event; competing command becomes stale | Pass |
| Earlier Policy on the same Charter | Preserved in history but never reports current after a later revision | Pass |
| Missing publication Event | Startup refusal | Pass |
| Event evidence changed after publication | Startup refusal | Pass |
| Noncanonical stored Capability order | Startup refusal | Pass |
| Existing schema-v11 Events and Mission guard | Sequence, JSON bytes, timestamps, and roster closure preserved at v12 | Pass |

## Interface and product-path cases

| Case | Expected behavior | Observed |
|---|---|---|
| Signed-out socket request | `not_authenticated` precedes project validation | Pass |
| Real socket publish/current/history | Exact Policy round trip through an independent beacond server | Pass |
| Session lock after publication | Current and history reads return `not_authenticated` | Pass |
| Tauri command adapter | Publish/current/history preserve the exact closed Policy type | Pass |
| Tauri capability manifest | Current/history/publish commands are explicitly allowlisted | Pass |
| Strict TypeScript boundary | Unknown lead or authority fields are refused | Pass |
| Missing required coordination Capability at TypeScript boundary | Refused as an invalid Project Policy | Pass |
| Duplicate or unordered evidence at TypeScript boundary | Refused rather than normalized outside the domain service | Pass |
| Project Policy composer | Sends exact Charter and expected prior Policy revisions | Pass |
| Required coordination control | Visible, checked, and not removable | Pass |
| Track Record threshold controls | Require at least one named positive count | Pass |
| UI boundary copy | No candidate evaluation, Lead selection, or authority claim | Pass |
| Automated accessibility scan | No axe violations in the exercised Policy composer | Pass |

Commands observed green on the completed implementation tree:

```sh
cargo test -p beacond --test project_policy
# 9 passed
cargo test -p beacond --test local_transport authenticated_client_publishes_project_policy_over_the_real_socket -- --exact
# 1 passed
cargo test -p beacond --lib version_eleven_store_preserves_event_bytes_when_adding_empty_project_policy_state
# 1 passed
cargo test -p beacon-desktop command_adapter_publishes_and_reads_project_policy_without_selecting_a_lead
# 1 passed
cd apps/desktop && npm test -- --run src/session.test.ts src/App.test.tsx
# 64 passed
scripts/gate all
# 424 Python tests and 70 desktop tests passed; desktop/native and workspace Rust tests,
# Clippy with warnings denied, formatting, lint, web build, Tauri build, and workspace build passed.
```

The whole-repository gate passed after the v12 migration preserved the Team and Mission closure
triggers and the Tauri capability assertion included every new command. Independent exact-candidate
review and Product Steward sign-off still belong to the branch handoff. This evidence does not
authorize merge, deployment, or live migration.
