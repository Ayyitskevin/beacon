> # ⚠ REFRAMED — the findings hold, the framing is superseded
>
> This page was written for a **federating read-only desk** that displayed seven other
> systems' approvals. Kevin's 2026-08-25 ruling replaced that with one unified codebase.
>
> **The findings survive and are load-bearing** — the seven mechanisms' shapes, the
> corrections to the prior review (Icarus binds wider than Janus *and* enforces; Janus
> deliberately does not), the four axes, the measured `unbound` case, and the
> operator-action table are the **specification of what the single authority mechanism
> must preserve.** They are cited that way in the data-model design.
>
> **The framing does not survive.** Wherever this page says "render", "display" or "the
> owning module", read it as a requirement on *our own* mechanism rather than a rule for
> showing someone else's. There is no owning module any more; we are it, which makes
> these obligations stronger rather than weaker.
>
> 1. `private-evidence/2026-08-25_mosaic-scope-ruling-unified-desktop-app.md` — the ruling
> 2. `private-evidence/2026-08-25_mosaic-datamodel-synthesis.md` — the current design

---
> ## Revision check — 2026-08-29
>
> The table under "How this page was produced" pins the trees as read on 2026-08-24. Five days
> later the findings still hold, but three of the pins moved and one line number with them:
>
> | Module | Was | Now | What moved |
> |---|---|---|---|
> | Icarus | `af41796` | `origin/main` @ `c919cd2` | `change-handoff-reader.ts:2740` → **`:2779`** (`if (expected !== planSha256) sourceInvalid();`, unchanged); `scripts/security-check.mjs:1829` → **`:2006`**. `authorizeLiveEvidenceRun` now has production callers (`live-evidence-executor.ts:186`, `:327`). Gate 1 closed 2026-08-23 (ADR 0055); Gate 2 open. |
> | Janus | `6b62110` | `6b62110` — **FROZEN 2026-08-29** (Janus PR #4) | No code change on `main`. PR #3 (pending) is a correctness fix: `doctor`, `decide` and `show` all flattened `verify_binding`'s `None` into silence, and `digest_of_live` lets a ruling be written with `bound_sha256 = NULL`. That is the `unbound` case this page measured, produced by a live path. |
> | Athena | deploy `eeb8ae3` | `origin/main` @ `8ddefc8`; **deploy still `eeb8ae3`** | Mechanism 3 unchanged. `main` is 30 commits ahead of production (migration 0080 in the gap); PR #402 (MWS-29) changed the desk's lease lanes, not approvals. |
> | Buzz | `78cbffe` | `78cbffe` (upstream +221) | Kind numbers unchanged (`kind.rs` diff empty). Upstream PR #6977 closed unmerged 2026-08-28. |
>
> Open question 5 below is **corrected**: ReplayDeck is not "the existing `fleet-eval` repo" —
> no such repo or tree exists anywhere, and `docs/BLUEPRINT.md` already says so. See
> [`docs/evaluations.md`](evaluations.md).
>
> This page now has six siblings, one per BLUEPRINT §3 surface, produced the same way:
> [`briefing.md`](briefing.md) · [`conversations.md`](conversations.md) ·
> [`knowledge.md`](knowledge.md) · [`models.md`](models.md) · [`evaluations.md`](evaluations.md) ·
> [`decisions.md`](decisions.md) · [`execution.md`](execution.md). `decisions.md` owns the Janus
> gate *model*; this page keeps the binding *mechanics*.

---

# Authority findings inherited by Beacon

**Status:** adopted for v0 · **Owner:** Lane A (contracts & doctrine) · **Last verified:** 2026-08-24

This page exists because six AI Town modules each have their own idea of what
"approved" means, and Mosaic is the first thing that will render them side by
side. Rendering them behind one affordance is how a reaction becomes an
approval by accident.

It is a doctrine page, so it states rules. It is also a claim about live code,
so every rule below cites the source that produced it and can be re-checked.

---

## How this page was produced

Everything here was re-derived from live code on 2026-08-24, not from the
prior review. That distinction matters: **the prior review's central factual
claim was wrong**, and it was wrong in the direction that would have made
Mosaic misrender the most safety-critical module in the fleet.

Trees read, with the revision each claim was checked at:

| Module | Tree | Revision | Note |
|---|---|---|---|
| Buzz | `/example/checkouts/buzz` | `78cbffe` | Block upstream OSS (`github.com/block/buzz.git`) |
| Buzz (2nd tree) | `/example/checkouts/buzz-daybreak` | `9b69a59fc` | Checked for divergence; **kind numbers identical** |
| Athena | `./deployment-example/athena` | `eeb8ae3` | Canonical. `GET /version` on the live service returns `eeb8ae36…`, so this checkout *is* production |
| Icarus | `/example/checkouts/icarus` | `origin/main` @ `af41796` | Repo-only, no daemon |
| Janus | `/example/checkouts/janus` | `6b62110` | |

`/example/checkouts/athena` is **stale** (`fc6210c`, 2026-07-09, on a feature
branch) and must not be read as Athena. So is `/example/checkouts/buzz/REPOS/athena`
(`67d6822`).

### Corrections to the prior review

The review at `private-evidence/2026-08-25_ai-town-professional-app-review-claude.md`
concluded that **only Janus binds to exact bytes and can be re-checked**, and
recommended that only Janus render as authority. Three findings overturn it:

1. **Icarus binds to bytes, over a wider surface than Janus, and enforces it.**
   `packages/core/src/policy.ts:136` defines `planApprovalDigest`, a digest
   over `schemaVersion`, `policyVersion`, `task`, `baseCommit`,
   `contextSha256`, `targets`, `provider`, `checks`, `sandbox`, `ceiling`,
   `plan`, and `readableManifestSha256`. It is re-derived and compared at
   `packages/core/src/change-handoff-reader.ts:2740` —
   `if (expected !== planSha256) sourceInvalid();`.
2. **The fields the review attributed to Icarus do not exist.** It named
   `approval_state` and `policy_digest`. `git grep policy_digest origin/main`
   returns nothing, on any path. The review named fields that aren't there and
   understated the machinery that is.
3. **Not every Janus binding is re-checkable.** `src/janus/core.py:141`
   `verify_binding` is tri-state, and `kind == "text"` returns
   `None, "inline text binding — nothing live to re-check"`. `None` means
   *cannot tell*, and the docstring is explicit that it "must never read as
   'fine'".

A fourth correction, to the framing rather than the facts: **there are not
five mechanisms.** Icarus alone has two digest-bound approval families
(`planApprovalDigest`, `liveEvidenceProfileApprovalDigest`) and five distinct
waiting states (`awaiting_approval`, `awaiting_decision`,
`awaiting_egress_approval`, `awaiting_human`, `awaiting_review`). The count is
not the point, and any page that leads with a count will be wrong again within
a month.

---

## The mechanisms

| # | Mechanism | Binds to | What it unblocks | Revoked / spent by | Re-checkable? |
|---|---|---|---|---|---|
| 1 | **Buzz** `KIND_APPROVAL_GRANT` 46030 / `DENY` 46031 | SHA-256 of an approval **token** (64-hex `d` tag) — a capability handle, not the action's content | A pending workflow approval record | `expires_at`; single-use (status leaves `Pending`); `approver_spec` restricts who may grant | **No** — the token names a record, not the bytes of what was approved |
| 2 | **Buzz** workflow approval 46010/46011/46012 | `(community_id, workflow_id, run_id, step_id, step_index)` | One waiting step of one workflow run | `expires_at`; status transition | **No** — no digest of the step's payload is recorded |
| 3 | **Athena** `approval_requests` | `(action_kind, target_kind, target_id)` — an **intent against a target**, explicitly *not* a payload | One retry of one gated write by one requester | `consumed_at` (single-use, in the retry's own transaction); `rejected` state | **No, by design** — see below |
| 4 | **Icarus** plan approval | `planApprovalDigest` over plan **and execution environment** (provider, checks, sandbox, ceiling, base commit, readable manifest) | An agent execution run against a pinned base commit | Digest mismatch invalidates the source outright | **Yes, and enforced** — mismatch calls `sourceInvalid()` |
| 5 | **Icarus** live-evidence profile approval | `liveEvidenceProfileApprovalDigest` | A live-evidence profile | As above | **Yes** |
| 6 | **Janus** ruling | One artifact: `file` (absolute path + content digest), `git` (repo + **pinned commit**), or `text` (inline) | Nothing mechanically — Janus records that a human ruled | `expired`, `withdrawn`, `superseded`; drift voids it advisorily | **`file`/`git`: yes, but reported not enforced. `text`: never** |

### Three of these are load-bearing subtleties

**Athena's non-binding is a deliberate safety property, not a gap.** Migration
`0063_approval_requests.sql` says it in its own words: *"Athena never serializes
an agent's mutation and replays it later on the agent's behalf: doing so would
re-execute a stale payload under authorization evaluated at a different moment,
which is a new trust boundary rather than a feature."* The operator approves an
intent; the agent's retry supplies the actual change and re-validates
everything. So Athena binds *less* on purpose, and Mosaic must not render that
as weakness.

**Janus deliberately does not enforce.** `verify_binding`'s docstring: *"Janus
states the drift and stops there, because enforcing it would put Janus in the
permission path."* Icarus enforces; Janus reports. This is the opposite of the
strength ordering the prior review implied.

**Buzz has real revocation semantics.** The review's table recorded "No" across
Buzz and hid `expires_at` and `approver_spec` entirely. A Buzz grant is
time-limited and approver-restricted even though it does not bind content.

---

## The rule

The prior review ranked authority on *binds to bytes*. That axis is now known
to be wrong: it puts Icarus and Janus in the same bucket while separating
Athena from Buzz for a reason that turns out to be a deliberate design choice
in Athena's favor.

> **Mosaic ranks nothing. It renders two facts per approval and refuses to
> summarize them: _who decided_, and _what the decision is attached to_.**

Those two are orthogonal, and collapsing either one is the failure mode this
page exists to prevent.

### How this composes with the roadmap's mandated vocabulary

The ORACLE roadmap fixes a five-value display vocabulary and calls it a hard
boundary: the UI *"must distinguish proposed, reviewed, Janus-authorized,
executed, and verified; it must never collapse them into a generic 'approved.'"*

**That vocabulary governs approvals, not every entry.** It is an approval
lifecycle, and only entries that carry an approval are described by it. An
entry's own `status` field carries the owning module's word verbatim — Janus
`superseded`, Athena `in_progress` — and is never mapped onto this list.

Forcing it everywhere collapses exactly what this page forbids. Janus's three
terminal-negative states (`withdrawn`, `expired`, `superseded`) have no home
in a five-value approval enum, so all three land on `proposed`, and **4 of 13
live gates then render identically to a live open gate**. A question nobody
answered and a question the world moved past are different facts. An Athena
issue is not an approval at all: its `done` either understates the work or
asserts a check nobody ran. Found by Lane B against live data.

That vocabulary is a **lifecycle** — where a thing is in its journey. The two
axes above are **provenance** — who decided and what the decision is pinned to.
They are orthogonal and both are needed: `executed` says nothing about who
authorized it, and `human` + `bytes-verified` says nothing about whether it has
run yet. Mosaic carries both, and neither substitutes for the other.

One value in that list does not survive this page's findings, and it is not
mine to change:

> **`janus-authorized` encodes the claim this page disproved.** It names one
> module as *the* authorizing mechanism, and Icarus authorizes with a strictly
> wider byte-binding that it also enforces (Janus deliberately does not
> enforce). Rendering an Icarus plan approval as anything other than authorized
> would understate the strongest guarantee in the fleet; rendering it as
> `janus-authorized` would be false.

Recommended: rename to `authorized`, and carry the deciding module in the
`decider` field where it already belongs. **This is a Kevin decision, not a
lane decision** — the roadmap calls these boundaries his, and the handoff
agrees. It is raised as a Janus gate rather than assumed. Until he rules,
adapters emit the lifecycle value `authorized` with `decider` populated, and
the renderer may display `janus-authorized` for Janus-decided entries without
any code depending on that string.

**Who decided** — the deciding party, not the mechanism's importance:

- `human` — a person ruled (Janus ruling; an Athena operator decision; a Buzz
  grant signed by a human pubkey).
- `policy` — a rule evaluated without a person in the loop at decision time.
- `agent` — an agent proposed or self-attested. **Never renders as an
  approval, whatever it is attached to** — `assures()` returns False for
  `agent` before it looks at anything else. An agent verifying content it
  computed itself is the highest-consequence case in the model, and for one
  commit this was a docstring promise the code did not keep. No live producer
  emits `agent` as of 2026-08-24; the value exists so an adapter has a correct
  thing to reach for instead of reaching for `human`.

**What it is attached to** — how tightly, and what checking it just produced:

- `bytes-verified` — binds a content digest, and Mosaic **re-derived it just
  now** and it matched.
- `bytes-drifted` — binds a content digest, re-derived, and it **did not
  match**. This is a finding, and must be visually louder than an approval.
- `bytes-unverifiable` — binds bytes that **can never be re-checked by
  anyone**. A permanent property of the binding, not of this run: a Janus
  `text`-kind binding has nothing live to re-check, and no amount of
  connectivity changes that. The bytes are at least *shown*, so a reader can
  judge them.
- `bytes-unchecked` — binds bytes Mosaic **did not check this run** (module
  unreachable, repo absent, offline). Transient: re-running may resolve it to
  verified or drifted.
- `unbound` — attached to **nothing at all**. No artifact to show, nothing to
  re-check against, ever.
- `intent` — binds an action against a target, not content (Athena).
- `handle` — binds a token or record id (Buzz 46030).
- `step` — binds a position in a run (Buzz 46011).

The last two bytes-* values are split because **"cannot ever be checked" and
"was not checked" are different facts that lead to different operator
actions** — one is a permanent limitation to accept, the other is an errand.
Rendering them alike would rebuild the collapse in miniature. Neither may
render as verified: `None` must never read as "fine".

**Who acts when the binding fails** — the operator-action axis:

- `enforced` — the owning module refuses to proceed. Icarus re-derives
  `planApprovalDigest` on read and calls `sourceInvalid()`.
- `reported` — the module states the drift and stops there, deliberately.
  Janus: *"enforcing it would put Janus in the permission path."*
- `none` — the mechanism binds no content, so nothing can drift. Its safety
  comes from elsewhere: single-use consumption, expiry, an approver check.

### What an operator actually does differently

This is the question the three axes exist to answer, and the one a renderer
must make visible. If every mechanism reads as "authorized" with a different
footnote, the collapse has been rebuilt in a subtler form.

`Authority.operator_action()` returns one of three values — `act`, `decide`,
`none` — and is three-valued rather than boolean on purpose. A live breach and
a standing limitation are both human work, and they are not the same work: one
is contained by acting, the other is closed by deciding. A boolean forces one
of them to be silent, and the first version of this table found out which one.

| Situation | What is true | Operator does | |
|---|---|---|---|
| `enforced` + `bytes-verified` | The module checked and proceeded | Nothing. Strongest state in the system | `none` |
| `enforced` + `bytes-drifted` | **The module already refused.** Execution stopped | Investigate at leisure. Nothing is at risk *because* it was enforced | `none` |
| `reported` + `bytes-verified` | Checked and holds, but nothing would stop it if it stopped holding | Nothing now. Re-check before relying on it later | `none` |
| `reported` + `bytes-drifted` | **Nothing is stopping anyone.** The ruling is void and the system does not know it | **Contain it.** Raise a new gate | `act` |
| any + `unbound` | **Attached to nothing.** No artifact was ever pinned, so no check can ever be run | **Decide.** Accept it as resting on the raiser's word, or re-raise with a checkable binding | `decide` |
| `reported` + `bytes-unverifiable` | Binds bytes nobody can re-derive, though it does show them | Same decision, with more to go on than `unbound` gives you | `decide` |
| any + `bytes-unchecked` | Mosaic could not look **this run** | Restore reachability and re-run. `beacon-legacy-desk doctor` owns this surface | `none` |
| `none` + `intent` / `handle` / `step` | Nothing binds content, by design | Read the module's own `meaning`. Safety lives in expiry, single-use, approver checks — not in a digest | `none` |

Two rows carry the weight. **`reported` + `bytes-drifted` is the only state
where the system is quietly wrong and a person must contain it** — and it is,
counter-intuitively, produced by Janus, the mechanism the prior review called
the strongest. And **`unbound` is the only state that can never improve on its
own**, which is why it demands a decision rather than a glance.

`bytes-unchecked` maps to `none` deliberately. It is a fact about this run
rather than about the decision, and with 2 of 7 product-trace hops joinable
today, flagging every unchecked entry would drown the two values that mean
something. It still fails `assures()`, so it keeps its warning affordance.

**This table shipped without an `unbound` row**, and the code faithfully
implemented the table: `operator_action()`'s boolean ancestor answered "no
action" for the four live unbound-and-approved gates this page is built
around. The weakest state in the ledger was also the quietest one. Caught in
review by Lane C, by executing the predicate rather than reading it.

### The case that proves it, measured

Lane B counted the binding kinds across all 13 live Janus gates on 2026-08-24:

| `binding_kind` | Count |
|---|---|
| `git` | 8 |
| **`null` (none)** | **4** |
| `file` | 1 |
| `text` | 0 |

Two findings, and the second is the one this page exists for:

1. **`text` has never been used.** The tri-state "cannot tell" analysis was
   built around `verify_binding` returning `None` for `text` bindings. That
   path is real in the code and has zero population. Worth encoding, but it is
   not the live problem.
2. **Four gates are bound to nothing, and all four are `approved`.**
   `g4deae14ca3d` and `g6b5df553432` among them. A ruling bound to nothing can
   never be re-checked against anything, by anyone — it is strictly weaker
   than a `text` binding, which can at least be *shown* to a reader who wants
   to judge it. And today it renders identically to the eight `git`-bound
   rulings sitting beside it in the same list.

That is the collapse, already present, in the module the prior review named as
the sole trustworthy one. It is not a hypothetical about a future shell
wiring a reaction to an approval. **Four approved rulings in the live ledger
carry no re-checkable commitment at all, and nothing on screen says so.**

Hence `unbound` as a distinct label, and hence the refusal to let any single
value stand in for "approved".

### Consequences

1. **No cross-module "Approved ✓" affordance exists in v0.** Not styled
   differently — *absent*. There is no widget whose job is to mean "approved".
2. **Every approval entry renders its owner's badge**, and the module name is
   never elided. "Approved" alone is not a renderable string in Mosaic.
3. **Every approval entry carries a `meaning` string supplied by its adapter**,
   in the module's own words, not Mosaic's paraphrase. This convention is
   lifted from Athena's desk, which already does exactly this per lane
   (`src/athena/aegis/desk.py`) — e.g. `"asked of you and not yet answered"`.
   Mosaic is not entitled to restate another module's semantics more briefly
   than the module does.
4. **`bytes-drifted` outranks the approval it belongs to.** A ruling whose
   artifact changed is a warning surface, never a green one. Janus's own words:
   *"A ruling approves specific bytes; it does not follow them. Treat any ruling
   on this gate as void."*
5. **Zero cross-module write controls in v0.** Not even a disabled one.

### Ordering by operator action is allowed. Ordering by authority is not.

"Mosaic ranks nothing" bans one specific thing: **ordering that correlates
with how much authority an entry carries.** It does not ban ordering.

A view may sort by `OperatorAction` — `act`, then `decide`, then `none` —
because that answers *what does this reader have to do*, which is a different
question from *which of these approvals is strongest*. The test is whether the
ordering tracks authority strength, and this one demonstrably does not; it
runs against it:

- `reported` + `bytes-drifted` sorts **first** (`act`). That is a Janus
  ruling — the mechanism the prior review called the strongest — with a broken
  binding.
- `enforced` + `bytes-drifted` sorts **last** (`none`). That is Icarus, the
  widest byte-binding in the fleet, having correctly refused.

An authority ranking would order those two the other way round. A worklist
puts the thing nobody is containing above the thing the system already
stopped. That inversion is the proof it is not the banned ranking sneaking
back in through sort order.

Three conditions on any such view:

1. **The document itself keeps its own order.** `compose()` sorts by recency
   and is the record. A worklist is a *view* of the document, never a
   re-ordering of it — `tests/contracts/test_compose.py` pins this.
2. **Order within a bucket by recency,** so no secondary sort can smuggle an
   authority ordering into the tiebreak.
3. **Name the ordering for what it is.** "Needs you first" is honest;
   "most important first" or "highest priority" is the collapse in a sort key,
   because a reader who learns that top means important will conclude that top
   means most trustworthy.

The same reasoning bans a summary count. `renders_as_authority: 1` is not an
ordering at all — it is the binary verdict this page rejects, stated as a
number.

---

## Open questions — Kevin's, not ours

1. **Does `human` + `bytes-verified` outrank `human` + `intent`?** This page
   says no — it renders both and ranks neither. Someone will eventually want a
   single "is this safe" indicator, and that is a doctrine decision, not a UI
   one. Recommend raising a Janus gate before anyone builds it.
2. **Is an Icarus plan approval a human decision or a policy evaluation?**
   Kevin's 2026-08-22 posture is that agents get read + propose only on Icarus
   and every mutation stops at a human approval. If that holds, Icarus plan
   approvals are `human` + `bytes-verified` — the strongest pair Mosaic can
   render. This should be confirmed rather than assumed, because it is the
   single highest-confidence label in the system.
3. **Buzz is upstream OSS on someone else's roadmap.** Kinds 46030/46011 can
   change without consulting AI Town. The Buzz adapter must record the kind
   number it observed, so a future divergence surfaces as a change rather than
   a silent relabel.
4. **Rename `janus-authorized` → `authorized` + `decider`?** Per the section
   above. This is the one mandated boundary this page recommends changing, and
   the recommendation is deliberately not self-executing.
5. **AI Town is seven capabilities, not six.** The roadmap names Buzz, Athena,
   Minerva, Vulcan, **ReplayDeck**, Janus, and Icarus, and puts ReplayDeck in
   the product trace at the evaluation step. ~~ReplayDeck is the existing
   `fleet-eval` repo (roadmap line 202; owner Kimi, reviewer Codex).~~
   **Corrected 2026-08-29:** there is no `fleet-eval` repo or tree; ReplayDeck
   names an intent, and the evaluation practice that exists lives in Icarus —
   see [`docs/evaluations.md`](evaluations.md). Both the
   Mosaic lane plan and every handoff in this thread describe six modules and
   omit it. Not blocking for v0 — the first slice needs two adapters, not seven
   — but the composition engine must not hardcode a six-module world, and the
   omission should be corrected before anyone writes "all modules".


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
