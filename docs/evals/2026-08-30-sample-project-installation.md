# Sample Project installation evaluation — 2026-08-30

This evaluation exercises the opt-in Sample Project on local real paths. Backend service cases use temporary schema-17 SQLite stores, real private directories, and the host Git executable. The transport case uses a real authenticated Unix socket. The Tauri adapter reaches that transport; desktop component tests mock only the Tauri boundary and run an accessibility audit.

| # | Case | Observable acceptance | Evidence | Result |
|---:|---|---|---|---|
| 1 | Unauthenticated status Query | Returns `not_authenticated`; creates no managed directory or Project. | `status_and_install_refuse_an_unauthenticated_operator_without_filesystem_effects` | PASS |
| 2 | Unauthenticated install Command | Returns `not_authenticated` before filesystem or database publication. | Same service case plus unauthenticated Unix transport assertions | PASS |
| 3 | Authenticated available status | Reports bundle v1 and the frozen SHA-256 identity before installation. | `authenticated_operator_installs_an_inert_supervised_sample_project` | PASS |
| 4 | First installation | Creates Project, Charter revision 1, Repository revision 1, and immutable installation/Event state at the derived private path. | Same service case using real SQLite/filesystem/Git | PASS |
| 5 | Supervised authority | Created Charter is `supervised`, `no_spend`, with prohibited external effects. | Same service case through `ProjectService` | PASS |
| 6 | Real Declared Checks | Exact configured `cargo fmt --all -- --check` and `cargo test --locked` both execute successfully against installed bytes in the test harness. | Same service case; external target directory prevents test artifacts entering the repository | PASS |
| 7 | Inert setup boundary | Installation leaves `runs` and `receipts` empty; implementation contains no model or network client and never calls Declared Check execution. | Same service case plus implementation audit | PASS |
| 8 | Repeat installation | Returns `already_installed` with the same record; one Project, installation row, and installation Event remain. | `repeat_install_is_explicitly_idempotent_and_revision_is_deterministic` | PASS |
| 9 | Deterministic identity | Independent installs produce the frozen initial Git revision; v1's numeric version, compiled bytes, digest, and revision remain pinned to literals. Ambient Git repository/worktree/index variables are cleared, and an orphaned destination with different commit metadata is refused even when its tree is unchanged. | Repeat case, `bundle_v1_identity_is_permanent_and_future_bundles_are_additive`, `git_environment_drops_ambient_repository_redirection`, and `orphaned_repository_with_a_different_commit_is_refused` | PASS |
| 10 | Bundle mismatch | Refuses before staging or database publication. | `bundle_mismatch_fails_before_staging_or_database_publication` | PASS |
| 11 | Symlinked destination root | Refuses without accessing or changing the symlink target. | `symlinked_managed_root_is_refused_without_touching_its_target` | PASS |
| 12 | Partial publication failures | Git or repository-tree sync failure removes staging and publishes no Project, installation Event, Run, or Receipt. An injected SQLite publication failure rolls back every row, leaves only the exact durable orphan, and a retry verifies and adopts it. | `git_failure_cleans_partial_staging_and_publishes_no_database_state`, `repository_tree_sync_failure_cleans_staging_before_publication`, and `database_publication_failure_leaves_only_an_exact_adoptable_orphan` | PASS |
| 13 | Installed repository tamper | A changed bundled file makes the status Query fail closed while persisted identity remains intact. | `installed_repository_tamper_fails_status_closed` | PASS |
| 14 | Reconciliation and migration integrity | Removing the installation Event makes real supervisor startup fail closed. Version 16→17 migration preserves prior Event bytes and has no FK violations; rolling migration 0017 back inside its transaction restores the exact v16 schema, sequences, roster triggers, Events, and foreign keys before a successful forward startup. | `startup_reconciliation_refuses_missing_installation_event`; `version_sixteen_store_preserves_event_bytes_and_adds_empty_sample_state`; `migration_seventeen_rolls_back_exactly_before_forward_startup` | PASS |
| 15 | Product path | Unix Query/Command authentication, Tauri Query/Command adapters and ACL grants, Projects/onboarding reachability, success status, and axe audit all pass. | focused local-transport, desktop Rust, `App.test.tsx`, and `SampleProject.test.tsx` cases | PASS |

Focused commands:

```text
cargo test -p beacond sample_project::tests -- --nocapture
cargo test -p beacond local_transport::tests::sample_project_query_and_command_are_authenticated_over_local_transport -- --exact --nocapture
cargo test -p beacond --lib tests::version_sixteen_store_preserves_event_bytes_and_adds_empty_sample_state -- --exact --nocapture
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib tests::command_adapter_installs_and_reports_the_opt_in_sample_project -- --exact --nocapture
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib tests::main_window_capability_grants_every_registered_account_command -- --exact --nocapture
cd apps/desktop && npm test -- --run src/SampleProject.test.tsx
cd apps/desktop && npm test -- --run src/App.test.tsx -t "exposes the opt-in Sample Project"
```

No case performs a model invocation, network request, Operator-repository access, Proposal/Ruling operation, or Beacon Declared Check execution. Case 6 invokes the declared command vectors directly as evaluation of the bundled repository; therefore it creates no Beacon execution evidence.
