# Project Repository evaluation — 2026-08-29

## Verification identity

| Field | Value |
|---|---|
| Beacon tree under test | branch `claude/project-repository-binding`, single commit on base `main@6294d79` |
| Beacon crate versions | `beacond` 0.1.0, `beacon-desktop` 0.1.0 |
| Schema version under test | 13 (`0013_project_repository.sql`) |
| Repository fixture declared by the tests | `6b62110…` at `<author checkout>/janus` — redacted, and since replaced; see the note below |
| How the fixture commit was resolved | `git rev-parse` against a repository on the author's machine, run once by the orchestrator on 2026-08-29 and hard-coded |
| What the fixture is | a declaration only — no test in this slice opens that directory, reads `.git`, or runs a declared check |
| Host | Linux, `cargo test`/`vitest` on the repository's own `.venv`, Node 22, npm 10.9.8 |

> **Redacted 2026-08-30.** This eval ran against a fixture that named an absolute path inside the
> author's home directory. ADR 0028 made Beacon a standalone public product and removed such
> fixtures; the tests now declare a fabricated path and a synthetic commit id instead. The rows
> above are preserved as a record of what was verified on 2026-08-29, with the path redacted. The
> verification itself is unaffected — the fixture was a declaration that no test ever opened.

Scope: schema-v13 migration, public Project Repository service, authenticated Unix transport, Tauri
command manifest and wire-error mapping, strict TypeScript parsing, and the accessible
Project-context Repository flow. No merge, deployment, live migration, worktree, clone, fetch,
process spawn, check execution, Proposal, Run, Receipt, or external effect is part of this
evaluation.

## Measured domain and persistence cases

| Case | Expected behavior | Observed |
|---|---|---|
| First binding on current Charter | Immutable revision 1 with exact Charter, source, base revision, checks, author, time, and Event | Pass |
| Signed-out current/history/bind | Authentication refusal precedes project lookup or draft validation | Pass |
| Missing Project | Refused inside the current Workspace with no partial state | Pass |
| Base revision of 39 characters | Refused as an invalid draft before persistence | Pass |
| Uppercase base revision | Refused rather than lowercased into a different claim | Pass |
| Non-hexadecimal base revision | Refused before persistence | Pass |
| Relative local path source | Refused rather than resolved against a working directory | Pass |
| Local path with a trailing slash | Refused rather than trimmed | Pass |
| Git URL without a scheme or scp form | Refused before persistence | Pass |
| Empty source | Refused before persistence | Pass |
| Zero declared checks | Refused rather than stored as an empty evidence claim | Pass |
| Duplicate declared check name | Refused instead of silently deduplicating intent | Pass |
| Empty or uppercase check name | Refused instead of normalized | Pass |
| Empty argument vector | Refused rather than treated as a runnable command | Pass |
| Empty or NUL-bearing argument | Refused before persistence | Pass |
| Absolute working directory | Refused rather than escaping the repository | Pass |
| Working directory containing `..` | Refused rather than escaping the repository | Pass |
| Noncanonical declared check order in a draft | Canonicalized to name order before persistence | Pass |
| Git URL binding | Accepted and stored without contacting the remote | Pass |
| Charter revision after binding | Preserved binding reports that it no longer applies; history intact | Pass |
| Stale expected Charter revision | Refused instead of silently adopting current Project intent | Pass |
| Rebinding after Charter revision | New immutable revision binds the new Charter | Pass |
| Stale expected prior Repository revision | Refused rather than overwriting concurrent intent | Pass |
| Unchanged draft on the same Charter | Refused rather than creating meaningless history | Pass |
| Concurrent first bindings | Exactly one commit and one Event; competing command becomes stale | Pass |
| Earlier revision on the same Charter | Preserved in history but never reports current after a later revision | Pass |
| Missing binding Event | Startup refusal (`ProjectRepositoryStateMismatch`) | Pass |
| Event evidence changed after binding | Startup refusal (`ProjectRepositoryEventMismatch`) | Pass |
| Noncanonical stored declared check order | Startup refusal | Pass |
| Existing schema-v12 Events and both roster guards | Sequence, JSON bytes, timestamps, and both closure triggers preserved at v13 | Pass |

