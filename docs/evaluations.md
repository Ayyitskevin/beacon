# Evaluations findings inherited by Beacon

**Status:** draft for review · **Produced by:** claude-code harvest agent · **Last verified:** 2026-08-29

Beacon's Blueprint says Evaluations "is the quality surface descended from ReplayDeck". This page
records what that surface actually descends from, re-derived from live code at pinned revisions.
Every rule cites the source that produced it. It states findings, not decisions; the decisions
are listed at the end as Kevin's.

Execution-side evidence (Icarus's effect ledger, Receipts, landing, rollback) is owned by
`docs/execution.md` and is only cross-referenced here.

---

## How this page was produced

| Source | Pin | What was read | Command form |
|---|---|---|---|
| Icarus | `/example/checkouts/icarus` `origin/main` = `c919cd2` | `docs/EVALS.md`, `docs/PLANS.md`, `docs/OPERATIONS.md`, `docs/THREAT_MODEL.md`, ADRs 0036/0045/0053/0066/0067, `fixtures/evals/manifest.json`, `scripts/{eval-fixtures,gate1-benchmark,gate1-benchmark-contract,gate1-benchmark-result-contract,gate2-benchmark-contract,gate2-explanation-cohort-contract,gate2-v2-evidence-adoption-contract,security-check}.mjs`, `tests/security/live-evidence-*.test.ts`, `docs/evals/2026-08-28-gate2-target-discovery-profile.md`, `package.json` | `git -C <tree> show c919cd2:<path>`; `git -C <tree> grep -n <pat> c919cd2 -- <paths>`. No `pnpm` was run; all numbers are recorded ones |
| Beacon | `/example/checkouts/beacon` @ `c59dde1` (read only) | `README.md`, `CONTEXT.md`, `AGENTS.md`, `docs/BLUEPRINT.md`, `docs/adr/0012`, `docs/authority.md`, all 11 `docs/evals/*.md`, `scripts/gate` | `cat -n`, `grep -n` |
| Chronos | `/example/checkouts/chronos-day` `origin/main` = `6034e10` | `docs/adr/ADR-0028-per-job-evidence-protocol.md`, `docs/TEST_RESULTS.md` | `git show origin/main:<path>` |
| ORACLE roadmap | `./private-evidence/knowledge/04-projects/roadmap/ai-town-repo-portfolio.md` (working copy) | lines 96–116, 198–232 | `cat -n \| sed -n` |
| Fleet practice | `./private-evidence/memory/*.md`; `./private-evidence/2026-08-2{8,9}_*.md` | seven memory files; the Gate 2 explanation-cohort HOLD→PASS cycle; the Janus PR3 harness failures; the PR #71 gate totals | `cat -n`, `grep -n` |

Nothing was checked out, pulled, or edited in any tree. Beacon's own `docs/evals` were counted
with `grep`, not re-run.

---

## ReplayDeck does not exist

The roadmap names ReplayDeck as "the working codename for the existing `fleet-eval` repo"
(`ai-town-repo-portfolio.md:202`; ownership table `:225`, proposed lead Kimi, reviewer Codex).
Three checks on 2026-08-29:

1. `gh repo list Ayyitskevin --limit 200` returns 30 repositories; `grep -iE 'eval|replay'` over
   them matches nothing (exit 1). The list does include `Icarus`, `Chronos`, `Janus`, `beacon`,
   and the three `icarus-gate1-*` fixture repositories, so the command reached the account.
2. `find ~ -maxdepth 3 -iname '*fleet-eval*'` returns nothing on this node.
3. `find ~ -maxdepth 3 -iname '*replay*'` returns nothing on this node.

Beacon's Blueprint already knows this: "ReplayDeck remains a native Beacon capability because
no legacy implementation exists" (`docs/BLUEPRINT.md:1010-1011`). `docs/authority.md:377-379`
repeats the roadmap's claim that it is "the existing `fleet-eval` repo"; that sentence is wrong.

So the Evaluations surface descends from two things, neither of which is a repo:

- **An intent** — the roadmap's MVP (`ai-town-repo-portfolio.md:107-114`): a CLI, a pinned
  task/trace manifest, two provider adapters, deterministic JSONL results, a human-readable
  comparison, **no automatic promotion**; "Keep the evaluator independent: Icarus must not grade
  itself"; keep out "leaderboards, training, online routing, automatic model promotion, and 'the
  judge model liked it' as the sole score".
- **A practice** — the evaluation discipline that exists in code, which lives almost entirely in
  Icarus (`docs/EVALS.md`, 22 dated records under `docs/evals/`, 53 `scripts/gate*`/eval files,
  32 `tests/security/*` contract tests at `c919cd2`), with a second, smaller instance in Chronos.

The rest of this page harvests the practice and checks the intent against it.

---

## What the Blueprint already specifies

- Evaluations manages "fixtures, scenarios, rubrics, deterministic checks, model judges,
  baselines, and Replays. Outcomes distinguish reproduced, diverged, not replayable, and not
  attempted. A green result names the exact Artifact and environment it evaluated"
  (`docs/BLUEPRINT.md:233-236`). The object row is "suites, cases, Replays, outcomes"
  (`:643`).
- **Evaluation**: "A repeatable procedure that judges an Artifact or Run against declared
  criteria" (`CONTEXT.md:255-257`). **Replay**: "A new Run of an Evaluation against pinned
  inputs and a recorded environment" (`:259-261`). **Track Record**: evidence-linked history
  "without collapsing into one score or becoming ambient authority" (`:166-169`;
  `AGENTS.md:61`). **Receipt**: "immutable account of a Run's inputs, effects, checks, usage,
  result, and content identities" (`:295-298`).
- ADR 0012: a Material Artifact "must be evaluated by an Agent other than its author"; the
  Evaluation "records both identities, criteria, inputs, findings, uncertainty, and conflicts.
  Deterministic checks supplement rather than replace independent evaluation. An evaluator can
  reject or request revision but cannot authorize execution" (`docs/adr/0012:7-15`;
  `AGENTS.md:73`; `BLUEPRINT.md:625-631`, `:1162-1163`).
