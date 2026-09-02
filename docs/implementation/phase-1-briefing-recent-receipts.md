# Phase 1 Briefing recent Receipts

BLUEPRINT §3 designs Briefing to show "the Project portfolio, active Missions, Team health, spend,
blocked work, recent receipts, and Attention Items". Before this slice Briefing rendered one of the
seven — Team health, as the Resident roster. This adds recent Receipts.

The Query, the transport and the TypeScript adapter already existed and were exercised by
`preflight_project_checks`; nothing rendered the list, and a cleanup pass nearly removed
`preflight_receipts` as dead registration. **No wire code and no daemon code changed here.**

## What a row says, and what it refuses to say

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
existing `projects` Query and merges. That is a decision with a cost — N+1 Queries per Briefing
open — taken because the alternative was a new Workspace-scoped Query, which this slice excludes. A
Workspace-scoped Query would make the fan-out dead code the day it exists.

**One Project that cannot be read fails the whole reading.** A partial merge presented as "recent"
would be a bounded list of an unknown subset, which is the same claim-of-completeness defect the
truncation line exists to prevent.

## Bounds, ordering, and the three empty-ish states

- **Five rows.** Briefing "optimizes for exceptions, not activity volume".
- **The daemon applies no LIMIT.** `check_preflight.rs` returns every Receipt a Project has, so the
  bound is entirely the desktop's. The adapter now caps the list at 512, matching the repository
  history cap; the panel then draws five.
- **Newest first**, by `startedAtMs` then `runId` — the same key the daemon orders each Project's
  Receipts on, extended across the merge so the order is stable.
- **Truncation is stated:** "Showing the 5 most recent of N. Open a Project to see its full
  preflight history." A list that silently drops rows is a claim of completeness nobody made.
- **Loading, empty and failed are three states.** A read that failed is not a Workspace with no
  Receipts, and the failure is retryable rather than terminal.

## Accessibility

A `<section aria-labelledby>` with an `<h2>` inside Briefing's `<h1>`, and a
`<ul aria-label="Recent Receipts">` of `<li>` — list semantics matching the Resident roster rather
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
npx vitest run src/App.test.tsx -t "recent Receipts"
npx tsc -b && npm run build
```

Every test was run against the wrong answer it kills before the code satisfied it; the mutants and
their observed failures are recorded in the build report for this slice.

## Evidence limits

- No Receipt has ever been produced by anything but a preflight, because preflight is the only Run
  kind that writes one today. "Recent Receipts" is therefore recent *preflight* Receipts, and the
  row says so rather than implying a general Run history.
- The `unconfirmed` Run state (`BLUEPRINT.md`) has no surface yet and no `PreflightOutcome` member;
  this slice cannot render one. The rule it stands for — a row states the recorded outcome and never
  upgrades it — is what the copy and its test enforce.
- Assertions run under jsdom, which lays nothing out: visible focus and rendered contrast for this
  list are unproven, exactly as `docs/evals/2026-08-30-desktop-accessibility-baseline.md` records for
  the rest of the interface.
- The fan-out has not been measured against a Workspace with many Projects.
