# Execution findings inherited by Beacon

**Status:** draft for review · **Produced by:** claude-code harvest agent ·
**Last verified:** 2026-08-29

Beacon's Execution surface "is descended from Icarus" and "is the only module allowed to cause
external effects" (BLUEPRINT.md:247-249@c59dde1). Icarus is the pillar where almost every rule was
produced by an incident or an adversarial finding rather than by design. This page records those
rules with the evidence that produced them, so Beacon's implementing seat does not re-derive them
from taste. It states findings, not decisions.

Boundary: `docs/authority.md` already owns `planApprovalDigest`, `liveEvidenceProfileApprovalDigest`
and the enforced-vs-reported axis — linked, not restated. `evaluations.md` owns Gate 2 scoring,
cohorts and EVALS.md. `models.md` owns Vulcan's admission and budgets; this page owns Icarus's
provider-pin binding (ADR 0053) only. This page owns: what a Proposal digest must cover and why
each field exists; the effect ledger, receipts and landing; credential and provider-host preflight;
recovery, interruption and crash-tail; headless default-deny; the module-naming machine guard; the
HOLDs; and the process findings.

---

## How this page was produced

Everything below was re-derived from the pinned trees on 2026-08-29. Background memories and
handoffs were read as CLAIMS and checked against the pin; the corrections are listed in the next
section.

| Source | Tree | Pin | Read as |
|---|---|---|---|
| Icarus | `~/ai-workspace/claude/icarus` | `origin/main` = **c919cd2** (PR #71 merged today) | `git -C <tree> show c919cd2:<path>` · `git -C <tree> grep -n <pat> c919cd2 -- <paths>` · `git -C <tree> log -S<field> c919cd2 -- <path>` |
| Beacon | `~/ai-workspace/claude/beacon` | **c59dde1** | `sed -n 'A,Bp' <file>` — read only |
| Memories | `~/.claude/projects/-home-kevin-lee-ai-workspace-claude/memory/{icarus-lane-state,icarus-agent-authority-posture,stacked-pr-phantom-merge-lesson,post-merge-branch-trap,guards-keyed-on-self-declaration,inherited-claims-hit-rate,unpushed-means-unreachable}.md` | — | claims to verify |
| Handoffs | `shared/handoffs/2026-08-22_ai-town-cohesion-findings-claude.md`, `…/2026-08-29_icarus-pr71-e2-hold-fix-review-request-claude.md` | — | claims to verify |
| Process detector | `git -C <tree> cherry origin/main origin/claude/e2-bench-compare` | — | re-run today |

Not done, deliberately: no `checkout`/`switch`/`pull`; no `pnpm` (gate counts are cited from
records, not re-run); the codex audit probe was located (`ls`) but not executed. The Icarus clone's
working tree is on an old branch and was ignored.

Line numbers are `path:line@c919cd2` unless marked `@c59dde1`. ADRs are `ADR NNNN@c919cd2`.

---

## What the Blueprint already specifies

- Execution turns approved Proposals into sandboxed Runs, enforces ceilings, supports interruption,
  performs declared checks, emits Receipts; only module with external effects (BLUEPRINT.md:247-249).
- One mutation path: Execution "accepts only a Proposal whose digest, policy version, target set,
  base revision, checks, sandbox, model/provider constraints, and budget still match"
  (BLUEPRINT.md:434-441); Safety Snapshot fails closed (:447-452); ADR 0004@c59dde1.
- Autonomy Profiles Observe / Supervised / Bounded; Alpha 1 defaults to Supervised; Bounded only
  for disposable local repositories (BLUEPRINT.md:400-415). Actions that always gate (:417-427).
- Failure recovery: ONE automatic Recovery Attempt under the same Proposal and remaining Budget
  Envelope, then reassignment with a Handoff (BLUEPRINT.md:368-373; ADR 0010@c59dde1).
- Pause Work: a Run "remains stopping or unconfirmed until a terminal Receipt or lease expiry
  proves its state"; the Operator reviews every unconfirmed Run before Resume (:375-389;
  ADR 0016@c59dde1). Worker leases bind Proposal and policy digests, effects, sandbox, network,
  time, budget (:552-566; ADR 0005@c59dde1).
- Receipt contents (BLUEPRINT.md:788-800); invariants "every mutation is attributable to an actor,
  Proposal, policy version, and Receipt", "a denied or stale Proposal causes zero repository
  mutation", "an interrupted Run reaches a terminal state and explains residual effects"
  (:906-909; AGENTS.md:135-148@c59dde1).
- Vocabulary (CONTEXT.md@c59dde1): Proposal :278, Gate :283, Ruling :287, Run :291, Receipt :295,
  Pause Work :304, Safety Snapshot :321, Binding :335, Recovery Attempt :161, Handoff :151,
  Worker :81, Autonomy Profile :274, Budget Envelope :171.