- "Terminal, broken, absent, and not evaluated are structurally distinct" (`BLUEPRINT.md:682`).
  "A successful process exit is not a successful Mission" (`:802-803`).

---

## Corrections to prior claims

1. **"ReplayDeck is the existing `fleet-eval` repo."** (`roadmap:202`, `authority.md:377-379`.)
   No such repo exists anywhere reachable. See above.
2. **"Icarus must not grade itself"** (`roadmap:109`) describes a mechanism the fleet never
   built. Icarus grades itself by code: `pnpm eval` runs Icarus-authored evaluators over
   Icarus-authored oracles inside the Icarus repo (`package.json:45@c919cd2`). The independence
   that actually happened was a **process**: a non-author seat re-measured the published numbers
   with its own probes and ran adversarial mutations against the report validator, holding twice
   before passing (`private-evidence/2026-08-28_icarus-gate2-explanation-cohort-review-v2-claude.md:5-12,44-56,83-90`;
   `…-v3-claude.md:5-8,72-87`). ADR 0012 turns that process into a recorded mechanism. It is
   an upgrade on the fleet's practice, not a description of it.
3. **"model judges"** (`BLUEPRINT.md:234`). No fleet code has ever scored an Evaluation with a
   judge model. Every Gate 2 score is deterministic and **recomputed by the validator** from raw
   observations — recall and precision from `selectedContext` against the manifest
   (`scripts/gate2-explanation-cohort-contract.mjs:519-535@c919cd2`), task success from exact
   target bytes and registered-check pass/fail. The roadmap's warning against "the judge model
   liked it" (`roadmap:113-114`) is the only fleet text on the subject.
