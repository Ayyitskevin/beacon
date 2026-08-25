# ADR 0001: What an adapter may read

- **Status:** Accepted (v0)
- **Date:** 2026-08-24
- **Deciders:** Lane A (contracts & doctrine), on a question raised by Lane B
- **Supersedes:** nothing

## Context

The inherited constraint, quoted exactly from the ORACLE roadmap
(`04-projects/roadmap/ai-town-repo-portfolio.md`, §"Unified product prototype:
Mosaic"):

> The existing modules keep their own state, invariants, and databases. There
> is no shared mega-database or filesystem reach-through.

Lane B hit this immediately. **Icarus is repo-only** — no daemon, nothing
listening. The only way to read it is through its git repository and its
on-disk records. Read at its most literal, the constraint forbids the Icarus
adapter outright, which would make the module with the fleet's most
comprehensive byte-binding (ADR: see `docs/authority.md`) the one module Mosaic
structurally cannot see. That is obviously not the constraint's intent, but
"obviously not the intent" is not a ruling, and an adapter should not encode
one lane's reading of a rule it did not write.

## Decision

**The boundary is about authority and mutability, not about the syscall used.**

An adapter may read a module's state when the read is:

1. **Through an interface the owning module publishes** — its HTTP API, its
   CLI, its documented export — *or* through an immutable, content-addressed
   artifact that module already treats as canonical; **and**
2. **Read-only by construction**, not by intention; **and**
3. **Incapable of observing state the owning module would not show a
   legitimate caller** with the same credentials.

Applied to the cases in front of us:

| Read | Verdict | Why |
|---|---|---|
| Athena `GET /desk`, `GET /approvals` | **Allowed** | The module's own published interface, with its own authorization applied |
| Janus via `janus list --json` | **Allowed** | The owner's own CLI. Lane B proposed holding this line; it is hereby the rule, not a preference |
| Icarus via **committed git objects** (`git show <rev>:<path>`, `git grep <rev>`) | **Allowed** | A commit is immutable and content-addressed. It is also precisely what Icarus's own `planApprovalDigest` binds (`baseCommit`) — reading the commit reads the same artifact the module's own guarantee is written against |
| Opening `~/.janus/janus.db`, Athena's `athena.db`, or any live SQLite file | **Forbidden** | This is the reach-through. It bypasses the owner's scopes, authorization, and activity trail, and it observes mid-transaction state no legitimate caller could see |
| Icarus's **working tree** (uncommitted files, run scratch dirs) | **Forbidden** | Mutable state mid-flight. Janus's own code makes this exact distinction: it binds a *pinned commit* rather than a worktree digest, because "a dirty worktree must not silently change what was approved" (`src/janus/core.py`) |

The line is therefore: **committed is readable, live is not.** A commit is a
published artifact; a working tree and an open database are somebody's
in-progress state.

## Consequences

- The Icarus adapter is unblocked, and must read at an explicit revision. It
  may not read `HEAD` as a floating name — same reasoning Janus records in
  `resolve_binding`: binding a name rather than a commit makes drift fire on
  every unrelated change, "which teaches the reader to ignore drift, which is
  worse than no drift check."
- Every adapter records **which revision or endpoint it read**, so a desk entry
  can be re-derived. An entry that cannot name its source is a bug.
- The read-only property must be enforced by a test, not a promise. Lane B owns
  that test (`tests/adapters/`), by agreement.
- **This ADR does not authorize reading a module's secrets** from a commit
  merely because they are committed. Content-addressed does not mean public.
- Mosaic still holds **no credentials of its own**. It reads with the operator's
  credentials, and a module that refuses the operator refuses Mosaic.

## Alternatives considered

- **Literal reading — forbid all filesystem access.** Rejected: it excludes
  Icarus entirely and would push the adapter toward standing up a service for
  Icarus, which is a far larger architectural change than the rule was
  protecting against.
- **Permissive reading — any read is fine, writes are the real boundary.**
  Rejected: opening a live SQLite file observes uncommitted transactions and
  bypasses authorization. "Read-only" is not the same as "harmless," and the
  operating model's separation of ownership is about who is entitled to answer
  a question, not only who may change an answer.
- **Defer to Kevin.** Rejected *for this question* — it is an implementation
  reading of an existing boundary, not a change to it, and the lane plan asks
  us to decide what is ours to decide. The one place this thread does defer is
  the `janus-authorized` rename (`docs/authority.md`), which changes a boundary
  rather than interpreting one.
