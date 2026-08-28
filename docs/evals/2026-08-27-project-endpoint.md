# Project real-endpoint evaluation — 2026-08-27

Scope: the public `LocalSupervisorClient` crossing the authenticated Unix-domain socket into the
independently owned `beacond` store. These are not direct database or mocked transport cases.

Command:

```sh
cargo test -p beacond --test local_transport authenticated_client_creates_and_reads_projects_over_the_real_socket -- --exact
```

| Case | Expected endpoint behavior | Observed |
|---|---|---|
| Locked Project list | `not_authenticated` | Pass |
| Locked Project creation | `not_authenticated` | Pass |
| Fresh authenticated list | Empty | Pass |
| Blank Project name | `invalid_project`; list stays empty | Pass |
| Control character in Project name | `invalid_project`; list stays empty | Pass |
| Blank purpose | `invalid_project`; list stays empty | Pass |
| More than 20 desired outcomes | `invalid_project`; list stays empty | Pass |
| Duplicate normalized desired outcome | `invalid_project`; list stays empty | Pass |
| Control character in a constraint | `invalid_project`; list stays empty | Pass |
| Complete Supervised Charter | Revision 1 with fixed safe boundaries | Pass |
| Exact Project read | Matches created Project | Pass |
| Project portfolio list | Contains exact created summary | Pass |
| Complete Observe Charter | Observe accepted without widening authority | Pass |
| Unknown Project identifier | `project_not_found` | Pass |
| List after two valid creations | Exactly two Projects | Pass |
| List after manual Session Lock | `not_authenticated` | Pass |
| List after supervisor restart before sign-in | `not_authenticated` | Pass |
| Re-authenticated list after restart | Both Projects preserved | Pass |
| Exact Project read after restart | Matches original immutable Charter | Pass |

Result: 19/19 endpoint expectations passed. Store-level creation atomicity, schema-v4 migration,
missing or timestamp-divergent Event refusal, Charter-history continuity, and startup reconciliation
are covered separately by `crates/beacond/tests/projects.rs` and the supervisor unit tests.