4. **Beacon's own matrices do not meet `BLUEPRINT.md:235-236`.** Across the 11 files in
   `docs/evals/` (563 lines), the `Observed` column holds 250 rows and all 250 read `Pass`;
   no file contains a 7–40-hex commit identifier (`grep -o -E '\b[0-9a-f]{7,40}\b'` → 0 in
   every file). They name commands and counts "on the completed implementation tree"
   (`2026-08-29-project-policy.md:50`) but not the exact Artifact or environment. Contrast
   Chronos, which pins "exact `main` `7752825bc…`" and warns "these numbers drift with every
   merged test — rerun before citing them" (`docs/TEST_RESULTS.md:10-11@6034e10`).
5. **"The last recorded gate totals 1259/203/351"** are real but come from a PR #71 worktree
   (`private-evidence/2026-08-29_icarus-pr71-e2-hold-fix-review-request-claude.md:5`), not from
   `origin/main`. The totals recorded on `main` are 1,224 unit/provider, 202 integration, 235
   security, "7 supported evaluations passed with 3 live-evidence cases explicitly not run"
   (`docs/PLANS.md:473-476@c919cd2`). Both are true of different trees; neither is true of
   Beacon.

---

## The invariants

Every row is a finding about live code. "Beacon must preserve" is what the finding implies,
not a decision.

