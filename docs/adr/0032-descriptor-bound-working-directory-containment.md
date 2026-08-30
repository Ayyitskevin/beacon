---
status: proposed
---

# Bind a Declared Check's working directory to a descriptor, not to a path

Proposed 2026-08-30. This record awaits Kevin's acceptance as Product Steward. No implementation
begins before that acceptance: the change is security-relevant under GOVERNANCE.md, so it requires
a written proposal, this decision record, one non-author review, and explicit sign-off.

## Context

Declared Check execution composes the mechanisms accepted in ADR 0026, ADR 0027, ADR 0029, ADR
0030, and ADR 0031. Preflight decides admissibility, a disposable worktree provides the tree, and
a Landlock ruleset bounds what a Run may write. The remaining seam is the working directory each
check runs in, and it is not contained.

`DeclaredCheckService::run_checks` derives each check's directory as
`worktree.path().join(&check.declared_working_directory)`. The comment above that line says the
declared directory "is re-resolved against *this* worktree." It is not. `Path::join` is lexical
concatenation. It performs no canonicalization, consults no filesystem, and decides no
containment. The only real containment logic in the codebase is `check_preflight::resolve_within`,
which canonicalizes and compares with `starts_with` — and it runs against the **source
repository**, a different tree from the disposable worktree the check will actually execute in.

The path is then handed to `Command::current_dir`. Rust's standard library performs that `chdir`
in the child after `fork` and **before** the `pre_exec` closure, which is where Beacon applies its
resource limits, its process group, and — last, deliberately — its Landlock restriction. So the
kernel resolves the joined string, following every symlink it encounters, at a moment when no
Beacon confinement is yet in force.

Three consequences follow, and only the third is speculative.

A **committed** symlink is sufficient. A repository may legitimately contain `tools -> /opt/vendor`
or `build -> ../shared`. Preflight canonicalizes such a path against the source repository and may
find it contained there; the same name inside the worktree can resolve somewhere else entirely,
because the worktree sits beneath a scratch root in the system temporary directory at a different
depth. Nothing re-checks it.

A **prior check** is sufficient, and this is the sharper case. Declared Checks run sequentially in
canonical name order, in one worktree, each with write authority beneath that worktree. The first
check can therefore create, replace, or repoint a directory entry that a later check's declared
working directory names. Beacon's own threat model already anticipates a check being hostile —
that is why the sandbox exists — so a check arranging the next check's cwd is inside the model,
not outside it.

A **concurrent** swap is possible in principle. Between the moment any check on the path is made
and the moment the child calls `chdir`, a process with write access to the worktree can replace a
component. Today there is no check to race, so this is not a distinct defect; it becomes one the
moment containment is implemented as a string comparison, which is precisely the wrong fix.

The blast radius is bounded and should not be overstated. The Landlock ruleset grants write rights
only beneath the worktree root, and it is built from an `O_PATH` descriptor to that root, so an
escaped cwd does not become an escaped write. Relative writes from an escaped directory resolve
outside the write target and are denied. What escapes is **reads**, which the ruleset grants over
`/` anyway, and **truthfulness**: the Receipt records a `working_directory` that no Beacon code
ever validated for the tree the check ran in.

That last point is why this is a defect rather than a hardening idea. SECURITY.md names "a guard
that does not guard" — "a check that reports enforcement it is not performing is a security bug
here even when nothing is currently exploitable through it." A comment claiming re-resolution over
code performing none is exactly that. ADR 0031 recorded truthful execution receipts and fenced
itself out of this explicitly: those were "record-contract defects, not reasons to change
containment or execution semantics." This record is the containment change 0031 deferred.

### The four invariables

**Where does state live?** The authoritative statement of "where this check runs" currently lives
in a `String` that is re-resolved by the kernel at every use. After this change it lives in a
retained directory descriptor held by the supervisor for the life of one check, with the path
string demoted to a record of what was observed at bind time.

**Where does feedback live?** In `ExecutionRecord.outcome` and in `ResolvedCheck.
resolved_working_directory`. Both already exist. A refusal is `ExecutionOutcome::NotStarted` with a
typed reason; a resolution that could not be proven is `Recorded::Absent` with a reason. No new
surface is required to report either.

**What breaks if I delete this?** Removing the descriptor binding returns Beacon to a lexical join
whose containment claim is false. Removing the `RESOLVE_BENEATH` flag while keeping the descriptor
removes the containment but keeps the appearance of it — the worst of the three states, and the
reason the flags are named in this record rather than left to implementation.

**When does timing work?** The binding must be established once, before spawn, and the child must
adopt it by descriptor rather than by name. `fchdir` must precede `landlock_restrict_self` in the
child, because Landlock is irreversible for the calling thread and the process must still be
permitted to change directory. It must also precede `exec`, because after `exec` there is no
Beacon code left to run.

