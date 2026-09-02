# Phase 1 Briefing recent preflight Receipts

BLUEPRINT §3 designs Briefing to show "the Project portfolio, active Missions, Team health, spend,
blocked work, recent receipts, and Attention Items". Before this slice Briefing rendered one of the
seven — Team health, as the Resident roster.

**This slice does not complete that item.** It adds recent **preflight** Receipts, and says so in
its heading, its description and its list label. The Query it reads filters
`runs.kind = 'check_preflight'` (`check_preflight.rs`), and Beacon already writes a second kind:
`ExecutionReceipt` for declared-check Runs, stored in the same `receipts` table and recorded as
`run.executed` (`declared_checks.rs`). A general, bounded, cross-kind recent-Receipts list needs a
Query behind the interface that the daemon does not yet have; that is a separate slice, sketched at
the end of this note.

The Query, the transport and the TypeScript adapter already existed and were exercised by
`preflight_project_checks`; nothing rendered the list, and a cleanup pass nearly removed
`preflight_receipts` as dead registration. **No wire code and no daemon code changed here.**

## What a row says, and what it refuses to say

The heading is "Recent preflight Receipts" rather than "Recent Receipts", and the description says
the list excludes execution Receipts. A kind-filtered subset presented under the general name is a
claim of completeness in the one place BLUEPRINT asks for completeness.

A Receipt is "an immutable account of a Run's inputs, effects, checks, usage, result, and content
identities" (`CONTEXT.md`). Each row shows the kind of Run, the outcome exactly as recorded, the Run
id, when it started, and which Project it belongs to — the last because a merged cross-Project list
is unreadable without it.

The outcome copy comes from the same closed set the Project panel renders rather than being written
again. Two vocabularies for one recorded fact is how a surface starts claiming more than the Receipt
says. In particular `verified` is **not** a success: preflight runs no check, and the row says
"Execution would be admissible — not a passing check". A test asserts the rendered list matches none
of *passed*, *succeeded*, *completed*, or a check mark.

## Where the list comes from

`preflight_receipts` is Project-scoped and Briefing has no Project, so Briefing fans out over the
existing `projects` Query and merges. That fan-out is **bounded and disclosed**, because an
unbounded one is not a design — it is an assumption about the Workspace, and 513 Projects meant 513
concurrent IPC calls and up to N × 512 Receipts held before sorting to five.

One reading opens at most **8 Projects**, newest first, **one at a time**, and keeps at most **256
rows** before sorting to the five it draws. One at a time is the bound, not a scheduling preference:
a page in flight is a page held, so a window of four concurrent reads of a 512-row page held 2,048
rows beside the 256 kept — three times the maximum this note states. The daemon dispatches on a
single accept loop, so the window bought little to begin with. When the Workspace has more Projects than the
budget the panel says so — "showing preflight Receipts for the 8 most recent Projects of 13" —
because a bounded reading presented without its bound is a partial answer wearing a complete one.

This is a stopgap. The right shape is a Workspace-scoped, server-bounded Query behind the interface;
the fan-out becomes dead code the day it exists.

**One Project that cannot be read fails the whole reading.** A partial merge presented as "recent"
would be a bounded list of an unknown subset, which is the same claim-of-completeness defect the
truncation line exists to prevent.

## Bounds, ordering, and the three empty-ish states

- **Five rows drawn.** Briefing "optimizes for exceptions, not activity volume".
- **256 rows held, applied while collecting.** Each Project's page is merged and trimmed, so what
  the reading holds never exceeds the cap plus one page — `RECEIPT_HOLD_LIMIT`, 768 — whatever the
  Workspace contains. Slicing at the end retained everything first, and then reported the capped
  length as the total — which made every row past the cap vanish from the disclosure. The panel
  reports the number actually found.

  The bound is **measured, not asserted**: `collectRecentReceipts` returns the most rows it ever
  held, and the guard reads it. Nothing else can see it. A rendered list of five looks identical
  whether the reading held 768 rows or 4,096, which is why the earlier guard — 4,096 fixture rows,
  asserting the rendered totals and five list items — survived a gather-every-page-then-slice
  implementation unchanged.

  The cap is the function's, not a caller's. It was briefly a parameter defaulting to the aggregate
  limit, which made this paragraph a promise about the default rather than about the function: a
  valid call passing 1,024 over eight full pages returned 1,024 rows and held 1,536, twice the bound
  stated above. Nothing in the interface passed it; one test did. A degree of freedom whose only
  user is a test is not a smaller interface, it is a wider one — so the parameter is gone and the
  ordering property is exercised at the real cap instead.