## Interface and product-path cases

| Case | Expected behavior | Observed |
|---|---|---|
| Signed-out socket request | `not_authenticated` precedes project validation | Pass |
| Real socket bind/current/history | Exact Repository round trip through an independent beacond server | Pass |
| Session lock after binding | Current and history reads return `not_authenticated` | Pass |
| Tauri command adapter | Bind/current/history preserve the exact closed Repository type | Pass |
| Tauri adapter declares nothing it did not read | No worktree path or check result appears on the wire | Pass |
| Invalid draft through the adapter | Surfaces as `invalid_project_repository`, not the generic account failure | Pass |
| Unchanged draft through the adapter | Surfaces as `unchanged_project_repository` | Pass |
| Stale prior revision through the adapter | Surfaces as `stale_project_repository` | Pass |
| Tauri capability manifest | Current/history/bind commands are explicitly allowlisted | Pass |
| Strict TypeScript boundary | Unknown worktree or result fields are refused | Pass |
| Short base revision at the TypeScript boundary | Refused as an invalid Project Repository | Pass |
| Unordered declared checks at the TypeScript boundary | Refused rather than normalized outside the domain service | Pass |
| Empty argv or escaping working directory at the TypeScript boundary | Refused as an invalid Project Repository | Pass |
| Project Repository composer | Sends exact Charter and expected prior Repository revisions and canonical checks | Pass |
| UI boundary copy | States "Declared, not verified — Beacon has not opened this repository." | Pass |
| Stale-Charter warning | Names the current Charter revision and asks for a new binding | Pass |
| Automated accessibility scan | No axe violations in the exercised Repository composer | Pass |

## Commands observed green on the tree named above

```sh
cargo test -p beacond --test project_repository
# 11 passed
cargo test -p beacond --test local_transport authenticated_client_binds_project_repository_over_the_real_socket -- --exact
# 1 passed (15 in the file)
cargo test -p beacond --lib version_twelve_store_preserves_event_bytes_when_adding_empty_project_repository_state
# 1 passed (32 in the crate's lib tests)
cargo test -p beacon-desktop command_adapter_binds_and_reads_project_repository_without_opening_it
# 1 passed
cargo test -p beacon-desktop repository_refusals_reach_the_interface_under_their_own_code
# 1 passed
cd apps/desktop && npx vitest run src/session.test.ts src/App.test.tsx
# 67 passed
scripts/gate all
# gate: all checks passed
# 424 Python tests, 73 desktop TypeScript tests, 14 desktop Rust tests, and 152 workspace Rust
# tests passed; Clippy with warnings denied, formatting, lint, web build, Tauri build, and
# workspace build passed.
```

## Mutation checks

| Mutation | Expected | Observed |
|---|---|---|
| Remove the unchanged-draft refusal in `bind` | `repository_revision_is_stale_safe_and_charter_changes_require_rebinding` fails | Fails as expected |
| Remove the canonical-order sort in `normalize_declared_checks` | `declared_checks_are_stored_in_canonical_name_order` fails | Fails as expected |
| Remove the `InvalidProjectRepository` arm from the Tauri wire-error table | `repository_refusals_reach_the_interface_under_their_own_code` fails | Fails as expected |

Each mutation was reverted with `git checkout --` on that file alone and the focused suite re-run
green before the next one.

The whole-repository gate passed after the v13 migration preserved both closure triggers and the
Tauri capability assertion included every new command. This evidence covers a declaration record
only: it does not establish that the declared repository exists at that commit, that its checks pass,
or that any check has ever run. Independent exact-candidate review and Product Steward sign-off
still belong to the branch handoff. This evidence does not authorize merge, deployment, or live
migration.