| # | Invariant | Source `path:line@pin` | Incident / evidence | What Beacon must preserve |
|---|---|---|---|---|
| 1 | **Unsupported is reported as unsupported, never converted to a pass.** Counts of passed, failed, and unsupported are separate fields | `docs/EVALS.md:5-6,28-30,193-195@c919cd2`; `scripts/eval-fixtures.mjs:73-79,1002-1012`; `scripts/gate2-explanation-cohort-contract.mjs:24-31` | M1 catalog carries two `supportStatus: "unsupported"` cases with a reason (`fixtures/evals/manifest.json:85-97,164-168`); they validate their fixtures and stay unsupported | Separate counters per outcome; a capability-absent case never enters the reproduced count |
| 2 | **Not-run is reported with its denominator, never hidden.** | `scripts/gate1-benchmark.mjs:958,1086`; `docs/PLANS.md:473-476`; `docs/THREAT_MODEL.md:132` ("absent checks are `not_run`"); `docs/OPERATIONS.md:745-750` | Gate 1 prints "3 passed, 0 failed, 3 live-evidence not run" every `pnpm eval` | A green summary that omits a not-attempted case is a false green |
| 3 | **A green names its exact inputs and environment by digest.** | `docs/EVALS.md:28-30,47-59`; `scripts/gate1-benchmark.mjs:410-424`; `docs/evals/2026-08-28-gate2-target-discovery-profile.md:40-51` | Report binds manifest SHA-256, fixture digests, provider/model/adapter, instruction- and routing-policy digests, Ollama model digests; "unknown fields … or digest mismatch fail validation rather than becoming a partial case" (`EVALS.md:58-59`) | Exactly `BLUEPRINT.md:235-236`; Beacon's own matrices currently violate it (Corrections §4) |
| 4 | **The fixture set is immutable; change is a new version that binds the old digest.** | `docs/EVALS.md:346-354`; `scripts/gate2-v2-evidence-adoption-contract.mjs:174-175,248-254`; ADR 0067 `:17-20` | Manifest v2 binds v1's SHA-256 and replaces two cases; "predecessor evidence cannot be adopted across drifted case" fails closed | Fixture Artifacts get content identity; edits are successors, never in-place |
| 5 | **Thresholds are predeclared and validated as literals; a failed run stays failed.** | `scripts/gate2-benchmark-contract.mjs:377,386,388-395,403-405`; ADR 0066 `:15-18,68-77`; ADR 0067 `:74-75` | 24/30 success and 0.80 plan acceptance are literal assertions in the manifest validator; two live runs failed them and were "accepted as an honest failed measurement" | A baseline/threshold lives in the fixture set, not in the runner; no post-hoc re-thresholding |
| 6 | **Repeated runs are the stability requirement — and nobody has met it.** | `docs/evals/…target-discovery-profile.md:35-36`; ADR 0067 `:70-72`; `docs/PLANS.md:113-116` | "one repetition is still insufficient stability evidence"; the open item is "across predeclared repeated runs" | A single-run Evaluation is labeled single-run; stability is a separate claim with N declared up front |
| 7 | **Leak-free: the evaluated party never sees the answer.** | ADR 0067 `:31-33`; `…target-discovery-profile.md:53-55`; `docs/EVALS.md:226-228` | Model gets path inventory + task + retrieved bytes; "expected targets, citations, findings, and evaluator answers remain absent"; retrieval fixed "without injecting manifest expected paths or their cardinality" | Expected-* fields are unreachable from the Run's input manifest |
| 8 | **A test of the test: the baseline must fail before the fix.** | `scripts/eval-fixtures.mjs:1496-1501`; `scripts/gate1-benchmark-result-contract.mjs:309-311` ("must reproduce at least one failure"); `docs/EVALS.md:173-176` | A second run applies only the check edit and leaves the defect: "verification must fail there, or the added assertions pin nothing" | Every regression fixture records its failing baseline as evidence |
| 9 | **A corpus must produce both verdicts, so an assertion cannot pass vacuously.** | `tests/security/live-evidence-credential-agreement.test.ts:191-197,285-295`; `tests/security/live-evidence-provider-origin.test.ts:143`; ADR 0045 `:226-227`; ADR 0053 `:99-100` | "its `catch` returned false for a malformed fixture and for a genuine refusal alike, so two of its three results silently became vacuous while the third kept working" | Every check kind proves ≥1 admit and ≥1 refuse in its own suite |
| 10 | **Observed and design-asserted effects are labeled separately, per counter.** | `scripts/gate2-explanation-cohort-contract.mjs:403-416`; `docs/EVALS.md:246-250` | Review F4: zero external-network effects were "closed-design assertions, not instrumented counters" and had been reported as counts (`…review-v2-claude.md:68-73`) | A zero that nobody measured is labeled as such |
| 11 | **The validator recomputes derived metrics; it does not trust them.** | `…explanation-cohort-contract.mjs:519-535`; `…review-v2-claude.md:60-66,83-90` | Mutation `precision 0.75 → 1.0` REJECTED; relabel `design-assertion → observed` REJECTED | A forged number in an Evaluation record is arithmetic-checked |
| 12 | **A timeout or truncation is a retained failed observation, never a dropped case.** | ADR 0066 `:42-45` (`finish_reason: length` fails even if the JSON parses); ADR 0067 `:54-57`; `docs/OPERATIONS.md:745-750,759-762` | "a document reporting 'two models measured' while silently omitting the third reads as complete"; "zero successful attempts is not a measurement" | Denominators are fixed by the fixture set, not by what finished |
| 13 | **Comparison rows must have asked the same question.** | `docs/OPERATIONS.md:721-728,740-744` | `BENCH_REQUEST_DRIFT` refuses the whole document; "one fleet model was recorded at five different token rates across four hand-rolled harnesses"; targets run sequentially, never in parallel | A comparison records the request once and checks every row against it |
| 14 | **Digests prove self-consistency, not runner authenticity.** | `…explanation-cohort-contract.mjs:90`; `…target-discovery-profile.md:98-99` | Stated as a limitation in every cohort report | Beacon must not read an unkeyed digest as attestation (cross-ref `docs/execution.md`) |
| 15 | **The benchmark never widens authority.** | `docs/OPERATIONS.md:716-719`; ADR 0066 `:15-18`; ADR 0067 `:77-78` | "an 'approve everything for the benchmark' flag would be exactly the digest-unbound authority widening this repository refuses elsewhere" | An Evaluation Run holds no more authority than any other Run; matches ADR 0012 "cannot authorize execution" |
| 16 | **Synthetic evidence is never promoted to a live result.** | `docs/EVALS.md:127-141`; `scripts/gate1-benchmark-contract.mjs:638-639` (3 required, 3 successes) | Loopback-provider passes label themselves `contract_passed_gate1_live_evidence_not_run` | Fixture-provider and live-provider Evaluations are distinct outcomes, never summed |
| 17 | **The report path is owned, private, identity-checked, and validated before and after the atomic write.** | `scripts/gate1-benchmark.mjs:95-127` (mode `& 0o077 == 0`, `0o700`, no symlink traversal); `docs/EVALS.md:97-100` | "Failure to validate or persist a report is itself a command failure, not evidence" | An Evaluation record that could not be persisted is a failed Run |
| 18 | **Adoption of prior evidence is exact-unchanged only, and is labeled as replay-validated, not new execution.** | `scripts/gate2-v2-evidence-adoption-contract.mjs:19,73-74,248-254`; `docs/EVALS.md:366-384` | Policy id `gate2-v2-exact-unchanged-evidence-adoption-v1`; "those seven source reports cannot be added directly into a synthetic 30/30 result" | Carrying an old green forward requires byte-identical case definitions and says so |
| 19 | **Gates are `&&`-chains of bare commands.** | `package.json:45-47@c919cd2`; Beacon `scripts/gate:13,23,30@c59dde1`; memory `gate-discipline.md:14-18` | "never `cmd \| tail` … a red `ruff check` sailed through. Twice burned in one session" | Beacon already has this; keep it |

