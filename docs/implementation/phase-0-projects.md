# Phase 0 Projects and immutable Charters

This checkpoint turns Projects into Beacon's first functional suite application after Briefing.
An authenticated Owner can create, list, and inspect Projects through the same typed supervisor
interface used by the desktop and local client.

## Contract

Project creation requires a trimmed name and a complete Charter revision 1: purpose, at least one
desired outcome, optional constraints and prohibitions, and either Observe or Supervised autonomy.
Names are limited to 120 characters, purpose to 2,000, and each Charter list to 20 distinct
single-line entries of at most 300 characters. Control characters and empty required values fail
closed.

Callers cannot submit a budget, execution authority, model, Route, Worker, capability, credential,
Plugin, Mission, or Run. The supervisor always records `no_spend` and `prohibited` external effects.
Project creation starts no work and makes Bounded autonomy unavailable.

## Ownership and state

`ProjectService` owns Project queries and creation behind the active Operator Account session.
`beacond` remains the sole store writer. Schema version 5 adds scoped Project rows, immutable
Charter revisions, immutable creation snapshots, and the `project.created` Event kind. One
immediate transaction writes revision 1, its snapshot, and its Event.

The mutable Project row points to an exact current Charter revision. Revision history must begin at
1 and remain contiguous. Startup independently reconstructs the creation record, validates every
stored Charter revision, and requires exactly one matching creation Event per Project. Event scope,
timestamp, payload, snapshot, and Charter must agree before the store can report ready. Future
Charter changes will append a revision rather than rewriting history.

## Interfaces and presentation

The Rust service, Unix-domain socket, Tauri commands, and TypeScript adapter transport the same
closed Project shapes. Unknown fields and widened authority values are rejected. Locked sessions
cannot list, read, or create Projects.

Briefing remains the ordinary launch surface. Projects is the only other enabled application in
the rail; Conversations, Knowledge, and Software remain visibly unavailable until they have real
implementations. The Projects workspace provides a keyboard-complete Fluent UI Charter composer,
portfolio list, and durable Charter record. Light, dark, and system themes share the same semantic
boundaries. Automated accessibility checks cover the composer.

## Verification

From the repository root:

```sh
cargo test -p beacond --test projects
cargo test -p beacond --test local_transport authenticated_client_creates_and_reads_projects_over_the_real_socket -- --exact
cd apps/desktop && npm test && npm run build
scripts/gate all
```

The real endpoint cases and observed results are recorded in
[`docs/evals/2026-08-27-project-endpoint.md`](../evals/2026-08-27-project-endpoint.md).