## Decision

A Declared Check's working directory becomes a **capability**, not a name. Beacon opens the
directory once, relative to a descriptor it already trusts, under kernel-enforced containment, and
the child adopts that exact directory by descriptor. No later step re-resolves a string.

Concretely: `Worktree` retains a trusted directory descriptor for its own root, opened when the
worktree is created and held for the worktree's life. For each Declared Check, Beacon opens the
declared working directory **relative to that descriptor** using `openat2(2)` with
`RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS`. The resulting descriptor is passed to the sandbox, and
the child calls `fchdir` on it inside `pre_exec`, before the Landlock restriction and before
`exec`. `Command::current_dir` is no longer used for Declared Check execution.

`RESOLVE_BENEATH` makes the kernel enforce, for every component of the resolution, that the result
is a descendant of the trusted descriptor. Absolute paths and absolute symlink targets are
rejected outright, and `..` cannot climb above the anchor. This is a property of the resolution
itself, not a comparison performed afterward, which is what makes it immune to the swap races
above: there is no window between deciding and using, because deciding *is* using.

`RESOLVE_NO_MAGICLINKS` refuses traversal through `/proc/self/fd/N` and similar synthetic links,
which are not ordinary symlinks and are not constrained by `RESOLVE_BENEATH` in the way a reader
would expect. A prior check with write authority could otherwise plant a path into `/proc` and
reach an arbitrary open descriptor.

Ordinary symlinks that resolve **within** the worktree remain permitted. `RESOLVE_NO_SYMLINKS` is
deliberately **not** used. Repositories legitimately contain internal symlinks, and refusing them
would reject correct projects to prevent an escape the anchor already prevents. The invariant
Beacon needs is "the result is beneath the worktree," not "no symlink was traversed."

Failure is closed and total. If `openat2` is unavailable, if the resolution is refused, or if the
bound directory cannot be proven live at spawn, the check does not start. It records
`ExecutionOutcome::NotStarted` with a typed reason. **There is no string-canonicalization
fallback.** A fallback would reintroduce exactly the check-then-use gap this record exists to
close, and would do so on the hosts least able to enforce anything.

### Deep module boundary

The interface is narrow: a caller asks for a working directory beneath a worktree and receives
either a bound capability or a typed refusal. The interface names no syscall, no flag, and no
kernel version. The implementation is deep: `openat2` semantics, flag selection, descriptor
lifetime, liveness proof, and the child-side ordering all live behind it, and none of them appear
in `declared_checks.rs`.

This boundary is chosen so the containment rule has exactly one home. Today the rule is stated in
a comment in `run_checks` and implemented nowhere; a version stated in two places would be worse
than either. After this change, `run_checks` composes and records, and cannot express an
uncontained working directory even by mistake, because the type it must pass cannot be constructed
from a string.

## Trust model and abuse cases

Beacon trusts the source repository's *identity* — the operator bound it — and trusts nothing
about its *contents*. It trusts the worktree root descriptor it opened itself. It trusts no path
string, including its own earlier ones, and no directory entry inside the worktree.

A Declared Check is a bounded adversary: it executes operator-declared `argv`, it has write
authority beneath the worktree, and it may be hostile or merely wrong. It may not escalate beyond
what the sandbox grants.

**Committed escape symlink.** A tracked entry points outside the tree. Refused at bind: the target
is not beneath the anchor.

**Prior-check swap.** Check one replaces the directory check two declares. Refused or contained at
bind, because check two's resolution is performed fresh against the anchor at its own spawn, and
the anchor cannot be climbed out of.

**Concurrent swap.** A process races the resolution. There is no check-then-use window to hit; the
descriptor names an inode, and a swapped entry afterward cannot retarget an already-open
descriptor.

**Magic link.** A planted path traverses `/proc/self/fd/N`. Refused by `RESOLVE_NO_MAGICLINKS`.

**Retained descendant.** A prior check leaves a descriptor open to a directory it created. That
descriptor confers whatever the sandbox already allowed it; it does not confer authority on a
*later* check, because the later check's cwd is bound from the worktree anchor, not inherited.
Beacon does not attempt to revoke descriptors held by a finished process; the process is gone and
its descriptors with it.

**Out of scope, stated so nobody assumes otherwise.** A check that legitimately reaches a
contained directory and then writes hostile content there is not prevented by this record. Reads
outside the tree remain permitted by the Landlock ruleset. Neither is a regression; both are
existing, documented properties.

## Descriptor lifecycle and child operation order

The worktree anchor is opened when `Worktree::create` succeeds, with
`O_PATH | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW`, and closed when the `Worktree` is dropped. It is
opened once so that every later resolution is anchored to the same inode Beacon validated, not to
whatever the path names later.

