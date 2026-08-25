# Authority in Mosaic

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
- `agent` — an agent proposed or self-attested. Never rendered as approval.

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

| Situation | What is true | What the operator does |
|---|---|---|
| `enforced` + `bytes-verified` | The module checked and proceeded | Nothing. Strongest state in the system |
| `enforced` + `bytes-drifted` | **The module already refused.** Execution stopped | Investigate at leisure. Nothing is at risk *because* it was enforced |
| `reported` + `bytes-verified` | Checked and holds, but nothing would stop it if it stopped holding | Nothing now. Re-check before relying on it later |
| `reported` + `bytes-drifted` | **Nothing is stopping anyone.** The ruling is void and the system does not know it | **Act.** Raise a new gate. This is the only combination that demands a human |
| `reported` + `bytes-unverifiable` | Cannot ever be confirmed | Accept it as resting on the raiser's word, or re-raise with a checkable binding |
| any + `bytes-unchecked` | Mosaic could not look | Restore reachability and re-run. `mosaic doctor` names what was missing |
| `none` + `intent` / `handle` / `step` | Nothing binds content, by design | Read the module's own `meaning`. Safety lives in expiry, single-use, approver checks — not in a digest |

The row that matters is the fourth. **`reported` + `bytes-drifted` is the only
state where the system is quietly wrong and only a person can fix it** — and
it is, counter-intuitively, produced by Janus, the mechanism the prior review
called the strongest. That inversion is the entire reason this page rejects
ranking.

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
   the product trace at the evaluation step. ReplayDeck is the existing
   `fleet-eval` repo (roadmap line 202; owner Kimi, reviewer Codex). Both the
   Mosaic lane plan and every handoff in this thread describe six modules and
   omit it. Not blocking for v0 — the first slice needs two adapters, not seven
   — but the composition engine must not hardcode a six-module world, and the
   omission should be corrected before anyone writes "all modules".


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
