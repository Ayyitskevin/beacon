---
status: accepted
---

# Prove Alpha 1 against the Operator's own fleet

Product Steward ruling, 2026-08-29: **"make my fleet the phase 2 disposable repo."** Beacon's
first real Mission runs against the systems it exists to absorb, not against a synthetic sample.

The delivery plan's Phase 2 exit read "every Alpha acceptance criterion in section 9 passes
against a disposable repository" without naming one. The bundled Rust task-tracker Sample
Project would satisfy the letter of that sentence and prove nothing the Operator needs to know.
The question Alpha 1 has to answer is whether Beacon can replace the frozen pillars it descends
from; only running against them answers it.

## The proving Workspace

One Workspace — the fleet — holds the frozen pillar repositories as Projects: Janus, Icarus,
Minerva, Vulcan, Athena, and the Buzz fork. The Operator is the fleet's owner. Alpha 1's
Residents are Beacon-native Agents on activated **local** Routes (the same local models the
fleet's isolated workers already run); the fleet's cloud harnesses (Claude Code, Codex, Kimi,
Grok, GLM) are **not** Residents in Alpha 1 — reaching an external harness is a Worker or
Connector question (ADR 0005, ADR 0008) and stays out of this decision.

**The first Mission's repository is Janus.** It is frozen as of this date, has no daemon, has
the smallest deterministic check in the fleet (`pytest`, 71 tests), is the pillar Beacon's own
Decisions surface descends from, and carries a real, small backlog: the tri-state
`cannot-verify` fix (Janus PR #3, pending), a missing CI workflow, and a gate whose relative
binding can never verify. A Mission that lands one of those through the complete loop — brief,
plan, candidate diff, non-author Evaluation, Ruling, sandboxed Run, Receipt, Outcome Document —
is the Alpha 1 vertical slice with real stakes and no risk to anything live.

## What "disposable" still means

The ruling renames the target, not the safety property. Execution's target is always a Git
worktree Beacon creates in its sandbox (BLUEPRINT §7); a pillar's canonical clone, its live
store, and its running service are never mutation targets. Concretely, in Alpha 1:

- Projects whose pillar has a live service on this host — Athena (`:8300`), Vulcan (`:8140`),
  Minerva (`:8765`), the Buzz relay — are **Observe** Projects. Beacon may read them, and a
  migration reader may open a store only as a verified-quiesced copy (ADR 0001). Nothing
  Beacon does may mutate them.
- Repositories with no daemon — Janus, Icarus — may be **Supervised** Projects. Every external
  mutation is a Ruling. **Bounded stays unavailable** for fleet Projects regardless of the
  Alpha default: the fleet is real work, and Bounded is reserved for repositories nobody
  would miss.
- The fleet's own always-gate list (schema or migration against live data, secrets, service
  replacement, anything public-facing) is already BLUEPRINT §5's "actions that always gate."
  Nothing here narrows it.
- The pillars are **targets, not dependencies.** Section 9's criterion that a fresh install
  completes the workflow "without Athena, Buzz, Minerva, Vulcan, Janus, or Icarus running"
  stands unchanged: Beacon routes inference to local models directly and runs the loop on a
  pillar's *repository*; it never needs a pillar's *service*.

## Consequences

Phase 2's exit now names its target. The bundled Sample Project remains the onboarding path
for a fresh public install and must still pass the same criteria; it is no longer what proves
the product to its first Operator.

Naming the target early lets Phase 1's kernel be shaped by it rather than in the abstract:
Route eligibility against a real local roster, Budget Envelopes against a ledger that already
meters seats per day, Resident identity against a roster that already has attribution rules,
Evaluation against a gate that already exists and already refuses false greens. Each of those
has an inherited-findings page (`docs/models.md`, `docs/decisions.md`, `docs/evaluations.md`)
saying what the fleet learned the hard way.

The first Mission will be judged by the fleet's own review culture — a non-author Evaluation is
what the round table already demands of every change — so Beacon's Evaluations surface is on the
critical path earlier than the vertical order in §10 implies. That ordering question is open
for the Product Steward; this record does not move it.

Accepted 2026-08-29 by the Product Steward. The reviewed branch merged to `main` as `30d3448` on 2026-08-29; the Steward ruled on 2026-08-29 that every ADR merged under this convention is accepted.