Per check, immediately before spawn: `openat2(anchor_fd, declared, { flags: O_PATH | O_DIRECTORY |
O_CLOEXEC, resolve: RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS })`. An empty or `"."` declared
directory binds the anchor itself. On refusal, the check is `NotStarted` and no process is
created.

Liveness is proven on the bound descriptor, not on a path: `fstat` and require `st_nlink > 0`. A
directory unlinked between bind and spawn is refused rather than entered, because a child whose
cwd has no name cannot resolve relative paths and would fail in a way that reads as a check
failure rather than a containment refusal. The residual window is inherent and bounded: an
unlinked cwd cannot be an escape, only an error.

Both descriptors are `CLOEXEC`. They are used in the child **before** `exec` and must not survive
into the executed program; `CLOEXEC` closes them at `exec` precisely then. This matches the
existing treatment of the Landlock ruleset descriptor, which is likewise used in `pre_exec` and
never leaks.

The child's order inside `pre_exec`, which is the load-bearing part of this record:

1. `fchdir(bound_fd)` — adopt the bound directory. First, because it must happen while the process
   may still change directory.
2. `setrlimit` for CPU, file size, and address space — unchanged.
3. `setpgid(0, 0)` — unchanged.
4. `landlock_restrict_self(ruleset_fd)` — last, unchanged, still irreversible.
5. `exec` — descriptors close.

`fchdir` is a single syscall that allocates nothing and takes no lock, so it is async-signal-safe
and legal between `fork` and `exec`, which the existing closure's safety comment already requires
of everything in it. `fchdir` on an `O_PATH` descriptor is supported on Linux 3.5 and later, far
below the floor `openat2` already sets.

`Command::current_dir` is removed from this path. Leaving it would perform a second, lexical
`chdir` before `pre_exec` runs, reintroducing the defect this record closes while appearing fixed.

## Kernel and syscall availability

`openat2` requires Linux 5.6. Beacon is Linux-first and already refuses to run a Declared Check on
a host whose Landlock capability it cannot prove, so a second kernel floor introduces no new class
of refusal. A host without `openat2` — `ENOSYS`, or `E2BIG` from an unrecognised `open_how` size —
produces `NotStarted` with a reason naming the missing capability. `EXDEV` and `EAGAIN` from
`RESOLVE_BENEATH` are refusals, not retries.

`SandboxCapability` is not extended in this record. The capability enum reports what the *sandbox*
enforces; working-directory binding is a property of dispatch, not of the ruleset, and conflating
them would make the sandbox report enforcement it does not perform — the failure mode SECURITY.md
names. If a later record wants a host-level capability probe for `openat2`, it should say so and
carry its own evidence.

## Receipt semantics

No wire shape changes. `ResolvedCheck.resolved_working_directory` is already
`Recorded<String>`, and `ExecutionRecord.working_directory` is already a `String`. Both keep their
types and their serialization.

What changes is truthfulness. `resolved_working_directory` becomes `Present` only when a
descriptor was actually bound, carrying the path as observed at bind time, and `Absent` with a
reason otherwise — including when binding was refused. `ExecutionRecord.working_directory` records
the same observed path and no longer records a lexically joined string that nothing validated.

A renamed directory is recorded honestly rather than re-resolved. Authority was bound to the
inode; a rename within the worktree moves the name, not the directory, and the check continues in
the directory it was granted. The Receipt therefore records the path as observed at bind time and
does not claim it is still the live name. This is a deliberate consequence of binding to a
capability, and it is the correct one: re-reading the name would reintroduce path trust.

Under ADR 0031, a refused bind is a terminal `NotStarted` record for that check. It does not
fabricate a working directory, does not stop dispatch of the remaining checks, and contributes its
uncertainty in the existing run-then-checks-then-cleanup order.

## Concurrency testing

Concurrency tests are deterministic or they are not evidence. No test in this work may sleep to
create an interleaving. Ordering is established with an explicit barrier or a pipe: the test
process blocks the child at a known point, performs the swap, releases it, and asserts the
outcome. A test that passes only because a `sleep` happened to be long enough is a test that
cannot fail for the right reason, and it will silently stop testing the race the first time CI is
slow.

## Smallest implementation fence

Acceptance authorizes only:

- `crates/beacond/src/worktree.rs` — retain and close the anchor descriptor; no change to creation
  policy, cleanup policy, `CleanupOutcome`, or any ADR 0030 behavior;
- `crates/beacond/src/sandbox/mod.rs` — accept a bound working-directory capability instead of a
  path; no change to `SandboxCapability`, admission, or ruleset construction;
