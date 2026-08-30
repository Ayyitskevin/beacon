---
status: proposed
---

# Beacon is a standalone product, and Alpha 1 proves itself on the bundled Sample Project

Product Steward ruling, 2026-08-30: Beacon is a separate application that will not be operated as
part of the Operator's fleet. The pillar repositories it descends from — Janus, Icarus, Athena,
Minerva, Vulcan, and the Buzz fork — remain live systems under active development and are not
frozen, absorbed, or replaced. Beacon is built for people who have none of them.

This supersedes ADR 0025, which named the fleet as Alpha 1's proving Workspace and Janus as its
first Mission. Alpha 1 instead proves itself against the bundled, disposable Rust task-tracker
Sample Project that section 9 already requires a fresh install to ship.

ADR 0025 dismissed that Sample Project because it "would satisfy the letter of that sentence and
prove nothing the Operator needs to know." That reasoning was correct under its premise, where the
Operator was the only user and the open question was whether Beacon could replace the pillars he
already ran. The premise has changed, and the argument inverts with it. The question Alpha 1 now has
to answer is whether a person with no fleet, no pillars, and no prior context can install Beacon and
complete the loop. Only the Sample Project answers that, because it is the only target such a person
has. A proving ground that requires the author's machine proves the opposite of what a standalone
product needs to demonstrate.

Whether Beacon should later run a Mission against its own repository is a stronger and more
self-referential test, and it is left open rather than decided here.

## What ADR 0025 established that still holds

"Disposable" names a safety property rather than a target. Execution's target is always a Git
worktree Beacon creates in its sandbox; a repository's canonical clone, its live store, and any
running service are never mutation targets. Bounded autonomy stays unavailable for any repository
whose loss would be felt, whatever the Alpha default becomes. Every external mutation remains a
Ruling.

Section 9's criterion that a fresh install completes the workflow "without Athena, Buzz, Minerva,
Vulcan, Janus, or Icarus running" stands, and is now satisfied by construction rather than by
argument: Beacon has no fleet dependency to sever.

The observation that Evaluations sits earlier on the critical path than the vertical order in
section 10 implies also survives, and applies with more force. A product whose first users are
strangers cannot ask them to trust unevaluated agent output.

## Consequences

Phase 2's exit names the Sample Project as its target. The Sample Project does not yet exist and
becomes required work rather than optional onboarding polish.

Test fixtures that name an absolute path inside the author's home directory are no longer
appropriate in a public repository and are removed. A binding fixture must be constructed by the
test rather than borrowed from the machine that wrote it.

The inherited-findings pages remain, reframed. They record design conclusions reached by operating
real systems, with provenance naming where each came from. That provenance is more valuable in a
public repository than in a private one, because it shows which rules were paid for. They are no
longer a record of absorption.

Monetization is deferred until there is a userbase to monetize. The optional hosted Cloud Link at
five dollars per month remains in Phase 5 where the delivery plan already places it. Its pricing and
region commitments leave the Alpha acceptance criteria, which keep the boundary that matters: a
Cloud Link is optional, synchronizes only interface preferences, and never carries Workspace data,
Projects, Missions, Agents, Policy, secrets, credentials, Routes, budgets, capabilities, Rulings, or
execution authority. That boundary is a safety property and does not depend on a business model.

Adoption becomes an Alpha concern rather than a later one. A product nobody can download cannot be
evaluated, so packaging and first-run reach the same standard as the workflow they demonstrate.

This proposal becomes accepted only with Product Steward sign-off on the reviewed branch.