Icarus → Beacon word map used below: plan manifest / approved plan → **Proposal**; plan approval,
review approval, landing decision → **Ruling**; live-evidence profile → **Proposal + Ruling** over
a target set; headless profile (H1) → **Autonomy Profile / Policy narrowing** (selection, never
authority); landing → **dispatch of a Run's external effects**; effect → **external effect**;
landing receipt / settlement → **Receipt**; case → **Mission/Run**; SunCeiling → **Budget
Envelope**; run lease → **Worker lease**; HOLD → **Gate awaiting a Ruling**; `reconcile-headless`
→ resolving an **unconfirmed Run**; `resume-headless` → an explicit **Recovery Attempt**.

---

## Corrections to prior claims

1. **`docs/authority.md` cites Icarus at `af41796`** (authority.md:47@c59dde1). That is five days
   and ~20 commits stale. Its `policy.ts:136` citation still holds; its
   `change-handoff-reader.ts:2740` citation is now `:2779` (`if (expected !== planSha256)
   sourceInvalid();`, with `sourceInvalid()` defined at `:235`).
2. **`authorizeLiveEvidenceRun` no longer has zero production callers.** Memory
   `icarus-lane-state` says it was "constructed ONLY in tests". At the pin it is called from the
   ADR 0055 executor: `packages/core/src/live-evidence-executor.ts:186` and `:327`. The Gate 1
   executor exists and ran live 3/3 on 2026-08-23 (`docs/evals/2026-08-23-gate1-live-3of3.md`).
3. **The effect ledger is still in-memory, but the restart hazard moved.** `#counts`,
   `#reachedStage`, `#spendUsd`, `#elapsedSeconds` are instance fields
   (`live-evidence-run.ts:224-228`). ADR 0055 did not persist the ledger; it rebuilds one from the
   durable journal's completed cases every loop (`live-evidence-executor.ts:180-197`, `replay()`),
   and counts a GitHub effect "only from the corresponding durable HTTP POST admission row". What
   survives a restart is therefore what the journal and landing ledger recorded — not the ledger.
4. **`scripts/security-check.mjs:1829`** (cohesion handoff, memory) is now `:2006` — the regex
   `/(?:\bAthena\b|\bMinerva\b|constellation\.event\.v1)/i` inside assertion
   `changeHandoffAddsNoApiBrowserOrControlPlaneSurface` (`:2001`), with the seam-word forbid
   `/\b(?:outbox|delivery|webhook|callback|queue|worker|receiver)\b/i` at `:2009`.
5. **`docs/OPERATIONS.md:1086-1088` / `:1075-1078`** (memory, ADR 0054 discussion) drifted: the
   effect-allowlist reading is `OPERATIONS.md:858-861`; the two acceptance cases (same-tab reload
   vs foreground-server restart) are `:1284-1290`. `:777-780` still says the draft-PR POST "is
   admitted at most once per landing, ever".
6. **`assertLiveEvidenceProfileMatchesManifest` is at `live-evidence-profile.ts:545`**, not
   `:417`; `decodeProvider` is `:241-314`, not `:253-265`; the old preflight line
   `live-evidence-run.ts:115` is now the usability check at `:163-178`.
7. **`git cherry` now prints `-` for both orphaned commits** (`dfd885d`, `710e059`), where the
   PR #71 handoff recorded `+`. `BenchOutputSpreadV1` is present (`bench.ts:3`, `index.ts:1` hits
   at the pin). The fix landed; the detector reads negative, as it should.
8. **Memory says PR #48's mutation check fails 18/52 tests without the predicate.** Not re-run
   here; cited as recorded.

Every other memory claim checked (decideLanding at `:2697`; `gateway.ts:192-199`;
`providers.ts:226`/`:361`; `provider.ts:57-82`; 0 `landing` hits in `packages/api/src/server.ts`;
four-effect allowlist) held at the pin.

---

## The invariants

