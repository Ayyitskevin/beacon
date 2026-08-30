# Sample Project installation

Beacon's Sample Project is an authenticated, opt-in setup action that installs a private bundled Rust task tracker as an ordinary Supervised Project. Installation is inert: it creates no Run, Receipt, Proposal, Ruling, model request, network request, or Declared Check execution.

## Interfaces and state

- `SampleProjectService::status` backs the authenticated `sample_project_status` Query. It returns either the frozen available bundle identity or the verified installation record.
- `SampleProjectService::install` backs the authenticated `install_sample_project` Command. It accepts no destination or repository input and returns `installed` or the explicit idempotent result `already_installed`.
- The local Unix transport and Tauri adapters preserve those typed results. The desktop Projects surface presents the action as optional and reports loading, success, and refusal through accessible status/alert regions.
- `sample_project_installations` is the immutable installation record. It binds the Workspace, Project, Charter revision, Repository revision, managed path, initial Git revision, bundle version, SHA-256 digest, provenance source, and installation time.
- `sample_project.installed` is the one new reconciled persisted Event. Existing `project.created` and `project.repository_bound` Events remain the normal domain records for the Project and Repository identities.

Migration 0017 expands schema version 16 to 17, admits the new Event kind, and adds the installation table and immutability triggers. Existing Event rows are copied with sequence, payload bytes, and timestamps unchanged. A direct transactional rollback guard proves the exact v16 schema, Events, `sqlite_sequence`, roster triggers, and foreign-key state are restored before the same store advances successfully. Startup verifies exact schema shape, foreign keys, every Sample Project Event against its installation row, and exactly one installation Event per row. Unknown, missing, duplicate, or mismatched evidence fails startup closed.

## Bundled repository

The source bundle lives under `crates/beacond/sample-project/` and contains only `Cargo.toml`, `Cargo.lock`, `README.md`, and `src/lib.rs`. It has no dependencies. Bundle version 1 uses SHA-256 with canonicalizer `beacon.sample-project.bundle.v1`; the digest covers the canonicalizer plus each ordered path, regular-file mode, byte length, and exact bytes.

Production derives the destination from the supervisor store, never from a Command argument:

```text
<store parent>/sample-projects/<workspace UUID>/task-tracker-v1
```

The Project Charter is Supervised with `no_spend` and prohibited external effects. Repository revision 1 is a local-path binding at the deterministic initial Git commit. Its real Declared Checks are:

```text
cargo fmt --all -- --check
cargo test --locked
```

Installation does not run them.

## Materialization and failure behavior

1. Authentication succeeds before filesystem work.
2. Beacon verifies its compiled bundle digest.
3. It refuses symlinked, non-directory, or non-private managed directories.
4. It writes owner-only files into a private random staging directory, initializes deterministic Git history without templates and in a closed subprocess environment, and verifies exact files, tracked paths, clean status, and the frozen v1 HEAD (`c5a2dad9164a1fd5346fdff3d3e2d8d17af922d4`). Ambient Git repository, worktree, index, object, configuration, hook, and prompt variables are not inherited.
5. Beacon syncs every file and directory in the verified repository from leaves to root.
6. Linux `renameat2(RENAME_NOREPLACE)` publishes the verified directory without replacing an existing path, then Beacon syncs the containing directory.
7. One immediate SQLite transaction publishes the Project, Charter, Repository binding, installation record, and their Events.

Filesystem rename and SQLite commit cannot form one cross-system transaction. If database publication fails after rename, no partial database state survives; a later authenticated install verifies and adopts only the exact bundled repository at the fixed path. Any differing path, permissions, symlink, file bytes, Git index, worktree status, or revision is refused. Git failure removes the private staging directory before returning.

The service never accepts an Operator repository locator and invokes Git only against its derived staging or installed path. Errors crossing the local transport are reduced to the stable `sample_project_unavailable` refusal; internal paths and process output are not exposed to the desktop.

## Verification

The focused service tests use real SQLite stores, filesystem publication, and Git. They cover authentication, deterministic provenance and revision, both real Declared Checks, repeat installation, digest mismatch, symlink refusal, Git and tree-sync cleanup, database rollback plus exact-orphan adoption, ambient Git-environment isolation, orphaned-repository revision refusal, installed-repository tamper, Event corruption, and absence of Run/Receipt rows. Unix transport, Tauri adapter/ACL, and accessible desktop tests cover the remaining public seams. The 15-case record is in `docs/evals/2026-08-30-sample-project-installation.md`.
