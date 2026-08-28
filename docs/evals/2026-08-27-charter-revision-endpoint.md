# Charter revision real-endpoint evaluation — 2026-08-27

Scope: the public `LocalSupervisorClient` crossing the authenticated Unix-domain socket into the
independently owned `beacond` store. These are real transport and persistence cases, not mocked
frontend calls or direct database writes.

Command:

```sh
cargo test -p beacond --test local_transport authenticated_client_creates_and_reads_projects_over_the_real_socket -- --exact
```

| Case | Expected endpoint behavior | Observed |
|---|---|---|
| Locked Project list | `not_authenticated` | Pass |
| Locked Charter history | `not_authenticated` | Pass |
| Locked Project creation | `not_authenticated` | Pass |
| Fresh authenticated list | Empty | Pass |
| Blank Project name | `invalid_project`; list stays empty | Pass |
| Control character in Project name | `invalid_project`; list stays empty | Pass |
| Blank purpose | `invalid_project`; list stays empty | Pass |
| More than 20 desired outcomes | `invalid_project`; list stays empty | Pass |
| Duplicate normalized desired outcome | `invalid_project`; list stays empty | Pass |
| Control character in a constraint | `invalid_project`; list stays empty | Pass |
| Complete Supervised Charter | Revision 1 with fixed safe boundaries | Pass |
| Exact Project read | Matches the created Project | Pass |
| Initial Charter history | Exact immutable revision 1 | Pass |
| Current-base revision | Advances to revision 2 and Observe | Pass |
| Revised Charter history | Exact revisions 1 and 2, oldest first | Pass |
| Stale-base revision | `stale_charter_revision`; no overwrite | Pass |
| Identical revision | `unchanged_charter`; no extra revision | Pass |
| Invalid revision body | `invalid_project`; no extra revision | Pass |
| Unknown Project revision | `project_not_found` | Pass |
| Revised portfolio summary | Revision 2 and Observe | Pass |
| Complete second Observe Project | Observe accepted without widened authority | Pass |
| Unknown Project read | `project_not_found` | Pass |
| List after two Projects | Exactly two Projects | Pass |
| List after manual Session Lock | `not_authenticated` | Pass |
| History after manual Session Lock | `not_authenticated` | Pass |
| Revision after manual Session Lock | `not_authenticated` | Pass |
| List after supervisor restart before sign-in | `not_authenticated` | Pass |
| Re-authenticated list after restart | Both Projects preserved | Pass |
| Exact current Project after restart | Revised Charter remains current | Pass |
| Exact history after restart | Both immutable revisions preserved | Pass |

Result: 30/30 endpoint expectations passed. Store-level atomicity, concurrency, schema-v5
migration, altered or missing Event refusal, history continuity, pointer reconciliation, and update
time reconciliation are covered separately by `crates/beacond/tests/projects.rs` and the supervisor
unit tests.
