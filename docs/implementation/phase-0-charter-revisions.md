# Phase 0 immutable Charter revisions

This checkpoint lets an authenticated Owner change Project intent without erasing the intent under
which earlier decisions were made. It completes the authority prerequisite for future Missions,
which must bind one exact Charter Revision.

## Command contract

`revise_charter(project_id, expected_current_revision, draft)` accepts the same closed Charter
fields as Project creation. The expected revision is mandatory optimistic concurrency: if another
accepted command already advanced the Project, Beacon refuses the stale edit instead of merging or
overwriting it. An identical normalized Charter is also refused because it represents no intent
change.

The only accepted autonomy profiles remain Observe and Supervised. Budget remains `no_spend` and
external effects remain `prohibited`; those fields are outputs, never caller-controlled inputs.
Publishing a revision starts no Mission or Run and invokes no model, Worker, Route, Plugin,
credential, network request, or capability.

## Persistence and reconciliation

One immediate transaction appends the next contiguous immutable Charter row, advances the
Project's current pointer, sets `updated_at_ms` to the new Charter's authored time, and writes one
`project.charter_revised` Event containing the exact new Charter and predecessor revision. Any
failure rolls back every part.

Schema version 6 expands the closed Event-kind vocabulary and preserves version-5 Projects and
Events transactionally. Startup refuses to report ready unless:

- revisions begin at 1 and remain contiguous;
- authored times are nondecreasing;
- the current pointer identifies the terminal revision;
- Project update time equals the current Charter's authored time;
- every revision after 1 has exactly one matching revision Event;
- every revision Event binds its exact stored Charter, predecessor, Workspace, Project, and time;
- no orphan or duplicate revision Event exists.

## Interfaces and presentation

The public Rust module, authenticated Unix socket, Tauri commands, and strict TypeScript adapter all
carry the same revision and history shapes. History is oldest-to-newest and must itself be
contiguous before the desktop accepts it.

The Projects application pre-fills a revision composer from current intent and names the exact
revision it will publish. The current record stays compact; complete preserved history is one
disclosure away. A stale edit remains open with explicit reload guidance. Light, dark, and system
themes retain the same meaning, and automated accessibility checks cover the revision composer.

## Verification

From the repository root:

```sh
cargo test -p beacond --test projects
cargo test -p beacond --test local_transport authenticated_client_creates_and_reads_projects_over_the_real_socket -- --exact
cargo test --workspace
cd apps/desktop && npm test && npm run build
scripts/gate all
```

The real endpoint cases are recorded in
[`docs/evals/2026-08-27-charter-revision-endpoint.md`](../evals/2026-08-27-charter-revision-endpoint.md).