---

## Load-bearing subtleties

**The four Blueprint outcomes are fewer than the fleet needed.** Icarus carries five per-measure
statuses — `measured`, `estimated`, `not_applicable`, `unsupported`, `not_measured`
(`scripts/eval-fixtures.mjs:73-79@c919cd2`) — each with a `reason` (`:1004,1010`), plus per-case
`passed`/`failed`/`unsupported`, plus two different "not attempted" facts: `not_run` (a
credential gate closed this case, `gate1-benchmark.mjs:958`) and `unexecutedCases` (this cohort
did not scope it, `…explanation-cohort-contract.mjs:30,394` → 25 of 30). A first mapping:

| Blueprint (`BLUEPRINT.md:234-235`) | Nearest Icarus fact | What the single value would collapse |
|---|---|---|
| reproduced | `passed`; byte-identical replay (`EVALS.md:398`) | pass-against-oracle vs replay-of-a-frozen-report (Invariant 18) |
| diverged | `failed` | a wrong answer vs a timeout vs `finish_reason: length` (Invariant 12) |
| not replayable | `unsupported` (permanent, capability absent) | a permanent capability gap vs an environment that cannot be pinned |
| not attempted | `not_run` / `unexecutedCases` / `not_measured` | gated-by-credential vs out-of-cohort vs measure-not-taken |

`estimated` has no home at all: ADR 0066 `:60-64` records cost as "an estimated relative
local-compute proxy … `actualBilledUsd` remains `null`". The memory
`absent-dead-broken-collapse.md:31-42,72-79` records the same collapse four times in one Mosaic
session and the refinement that it is four states, not three; `BLUEPRINT.md:681-682` already
commits Beacon to that. The finding is that the `reason` field, not a fifth enum value, is what
kept Icarus honest.

**Two things called "replay".** Beacon's Replay is "a new Run … against pinned inputs and a
recorded environment" (`CONTEXT.md:259-261`). Icarus has that (`EVALS.md:398`) and also a
second thing: the adoption "replay receipt", which re-validates a frozen report **without
re-executing** and labels itself so (`…adoption-contract.mjs:73-74`). The second is closer to
re-validation of a Receipt than to a Replay. Conflating them would let a frozen report count as
a fresh Run.

**"Evaluator" means code in Icarus and an Agent in Beacon.** `evaluator: "production_lifecycle"`
(`fixtures/evals/manifest.json:24`) names a deterministic procedure. ADR 0012's Evaluator is an
Agent with identity. Icarus's evaluators are therefore Beacon's "deterministic checks"
(`adr/0012:14`), and Icarus has zero examples of the Agent kind.

**"Baseline" already has three meanings.** Icarus: the fixed single-model comparison arm
(ADR 0066 `:49`), and the pre-fix failing check that must reproduce (`…result-contract.mjs:309`).
Beacon `BLUEPRINT.md:234` lists "baselines" beside "rubrics" without saying which.

