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

A **committed** relative symlink can be sufficient. For example, from a source repository's `nested/`
directory, `cwd -> ../../project/inside` can canonicalize back into that source repository. The same
relative target in Beacon's `scratch/tree/nested/` climbs out of `tree`, because the disposable
worktree has a different parent layout. Preflight can therefore admit the source-tree path while
the copied entry escapes the execution tree. Nothing checks the latter today.

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
retained directory descriptor held by the supervisor for the life of one check; the string records
only the worktree locator Beacon attempted to bind.

**Where does feedback live?** Runtime feedback lives in `ExecutionRecord`: its `outcome` says
whether the target program started, and its required `working_directory` string names the
worktree locator Beacon attempted to bind. `ResolvedCheck.resolved_working_directory`
lives inside the embedded preflight receipt and remains unchanged historical evidence about the
source repository. Execution must not rewrite evidence that admitted it.

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

Concretely: `run_checks` constructs one opaque `sandbox::WorktreeLauncher` before the first check.
The launcher opens and retains the worktree root descriptor and builds the Run's Landlock write
rule from that **same descriptor**, never by reopening its pathname. For each Declared Check, the
launcher opens the declared working directory relative to its root descriptor using `openat2(2)`
with `RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS`. The resulting descriptor is passed to the bounded
runner, and the child calls `fchdir` on it inside `pre_exec`, before the Landlock restriction and
before `exec`. `Command::current_dir` is no longer used for Declared Check execution.

`RESOLVE_BENEATH` makes the kernel enforce, for every component of the resolution, that the result
is a descendant of the trusted descriptor. Absolute paths and absolute symlink targets are
rejected outright, and `..` cannot climb above the anchor. This is a property of the resolution
itself, not a comparison performed afterward, which is what makes it immune to the swap races
above: there is no window between deciding and using, because deciding *is* using.

`RESOLVE_NO_MAGICLINKS` refuses traversal through `/proc/self/fd/N` and similar synthetic links,
which are not ordinary symlinks. Current kernels also disable magic-link resolution as a side
effect of `RESOLVE_BENEATH`, but Linux documents that behavior as subject to change; naming
`RESOLVE_NO_MAGICLINKS` makes the required policy explicit and future-proof.

Ordinary symlinks that resolve **within** the worktree remain permitted. `RESOLVE_NO_SYMLINKS` is
deliberately **not** used. Repositories legitimately contain internal symlinks, and refusing them
would reject correct projects to prevent an escape the anchor already prevents. The invariant
Beacon needs is "the result is beneath the worktree," not "no symlink was traversed."

Failure is closed and total. If `openat2` is unavailable or refuses resolution, or if child setup
observes an already-unlinked descriptor, the target program does not start. The record uses the
typed `ExecutionOutcome::NotStarted` variant with a stable categorized reason. **There is no
string-canonicalization fallback.** A fallback would reintroduce exactly the check-then-use gap
this record exists to close. An unlink that wins only after the child-side liveness check remains
contained by the descriptor, as specified below, rather than being overstated as detectable.

### Deep module boundary

The interface is narrow: `WorktreeLauncher::bind(worktree_path, capability)` returns an opaque
launcher whose internal state is either `Ready { root_fd, ruleset, sandbox_version }` or
`Refused { reason, sandbox_version }`; `launcher.run(argv, declared_cwd, bounds, interrupt)` always
returns one `ExecutionRecord`. The caller handles no descriptor, syscall flag, or resolver error.
The implementation is deep: root binding, Landlock construction, `openat2` semantics, descriptor
lifetime, liveness checks, error classification, and child-side ordering all live behind it.

This boundary is chosen so the containment rule has exactly one home. Today the rule is stated in
a comment in `run_checks` and implemented nowhere; a version stated in two places would be worse
than either. After this change, `run_checks` composes and records; it passes only a declared
repository-relative locator to a launcher that has no uncontained execution path. Initializing the
launcher before the first check also keeps its root stable without making worktree creation own a
sandbox concern or lose a cleanup-capable `Worktree` handle when descriptor setup is refused.

## Trust model and abuse cases

