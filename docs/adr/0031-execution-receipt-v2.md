---
status: accepted
---

# Record truthful Declared Check execution in Receipt v2

Product Steward ruling, 2026-08-30: accept this Receipt v2 contract and its bounded backend
implementation fence.

Declared Check execution currently records Receipt v1 after composing the preflight, Landlock,
bounded-command, disposable-worktree, and cleanup mechanisms accepted in ADRs 0027, 0029, and
0030. The execution is real, but its Receipt omits the `ExecutionBounds` the caller configured,
inherits preflight uncertainty that execution may have discharged, calls a Run `Completed` even
when an `ExecutionRecord` says its command was `NotStarted`, and uses that outcome both before and
after process creation. Those are record-contract defects, not reasons to change containment or
execution semantics.

If this proposal is accepted, new Declared Check executions emit execution Receipt v2. V2 adds a
required `per_check_bounds: ExecutionBounds` field. The name is deliberate: the same configured
bounds apply independently to every Declared Check, rather than forming one budget shared by the
whole Run. The Receipt records the exact configured values; it does not infer them from outcomes
or substitute defaults during replay.

## Terminal records, summaries, and Run states

Once execution has created its disposable worktree and begins dispatching checks, every
declaration produces exactly one terminal `ExecutionRecord` in the Repository Binding's canonical
check order. Failure before `Command::spawn` succeeds is a record with
`ExecutionOutcome::NotStarted`. If spawn succeeds but Beacon cannot await or otherwise establish a
terminal process outcome, v2 records `ExecutionOutcome::ObservationFailed { reason }`. Neither is
a reason to omit that declaration or abort recording later declarations. Before dispatch, an
inadmissible preflight or unavailable worktree fabricates no per-check records because no truthful
execution working directory or observation exists. "Terminal record" means the evidence item is
final and immutable; `ObservationFailed` does not claim the spawned process terminated.

The public `ExecutionSummary` vocabulary remains `NotAdmissible`, `WorktreeUnavailable`, and
`Completed`, and adds one `Incomplete` variant:

- `NotAdmissible`: preflight refused execution; there are no check records and the Run state is
  `refused`.
- `WorktreeUnavailable`: preflight admitted execution but no worktree was available; there are no
  check records and the Run state is `refused`.
- `Incomplete`: dispatch began and at least one record is `NotStarted` or `ObservationFailed`; the
  Run state is `incomplete`.
- `Completed`: dispatch began and every record positively establishes one of the terminal outcomes
  `Exited`, `Signalled`, `Bounded`, or `Interrupted`; the Run state is `verified`.

`Completed` and `verified` describe completeness of execution evidence, never check or Mission
success. An `Exited` command with a non-zero code, a `Signalled`, `Bounded`, or `Interrupted`
command, and a kept worktree may therefore belong to a `Completed`/`verified` Run: each is a
terminal observation of a command that started. Its exact result and uncertainty remain in the
records. Conversely, mapping a partially executed Run to `refused` would be false because effects
may already have occurred. `unconfirmed` remains reserved for a lease that expires without a
terminal Receipt.

There is only one `Incomplete` summary. Separate `NoChecksStarted`, `PartiallyExecuted`, and
observation-failure summaries would duplicate exact facts already visible in the ordered `checks`
collection: a reader can distinguish `NotStarted` from `ObservationFailed` and see which commands
have positive terminal outcomes. One summary answers the run-level question: whether every
declaration has a positively established terminal outcome.

## Residual uncertainty

V2 recomputes its top-level `not_verified` from typed execution observations. It never clones the
embedded preflight's prose. The embedded `admitted_by` Receipt remains unchanged historical
evidence of what preflight knew; the execution list states only what remains uncertain after this
Run.

Entries are generated deterministically in this order: run-wide gaps, then checks in canonical
check order, then cleanup. Within a check, the outcome gap precedes `output_truncated`, which
precedes `output_incomplete`. Cleanup reasons retain their typed order. The derivation is:

- `Exited { code }` adds no outcome uncertainty. The code, including a non-zero code, is evidence.
- `Signalled` records that the process started and its signal, but that normal completion and an
  exit code were not established.
- `Bounded` records the exact ceiling that ended the process, but not how it would have completed
  with a larger bound.
- `Interrupted` records the interruption, but not how the command would have completed.
- `NotStarted` is restricted to failure before spawn succeeds. It records its exact reason and
  establishes that no process ran, while leaving that check's status and intended output and
  effects unobserved.
- `ObservationFailed` records its exact post-spawn observation failure. Because a process did run,
  its terminal status, complete output, effects, and possible continued activity remain uncertain.
- `output_truncated` records that output existed beyond the configured capture ceiling.
- `output_incomplete` records that collection stopped before EOF, so the captured output is not
  known complete. It is independent of truncation; both entries remain when both flags are true.
- `CleanupOutcome::Removed` adds no cleanup gap. `CleanupOutcome::Kept` adds each exact typed keep
  reason; a reason count is not a substitute for the reasons.