| # | Invariant | Where | The incident that produced it | Beacon must preserve |
|---|---|---|---|---|
| 1 | A Proposal is bound by its **bytes**; the gate hashes what it parses | `live-evidence-profile.ts:545-570`; ADR 0052 | Function took parsed object + digest string as independent args; an edited manifest (paid anthropic @ $500, owner=attacker) was ADMITTED beside the reviewed digest. Three independent reproductions | Any "digest matches" check computes the digest from the bytes it will act on. One parameter, no pair |
| 2 | Preflight asserts the **strictest consumer's** predicate | `live-evidence-run.ts:92-98`, `:175`; `gateway.ts:192-199`; `providers.ts:226`, `:361`; ADR 0045 amendment 2026-08-22 | Old check `hasOwn && !== ""` admitted 11 of 16 environment shapes every consumer rejected; case three would die on an unusable token after cases one and two had opened real PRs | Credential preflight = 8..512 chars, no whitespace/NUL/control; one predicate, not per-provider dispatch; value reaches one boolean and never a message, log, record |
| 3 | Preflight-admits ⟹ consumer-accepts, proven against REAL constructors | `tests/security/live-evidence-credential-agreement.test.ts:285-336`; `tests/security/live-evidence-provider-origin.test.ts:143-164` | A synthetic fixture "can be made to agree with itself" | The agreement test constructs the real gateways and asserts the corpus yields both verdicts, so it cannot pass vacuously |
| 4 | Provider pin binds the **host**, not the name | `live-evidence-profile.ts:287-314`; `provider.ts:57-82`; ADR 0053 | `decodeProvider` re-derived a weaker URL rule; `ollama` @ `https://exfil.attacker.example/?k=secret#f` and plaintext `http://198.51.100.9:11434/` were admitted; rates 0 meant the $0 ceiling could never fire | Remote requires HTTPS; hosted kinds pinned to their own origin; local kinds must be loopback (stricter than the gateway, because the gateway has no origin invariant) |
| 5 | Money and model pins are bound, not intended | `live-evidence-profile.ts:700-720`; ADR 0050 | Profile with exact digest and correct repos could pin `anthropic/claude-opus-5` @ `maxSpendUsd: 500`. "An approval that binds everything except the spend is not a spending authority" | Provider kind agreement; `paid:false` ⇒ rates null/0; child spend ceiling ≤ parent's; missing field = refusal, never permission |
| 6 | Returned authorization is **frozen and copied**; the ledger re-checks and copies | `live-evidence-run.ts:180-195`; ADR 0045 property 4 | `effects` and `budgets` were returned by reference; `effects.push("github.ref.force_update")` after approval widened a digest-bound grant | `readonly` is compile-time; freeze at runtime; consumers copy |
| 7 | Closed effect set, exact **ordered** equality; the list is also the chain | `live-evidence-profile.ts:52-57`, `:364-368`; `live-evidence-run.ts:196-212`; ADR 0054 | "Authority that can be widened by appending an entry is not a bound"; counting alone lets a runner report a complete multiset it never earned | Named-effect allowlist compared exactly; per-Run stage order enforced (repeat current stage ok, skip/backwards refused) |
| 8 | The irreversible POST is protected by a **durable UNIQUE index** | `gate1-schema.ts:466-468`; `landing-ledger.ts:2267-2271` | A lost or ambiguous response must be reconciled by reads, never by a second POST | `one_create_pr_post_per_landing` in the store, plus a load-time check that refuses any store that lost it |
| 9 | Admit to the durable ledger before I/O, settle after; reconcile by reads or byte-identical immutable replay | `OPERATIONS.md:777-780`, `:797-799`; ADR 0027 | Interrupted mutation with unknown outcome | Persisted intent precedes every external request; no blind repeat |
| 10 | ONE decision record, re-read at every stage | `landing-coordinator.ts:2697-2713` (decideLanding); re-read at `:823-828`, `:1144-1149`, `:1323-1327`, `:1648-1654` | Three external mutations, one approval — the record (`landingSha256`, `decision === "approve"`) is loaded fresh from the store at dispatch and at each of upload / ref-create / draft-PR | Do not cache the Ruling in the process; re-read the store before each effect |
| 11 | Landing is CLI-only; no daemon, queue or listener in the authority path | `packages/api/src/server.ts` (0 hits for `landing`); ADR 0055 | Kept the API a review surface | Effects come from one-shot, lease-held invocations that exit |
| 12 | Pinned commit epoch is ONE optional field, never a runtime clock | `landing-coordinator.ts:101`, `:2672`; `OPERATIONS.md:781-796`; ADR 0051 | Reproducing a pinned SHA-1 needs the epoch; pinning `now()` would backdate the entire evidence trail | Pin moves the commit instant only; every other timestamp stays on the clock; pin recorded durably; future refused |
| 13 | SIGKILL between admitted operations → exactly one charged interruption; repeated recovery byte-identical; ordinary resume refused | `tests/integration/headless-worker-crash-recovery.test.ts:223-224`, `:236`, `:259`, `:320`; ADR 0049; `docs/evals/2026-08-21-headless-crash-tail.md` | A started operation with no result has an unknown external effect | See "crash-tail" below |
| 14 | Continuation is explicit, exactly-once, admitted only when every crash-tail effect is `durably_settled` or `no_effect` | `headless-reconstruction.ts:38`; ADR 0057, 0058, 0063 | Stage re-entry keys on successor intent, so an unrecorded intent would re-execute a settled effect | Classify before any retry; refuse over `ambiguous` |
| 15 | Default-deny profile: one task, concurrency 1, `childRuns: "deny"`, `scheduledRuns: "deny"`, `mutation` absent ⇒ `"propose"` | `headless-profile.ts:40-52`; ADR 0046, 0059, 0060 | A profile that embeds a provider URL, tools or credentials becomes a second authority kernel | Profiles select; they never grant. Apply is a second Ruling bound to the proposed bytes |
| 16 | Kernel sandbox beneath the grant pipeline; unsupported host = honest no-op with a warning event | `landlock.ts:21-25`; ADR 0062 | Policy violation should be impossible, not merely refused | Never a silent fallback, never a hard failure |
| 17 | Runtime source may not name other modules or seam words | `scripts/security-check.mjs:2001-2017` | Athena built the callback half; Icarus refused to name Athena (stalemate, not gap) | See "machine guard" below |
| 18 | Executor accepts only completed runs created AFTER the profile approval; the run map grants no capability | ADR 0055 | A persisted run from before adapter approval cannot be relabeled and reused | Evidence never grants authority; the Ruling is the only source |