**The retained imperfect number is the tell of an honest report.** Scaffold-A recall is `0.75`
in one case and the report "retains that expected-context omission rather than forging a perfect
per-case score" (`EVALS.md:337-340`). Precision `0.75` four times, macro `0.80`, was reproduced
to the digit by an independent probe (`…review-v2-claude.md:44-56`). A matrix with no non-Pass
value anywhere (Beacon's 250/250) cannot show this property.

**The harness lies more often than the code.** Recorded failure modes, each of which produced a
plausible wrong answer rather than a crash:

- Python's `.pyc` validator (mtime + size) reused mutant 1's bytecode for mutant 2, so the
  mutation runner reported a killed mutant as SURVIVED
  (`private-evidence/2026-08-29_janus-pr3-unverifiable-binding-claude.md:27`). Fix: clear
  `__pycache__` between mutants.
- A duplicate `_cli` helper: Python binds at call time, so new tests silently exercised the
  pre-existing helper (`…janus-pr3…:28`).
- A scratchpad clone whose venv put the **real** repo's `src` on `sys.path`: a mutant applied to
  the clone was never imported and "survived" (memory `verify-the-harness-too.md:24-33`). The
  asymmetry: a mutant that fails proves test and harness; a mutant that survives "proves nothing
  until you have located the module that actually ran" (`:35-44`).
- zsh does not word-split unquoted expansions; a loop reported three working configs broken
  (`verify-the-harness-too.md:11-17`).
- `caplog` bypasses the production formatter; negative assertions pass vacuously without
  `at_level` (`gate-discipline.md:21-26`). `grep -q` under `pipefail` SIGPIPEs its writer and
  reports failure **because it matched** (`:52-64`).
- Four reviewers died on a usage limit and returned `{"survived":[],"refuted":[]}`; re-run found
  17 findings (`empty-agent-result-is-not-a-pass.md:11-15`).
- A filter over a mistyped JSON key returned an empty list that read as clean
  (`absent-dead-broken-collapse.md:44-52`).
- A guard keyed on a self-declared `kind` was dodged by mislabeling; all three instances passed
  their own suites (`guards-keyed-on-self-declaration.md:8-23`).

Chronos records the countermeasure as protocol: "each conjunct verified by reverting it alone
and watching a named test fail (18/18)" (`docs/adr/ADR-0028-…md:9-10@6034e10`). Beacon's own
throttle evaluation does the same by hand — unfixed fails, repaired passes, reverted fails again,
restored passes (`docs/evals/2026-08-28-operator-throttle-slow-cpu.md:47-52@c59dde1`).

**Beacon's native matrix, as it exists.** Shape: a Scope paragraph naming what is *not*
evaluated; `Case | Expected behavior | Observed` tables; a commands block with counts; a closing
sentence that the evidence "does not authorize merge, deployment, or live migration"
(`2026-08-29-project-policy.md:1-7,8-31,50-71`). Two files add a Research basis with external
citations (`2026-08-28-resident-lifecycle.md:7-20`); one measures under a swept CPU quota with a
before/after column (`…throttle…:32-46`). What it has: predeclared expectations, explicit scope
exclusions, a revert check, and a refusal to let a green authorize anything. What it lacks:
commit pins, digests, an environment record, any non-Pass vocabulary, and a machine-validated
record — it is prose a reader trusts, not a report a validator recomputes.

---

## The measured case

| Measure | Value | Source |
|---|---|---|
| Gate 2 routed successes, live, leak-free (r7) | **16/30** vs threshold **24/30** | `docs/evals/2026-08-28-gate2-target-discovery-profile.md:11@c919cd2` |
| Gate 2 first-pass plan acceptance | 0.7667 vs 0.80 | `:12` |
| Per class, routed `code` | repair 7/10 · refactor 4/5 · explanation 3/5 · security review 2/5 · **scaffold 0/5** | `:24-30` |
| Fixed `code-fast` baseline | 5/30, repeated once; "one repetition is still insufficient stability evidence" | `:11,35-36` |
| Prior live run (ADR 0066) | routed 9/30, plan 0.6667, security 0/5, scaffold 0/5 | `docs/adr/0066…md:68-72` |
| Deterministic cohorts | 5/5/5/5/4/4 executed and passed; each reports its own `unexecuted` (25, 25, 25, 25, 26, 26) | `docs/EVALS.md:242,268,294,321,331,363` |
| v2 adoption aggregate | 30 replay-validated, recall 0.9917, precision 0.8083, `allGate2ThresholdsMet: false` | `docs/EVALS.md:386-396` |
| Gate 1 | contract 3 passed / 0 failed / 3 live-evidence not run every `pnpm eval`; credential-gated live 3/3 completed 2026-08-23 | `gate1-benchmark.mjs:1086`; `docs/PLANS.md:5-6` |
| M1 catalog | 10 scenario classes, 7 executed outcomes, 2 unsupported with reasons | `docs/EVALS.md:143-167`; `manifest.json:85-97,164-168` |
| Test suite on `main` | 1,224 unit/provider · 202 integration · 235 security | `docs/PLANS.md:473-476` |
| Test suite, PR #71 worktree | 1259 · 203 · 351 | `…pr71-e2-hold-fix-review-request-claude.md:5` |
| Explanation-cohort review cycle | HOLD (v1) → HOLD (v2, one residual) → PASS (v3); reviewer's probe reproduced macro precision 0.80 exactly on both heads | `…review-v2-claude.md:5,44-56`; `…-v3-claude.md:5,76-87` |
| Beacon `docs/evals` | 11 files · 563 lines · 250 Observed rows · 250 `Pass` · 0 commit pins | `grep`, 2026-08-29 |
| Chronos | 4365 passed, 1 skipped (credential-gated IBKR smoke, "never been run against a real gateway") at exact `7752825bc…` | `docs/TEST_RESULTS.md:10-26@6034e10` |

The two numbers that carry the page: **16/30 with one baseline repetition**, and **250/250 Pass
with zero pins**. The first is the fleet's most rigorous evaluation reporting itself as a
failure. The second is Beacon's evaluation surface unable to report a failure at all.

---

## Dead seams — never built or closed

- **`fleet-eval` / ReplayDeck** — never created; no repo, no tree, no code. Its MVP list exists
  only as prose (`roadmap:107-110`).
- **Automatic promotion** — explicitly excluded by the roadmap (`:108,112-113`) and by Icarus:
  "Mock or synthetic … evidence is never promoted" (`EVALS.md:141`); a failed live run "remain[s]
  a failure rather than motivating a post-hoc threshold change" (ADR 0066 `:17-18`).
- **Leaderboards, training, online routing by eval** — never built. Icarus evaluates routing by
  a paired offline comparison (`code-fast` vs `code`, ADR 0066 `:47-51`) and `bench compare` is
  "measurement only … no approval or grant" (`OPERATIONS.md:714-716`).
- **Judge-model scoring** — never built anywhere in the fleet (Corrections §3).
- **Live code-change benchmarking inside `bench compare`** — deliberately absent
  (`OPERATIONS.md:716-719`).
- **Gate 2 exit** — open; routed 16/30, scaffold 0/5, stability unmeasured
  (`PLANS.md:113-116`). Supervised canary **HOLD** (`PLANS.md:19-23`).
- **Two Milestone-1 classes** still unsupported: module refactor and read-only security findings
  (`EVALS.md:188-195`) — the Gate 2 cohorts "do not retroactively convert" them.
- **Janus has no CI at all**; its gate is `pytest` by hand (`…janus-pr3…:33`).

---

## Consequences for Beacon's Evaluations

Each is testable; none is decided here.

1. **Four outcomes are the floor.** Every Evaluation record carries, per case and per measure, an
   outcome plus a `reason`, and separate counts for reproduced / diverged / not replayable / not
   attempted. Test: a fixture whose capability is absent yields a record whose reproduced count
   excludes it and whose not-replayable entry names why.
2. **A green names bytes and environment.** Every record binds the fixture-set digest, each
   fixture's content identity, and the environment (provider, model identity, adapter version,
   sandbox/image digest, instruction and policy digests). Test: two Replays of identical inputs
   produce byte-identical records; a record with one altered digest fails validation.
