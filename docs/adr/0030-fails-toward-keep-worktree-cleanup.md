---
status: accepted
---

# Keep disposable worktrees whenever cleanup safety is not proven

Product Steward ruling, 2026-08-30: accept the fails-toward-keep cleanup policy already
implemented by Beacon's disposable Git worktrees.

A disposable worktree is still a checkout that may contain work. Beacon removes one only through
an explicit `Worktree::close` gate whose positive result proves that ordinary Git removal
succeeded. Every other result reports `Kept` with one or more typed reasons instead of forcing or
assuming removal.

The gate first resolves the worktree path and requires it to remain inside the scratch root Beacon
created for it. It also requires the `.git` file that identifies a linked worktree. These are
structural checks, not a claim that Git still considers the worktree registered. Git gets the final
word when removal runs.

If those checks hold, Beacon runs a bounded `git status --porcelain --untracked-files=all
--ignore-submodules=none` inside the worktree. It removes inherited `GIT_DIR` and `GIT_WORK_TREE`
so the environment cannot redirect that check. Tracked changes produce `Dirty`; untracked,
non-ignored content produces `UntrackedContent`. Operator configuration cannot hide untracked
content because the command-line setting is explicit. Ignored content is deliberately not a hold:
disposable worktrees are expected to accumulate build output.

Beacon re-runs the structural checks after `git status` returns because the path may have moved,
been replaced, or lost its marker while the process ran. An escaped path produces `PathEscapes`;
an absent, unreadable, or structurally unprovable worktree produces `NotAWorktree`. A failed check
produces `CheckFailed`, and a check that exceeds its bound produces `GateTimedOut`. Any hold keeps
the worktree without attempting removal.

Only a gate with no holds invokes bounded `git worktree remove`, without `--force`. A Git refusal
produces `RemovalRefused`; a timeout produces `GateTimedOut`; and inability to run or observe Git
produces `CheckFailed`. None may be reported as `Removed`. Beacon never recursively deletes the
checkout. It removes the surrounding scratch root only when Git has removed the worktree and left
that root empty.

Dropping a `Worktree` does not clean it up. There is intentionally no deleting destructor because
a destructor could neither preserve nor report the reason for refusal. Likewise, cleanup after a
failed creation removes only an empty scratch root; partial content is not recursively erased.

## Consequences

`CleanupOutcome` remains a closed, strict wire value: `Removed` means ordinary Git removal
returned success, while `Kept` carries the observed reasons. Unknown outcome shapes and unknown
reasons are refused during deserialization.

Declared Check execution records the cleanup outcome in its Receipt. A kept worktree adds residual
uncertainty rather than being retried, forced, or hidden. The check outcomes remain separate from
the cleanup outcome: a process exit does not prove cleanup, and cleanup does not decide Mission
success.

Clean worktrees, including ignored build output, may be removed. Tracked modifications, untracked
non-ignored content, structural uncertainty, check failure, timeout, and Git refusal all fail
toward keeping the checkout. Tests cover each of those branches, combined dirty and untracked
reasons, the post-check structural re-read, drop without close, strict outcome serialization, and
successful removal of a clean linked worktree.

This policy governs Beacon-created disposable worktrees only. It grants no authority to delete a
canonical repository, use recursive removal, override Git, or weaken the Landlock and resource
bounds that govern commands run inside the worktree.