- `crates/beacond/src/sandbox/bounded.rs` — add `fchdir` as the first `pre_exec` step and remove
  `current_dir`; no change to limits, process group, output collection, ceilings, or the Landlock
  call's position as last;
- a new module for the `openat2` binding and its flags, sited beside `landlock.rs` because it is
  the same class of thing: a raw syscall reached through `libc` with its own safety argument;
- `crates/beacond/src/declared_checks.rs` — bind per check, record truthfully, refuse closed;
- `crates/beacond/tests/declared_checks.rs`, `tests/sandbox.rs`, `tests/worktree.rs` — the matrix
  below; and
- this ADR.

No preflight behavior or Receipt contract change, no migration, no schema change, no wire,
transport, or desktop interface change, no new dependency, and no change to Proposal, Ruling, or
authority. Declared Check execution remains a backend-only Owner-bound method unavailable over
Beacon's production transport; this record does not make it an Agent execution path.

## Non-goals

This record does not sandbox reads, does not narrow the Landlock read grant over `/`, does not
change write confinement, does not add a `SandboxCapability` variant, does not introduce
`RESOLVE_NO_SYMLINKS`, does not revoke descriptors held by earlier checks, does not make execution
reachable over the wire, and does not alter `check_preflight`'s source-repository resolution —
which remains correct for what it decides, namely admissibility against the bound repository.

## Required RED acceptance matrix

Implementation begins with tests that fail against these plausible wrong answers. Each must be
observed red before the change and green after.

1. **Committed escape symlink.** A worktree containing a tracked `escape -> /tmp` declared as the
   working directory yields `NotStarted` with the containment reason, no process, and
   `resolved_working_directory` `Absent`. Fails if the check runs, if the cwd is outside the
   worktree, or if a working directory is fabricated in the Receipt.
2. **Prior-check swap.** Two checks in canonical order; the first replaces the second's declared
   directory with a symlink outside the worktree. The second is refused. Fails if it runs at all,
   and fails if the first check's refusal is what makes the test pass.
3. **Concurrent swap.** With a barrier or pipe — never a sleep — the entry is replaced between
   bind and spawn. The outcome is the bound directory or a typed refusal, never an escaped cwd.
   Fails if it passes only under a timing assumption.
4. **Magic link.** A declared directory traversing `/proc/self/fd/N` is refused. Fails if it
   resolves, and fails if the refusal reason names containment rather than the magic-link rule.
5. **Deleted cwd.** The bound directory is unlinked between bind and spawn: `NotStarted`, no
   process. **Renamed cwd.** The bound directory is renamed within the worktree: the check runs in
   it and the Receipt records the bind-time path without claiming it is current.
6. **Internal symlink.** A symlink resolving *within* the worktree is accepted and runs there.
   This is the test that fails if containment is implemented as `RESOLVE_NO_SYMLINKS` or as a
   string prefix comparison, and it is why it is in the matrix.
7. **Canonical Receipt.** For a plainly contained directory, `resolved_working_directory` is
   `Present` with the observed path and `ExecutionRecord.working_directory` equals it. Fails if
   either records a lexical join.
8. **Later-check continuation.** A refused bind in check two does not stop check three: one record
   per declaration, canonical order, existing summary and Run-state rules from ADR 0031 unchanged.
9. **Existing write confinement.** A check that reaches a contained directory still cannot write
   outside the worktree. This is a regression guard on ADR 0029, and it must be red if the
   ruleset's write target is disturbed.

A tenth is a property, not a case: no test may pass while `Command::current_dir` remains on this
path. A grep-level assertion is acceptable evidence for it.

## Rollout and reversal

One change on `main` behind the review this record requires. There is no feature flag: a flag
would mean shipping both a contained and an uncontained dispatch path, and the uncontained one is
the defect.

Reversal is a revert of a single commit. No migration is added, so no data is written that a
revert would strand, and Receipts produced under the new behavior remain valid v2 Receipts under
the old code because no field's type or meaning changes — only which values are truthful.

The risk of accepting is a new refusal class on hosts below Linux 5.6, which Beacon's existing
Landlock floor already largely excludes. The risk of not accepting is that Beacon continues to
state a containment guarantee it does not implement, in a component whose entire purpose is to run
untrusted commands safely.

## Product Steward decision

Kevin decides. This record is `proposed` and no implementation may begin under it.

- **Accept** — implementation proceeds within the fence above, with the RED matrix observed red
  first and one non-author review binding to the exact commit.
- **Accept with amendment** — most likely on the symlink policy (whether internal symlinks stay
  permitted) or on whether an `openat2`-unavailable host should refuse Declared Check execution
  entirely rather than per check.
- **Reject** — the comment in `run_checks` claiming re-resolution must then be corrected to state
  what the code actually does, because leaving it is the SECURITY.md defect independent of any
  containment work.