---

## The Proposal digest, field by field

authority.md already lists the fields; this section records **why** each one is in the digest,
which is the part a re-derivation from taste would lose. `planApprovalDigest`
(`policy.ts:136-172`) hashes:

| Field | Why it is bound | Source |
|---|---|---|
| `schemaVersion: 4`, `policyVersion` | A Ruling applies to one policy version; a schema bump invalidates every prior digest | `policy.ts:156-157`; BLUEPRINT.md:242-243@c59dde1 |
| `task` | The instruction the operator read | `policy.ts:158` |
| `baseCommit` | Pinned as soon as known, before context; the landing candidate is built against it | ADR 0009; `policy.ts:159` |
| `contextSha256` | The exact untrusted context bytes the model saw; egress approval binds it | ADR 0005; ADR 0026:34; `policy.ts:160` |
| `targets` | Operator-selected mutable paths; a `mutation.patchset` grant naming a path outside them is refused | ADR 0023; `policy.ts:161`, `:354` |
| `provider` | Prevents a stable provider-profile id from selecting a different provider/model than approved | ADR 0047; `policy.ts:162` |
| `checks`, `sandbox` | Registered checks and the immutable `@sha256` sandbox image are part of what was approved | ADR 0007; `policy.ts:163-164`, `:619` |
| `ceiling` | The SunCeiling (Budget Envelope) the run may reserve against | ADR 0009; `policy.ts:165` |
| `plan` | Includes the grant list and `iterationCeiling`: "changing any grant changes the plan approval digest" | ADR 0026:92, :164; `policy.ts:166` |
| `readableManifestSha256` | What the agent may READ during the run; `null` (no read grant) and an empty manifest digest to different values, so a no-read approval cannot be replayed as an empty-scope one | `policy.ts:147-151`, `:167-169`; ADR 0026:50-52 |

Field history is compressed into the 2026-07-30/31 foundation commits (`187a4ed`, `bba1591`);
the per-field reasons live in the ADRs above, not in per-field commits. The second family, the
live-evidence profile (`liveEvidenceProfileApprovalDigest`,
`live-evidence-profile.ts:416`), was hardened by five findings in six days, each one adding a
binding the previous reviewer had not thought to compare:

| Added | Binding | Found by |
|---|---|---|
| ADR 0045 (08-18) | manifest digest, case-id bijection, per-case `owner/repository/baseBranch` | review: exact digest + exact case ids still aimed a case at `unreviewed-repository` |
| PR #45 (08-22) | identity **distinctness** | review: two cases on one repo → two draft-PR POSTs while each ledger count stayed at 1 |
| ADR 0050 (08-21) | provider kind; `paid:false` ⇒ rates 0; `maxSpendUsd ≤ maxCostUsd` | adversarial planning pass |
| ADR 0052 (08-22) | the manifest **bytes** | adversarial sweep + refuting reviewer + probe |
| ADR 0053 (08-22) | provider **host** | mirror of the credential finding |
| ADR 0051 (08-22) | commit epoch on the landing record (not the digest) | executor design |

ADR 0050 names the defect class: "the fields that are easy to compare get bound, and the field
that decides the blast radius does not." ADR 0050 also adds a key-set tripwire so a field added
later "would be silently unsigned" no longer.

**What BLUEPRINT.md:439-441 is missing relative to Icarus's two digests:**

1. A **context digest** — the exact bytes the model was shown (`contextSha256`).
2. A **readable-set digest** — what the Run may read, with `null ≠ empty`.
3. A **named external-effect allowlist** in the Proposal, compared by exact ordered equality;
   Bounded's "target allowlist" (BLUEPRINT.md:409) names targets, not effects.