- **The daemon applies no LIMIT.** `check_preflight.rs` returns every Receipt a Project has, so the
  bound is entirely the desktop's. The adapter now caps the list at 512, matching the repository
  history cap; the panel then draws five.
- **Newest first**, by `startedAtMs` then `runId` — the same key the daemon orders each Project's
  Receipts on, extended across the merge so the order is stable.
- **Truncation is stated, and so is the dead end:** "Showing the 5 most recent of N. Beacon has no
  full preflight history view yet, so the rest are recorded but not readable here." A list that
  silently drops rows is a claim of completeness nobody made — and pointing at a Project history
  view that does not exist is a second claim on top of it. No Project-open path reads this Query.
- **Loading, empty and failed are three states.** A read that failed is not a Workspace with no
  Receipts, and the failure is retryable rather than terminal.

## Accessibility

A `<section aria-labelledby>` with an `<h2>` inside Briefing's `<h1>`, and a
`<ul aria-label="Recent preflight Receipts">` of `<li>` — list semantics matching the Resident roster
than a new role. Loading, empty and failed are `role="status"`, never `role="alert"`.

**Nothing polls.** The list loads once when the Workspace opens and re-loads only on the explicit
Refresh control, so nothing can move focus under a keyboard user. No `autoFocus`, no `.focus()`, no
custom `onKeyDown`; the only tab stop added is the Refresh button, in document order after the list.
`axe.run` with `color-contrast` disabled runs on both the component and the rendered shell, as every
other panel test does. Outcome is never carried by colour — the text carries it.

## Verification

From `apps/desktop`:

```sh
npx vitest run src/BriefingReceipts.test.tsx
npx vitest run src/App.test.tsx -t "recent preflight Receipts"
npx tsc -b && npm run build
```

Every test was run against the wrong answer it kills before the code satisfied it; the mutants and
their observed failures are recorded in the build report for this slice.

## Evidence limits

- **Execution Receipts exist and are not shown.** `ExecutionReceipt` is written for declared-check
  Runs and stored in the same `receipts` table; this Query filters to `check_preflight` only. The
  earlier version of this note claimed preflight was the only Run kind that writes a Receipt. That
  was wrong, and the surface was named on the strength of it.
- The `unconfirmed` Run state (`BLUEPRINT.md`) has no surface yet and no `PreflightOutcome` member;
  this slice cannot render one. The rule it stands for — a row states the recorded outcome and never
  upgrades it — is what the copy and its test enforce.
- Assertions run under jsdom, which lays nothing out: visible focus and rendered contrast for this
  list are unproven, exactly as `docs/evals/2026-08-30-desktop-accessibility-baseline.md` records for
  the rest of the interface.
- The fan-out has not been measured against a Workspace with many Projects.

## The slice this is not

A general recent-Receipts surface needs a **daemon Query**, not a wider fan-out in the interface: one
Workspace-scoped, kind-agnostic, server-bounded read that returns the N most recent Receipts of ANY kind across
every Run kind, already ordered, with a total so truncation stays disclosable. It belongs behind the
interface because the ordering, the bound and the join across kinds are domain work, and because the
desktop cannot bound what it does not select. The row shape generalises with one addition — the Run
kind, which today is a constant — and the closed outcome vocabulary becomes per-kind. Until that
Query exists, Briefing's list is preflight-only and says so.
