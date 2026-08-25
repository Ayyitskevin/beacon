> # ⚠ SUPERSEDED — DO NOT FOLLOW THIS AS INSTRUCTION
>
> **Kevin ruled on 2026-08-25 that Mosaic is ONE unified codebase absorbing all seven
> AI Town capabilities, shipped as a Tauri desktop app with its own SQLite store.**
> This file describes the *federating read-only desk* that preceded that ruling, and
> several of its rules are now the exact inverse of the product being built —
> including "no shared database", "Mosaic holds none", "read-only by construction",
> and "there is no shell".
>
> It is kept, not deleted: the federated adapters it governs are the migration
> readers, the only code that knows how to read the old systems. **But it must stop
> instructing.** If you are an agent and this file loaded automatically, stop and read
> these instead, in order:
>
> 1. `private-evidence/2026-08-25_mosaic-scope-ruling-unified-desktop-app.md` — the ruling
> 2. `private-evidence/2026-08-25_mosaic-datamodel-synthesis.md` — the current design
> 3. `src/mosaic/store/` — the actual product; `src/mosaic/{adapters,render,contracts}/`
>    is the superseded one
>
> **Why this banner exists rather than a quiet edit:** three sessions built the wrong
> product for ten hours because agent-authored documents kept instructing after their
> premise was deleted. A file that auto-loads is the highest-risk form of that. The
> rules below were correct for federation; they are not correct now, and the difference
> is not visible from inside them.

---

# AGENTS.md — Mosaic repository contract

## Purpose

> Mosaic composes what the modules publish. It grants no authority, and
> reading it is never grounds to act.

This file narrows the fleet-wide autonomy contract for this repository. It
deliberately does **not** restate what Mosaic is: `CONTEXT.md` is the
definition of record, and two definitions by two authors diverge by the second
week. Read `CONTEXT.md` and `docs/authority.md` before changing the domain
model, the desk schema, or anything touching authority labels.

## Working agreement

- **Read-only by construction.** The adapter protocol has no write method. A
  change that adds one — even disabled, even behind a flag — is wrong here
  regardless of how convenient it is. There are zero cross-module write
  controls in v0, not even a disabled button.
- **Never collapse the authority labels.** The renderer must distinguish
  proposed / reviewed / janus-authorized / executed / verified and must never
  present them behind one generic affordance. If a change makes two of them
  render identically, the change is the problem.
- **Do not guess a label.** An unrecognised authority label is reported as
  unrecognised. Mapping an unknown label onto a known one is the collapse this
  repository exists to prevent.
- **No new state.** No shared database, no filesystem reach-through into
  another module's store, no second orchestrator, no second chat, no second
  model gateway, no duplicated task/evidence/decision store.
- **A failing join is a result.** Report the missing link; never synthesize
  one to make output look complete.
- **Every entry carries its provenance**: source module, owner, status,
  timestamp, schema version, canonical link, authority label.
- **`status` is the owning module's own word.** Render it verbatim and never
  switch on it — it is an open set (`superseded`, `withdrawn`, `in_progress`,
  `done`), not Mosaic's vocabulary. Everything Mosaic *decides* from lives in
  `authority`, whose axes are closed enums. An unknown status is displayed
  quietly; an unknown authority value must stay loud.
- **Scope whole-tree tools to your own paths.** `ruff format src tests`, a
  bulk codemod, a find-and-replace across the repo — each is a quiet way to
  write into another lane's files. Two lanes did exactly this on 2026-08-24;
  one landed two of another lane's files in its commit and had to back them
  out. Name your own directories explicitly.

## Red and green boundaries

Green in your own checkout or worktree: documentation, tests, local code,
fixtures, renderers, reversible refactors.

Human review required before merge for: the desk schema, the authority label
enum and its resolution rules, the adapter protocol, anything that creates a
cross-module seam, and anything that would let a Mosaic read stand in for a
decision.

**Repository creation and `git push` are RED and unauthorized.** This repo is
local-only until Kevin says otherwise. No `gh repo create`, no push.

## Four invariants to re-check

- **State** lives in the other modules. Mosaic holds none; a cache would be
  new state and needs a superseding decision.
- **Feedback** lives in `mosaic doctor`, structured refusals, and tests. A
  refusal is a sentence, not a traceback.
- **Deleting** an adapter must show up as a named missing join in `doctor`,
  never as a silently shorter desk.
- **Timing**: every entry is an observation taken at a moment. A re-checked
  binding and an unchecked one must never render the same way.

## Lanes

Three sessions build the foundation in parallel with disjoint file ownership;
see `private-evidence/2026-08-24_mosaic-foundation-lanes.md`. Work in your own
git worktree. Never edit another lane's files or working directory.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
