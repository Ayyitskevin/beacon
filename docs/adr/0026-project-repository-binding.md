---
status: accepted
---

# Bind a Project to its repository and declared checks before any execution path exists

The repository a Project works on is Project intent, not an execution detail discovered later. An
authenticated Operator may bind an immutable Project Repository revision only against the exact
current Charter revision and the expected prior Repository revision. Each revision names one source
(an absolute local path or a Git URL), one pinned 40-character lowercase base revision, and one or
more Declared Checks, each identified by a distinct name, a non-empty argument vector, and a
repository-relative working directory. Declared Checks are stored in canonical name order, and a
shell string cannot be declared: only an argument vector is accepted.

Binding is a declaration. Beacon does not open, clone, fetch, or read the named repository, does not
confirm that the base revision exists, and does not run a Declared Check. Whether the pinned commit
is reachable and whether a check passes are liveness claims that belong to a later Execution or
Evaluation slice, which must re-verify that a Proposal's repository, pinned base revision, and
checks still match before anything runs.

## Consequences

Charter publication can make the latest Repository Binding stale without rewriting it. Rebinding
preserves the previous revision, binds the new revision to the new Charter, and records one
attributable `project.repository_bound` Event. Only the latest revision can report that it applies
to the current Charter.

Binding creates no worktree, checkout, Proposal, Run, Receipt, Evaluation, Assignment, Route,
Invocation, credential, budget, network scope, external effect, or execution authority. Check
execution, evidence, and any second author kind remain separate decisions.

This proposal becomes accepted only with Product Steward sign-off on the reviewed branch.
