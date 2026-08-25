> # ⚠ NARROWED — a checked precondition for migration readers, not a prohibition
>
> ADR 0001 decided **what an adapter may read across module boundaries.** Kevin's
> 2026-08-25 ruling made Mosaic one unified codebase with its own store, so it no
> longer governs the product. It still governs the **migration readers** in
> `src/mosaic/adapters/`, which do read old systems across a boundary.
>
> **Fable ruled 2026-08-25 on how, and the distinction is the useful part.** This
> ADR's reason — *opening another system's database bypasses its scopes, its
> authorization and its activity trail, and observes mid-transaction state* — is a
> property of a **live** system. A quiesced source (stopped service, frozen snapshot)
> has no scopes to bypass, no activity trail to skip, and no mid-transaction state to
> observe. The reason survives verbatim; only its scope is made explicit.
>
> So: **a migration reader may open a source database only against a verified-quiesced
> source, fail-closed.** Not "never", and not "whenever convenient".
>
> **"Verified" needs a mechanism, or it is a promise in prose** — the failure mode this
> repository has hit four times. A reader asserting quiescence in a comment has verified
> nothing. Concrete, cheap checks, in order of strength:
>   1. the source path is a **copy or snapshot**, not the live file (compare paths);
>   2. for SQLite, **no `-wal` / `-shm` companions** beside the file — their presence is
>      positive evidence of a live connection;
>   3. open with SQLite's `immutable=1`, which makes read-only a property of the handle
>      rather than of the caller's intent.
> Fail-closed means an unverifiable source is treated as live and refused, never assumed
> quiet because nothing looked busy.
>
> **Executing a migration against live data remains RED** — a Kevin gate, unchanged.
>
> This ADR was cited by a test guard *after* the ruling narrowed it, forbidding `sqlite3`
> package-wide and blocking the unified store from owning its own database. That is what
> a document still instructing past its scope looks like when it reaches code, and it is
> why this banner states the scope rather than just the verdict.
>
> 1. `shared/handoffs/2026-08-25_mosaic-scope-ruling-unified-desktop-app.md` — the ruling
> 2. `shared/handoffs/2026-08-25_mosaic-datamodel-synthesis.md` — the current design

---

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