For a dispatched execution, base-revision reachability from a ref remains unverified because the
object-store check established presence only. UDP egress remains unverified when at least one
record proves that a process started, including `ObservationFailed`, because Landlock restricts TCP
only. Complete filesystem effects and a diff digest also remain unrecorded by this slice.
Preflight's blanket "never runs a check" gap is discharged and replaced with the exact per-check
gap for `NotStarted` or `ObservationFailed`. Preflight's CPU, memory, and wall-clock gap is
discharged once v2 records the exact per-check bounds and the execution records the resulting
observations. Proven sandbox and working-directory containment do not remain as uncertainty.

An early `NotAdmissible` or `WorktreeUnavailable` Receipt instead derives the still-live gaps from
its typed preflight outcome and the observed refusal point, followed by the fact that the declared
checks did not run. It does not copy strings from `admitted_by.not_verified`, and it does not claim
that configured bounds were exercised. `NotAdmissible` exhaustively retains the failed typed
preflight stage and every later stage that was not established, in execution order. A non-empty,
complete set of resolved-check paths may discharge source reading, object presence, and containment
only where that captured-binding evidence is sound; it never proves sandbox enforcement or a
changed current binding. `WorktreeUnavailable` always records the same three gaps: retained
reachability-from-ref, worktree creation and possible partial-checkout effects at the observed
refusal point, and that no check ran. Dynamic Git or filesystem error display text is not persisted.

## Persistence and replay

### Proposed R48 amendment — the typed owner owns the outcome vocabulary

Proposed pending Product Steward acceptance when R48 merges. Per FABLE-RULINGS R46.4, membership
in a Receipt outcome vocabulary is defined by the closed, versioned Rust type and its owning
decoder, not by a shared SQL `CHECK`. The `receipts.outcome` column remains a projection which must
equal the decoded Receipt at every owning read before any value is returned. SQL continues to own
structural durability—Receipt version bounds, valid JSON, identity and time constraints, foreign
keys, indexes, and immutability—but does not duplicate the variants of several typed producers in
one schema constraint. Consequently, a reader presented with a token outside its compiled enum
refuses through the typed owner even when the row is otherwise SQL-valid; historical versions and
their closed decoders remain readable, and a known token still must satisfy its owner-specific
coherence invariants.

Migration 0016 preserves every existing Receipt and event byte-for-byte. It admits Receipt
versions 1 and 2, admits the execution outcome `incomplete`, adds the terminal Run state
`incomplete`, and retains all existing values. The migration changes no live installation in this
decision slice; applying it belongs to the later reviewed implementation.

Execution Receipt v1 and its matching `run.executed` event remain immutable and replayable. Startup
uses a complete, recursively closed private v1 decoder for the entire serialized graph. That graph
includes the original Receipt and summary; every nested `ExecutionRecord`, `ExecutionOutcome`, and
ceiling; the embedded `PreflightReceipt`, `PreflightOutcome`, resolved-check, explicit-absence, and
source-kind shapes; and the `CleanupOutcome` and `KeepReason` shapes. The v1 outcome keeps its
historical `NotStarted` meaning, including old post-spawn await failures; it does not admit
`ObservationFailed`. A current shared nested type may be reused only when its own contract is
frozen or explicitly versioned and the Receipt binds that version.

The v2 decoder is separately and recursively closed over its complete nested graph, including the
v2 execution outcomes, embedded preflight, cleanup, and bounds shapes. Adding a field or variant at
any reachable unversioned nested type requires a new execution Receipt version rather than
silently widening v2. Startup first reads the explicit JSON version and dispatches to that decoder.
It never deserializes v1 as v2, guesses missing bounds, supplies `ExecutionBounds::default()`, or
lets a current nested type silently widen either versioned contract.

Startup reconciliation requires all representations to agree:

- SQL `receipts.version` and `receipts.outcome` equal the decoded JSON version and outcome for
  both preflight and execution Receipts;
- `runs.kind` is `declared_checks` for an execution Receipt;
- `NotAdmissible` and `WorktreeUnavailable` pair only with Run state `refused`;
- `Incomplete` pairs only with Run state `incomplete`;
- `Completed` pairs only with Run state `verified`;
- `check_preflight` Runs never use the execution-only `incomplete` state;
- every terminal `refused`, `incomplete`, or `verified` Run has exactly one Receipt, while
  `preflighting` and `unconfirmed` have none; and
- every terminal `check_preflight` Run has exactly one matching `run.preflighted` event;
- every terminal `declared_checks` Run has exactly one matching `run.executed` event;
- `preflighting` and `unconfirmed` Runs have no matching terminal event of either kind; and
- that event and the stored Receipt decode to the same versioned value and remain exactly
  byte-equal.

An unknown version, malformed versioned shape, unknown summary or outcome, or mismatch among JSON,
SQL, Run state, and event fails startup reconciliation closed. V1 remains accepted only through its
v1 contract; forward compatibility is not silent acceptance.

## State, feedback, deletion, and timing invariants

The managed SQLite Run, Receipt, and event rows remain the durable state. Operator-facing feedback
comes from the summary, every terminal check record, exact configured bounds, cleanup outcome, and
ordered residual uncertainty rather than from a synthesized success claim. Receipts, events, and
opened Runs remain immutable and undeletable; migration rebuilds copy their bytes rather than
rewriting their meaning. Run timestamps enclose the whole attempt, each record keeps its measured
elapsed time, the configured bounds apply anew to each check, and cleanup occurs after the final
dispatch record before the terminal Receipt is committed atomically with its Run state and event.