4. **Provider host/origin**, not only "model/provider constraints"; local kinds loopback-only.
5. **Credential reference names** with a usability preflight before the first effect.
6. **Pricing pins**: unpaid ⇒ zero rates; child ceiling ≤ parent (the Envelope hierarchy covers
   the second, not the first).
7. A per-target **automation assessment** (`derivativeEffects`): creating a head ref or draft PR
   runs the target repository's own CI with its secrets (`OPERATIONS.md:866-871`).
8. The **byte** binding rule itself: Beacon's Proposal is "identified by its digest"
   (CONTEXT.md:278-280); Icarus's lesson is that identification is not binding until the
   evaluator parses the bytes it hashed.

---

## Load-bearing subtleties

**Mismatch unrepresentable beats mismatch detected.** ADR 0052 rejected keeping the
`(object, digest)` pair "and documenting that callers must pass corresponding values", rejected a
`{bytes, digest}` object ("the pair returns wearing a different hat"), and rejected validating in
the executor ("authority checks belong at the boundary that grants authority"). With one parameter
"there is no pair to disagree". The same shape recurs: ADR 0051 rejected the existing `now` seam
because it would backdate the trail; ADR 0053 rejected an operator acknowledgement flag because
"authority that a field can widen is not a bound".

**A preflight weaker than its consumer is a claim about a run it cannot deliver.** Both the
credential (ADR 0045 amendment) and the host (ADR 0053) findings are "a gate asserting a weaker
predicate than the code behind it". The fix each time was to call the consumer's own function
(`parseProviderBaseUrl`) or copy the strictest consumer's constant, then prove agreement against
the real constructors — not to transcribe the rule a fourth time.

**Durable index for the irreversible POST; journal replay for budget and stage.** The UNIQUE
index (`gate1-schema.ts:466-468`) guarantees at most one draft-PR POST per landing across any
number of process deaths. Spend and reached-stage live in the in-memory ledger
(`live-evidence-run.ts:224-228`) and are rebuilt from `journal.completedCases`
(`live-evidence-executor.ts:180-197`). ADR 0054's approval rests on chain-order and budget
guarantees; the restart the gate demands (`OPERATIONS.md:1284-1290`) is exactly what an in-memory
ledger does not survive, which is why the journal — owner-only, fsynced, absent-only publication,
append-only history (ADR 0055) — is the durable truth for budget, and the landing ledger is the
durable truth for effects. Beacon's Receipt/Run state must persist, per Run: reached stage,
per-effect admission rows (before I/O) and settlements (after), cumulative spend and elapsed, and
the reservation charged to an interrupted operation.

**"Separate approval for each external mutation" is an allowlist, not a click.** ADR 0054 settled
that the manifest's `separate_approval_for_each_external_mutation` means the digest-bound
four-effect allowlist (`OPERATIONS.md:858-861`), not per-HTTP-request approval; the per-occurrence
reading would have cost a schema migration and re-approval "for no safety the allowlist lacks".
The reading "was already written down and nobody had read it".

**Lease expiry proves nothing about a remote effect.** ADR 0055: "A mutation failure is
re-observed as a fresh ambiguity boundary even when the current process began with `resume`. It
writes a blocked receipt and exits. Only a later explicit `resume` invocation receives read-based
reconciliation authority." ADR 0057 classifies each crash-tail effect `no_effect` /
`durably_settled` / `ambiguous` from durable bytes alone, and "calling that effect absent would
risk duplicating it".

---

## The measured case

| Measurement | Value | Source |
|---|---|---|
| Icarus self-hosting as canary | **184** secret-shaped spans in **68 of 408** tracked files (scanner implementation, adversarial tests, security ADRs, redaction fixtures); the audit "correctly stops at the first finding and has no test- or documentation-path exception" | `docs/evals/2026-08-27-supervised-canary-clearance.md:42-51` |
| Same survey, scanner fix | baseline 11/20 correct (9 false positives on workflow controls) → candidate 20/20; all 10 synthetic credentials still detected | same file :22-38 |
| Credential preflight | **11 of 16** environment shapes admitted by `hasOwn && !== ""` while `GithubGateway` rejected all eleven | ADR 0045 amendment; `tests/security/live-evidence-credential-agreement.test.ts:29` |
| Manifest-bytes binding | making the digest gate vacuous fails **3 of 9** tests (test runs against the REAL committed manifest) | ADR 0052 Verification |
| Host binding | removing the loopback bound fails 1 test; reverting to the re-derived URL rule fails 6 | ADR 0053 Verification |
| Effect allowlist | **4** effects, exact ordered equality | `live-evidence-profile.ts:52-57` |
| Gate 1 live | **3/3** against three disposable private repos, Actions disabled; loopback Ollama `qwen3.8:27b`, zero price; profile digest `f7a87296…`, manifest digest `1b79bf42…`; replayed byte-identically without a credential | `docs/evals/2026-08-23-gate1-live-3of3.md`; ADR 0055 |
| Executor fault campaign | **20/20** real-process cases (lost response → block exit 3; changed manifest/profile on resume → exit 1; symlink/shared-readable journal → refused) | `docs/evals/2026-08-23-gate1-headless-executor.md` |
| H3a crash tail | 25/25 focused; full gate then 1,005 / 172 / 183 | `docs/evals/2026-08-21-headless-crash-tail.md` |
| Last full gate | **1259 / 203 / 351** (unit / integration / security), exit 0, at `603b727` whose tree is identical to c919cd2 | PR #71 handoff 2026-08-29 (not re-run here) |
| Gate 2 (pointer only) | routed 16/30 vs 24/30 threshold; plan acceptance 0.7667 vs 0.80 — Gate 2 open | `docs/PLANS.md:15-22`; see `evaluations.md` |