3. **The validator recomputes.** Any derived metric (recall, precision, rate, cost) is
   recomputed from raw observations at validation. Test: forge one metric → rejected.
4. **Both verdicts, per check kind.** Every deterministic check's corpus proves at least one
   admit and one refuse. Test: a suite-level assertion per check kind, as in
   `live-evidence-credential-agreement.test.ts:285-295`.
5. **The baseline must fail.** A regression fixture records its pre-fix failing run as evidence.
   Test: a fixture whose baseline passes is rejected at validation, as in
   `gate1-benchmark-result-contract.mjs:309-311`.
6. **Observed vs design-asserted, per counter.** Zero-effect claims carry the label. Test: relabel
   one → rejected.
7. **Thresholds live in the fixture set as literals.** A failed Evaluation is retained as failed;
   a new threshold is a new fixture-set version binding the old digest.
8. **Stability is a separate, predeclared claim.** A record from one run is labeled single-run;
   any Track Record entry derived from it says so. N repetitions are declared before the first.
9. **Leak-free by construction.** Expected outputs are not reachable from the evaluated Run's
   input manifest. Test: the Run's input digest is computed over a structure that has no
   expected-* fields.
10. **Author ≠ Evaluator is recorded (ADR 0012) — and the Evaluator's harness is itself checked.**
    A surviving mutant is unexplained until the executed module is located; caches are cleared
    between mutants; a probe is made to fail on purpose before its first green is trusted.
