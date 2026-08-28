# Starter Organization real-endpoint evaluation — 2026-08-27

Scope: the public `LocalSupervisorClient` crossing the authenticated Unix-domain socket into the
independently owned `beacond` store. These are not direct database or mocked transport cases.

Command:

```sh
cargo test -p beacond --test local_transport authenticated_client_decides_the_starter_organization_over_the_real_socket -- --exact
cargo test -p beacond --test local_transport authenticated_client_discards_the_starter_organization_over_the_real_socket -- --exact
```

| Case | Expected endpoint behavior | Observed |
|---|---|---|
| Locked proposal read | `not_authenticated` | Pass |
| Fresh authenticated read | Version 1, `proposed` | Pass |
| Stale proposal version | `unsupported_starter_proposal` | Pass |
| Empty acceptance | `invalid_starter_roster` | Pass |
| Duplicate Builder role | `invalid_starter_roster` | Pass |
| Blank Display Name | `invalid_starter_roster` | Pass |
| Control character in Display Name | `invalid_starter_roster` | Pass |
| 81-character Display Name | `invalid_starter_roster` | Pass |
| Read after invalid edits | Still `proposed`, no Residents | Pass |
| Edited three-role acceptance | `accepted`, three inactive Residents | Pass |
| Acceptance after acceptance | `starter_decision_recorded` | Pass |
| Read after manual lock | `not_authenticated` | Pass |
| Explicit discard | `discarded`, no Residents | Pass |
| Acceptance after discard | `starter_decision_recorded` | Pass |

Result: 14/14 endpoint expectations passed. Store restart reconciliation, invalid-edit atomicity,
and concurrent-decision serialization are covered separately in
`crates/beacond/tests/starter_organization.rs`.
