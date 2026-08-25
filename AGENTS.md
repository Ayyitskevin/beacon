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
