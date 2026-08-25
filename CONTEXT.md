# CONTEXT.md — what Mosaic is

The definition of record. `AGENTS.md` and `README.md` both defer here; if any
of them disagree, this file wins and the other should be cut rather than
reconciled.

## The one-sentence version

**Mosaic composes what the seven AI Town modules already publish into one
read-only document, and refuses to summarize their authority.**

## Domain vocabulary

Words below are used in exactly this sense throughout the codebase. Where a
word already means something specific to an owning module, that module's
meaning wins and Mosaic adopts it rather than coining a synonym.

**Module** — one of the seven AI Town capabilities: Buzz (conversation),
Athena (work), Minerva (evidence), Vulcan (provenance), ReplayDeck
(evaluation, the existing `fleet-eval` repo), Janus (pending human authority),
Icarus (bounded execution). Seven, not six — the roadmap's product trace
includes ReplayDeck at the evaluation step, and every early Mosaic handoff
omitted it.

**Adapter** — a read-only reader for one module. Lives in
`src/mosaic/adapters/`. Has no write method, by protocol shape rather than by
convention.

**Entry** — one fact from one module, carrying its source, owner, status,
timestamp, the owning module's schema version, a canonical link, provenance,
and optionally an authority label.

**Desk** — the composed document. Flat, ranked by recency, `desk.v1`.

**Join** — a link between two modules' facts (a Janus gate to the Athena issue
that commissioned it). Mosaic never invents one. **A missing join is a result,
not an error** — naming precisely which joins do not exist is the point of v0.

**Authority label** — the four-axis description of one approval: who decided,
what it is pinned to, who acts on drift, and where it sits in its lifecycle.
See `docs/authority.md`, which is the reasoning; `contracts/authority.py` is
the executable form.

**Provenance** — the exact endpoint or pinned revision an entry was read from,
plus what the module reported about its own version and tree state. An entry
that cannot name its source is a bug.

## Invariants

These are the things that make Mosaic safe to look at. A change that breaks
one of them is wrong here regardless of how much nicer it makes the output.

1. **Read-only by construction.** The adapter protocol has no write method,
   no dispatch, no generic `call`. Zero cross-module write controls in v0 —
   not even a disabled one.
2. **It composes, it does not compute.** Every entry is the owning module's
   own read. Mosaic never derives a fact, reconciles a disagreement between
   two modules, or fills in a missing value. If two modules disagree, both
   statements appear.
3. **Never collapse authority.** No cross-module "Approved ✓" affordance
   exists — not styled differently, *absent*. Every approval wears its
   owner's badge and carries the owning module's own `meaning` string.
4. **A missing join is reported, never synthesized.** Papering over an absent
   link is the single most damaging thing this tool could do.
5. **An empty read is not a failed read.** They are different facts about the
   world, and rendering them alike lies by omission.
6. **Mosaic ranks nothing.** Ordering is recency, explicitly not importance.
7. **Committed is readable; live is not.** ADR 0001. Reading a pinned commit
   or a module's published interface is fine; opening a live database or a
   working tree is the reach-through the roadmap forbids.
8. **Mosaic holds no credentials of its own.** It reads with the operator's,
   and a module that refuses the operator refuses Mosaic.
9. **Reading Mosaic is never grounds to act.** It displays who decided what.
   It grants nothing.

## What Mosaic is not

Not a database, not an orchestrator, not a second chat, not a second model
gateway, not a work tracker, not a writer, and not an authority. It is not
permission to fork Buzz or duplicate Athena. Mosaic earns a repo only if it
owns distribution, session/reference composition, and the cross-module desk
contract; otherwise it should stay a client/adapter layer.

## Why there is no shell

The first slice is a document, not an application. Choosing a shell is the
most expensive, least reversible decision available, and nothing has yet
shown the seven modules can produce one coherent cross-module document at
all. Whatever shell eventually wins just renders this document, so none of
this work is thrown away.

**v0 is done when** `mosaic desk` composes a real document from at least two
live modules and `mosaic doctor` names precisely which joins in the product
trace do not exist yet. As of 2026-08-24 that number is 2 of 7 hops joinable.

## Layout

| Path | Holds |
|---|---|
| `src/mosaic/contracts/` | The desk envelope, authority labels, adapter protocol |
| `src/mosaic/compose.py` | Pure composition — zero I/O |
| `src/mosaic/adapters/` | One read-only reader per module |
| `src/mosaic/doctor.py` | What is reachable, what is missing, what a join fails on |
| `src/mosaic/render/` | JSON and text renderers; consume the document, never the classes |
| `docs/authority.md` | Why the labels are shaped as they are, with citations |
| `docs/adr/` | Decisions, with the alternatives considered |

Renderers take the document as a plain mapping rather than the contract
classes, deliberately: the display layer depends on the *document*, so any
shell that later renders a desk inherits these rules without importing
Mosaic's internals.

## Standing constraints

Python 3.11+, src layout, ruff line-length 100, Apache-2.0, and **zero runtime
dependencies** (CI fails if a clean install pulls in any other distribution) —
matching Janus and Minerva.

No `gh repo create`, no `git push`. Kevin has not authorized a public repo;
that is his call and it is RED. Local commits only.
