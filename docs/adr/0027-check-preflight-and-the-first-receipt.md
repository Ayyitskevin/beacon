---
status: accepted
---

# Verify a Project's declared checks before an execution path exists, and refuse in a typed Receipt

ADR 0026 made binding a declaration and deferred every liveness claim, requiring that a later slice
re-verify a Project's repository, pinned base revision, and Declared Checks before anything runs.
This is that verification, and it is all of it: preflight establishes whether execution would be
admissible and then refuses to execute. An authenticated Operator may preflight a Project's latest
Repository Binding. Preflight runs no Declared Check, spawns no process, opens no network
connection, and creates no worktree or checkout. Its only external effect is a bounded read of the
named repository.

Preflight is scoped to local-path sources. Confirming that a pinned revision exists in a Git URL
requires a network fetch, which the operating envelope denies by default, so a Git URL binding
terminates as `SourceKindUnsupported` rather than reporting a verification it did not perform. A
preflight that silently means something weaker for one source kind is worse than one that refuses.

Verification proceeds in order, and each step has its own terminal outcome: the latest Repository
Binding applies to the current Charter revision, or `BindingStaleForCharter`; the source path exists,
is readable, and is a Git repository, or `RepositoryUnreadable`; the pinned base revision is
reachable in it, or `BaseRevisionUnreachable`; the Declared Checks still match the bound revision, or
`DeclaredChecksChanged`; every check's working directory, resolved and canonicalized, lies inside the
repository root, or `WorkingDirectoryEscapes`. Containment is decided on the resolved path and never
on the declared string, because a guard that reads the value the guarded path supplies can be
defeated by supplying a different one. A step that returns a negative answer, a step that errors, and
a step that does not finish are three distinct outcomes — the last two are `PreflightFailed` and
`PreflightTimedOut` — and they never collapse into one another.

Execution additionally requires a sandbox capability, which does not exist. A binding that satisfies
every verification step therefore terminates as `SandboxUnavailable`. This is a refusal, not a
defect, and it remains the correct outcome whenever a sandbox is absent, unsupported, or unhealthy;
the outcome does not become obsolete when a sandbox ships. `Verified` is reserved for the state in
which execution is admissible and is unreachable until then.

Every preflight emits exactly one Receipt. Receipt v1 records the resolved argument vectors and
working directories that would have run, the bound and current Charter revisions, the sandbox and
policy versions, the preflight start and end, the terminal outcome, and, as a required field,
everything it did not verify. Fields the Receipt specification requires but a preflight cannot honour
— Proposal and Ruling digests, resulting revision, filesystem effects and diff digest, model
Invocations and cost — are recorded as explicitly absent rather than as empty or zero, so that a
later reader can distinguish a value that was measured from one that was never available. A verified
preflight is not a passing check, and a Receipt must not be readable as though it were.

A preflight is a Run and carries a lease. A Run that ends without a terminal Receipt is
`Unconfirmed`, not absent, and its lease expiry is what proves that state.

## Consequences

Beacon gains its first Receipt, its first Run, and its first lease, at the smallest scale on which
they can be exercised. Because the Receipt schema is fixed against a producer that can fill only part
of it, the explicit-absence encoding is load-bearing from the first row rather than retrofitted when
execution arrives.

Until a sandbox exists the operator-visible result is drift detection: a repository that moved, a
Charter republished under a stale binding, a check list edited since binding, or a working directory
that escapes its repository. These are real failures that are currently invisible.

Preflight grants nothing. It creates no Proposal, Ruling, Assignment, Route, Invocation, credential,
budget, network scope, external effect, worktree, or execution authority, and it cannot be composed
into a path that runs a check; refusing to execute is a property of its design and not a
configuration. Sandboxed execution, Git URL sources, evidence, and Evaluation remain separate
decisions.

This proposal becomes accepted only with Product Steward sign-off on the reviewed branch.