Beacon trusts the source repository's *identity* — the operator bound it — and trusts nothing
about its *contents*. It trusts the worktree root descriptor it opened itself. It trusts no path
string, including its own earlier ones, and no directory entry inside the worktree.

A Declared Check is a bounded adversary: it executes operator-declared `argv`, it has write
authority beneath the worktree, and it may be hostile or merely wrong. It may not escalate beyond
what the sandbox grants.

**Committed relative escape.** A tracked relative entry is source-contained but resolves outside
the disposable tree because the two roots have different parent layouts. Refused at execution
bind: the target is not beneath the launcher anchor.

**Prior-check swap.** Check one replaces the directory check two declares. Refused or contained at
bind, because check two's resolution is performed fresh against the anchor at its own spawn, and
the anchor cannot be climbed out of.

**Concurrent swap.** A process races the resolution. There is no check-then-use window to hit; the
descriptor names an inode, and a swapped entry afterward cannot retarget an already-open
descriptor.

**Magic link.** Current `RESOLVE_BENEATH` behavior already refuses a planted path into
`/proc/self/fd/N`; the explicit `RESOLVE_NO_MAGICLINKS` flag preserves that refusal if the kernel's
documented incidental behavior changes.

**Retained or detached descendant.** A prior check may leave a process alive after Beacon has
stopped observing its direct child, and that process may retain descriptors and the existing
Landlock write authority beneath the worktree. This record does not pretend to revoke either.
Instead, every later check resolves from Beacon's independently retained root descriptor and
adopts only the newly bound cwd descriptor, so a detached process may cause a refusal or in-tree
mutation but cannot redirect the later check outside the anchor.

**Out of scope, stated so nobody assumes otherwise.** A check that legitimately reaches a
contained directory and then writes hostile content there is not prevented by this record. Reads
outside the tree remain permitted by the Landlock ruleset. Neither is a regression; both are
existing, documented properties.

## Descriptor lifecycle and child operation order

The launcher opens the worktree anchor before the first untrusted check, with
`O_PATH | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW`, and retains it until `run_checks` returns. The
launcher and its descriptors are therefore dropped before `Worktree::close` begins cleanup. The
anchor is opened once so every later resolution and the Landlock write rule refer to the same
directory object, not to whatever a subsequently reopened path might name.

Per check, immediately before spawn: `openat2(anchor_fd, declared, { flags: O_PATH | O_DIRECTORY |
O_CLOEXEC, resolve: RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS })`. The valid root declaration `"."`
binds the anchor itself; empty declarations are already invalid Repository Bindings. On resolver
refusal the record is `NotStarted` and no target program executes.

Name liveness is checked on the bound descriptor in the child: `fstat` requires `st_nlink > 0`
immediately before `fchdir`. If it already observes an unlink, `pre_exec` fails and the target is
`NotStarted`. This is a usability check, not the containment proof. An unlink raced after `fstat`
may still leave the child in the pinned, now unnamed directory; it cannot retarget the descriptor
or escape the worktree. The Receipt therefore makes no stronger claim that the bind-time locator
remains a live name.

Both descriptors are `CLOEXEC`. They are used in the child **before** `exec` and must not survive
into the executed program; `CLOEXEC` closes them at `exec` precisely then. This matches the
existing treatment of the Landlock ruleset descriptor, which is likewise used in `pre_exec` and
never leaks.

The child's order inside `pre_exec`, which is the load-bearing part of this record:

1. `fstat(bound_fd)` and refuse an already-unlinked directory.
2. `fchdir(bound_fd)` — adopt the bound directory while it cannot be retargeted by name.
3. `setrlimit` for CPU, file size, and address space — unchanged.
4. `setpgid(0, 0)` — unchanged.
5. `landlock_restrict_self(ruleset_fd)` — last, unchanged, still irreversible.
6. `exec` — descriptors close.

`fstat` and `fchdir` are syscalls that allocate nothing and take no userspace lock, so they are
legal between `fork` and `exec`, which the existing closure's safety comment already requires of
everything in it. `fchdir` on an `O_PATH` descriptor is supported on Linux 3.5 and later, far below
the floor `openat2` already sets.

