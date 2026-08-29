# Phase 1 Project Repository

This checkpoint adds Beacon's first durable Project Repository declaration without claiming that
the repository exists, that the pinned commit is reachable, or that any declared check has ever
run. An authenticated Operator can bind one repository, one pinned base revision, and one or more
Declared Checks to a Project.

It is numbered Phase 1 rather than Phase 0 because it is the first slice under
[ADR 0025](../adr/0025-prove-alpha-against-the-operators-fleet.md), which opens the Alpha 1 kernel
work that proves Beacon against the Operator's own fleet; the Phase 0 checkpoints record the
Workspace, Project, and roster foundation this slice builds on.

## Closed declaration boundary

The public command accepts a Project ID, the exact current Charter revision, the expected prior
Repository revision, and a closed Repository draft. The draft names a source kind of `local_path`
or `git_url`, a source, a base revision, and at least one Declared Check. A `local_path` source
must be absolute and must not end in a slash. A `git_url` source must be an `https://` or `ssh://`
URL or an scp-style `user@host:path` address. The base revision must be exactly 40 lowercase
hexadecimal characters. Each Declared Check needs a distinct name matching `[a-z0-9][a-z0-9._-]*`
of at most 64 characters, a non-empty argument vector whose elements are non-empty and free of NUL,
and a repository-relative working directory that is `.` or a POSIX path that is never absolute and
never contains `..`. Beacon canonicalizes the check list by name before persistence.

A shell string cannot be declared, because the command accepts only an argument vector. The
persisted author kind is `operator`, matching the command subject that exists.

Binding records a declaration only. Beacon does not open, clone, fetch, or read the named
repository, does not confirm the base revision exists, and does not run a declared check. It
creates no worktree, checkout, Proposal, Run, Receipt, Evaluation, Assignment, Route, Invocation,
credential, budget, network scope, external effect, or execution authority.

## Persistence and reconciliation

Schema version 13 adds immutable `project_repository_revisions`. Every row belongs to one Project
and has a composite foreign key to the exact Charter revision it interprets. Revision numbers are
contiguous per Project, author times are monotonic, and update/delete triggers preserve bound
history. Binding checks the Charter and the expected prior Repository revision inside one immediate
transaction, then writes the row and one `project.repository_bound` Event atomically.

Startup compares every Repository Event to its exact stored revision, requires one Event per
revision, and refuses missing, malformed, widened, noncanonical, or mismatched state. The v12→v13
migration rebuilds the closed Event-kind table transactionally while preserving earlier Event
sequence, JSON bytes, timestamps, and both the Team formation and Mission admission roster guards.

The latest Repository revision reports whether it still applies to the current Charter. A Charter
revision makes that answer false without changing history. Rebinding against the new Charter
creates the next Repository revision; earlier revisions never become current again.

## Interfaces and presentation

The same types cross the Rust service, authenticated Unix socket, Tauri commands and capability
allowlist, and strict TypeScript adapter. Signed-out sessions cannot read or bind a Repository.
The Projects application presents the binding beside the owning Charter, preserves revision
history, warns when the Charter has made the binding stale, and states plainly that the record is
declared, not verified — Beacon has not opened this repository.

The Tauri wire-error table gained arms for the four Repository codes, so a refusal now reaches the
interface under its own code instead of the generic account-service message.

## Verification

From the repository root:

```sh
cargo test -p beacond --test project_repository
cargo test -p beacond --test local_transport authenticated_client_binds_project_repository_over_the_real_socket -- --exact
cargo test -p beacond --lib version_twelve_store_preserves_event_bytes_when_adding_empty_project_repository_state
cargo test -p beacon-desktop command_adapter_binds_and_reads_project_repository_without_opening_it
cargo test -p beacon-desktop repository_refusals_reach_the_interface_under_their_own_code
cd apps/desktop && npm test && npm run build
scripts/gate all
```

The measured cases, the Beacon tree they were observed on, and the Janus fixture commit they
declare are recorded in
[`docs/evals/2026-08-29-project-repository.md`](../evals/2026-08-29-project-repository.md).