---

## HOLDs that bind today

1. **Live execution, unattended use, merge, deployment, live-state migration — closed.**
   `docs/PLANS.md:21-26`; `docs/ROADMAP.md:96-100`; ADR 0055 consequences; the 3/3 record
   "authorizes no merge, deployment, force-push, reference update or deletion, live-state
   migration, or unattended execution against an active repository". The three PRs remain drafts.
2. **Supervised active-repository canary — HOLD** since the 2026-08-27 survey (row 1 of the
   measured case). Icarus "cannot currently be the active-repository canary target without a
   separate self-hosting policy decision"; held "until the operator selects a different active
   repository or separately approves a designed self-hosting boundary". Reviewers on PR #67 (codex,
   grok, claude) each refused test/doc path exemptions without a design decision.
3. **ADR 0025 — third-party review and rotation outstanding.** Pinned `anomalyco/opencode/github`
   `77fc88c8` "has not been reviewed by this repository" (ADR 0025 decision); two items "remain with
   Kevin": rotate `OPENCODE_API_KEY`, add required reviewers to the `opencode` environment. The
   hold "narrows but does not fully lift". Branch protection on `main` was `protected: false`.
4. **ADR 0027 amendment needed** (ADR 0043 "Open questions"): (a) GitHub identity case in the
   pull-request `head` filter — needs one live observation nobody is authorized to make; failure
   direction is a 422 hold, not a duplicate PR; (b) `per_page=100` against the 1 MiB response
   ceiling can deadlock a legitimate reconciliation read.
5. **Effectful/control session continuation, Vulcan application/children, H3b fork — closed**
   (ADR 0063 status; `docs/PLANS.md:25-26`).
6. **Kevin's 2026-08-22 posture:** agents get read + propose only on Icarus; every mutation stops
   at a human approval; no standing pre-approvals for change classes (reserved for Gate 6);
   Gate 1 before any integration. Fleet-wide since 2026-08-23 as "Harness boundary — probes vs.
   mutations".

---

## Crash-tail ↔ Pause Work, unconfirmed Run, Recovery Attempt

What Icarus proved (`tests/integration/headless-worker-crash-recovery.test.ts`; ADR 0049): a real
child process is `SIGKILL`ed after `operation.started` is committed and the second provider
request is accepted (`:223-224`); `run reconcile-headless` in a fresh process yields exactly one
`operation.interrupted` (`:236`, `:259`), charges the full reserved runtime/tokens/cost, appends one
`icarus.headless.worker-interruption.v1` settlement with disposition
`requires_binding_reconstruction`; a second reconciliation returns byte-identical JSONL; ordinary
`run resume` is refused with `HEADLESS_BINDING_RECONSTRUCTION_REQUIRED` before any provider
request (`:320`); the source Git fingerprint is unchanged. Continuation (`run resume-headless`,
ADR 0058) is a separate, lease-held, exactly-once act admitted only when reconstruction equals the
persisted binding and no tail effect is `ambiguous`.

| Beacon term | Icarus counterpart | Gap |
|---|---|---|
| Pause Work → Run "stopping or unconfirmed" | worker with a start event and no settlement | Icarus also proves the **uncooperative** stop (SIGKILL); Beacon's "bounded interruption request" is the cooperative path only |
| "until a terminal Receipt or **lease expiry** proves its state" (BLUEPRINT.md:381-382) | expiry proves nothing about a remote effect; only reads reconcile (ADR 0055, 0057) | expiry may close the lease, not the effect; the Run must go to `ambiguous`, not terminal |
| Operator reviews every unconfirmed Run before Resume | `reconcile-headless` output + `no_effect / durably_settled / ambiguous` per effect | Beacon needs the closed three-way classification in the Receipt, not free text |
| ONE **automatic** Recovery Attempt (BLUEPRINT.md:368-373; ADR 0010) | continuation is **explicit**, exactly-once, refused over `ambiguous` (ADR 0058) | an automatic attempt over an ambiguous external effect is the duplicate ADR 0057 forbids — open question below |
| Recovery "never refreshes budget" | interrupted operation charged its full reservation | Beacon's Envelope must charge the interrupted operation's worst-case reservation, or a crash refreshes budget by accident |
| Receipt "explains residual effects" | reconstruction v1/v2 digest-bound to the binding | reconstruction is read-only and grants nothing (ADR 0057) |

