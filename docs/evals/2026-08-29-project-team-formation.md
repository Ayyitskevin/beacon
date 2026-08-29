# Project Team formation evaluation — 2026-08-29

Scope: schema-v10 migration, public Team service, authenticated Unix transport, Tauri command
manifest, strict TypeScript parsing, and the accessible Project-context Team flow. No deployment or
live migration is part of this evaluation.

## Measured domain and persistence cases

| Case | Expected behavior | Observed |
|---|---|---|
| Three eligible Residents | One Project Team with exact formation configurations | Pass |
| One eligible Resident | Valid Team; Alpha's three-Resident scenario is not a cardinality rule | Pass |
| Empty or duplicate roster | Refused before any Team or Event write | Pass |
| Missing Project or configuration | Refused within current Workspace scope | Pass |
| Stale configuration ID | Refused instead of silently adopting current state | Pass |
| Inactive Resident | Refused as currently ineligible | Pass |
| Suspend, clear, and reconfigure after formation | Formation proof fixed; current eligibility derived | Pass |
| Concurrent identical commands | Two distinct immutable Team IDs and two Events | Pass |
| Team row update | Rejected by append-only trigger | Pass |
| Member insert after formation Event | Rejected because the immutable roster is closed | Pass |
| Malformed formation Event | Startup refusal | Pass |
| Existing schema-v9 Event | Sequence, JSON bytes, and timestamp preserved at v10 | Pass |

## Interface and product-path cases

| Case | Expected behavior | Observed |
|---|---|---|
| Signed-out socket request | `not_authenticated` before domain validation | Pass |
| Real socket form/list/get | Exact Team round trip through independent beacond server | Pass |
| Unknown Team | Closed `team_not_found` error | Pass |
| Tauri command manifest | List/get/form commands explicitly allowlisted | Pass |
| Strict TypeScript boundary | Unknown lead/authority fields and empty rosters refused | Pass |
| Project Team composer | Submits exact Resident/configuration pairs | Pass |
| UI boundary copy | No permanent lead; no Mission, Run, spend, invocation, or effect | Pass |
| Automated accessibility scan | No axe violations in exercised Team composer | Pass |

Commands already observed green during implementation:

```sh
cargo test -p beacond --test team
cargo test -p beacond --test local_transport authenticated_client_forms_and_reads_a_project_team_over_the_real_socket
cargo test -p beacond --lib
cargo test -p beacon-desktop --lib
cd apps/desktop && npm test -- --run src/session.test.ts
cd apps/desktop && npm test -- --run src/App.test.tsx
cd apps/desktop && npm run build
```

The whole-repository gate, independent exact-candidate review, and Product Steward sign-off belong
to the branch handoff. This evidence does not authorize merge, deployment, or live migration.