`Command::current_dir` is removed from this path. Leaving it would perform a second, lexical
`chdir` before `pre_exec` runs, reintroducing the defect this record closes while appearing fixed.

## Kernel and syscall availability

`openat2` requires Linux 5.6. Beacon is Linux-first and already refuses to run a Declared Check on
a host whose Landlock capability it cannot prove, so a second kernel floor introduces no new class
of refusal. A host without `openat2` — `ENOSYS`, or `E2BIG` from an unrecognised `open_how` size —
produces `NotStarted` with a reason naming the missing capability. `EXDEV` and `EAGAIN` from
`RESOLVE_BENEATH` are refusals, not retries. The private resolver classifies `EXDEV`, `ELOOP`, and
`EAGAIN` as unsafe or raced resolution; `ENOENT` and `ENOTDIR` as unavailable or not a directory;
and `ENOSYS`, `EINVAL`, `E2BIG`, or policy `EPERM` as an unavailable resolver. The Receipt carries a
stable human-readable category and observed errno, never an unbounded retry loop.

`SandboxCapability` is not extended in this record. The capability enum reports what the *sandbox*
enforces; working-directory binding is a property of dispatch, not of the ruleset, and conflating
them would make the sandbox report enforcement it does not perform — the failure mode SECURITY.md
names. If a later record wants a host-level capability probe for `openat2`, it should say so and
carry its own evidence.

## Receipt semantics

No wire shape or preflight semantic changes. The embedded
`ResolvedCheck.resolved_working_directory` remains exactly what ADR 0027 produced: historical
source-repository resolution evidence. Execution neither edits nor reinterprets the `admitted_by`
receipt that authorized it.

`ExecutionRecord.working_directory` remains a required `String` for every dispatched declaration,
including `NotStarted`. Its precise meaning is the absolute worktree locator Beacon attempted to
bind: `worktree_root.join(declared_working_directory)`. It is display evidence, not the authority
used for execution, not a canonical path, and not a claim that a target program entered it. A
successful record means the child entered the directory descriptor atomically selected for that
locator; a refused record preserves the attempted locator while `outcome.reason` says why no target
program started.

If the selected directory is renamed after binding, the descriptor continues to name the selected
directory object while `working_directory` remains the bind-time locator. Beacon does not
canonicalize the name again to make the Receipt look current: a second pathname walk could describe
an attacker-controlled replacement instead of the capability actually used. Durable inode identity
would require a later Receipt version and is outside this slice.

Under ADR 0031, a refused bind is a terminal `NotStarted` record for that check. It does not stop
dispatch of the remaining checks and contributes its uncertainty in the existing
run-then-checks-then-cleanup order.

## Concurrency testing

Concurrency tests are deterministic or they are not evidence. No test in this work may sleep to
create an interleaving. Ordering is established with an explicit barrier or a pipe: the test
process blocks the child at a known point, performs the swap, releases it, and asserts the
outcome. A test that passes only because a `sleep` happened to be long enough is a test that
cannot fail for the right reason, and it will silently stop testing the race the first time CI is
slow.

## Smallest implementation fence

Acceptance authorizes only:

- a new private `crates/beacond/src/sandbox/worktree_launcher.rs` — own ready/refused launcher
  state, the root descriptor, per-check `openat2` resolution, stable refusal categories, and
  bind-time display locators;
- `crates/beacond/src/sandbox/mod.rs` — expose the opaque launcher only within `beacond` and route
  the existing test-facing helper through descriptor binding; no change to `SandboxCapability` or
  admission policy;
- `crates/beacond/src/sandbox/bounded.rs` — add child-side `fstat` and `fchdir`, remove
  `current_dir`, and keep the existing limits, process group, output collection, ceilings, and
  Landlock-last rule;
- `crates/beacond/src/sandbox/landlock.rs` — grant worktree writes from the launcher's retained root
  descriptor rather than reopening its pathname; global read policy remains unchanged;
- `crates/beacond/src/declared_checks.rs` — construct one launcher before the first check, dispatch
  through it in canonical order, and preserve later-check continuation;