11. **Timeouts and truncations are retained failures with fixed denominators.**
12. **Beacon's own `docs/evals` pin the exact commit and admit non-Pass verdicts.** Test: every
    file names a 40-hex head; the Observed vocabulary includes at least `Fail`, `Unsupported`,
    and `Not run`.
13. **A judge model's verdict is one Finding, never the outcome.** If model judges are built,
    their output is recorded beside deterministic results with its own uncertainty, and no
    record's outcome is set by it alone.
14. **No Evaluation result changes a Route, a threshold, or authority automatically.** An
    Evaluation Run holds no more authority than any other Run.
15. **Keep `scripts/gate` an `&&`-chain.** It already is.

---

## Open questions — Kevin's, not ours

1. **Does `fleet-eval` get built as Beacon's Evaluations, or stay a codename?** `BLUEPRINT.md:1010`
   says native; `roadmap:225` still assigns a lead (Kimi) and reviewer (Codex) to a repo that
   does not exist. Is that ownership void, or does it transfer to the Beacon seat?
2. **Which of Icarus's ten measurement keys become Beacon's?** `taskSuccess`, `testSuccess`,
   `incorrectEdits`, `contextRetrievalQuality`, `toolFailures`, `runtime`, `tokenUsage`,
   `apiCost`, `humanApprovalFrequency`, `rollbackSuccess` (`manifest.json:4-15@c919cd2`). The
   last two are Execution-side facts (`docs/execution.md`). And `estimated` — cost by proxy —
   has no Blueprint outcome.
3. **Is "not replayable" a property of the case or of this run?** Icarus's `unsupported` is
   permanent; its `not_run` is transient. `BLUEPRINT.md:682` wants both distinct; the four-value
   list at `:234-235` may not carry it without a `reason`.
4. **"model judges" (`BLUEPRINT.md:234`)** — nothing in the fleet has ever scored with one and
   the roadmap warns against it as a sole score. Build it, or drop the word?
5. **Which "baseline"?** Comparison arm, pre-fix failing check, or something else.
6. **Does ADR 0012's non-author Evaluator apply when the Evaluation is fully deterministic?**
   Icarus's evaluators are code; ADR 0012 `:14` says deterministic checks "supplement".
7. **Where does the cross-seat HOLD→PASS cycle live in Beacon?** It is the independence
   Icarus actually had; it is neither an Evaluation record nor a Ruling today.
8. **Are the 11 existing matrices retroactively pinned**, or accepted as historical prose?


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
