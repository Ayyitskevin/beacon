# Recent Receipts: one Workspace-scoped, bounded, cross-kind Query

BLUEPRINT:171 gives Briefing "recent receipts". Until this slice the only Receipt Query was
Project-scoped, so the desktop fanned out over `projects` and chose which Projects to open, in what
order, how many rows to hold, and how to merge them. AGENTS.md:173 is explicit that a UI app does
not join module tables, rebuild domain objects, or encode domain policy; that fan-out did all three.
This module owns the join, the order, the bound, and the per-kind vocabulary.

The panel reads this Query and nothing else. The Project-scoped `preflight_receipts` path is
**retired end to end** — service method, transport command/result/client helper, Tauri command,
capability entry, generated permission file, `build.rs` command list, the TypeScript adapter and its
validator, and the tests of each. Nothing else read it: the only production consumer was Briefing's
fan-out, and every other reference was a test of the path itself.

The panel adds nothing to the reading. No sort (the daemon defines the order and the adapter refuses
a page that is not in it), no cap of its own beyond the five rows it draws, and no count it computed
— every number it shows is a number the page carries.

## The window is over Runs, not Receipts

`LIMIT` is applied to **Runs** and the rows are partitioned afterwards. Two consequences, and every
field is named for them:

- a request for 50 can return fewer than 50 Receipts while older Receipts exist;
- `more_runs_exist` can be true because an older Run **without** a Receipt exists, so it never claims
  another Receipt exists.

`run_window_applied`, `runs_in_flight_in_window` and `runs_unconfirmed_in_window` all say the same
thing in their names. The alternative — limit a Receipt-driven stream — cannot bound its scan when a
run of receiptless Runs sits at the head of the order.

Driving from `runs` with a `LEFT JOIN` rather than from `receipts` is not a style choice.
**Filtering for "has a Receipt" and then applying `LIMIT` is a different window**: the receiptless
counts would then be drawn from the whole table rather than from the rows the Receipts came from.

## An unconfirmed Run is not a Receipt, and its count is not the Resume Work review

CONTEXT.md:305 defines a Receipt as "an immutable account of a Run's inputs, effects, checks, usage,
result, and content identities", and `check_preflight.rs` states that an unconfirmed Run is precisely
one that never recorded a Receipt. A row claiming to be its Receipt would be evidence asserting
itself where none exists. Silence is the other failure, so receiptless Runs are **counted** beside
the rows — split into a live lease and an expired one, because BLUEPRINT:385 reviews those as two
conditions.

**That count is not the Resume Work set.** An unconfirmed Run older than the window does not appear
in it, and the review before Resume Work needs every unresolved Run. Reading this bounded subset as
exhaustive would give it a meaning it does not have; the Resume surface must query for them.

## What the daemon validates

The whole stored tuple, before a row exists: `(kind, state, version, outcome, receipt presence,
ended_at_ms, recorded_at_ms)`. Checking kind and outcome alone leaves the contract half-enforced —
ADR 0031 makes the state pairing part of the Receipt contract.

| Run kind | outcome / summary | required `runs.state` |
|---|---|---|
| `check_preflight` | `verified` | `verified` |
| `check_preflight` | the other nine | `refused` |
| `declared_checks` | `not_admissible`, `worktree_unavailable` | `refused` |
| `declared_checks` | `completed` | `verified` |
| `declared_checks` | `incomplete` | `incomplete` |

Versions are paired with kinds, which the shared `version IN (1, 2)` column cannot do:
**preflight admits v1 only; execution admits v1 (historical) or v2 (current).**

Both directions of Receipt presence are enforced: no Receipt means the state is `preflighting` or
`unconfirmed`; a Receipt means the state is terminal, agrees with the table above, and carries both
times. `runs.state` is deliberately **not** on the wire, so the adapter cannot check it — which is
why it is checked here, where the field is.

## No payload body crosses this Query, and none is parsed

An execution Receipt embeds the whole preflight Receipt that admitted it, so returning Receipts would
put a Receipt inside a Receipt inside a list and make the cost scale with payload size rather than
row count. `receipt_json` is not in the projection. The error type has no `Json` arm, and that
absence is the point: there is no payload to fail on.

This is deliberately **not** a promise that a malformed Receipt is ignored. The Query does not select
the column; successful startup reconciliation remains the module's precondition.

## The index, and what "no migration" means here

Migration `0020_recent_receipts_index.sql` adds one index:

```sql
CREATE INDEX runs_workspace_started ON runs (workspace_id, started_at_ms DESC, id DESC);
```

It is required. The shipped `runs_project_started(workspace_id, project_id, started_at_ms DESC)`
puts `project_id` between the Workspace and the order key, so SQLite satisfies the `WHERE` from it and
must still sort every matching Run before `LIMIT` helps (`USE TEMP B-TREE FOR ORDER BY`). A Query
whose cost grows with the Workspace is not bounded; it merely returns a bounded answer.

What is **not** required is a migration in the sense this repository usually means one: no table is
renamed, no row copied, no trigger dropped, and no Receipt byte rewritten. `runs`/`receipts` columns,
versions and triggers are untouched, so nothing conflicts with #33.

`the_reading_walks_the_workspace_index_and_sorts_nothing` asserts the plan names the index and
contains no `TEMP B-TREE`; it fails if the index is dropped or the `ORDER BY` drifts. No row
assertion can see that difference.

## What is bounded structurally, and cannot be observed

The rows iterator is stepped at most `window + 1` times and each row is classified as it arrives, so
the reading never holds the Workspace in memory. **No test proves this**, and the report for this
slice says so: removing the SQL `LIMIT`, the iterator bound, or both leaves every test green, because
the classification loop breaks at the window whatever the query returned. The observable guard is the
query plan; the memory bound is structural, and stating it here is the substitute for a guard that
cannot exist.

## Ordering

`runs.started_at_ms DESC, runs.id DESC` — unchanged from the Project-scoped Query and total within a
Workspace, because `(workspace_id, id)` is the primary key. **The UUID tie-break is not causal:**
with pin-at-open landed, an execution Run and its pinned preflight can open in the same millisecond,
so no copy may imply sequence within a tie, and a TypeScript order validator must compare canonical
lowercase UUIDs by code point to match SQLite `BINARY` rather than by locale.

The key is also a cursor key, so paging can be added later without changing the order or the index.

## Errors

No new `LocalErrorCode`. A cross-kind read fails for the same two reasons a preflight read does — a
closed session, or a store it cannot trust — and reuses `preflight_unavailable`. The desktop must
translate that code at the recent-Receipts seam, because "Beacon could not record the preflight" is
false for a Query that records nothing.