- focused `crates/beacond/tests/declared_checks.rs` and sandbox tests — the matrix below; and
- this ADR.

No `worktree.rs`, preflight behavior, Receipt contract, migration, schema, wire, transport, or
desktop interface change; no new dependency; and no change to Proposal, Ruling, or authority.
Declared Check execution remains a backend-only Owner-bound method unavailable over Beacon's
production transport; this record does not make it an Agent execution path.

## Non-goals

This record does not sandbox reads, does not narrow the Landlock read grant over `/`, does not
change write confinement, does not add a `SandboxCapability` variant, does not introduce
`RESOLVE_NO_SYMLINKS`, does not revoke descriptors held by earlier checks, does not make execution
reachable over the wire, and does not alter `check_preflight`'s source-repository resolution —
which remains correct for what it decides, namely admissibility against the bound repository.

## Required RED and regression acceptance matrix

Implementation begins by recording which guards fail on the current code. Rows marked **RED** must
be observed failing before the change and green after. Rows marked **REGRESSION** express behavior
the current code already gets right and must remain green; calling those red would be false
evidence.

1. **RED — committed relative escape.** A tracked relative symlink that canonicalizes inside the source
   repository but climbs outside the differently rooted disposable `tree` passes preflight, then
   yields a per-check `NotStarted` containment refusal. The execution record retains the attempted
   worktree locator; the embedded preflight receipt remains byte-for-byte unchanged. Fails if the
   test is rejected before dispatch or if the target program runs.
2. **RED — prior-check swap.** Two checks in canonical order; the first replaces the second's declared
   directory with a symlink outside the worktree. The second is refused. Fails if it runs at all,
   and fails if the first check's refusal is what makes the test pass.
3. **RED — concurrent swap.** With a barrier or pipe — never a sleep — the entry is replaced between
   bind and spawn. The outcome is the bound directory or a typed refusal, never an escaped cwd.
   Fails if it passes only under a timing assumption.
4. **RED — magic-link flag.** A module-level assertion must fail if the resolver's exact flag set omits
   `RESOLVE_NO_MAGICLINKS`; a behavioral `/proc/self/fd/N` case is retained as defense in depth.
   The flag assertion matters because current kernels also reject magic links under
   `RESOLVE_BENEATH`, so behavior alone cannot detect the plausible wrong implementation.
5. **RED — deleted and renamed cwd.** With a private barrier after descriptor binding and before child
   setup, deletion observed by child `fstat` yields `NotStarted` and no target-program effect.
   Renaming the selected directory and installing an outside symlink at its old name still runs in
   the descriptor-pinned original. A deletion raced after `fstat` may produce a contained unnamed
   cwd; no outcome may enter the outside replacement or claim name liveness.
6. **REGRESSION — internal symlink.** A symlink resolving *within* the worktree is accepted and runs there.
   This is the test that fails if containment is implemented as `RESOLVE_NO_SYMLINKS` or as a
   string prefix comparison, and it is why it is in the matrix.
7. **REGRESSION — Receipt separation.** The embedded preflight `resolved_working_directory` remains the
   source-tree value and is unchanged after execution. `ExecutionRecord.working_directory` is the
   attempted disposable-worktree locator on both success and refusal. Fails if execution mutates
   preflight evidence, calls the display locator canonical, or omits the attempted locator.
8. **REGRESSION — later-check continuation.** A refused bind in check two does not stop check three: one record
   per declaration, canonical order, existing summary and Run-state rules from ADR 0031 unchanged.
9. **REGRESSION — existing write confinement.** A check that reaches a contained directory still cannot write
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
the old code: the embedded preflight remains unchanged, and the execution record still carries the
same required worktree locator string and typed outcome.

The kernel version itself adds no effective floor because Landlock arrived after `openat2`; the
acceptance risk is a new fail-closed refusal on otherwise Landlock-capable hosts whose policy blocks
the resolver syscall or whose ABI response is unusable. The risk of not accepting is that Beacon
continues to state a containment guarantee it does not implement, in a component whose entire
purpose is to run untrusted commands safely.

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
