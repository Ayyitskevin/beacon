# Resident Configuration evaluation — 2026-08-29

Scope: schema-v9 migration, public configuration service, authenticated Unix transport, Tauri
commands and capability allowlist, strict TypeScript parsing, and the desktop configure/clear flow.
The transport sweep uses an actual private Unix socket and an independently running beacond server.

## Live authenticated-socket measurement

Command:

```sh
cargo test -p beacond --test local_transport authenticated_client_configures_and_clears_a_resident_over_the_real_socket -- --exact
```

| # | Real socket case | Expected | Observed |
|---:|---|---|---|
| 1 | Read Capability catalog while signed out | `not_authenticated` | Pass |
| 2 | List or submit an invalid configuration while signed out | `not_authenticated` first | Pass |
| 3 | Read catalog after sign-in | Exactly seven closed Capabilities | Pass |
| 4 | List before first configuration | Empty | Pass |
| 5 | Configure with an empty Capability set | Invalid configuration | Pass |
| 6 | Configure an unknown Resident | Resident not found | Pass |
| 7 | Configure from the exact initial state | Append configured decision | Pass |
| 8 | Inspect binding budget | Exactly `no_spend` | Pass |
| 9 | Inspect active Resident and model result | Eligible for Team formation | Pass |
| 10 | Re-read configuration | Exact persisted view | Pass |
| 11 | Replay initial configuration | Stale configuration | Pass |
| 12 | Clear from exact configured decision | Cleared and ineligible | Pass |
| 13 | Replay prior clear | Stale configuration | Pass |
| 14 | Reconfigure, then deactivate exact model | Same decision becomes model-ineligible | Pass |
| 15 | List after locking session | `not_authenticated` | Pass |

## Additional measured boundaries

| Case | Expected behavior | Observed |
|---|---|---|
| Mixed work/artifact/evidence selection | Canonical round-trip order | Pass |
| Inactive → active → suspended → active | Eligibility changes without configuration rewrite | Pass |
| Model deactivation then new activation | Old activation ID never resurrects | Pass |
| Concurrent initial configuration | One winner and one stale command | Pass |
| Clear then reconfigure | Linear predecessor chain | Pass |
| Retired Resident | History visible; set and clear refused | Pass |
| Decision update or delete | Rejected by append-only trigger | Pass |
| Matching Event missing or changed | Startup refusal | Pass |
| Capability inserted outside decision | Startup refusal | Pass |
| Existing schema-v8 Event | Sequence, kind, payload, and time preserved at v9 | Pass |
| Strict TypeScript adapter | Unknown authority-shaped fields refused | Pass |
| Desktop configure and clear | Explicit selection and clear confirmation | Pass |
| Desktop accessibility scan | No axe violations in exercised flow | Pass |

The whole-repository gate, independent review, and exact-candidate Product Steward approval belong
to the branch handoff. This evidence does not itself authorize merge, publication, or execution.
