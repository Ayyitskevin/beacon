# Project Mission admission evaluation — 2026-08-29

Scope: schema-v11 migration, public Mission service, authenticated Unix transport, Tauri command
manifest, strict TypeScript parsing, and the accessible Project-context Mission flow. No merge,
deployment, live migration, model invocation, or external effect is part of this evaluation.

## Measured domain and persistence cases

| Case | Expected behavior | Observed |
|---|---|---|
| Current Charter and eligible Team | One admitted Mission with exact Charter and member configurations | Pass |
| Empty outcome or success conditions | Refused before any Mission or Event write | Pass |
| Duplicate or oversized brief items | Refused rather than normalized ambiguously | Pass |
| Missing Project or Team | Refused within current Workspace and Project scope | Pass |
| Stale Charter revision | Refused instead of silently adopting current intent | Pass |
| Stale, omitted, reordered, or extra Team member | Refused instead of changing admission evidence | Pass |
| Member configuration from outside the owning Team | Rejected independently by the relational schema | Pass |
| Currently ineligible member | Refused even when the formation snapshot was eligible | Pass |
| Configuration changes after admission | Admission evidence remains fixed | Pass |
| Concurrent identical commands | Two distinct immutable Mission IDs and Events | Pass |
| Mission or member-row mutation | Rejected by append-only triggers | Pass |
| Missing or malformed admission Event | Startup refusal | Pass |
| Existing schema-v10 Events and Team guard | Sequence, JSON bytes, timestamps, and roster closure preserved at v11 | Pass |

## Interface and product-path cases

| Case | Expected behavior | Observed |
|---|---|---|
| Signed-out socket request | `not_authenticated` precedes domain validation | Pass |
| Real socket form-Team/admit/list/get | Exact Mission round trip through independent beacond server | Pass |
| Unknown Mission | Closed `mission_not_found` error | Pass |
| Tauri command manifest | List/get/admit commands explicitly allowlisted | Pass |
| Strict TypeScript boundary | Unknown lead fields and empty success conditions refused | Pass |
| Project Mission composer | Submits exact Charter, Team, and current member configurations | Pass |
| UI boundary copy | No Team Lead, Assignment, Run, spend, invocation, or effect | Pass |
| Automated accessibility scan | No axe violations in the exercised Mission composer | Pass |

Commands observed green during implementation:

```sh
cargo fmt --all -- --check
cargo test -p beacond
cargo test -p beacon-desktop main_window_capability_grants_every_registered_account_command
cd apps/desktop && npm test -- --run src/session.test.ts src/App.test.tsx
scripts/gate all
```

The whole-repository gate passed after the final schema hardening. Independent exact-candidate
review and Product Steward sign-off still belong to the branch handoff. This evidence does not
authorize merge, deployment, or live migration.