## Smallest implementation fence

Acceptance authorizes only the later backend persistence slice in:

- `crates/beacond/src/declared_checks.rs` — v1/v2 types, summary derivation, uncertainty derivation,
  and versioned replay;
- `crates/beacond/src/check_preflight.rs` — only generic startup Receipt/event cardinality and exact
  preflight startup reconciliation, with no preflight execution behavior or Receipt
  shape/serialization change;
- `crates/beacond/src/sandbox/bounded.rs` — only classify the existing post-spawn observation-error
  branch as `ObservationFailed` while keeping pre-spawn failures `NotStarted`;
- `crates/beacond/src/lib.rs` — migration registration, schema reconciliation, and migration/replay
  tests;
- `crates/beacond/migrations/0016_execution_receipt_v2.sql` — byte-preserving schema expansion;
- `crates/beacond/tests/declared_checks.rs` — observable execution-contract regressions; and
- `crates/beacond/tests/mission.rs` — schema-version consumer evidence only; and
- `crates/beacond/tests/team.rs` — schema-version consumer evidence only; and
- this ADR.

No `check_preflight` execution or Receipt contract, wire or desktop interface, dependency,
Proposal, Ruling, authority, or live migration changes in this decision slice. The later bounded
source change is classification only: it changes no containment, enforcement, configured bound,
process-control, or output-collection behavior. Declared Check execution remains a backend-only
Owner-bound method unavailable over Beacon's production transport; this record does not make it an
Agent execution path.

## Required RED matrix

Implementation begins with tests that fail against these plausible wrong answers:

1. A v2 execution round-trip asserts `per_check_bounds` exactly equals non-default caller input and
   fails if bounds are omitted, renamed as aggregate Run bounds, or defaulted.
2. A migrated store containing real v1 Receipt and event bytes asserts those bytes are unchanged
   and startup replays them through the complete recursive v1 shape; a v1 payload must fail v2
   decoding rather than acquire guessed bounds.
3. A multi-check execution with a missing executable between started commands asserts one record
   per declaration, canonical order, `Incomplete`, and Run state `incomplete`; it fails if dispatch
   stops early, the record is dropped, or the Run is called `Completed`, `verified`, or `refused`.
4. A post-dispatch case in which every record is `NotStarted` still asserts the single `Incomplete`
   summary and `incomplete` state; no second zero-start summary is introduced.
5. Early inadmissible-preflight and worktree-creation failures assert `refused`, no fabricated
   records, no fabricated working directory, and fixed recomputable gaps with no dynamic error
   prose, while the positive terminal outcomes—including non-zero exit, signal, bound, and
   interruption—remain eligible for `Completed`/`verified`.
6. Declared Check integration runs `setsid sleep 5 & echo started; exit 0` on an enforcing host and
   asserts `Exited { code: 0 }`, `output_incomplete = true`, `output_truncated = false`, and the
   matching uncertainty. A separate small-output cap asserts truncation without incompleteness;
   both flags together produce both entries. Classification changes do not alter collection.
7. Removed and kept worktrees assert no cleanup gap versus every exact typed keep reason, and
   uncertainty ordering is run, canonical checks, cleanup rather than collection accident.
8. A dispatched v2 Receipt asserts retained reachability-from-ref, conditional UDP, and
   filesystem-effect/diff gaps, while rejecting copied preflight never-ran-checks and resource-bound
   prose. Table-driven early-refusal cases cover every `PreflightOutcome` and retain each later
   unestablished stage. Per-outcome cases assert exact `NotStarted`, `Interrupted`, and
   `ObservationFailed` gaps and that non-zero exit is not uncertainty.
9. Tamper cases independently mismatch SQL version, SQL outcome, Run state, Receipt JSON, and event
   JSON; malformed and unknown v1/v2 shapes also fail startup closed. For each version, strict
   decoder cases inject an unknown embedded preflight outcome and unknown cleanup outcome or keep
   reason and assert rejection, proving recursive closure rather than top-level strictness alone.
   Unknown summaries and execution outcomes, missing or duplicate identical `run.executed` events,
   a relabelled execution with no matching `run.preflighted` event, and a `check_preflight` Run
   carrying execution-only state `incomplete` also fail closed. Independent preflight cases
   mismatch SQL version and outcome while leaving its exact v1 JSON and event valid. V1 rejects
   both `incomplete` and unknown future summaries.
10. A synthetic v2 `ObservationFailed` record asserts `Incomplete`, Run state `incomplete`, its
    exact status/output/effects uncertainty, and retained UDP uncertainty. Strict-decoder cases
    assert v2 accepts that shape, v1 rejects it, and a historical v1 `NotStarted` await-error record
    remains replayable only through the frozen nested v1 decoder.

Accepted 2026-08-30 by the Product Steward. The implementation still requires non-author review of
the exact commit before merge under the repository's moderate-change rule.