---

## Default-deny headless profiles ↔ Autonomy Profiles

| Beacon profile | Icarus mechanism | Note |
|---|---|---|
| Observe | read-only session tools; `propose` stops before any byte is materialized (exit 10); `read-only` Landlock profile | propose-only is the **default**, not a mode |
| Supervised | plan approval + review approval + landing decision + effect allowlist, all human, all digest-bound | Icarus's whole product today |
| Bounded | `mutation: "apply"` opt-in inside a digested profile; `apply-headless` is a second act bound to the **patch-set digest** (ADR 0060); children depth 1, sequential, cumulative envelopes (ADR 0059); doom-loop guard exits on the third identical tool call | Icarus has **no** standing pre-approval for a change class; Bounded is one |

Profiles are selection, never authority (ADR 0046): "Provider URLs, models, pricing, credentials,
grants, commands, approvals, and executable hooks have no field in the grammar." Resolution is
pure and refuses anything outside the project ceiling or approved plan.

---

## The machine guard, and what Beacon must do instead

`scripts/security-check.mjs:2001-2017` forbids, in runtime source, the words `Athena`, `Minerva`,
`constellation.event.v1`, any of `outbox|delivery|webhook|callback|queue|worker|receiver` in the
handoff core, and any table or identifier named `handoff|outbox|delivery`. It protects the
export-only boundary of ADR 0042: the handoff pack "ends at that filesystem boundary. It does not
deliver the artifact, contact Athena, or define the final … contract" (ADR 0042:36-37). Athena
built the other half (migration `0067_icarus_dispatch.sql`, HMAC `/callbacks/icarus`,
`dispatch_to_icarus`); nobody owns `constellation.event.v1` (cohesion handoff 2026-08-22).

Beacon inverts the premise: one codebase, so the names cannot be forbidden. The guard's
**property** still applies and must be re-expressed on capability, not on names (memory
`guards-keyed-on-self-declaration`): no module other than Execution imports the process, network
or repository-mutation primitives; nothing inbound to Execution is a command — a Mission, Claim,
Thread or Evaluation arrives as data and stops at a Gate; there is no callback, queue or receiver
that turns another module's record into an effect. A static assertion over the Rust module graph
is the direct descendant of `security-check.mjs`.

---

## Dead seams — tried and closed

- **`codex/packet4-fake-github-landing`** — HOLD 2026-08-25 (`docs/PLANS.md:195-215`): an in-core
  fake GitHub gateway and ~4,700 lines of pre-S2a landing persistence, superseded by
  `packages/github-gateway` + the coordinator chain; rebase conflicts at the first commit
  (7,582-line `landing-ledger.ts` divergence). Deterministic tests use the loopback HTTP seam
  instead. Branch kept as archival record.
- **ADR 0042 file export** as the Athena exchange — not dead, but a stop: the seam was never built
  past the filesystem, by design.
- **ADR 0056's `vulcan` exclusion** → superseded by ADR 0065; **ADR 0058's blanket session-tail
  refusal** → partially superseded by ADR 0063's read-only batch.
- **E2 bench spread flag** — landed today (`c919cd2`, "flags and does not correct": recomputing a
  "clean" mean was rejected as "the editorial move that makes a benchmark unfalsifiable").

---

## Process findings — what Receipt and landing verification must check

1. **Phantom merge** (Icarus #34/#42; #49 merged into #48's branch twelve seconds after #48 merged
   to `main`, both showing MERGED). "MERGED" is a claim about the PR's base, not the default branch.
2. **Post-merge push to a merged branch** (#53 → #71): two review-HOLD commits pushed after the
   merge never reached `main`; `BenchOutputSpreadV1` absent for five days. Detector:
   `git cherry origin/main <branch>` printing `+`. Re-run today: both `-`.
3. **Rebase-merge breaks ancestry checks**: PR #71's commits are not ancestors of `c919cd2`; content
   landed, SHAs did not. Verify by tree/content (`git diff origin/main <branch>` empty; `git grep`
   for the expected symbol), not by `merge-base --is-ancestor` alone.
4. **Decayed audit probe**: `~/ai-workspace/codex/reviews/icarus-pr45-remediation-audit-probe.mjs`
   (present, 2026-08-22) prints `reproduced: false` because ADR 0050 made `modelAdapter`/`budgets`
   mandatory, so its fixture dies at `assertLiveEvidenceProfileMatchesManifest` one line before the
   preflight and a bare catch swallows it. A green probe must be proven to still reach the code
   under test.
5. **Merge before verdict** (#38, #41, #45, #71): the review became a post-merge audit each time.
   **Post-merge branch trap**: `gh pr merge --delete-branch` leaves the worktree on `main`.
   **Unpushed ≠ unreachable**: measure reachability from any remote ref plus `patch-id`, not `@{u}`.

For Beacon: a Receipt that claims "landed" must record (a) the resulting revision, (b) that it is
reachable from the declared target ref, AND (c) that the expected content is present in that
tree — the three checks that catch the three failure shapes above. A remote's status badge is an
input, never evidence.

---

## Consequences for Beacon's Execution

1. Execution's admission function takes the Proposal **bytes**, hashes them, parses the same
   bytes; no `(object, digest)` pair anywhere in the authority path. Test: a parsed object where
   bytes are required is refused; one flipped byte invalidates; a reformat invalidates.
2. Credential preflight = the strictest consumer predicate, one predicate, tested against the real
   gateway constructors with a corpus that yields both verdicts.
3. Provider pins bind the origin through the same parser the gateway uses; local kinds are
   loopback-only in the record even where the gateway would accept remote.
4. The named external-effect set is in the Proposal digest, compared by exact ordered equality,
   and doubles as the per-Run stage order.
5. Irreversible external POSTs are guarded by a durable UNIQUE constraint in the store, plus a
   load-time check that refuses a store that lost it.
6. Every external request is admitted to the store before I/O and settled after; a lost response
   yields `ambiguous`, reconciled only by reads in a later explicit act.
7. The Ruling record is re-read from the store immediately before each effect; never cached.
8. Persist per Run: reached stage, per-effect admissions/settlements, cumulative spend and
   elapsed, the charged reservation of any interrupted operation — the in-memory ledger is a view.
9. Prove the uncooperative stop: SIGKILL a real Worker between an admitted operation and its
   result; require exactly one charged interruption, byte-identical repeated reconciliation, no
   extra provider request, and refusal of ordinary resume.
10. Recovery Attempt (ADR 0010@c59dde1) runs only after classification; refuses over `ambiguous`;
    charges, never refreshes.
11. Autonomy Profiles are selection: no field in a profile may carry a URL, model, price,
    credential, grant, command or approval. Apply is a second Ruling bound to proposed bytes.
12. A capability-keyed static assertion over the module graph replaces the name-keyed guard.
13. Frozen, copied authorization objects; consumers re-check and copy rather than retain.
14. A landed Receipt records revision + reachability + content presence.
15. A pinned commit instant, if ever needed, is one optional field on the landing record; the
    clock is never seamed.

---

## Open questions — Kevin's, not ours

1. **Canary target.** Which active repository, or a designed self-hosting boundary? Test/doc path
   exemptions were refused three times as "a separate design decision". Beacon's Alpha 1 bundled
   disposable repository (README.md:117-118@c59dde1) may be the answer Icarus never had.
2. **Is Execution's decider human or policy?** authority.md Q2. Icarus today: every mutation stops
   at a human (Kevin 2026-08-22), and it explicitly holds no standing pre-approval for a change
   class. Beacon's **Bounded** profile (ADR 0004@c59dde1, accepted) IS a standing pre-approval.
   Contradiction with an accepted ADR — recorded, not resolved.
3. **Automatic Recovery Attempt vs exactly-once continuation.** ADR 0010@c59dde1 (accepted) says
   one automatic attempt; ADR 0058 admits continuation only after read-based classification and
   never over `ambiguous`. Which wins when the failed operation was an external mutation?
4. **Lease expiry as proof.** ADR 0005/0016@c59dde1 (accepted) let expiry prove a Run's state;
   Icarus's record says only reads prove a remote effect. Does an expired lease close the Run or
   mark it `ambiguous`?
5. **The Athena↔Icarus adopter.** In one codebase the question becomes: may the Missions surface
   submit Proposals to Execution directly (the callback Icarus refused), and what replaces the
   export-only guarantee that made Icarus's evidence trustworthy?
6. **Readable-set binding.** Does Beacon bind what a Run may READ into the Proposal digest, with
   `null ≠ empty`? Icarus does; BLUEPRINT.md:439-441 does not list it.
7. **Budget persistence.** Journal replay (Icarus) or store-persisted per-Run ledger (this page's
   consequence 8)? The partial-case spend path under a mid-case crash was not verified here.
