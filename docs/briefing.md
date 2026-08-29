# Briefing findings inherited by Beacon

**Status:** draft for review · **Produced by:** claude-code harvest agent · **Last verified:** 2026-08-29

BLUEPRINT §3 says Briefing is "descended from Athena" (`docs/BLUEPRINT.md:170@c59dde1`). Athena's
desk, lease, delegation and attention code is where the fleet learned, by being wrong, what an
orientation read may and may not claim. This page re-derives those lessons from the code at a
pinned revision and from the live service, so Beacon's Briefing, Agent desk packet, Assignment
model and Attention Items inherit findings rather than taste. It states findings, not decisions.

---

## How this page was produced

Everything below was read on 2026-08-29 with no checkout, switch, pull or write in any tree.

| Source | Tree | Revision | Read with |
|---|---|---|---|
| Athena code + docs | `~/ai-workspace/athena-claude` | `origin/main` = **8ddefc8** | `git -C ~/ai-workspace/athena-claude show origin/main:<path> \| cat -n` · `git -C … grep -n <pat> origin/main -- <paths>` |
| Athena LIVE deploy | `~/deploy/athena` | **eeb8ae3** (30 commits behind; MWS-15/16/17/18/29 and migration 0080 in the gap) | `GET /version` → `"commit":"eeb8ae36…"`; `git -C ~/ai-workspace/athena-claude log --oneline eeb8ae3..origin/main` |
| Beacon | `~/ai-workspace/claude/beacon` | **c59dde1** | `README.md`, `CONTEXT.md`, `AGENTS.md`, `docs/BLUEPRINT.md`, `docs/authority.md`, ADRs 0001/0004/0005/0006/0010/0023 |
| Athena board | live `:8300` | — | `GET /issues/{48,60,92,34,36}`, `GET /issues?limit=100&include_closed=1` |
| Prior claims | memories `athena-workspace`, `athena-hardening-run`, `buzz-athena-integration`; handoffs `2026-08-22_ai-town-cohesion-findings-claude`, `2026-08-29_athena-mws29-…-plan-claude`, `2026-08-29_athena-pr402-…-review-request-claude`, `2026-08-21_buzz-athena-integration-analysis`, `2026-08-22_minerva-athena-seam-research-claude` | — | read as claims (R5), checked against the pin |
| PR #402 | GitHub | — | `gh pr view 402 --json state,mergedAt,mergeCommit` |

Every citation below is `path:line@8ddefc8` unless another pin is named. Where a finding exists
only on `main` and not in the live deploy, the row says so.

Live probes (all HTTP GET, bearer = the Claude seat's member-role token from
`~/ai-workspace/claude/.secrets/athena-token`; `$ATHENA_URL` = `http://100.125.80.91:8300`):

```bash
curl -sS "$ATHENA_URL/version"                                   # eeb8ae36…, tree_state clean
curl -sS -H "Authorization: Bearer $ATHENA_API_TOKEN" "$ATHENA_URL/desk"
curl -sS -H "Authorization: Bearer $ATHENA_API_TOKEN" "$ATHENA_URL/issues/36/lease"
curl -sS -H "Authorization: Bearer $ATHENA_API_TOKEN" "$ATHENA_URL/openapi.json"
curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $ATHENA_API_TOKEN" \
  "$ATHENA_URL/fleet/active-work"                                # 403 (member token)
curl -sS -o /dev/null -w '%{http_code}' "$ATHENA_URL/issues"      # 200 anonymous
curl -sS -o /dev/null -w '%{http_code}' "$ATHENA_URL/desk"        # 401 anonymous
```

Observed values are in "The measured case" below.

---

## What the Blueprint already specifies

Not restated here; cited so the findings can be read against it.

- Briefing shows portfolio, Missions, Team health, spend, blocked work, receipts, Attention Items,
  and "optimizes for exceptions, not activity volume" — `docs/BLUEPRINT.md:168-172@c59dde1`.
- Two sections, **Since your last visit** and **Needs attention now**; refresh and opening the
  Briefing never advance the acknowledged cursor; only **Mark caught up** does —
  `docs/BLUEPRINT.md:281-287`, invariant `AGENTS.md:99`.
- The Agent desk packet: identity, Assignment, Mission, Capabilities, messages, evidence,
  blockers and the allowed commands; "a projection, not a prompt transcript"; versioned and
  re-issuable — `docs/BLUEPRINT.md:319-330`.
- Attention semantics: an Attention Item exists only when a condition is unresolved, a named actor
  is eligible, and the concrete next action can be stated; routine success, background activity,
  enforced refusals and not-yet-run checks stay in Activity — `docs/BLUEPRINT.md:345-365`,
  `AGENTS.md:74`.
- Failure recovery: one Recovery Attempt, then reassignment with a Handoff or an Attention Item —
  `docs/BLUEPRINT.md:367-373`, ADR 0010.
- Persistence rules that these findings lean on: meaningful nulls forbidden (`AGENTS.md:198`);
  clock-derived verdicts computed at read time (`AGENTS.md:204`); terminal, absent, broken and
  not-evaluated are distinct (`AGENTS.md:146`).

### Vocabulary map — Athena word → Beacon word

| Athena | Beacon | Note |
|---|---|---|
| issue | the unit of work an Assignment is about | Beacon has no ticket object (`CONTEXT.md:141-145`: avoid "Task, ticket, job"); an Athena issue is smaller than a Mission |
| lease (possession, `issue_leases`) | **Assignment** (provisional — see open question 1) | `CONTEXT.md:147-149` says Assignment *avoids* "Lease"; the Blueprint's "lease" is the Worker Run lease (`docs/BLUEPRINT.md:555-566`), a different object |
| delegation (`issue_contributors`, the eligible set) | Assignment's eligibility half | Athena splits eligible / accountable / holding into three relations (`leases.py:4-7`) |
| claim handoff (typed yield packet) | **Handoff** | `CONTEXT.md:151-153`; Athena's fields are the concrete shape (`lease_commands.py:347-366`) |
| desk (`GET /desk`) | **Agent desk packet** | `docs/BLUEPRINT.md:319-330` |
| desk cursor (`POST /desk/cursor`) | Briefing cursor, **Mark caught up** | `docs/BLUEPRINT.md:284-287` |
| fleet-attention rollup + ranked "Now" queue | **Attention Items** / Attention drawer | `docs/BLUEPRINT.md:275, 345-365`; the queue is main-only (MWS-16, in the deploy gap) |
| Mission Control / admin dashboard | **Briefing** | Athena's is admin-only; the desk is per-reader |
| run controls, worker kill requests | Pause Work + bounded interruption request | `docs/BLUEPRINT.md:377-381`; Athena records the ask and never signals a process |
| "claim" / "mission" homonyms | — | `knowledge.md` owns the homonym finding; Athena's "mission" is a display string, not a table |

---

## Corrections to prior claims

1. **PR #402 is merged, and the pin IS the fix.** The review-request handoff
   (`2026-08-29_athena-pr402-mws29-review-request-claude.md`) records the PR as "OPEN, MERGEABLE,
   not merged". `gh pr view 402` returns `state: MERGED`, `mergedAt: 2026-08-29T14:32:07Z`,
   `mergeCommit: 8ddefc8…`. Every desk citation below is therefore post-MWS-29 code; the live
   deploy (eeb8ae3) still runs the pre-fix desk (`schema: athena.agent_desk.v1` observed live).
2. **The fleet boot-block copies are stale against main.** This node's `~/.claude/CLAUDE.md:770`
   says "expired leases linger on the desk"; the canonical block at
   `docs/AGENT_BOOT.md:67-71@8ddefc8` says a lapsed claim "lists under `work.leases.lapsed` until
   you `complete_claim` it with its generation". The copy describes the live deploy, the
   canonical text describes main; `AGENT_BOOT.md:98-102` exists precisely because copies drift.
3. **"Closing anonymous reads breaks `/readyz`" did not reproduce.** Memory `athena-hardening-run`
   still says `ATHENA_ANONYMOUS_READS=0` returns 401 on `/readyz` and breaks the uptime check.
   `src/athena/main.py:182-187` records that `/readyz` is registered with no browser dependency,
   the gate never sees it, and "an earlier report that the flip closes /readyz did not reproduce"
   (pinned by `tests/test_anonymous_reads_closed.py`). Memory `athena-workspace` already carries the
   correction; the hardening memory does not.
4. **MWS-29's second lane was not a bug.** Issue #60 filed "a completed delegation stays in the
   inbox" as a defect. `complete_claim` releases the lease only (`lease_commands.py:514-516`) and
   the inbox filters on the issue's status category (`delegations.py:103-106`); the inbox item
   already says so (`delegations.py:319-323`). Closed by one field-guide sentence
   (`src/athena/field_guide/01_your_desk.md:24-26`), not by code.
5. **The desk defect is still live.** The plan handoff describes four lapsed rows as a reproduced
   defect; it remains reproducible today on `GET /desk` (below), because the fix is on `main` and
   the deploy has not moved. Bringing the deploy forward is a separate RED action (review-request
   handoff, last paragraph).

---

## The invariants

| # | Invariant | Where it lives (`@8ddefc8`) | The incident or reasoning that produced it | What Beacon must preserve |
|---|---|---|---|---|
| 1 | **The desk composes; it does not compute.** Every lane is the owning reader's own output with this actor's visibility applied; the desk is never a second source of truth | `src/athena/aegis/desk.py:14-19`; `docs/DESK.md:49-54`; `fleet_attention.py:10-12` | Orientation used to take five or six reads with different bounds and no "what changed" (`desk.py:3-8`). An aggregate that re-derives state "eventually lies" (`docs/EXCEPTION_SURFACES.md:28-31`) | Briefing and the Agent desk packet are projections over the Work module's own queries (`docs/BLUEPRINT.md:329`, `AGENTS.md:88`); a lane that disagrees with its owning query is a bug |
| 2 | **Every lane carries a `meaning` string in the owner's words** | `desk.py:208, 212-215, 221, 229-233, 252-263` | Lifted into `docs/authority.md` consequence 3 already; the desk is its origin | Each Briefing/desk lane says what its items are, in the owning module's words, not the UI's paraphrase |
| 3 | **Every lane is bounded, and a clipped list says so.** Limits are constants; totals are counted against the same instant as the items; the change counter caps at 500 and reports `capped` | `desk.py:51-58, 60-64, 89-91, 205, 241`; `leases.py:179-182`; `fleet_work.py:281-286`; `fleet_attention.py:114-127` | "0 need attention" was ambiguous between "none do" and "none of the ones we looked at do" (`fleet_work.py:282-285`); `examined_count` was added for it | Every count in Briefing names its scope and window; a bounded read reports `examined`, `total`, `clipped` |
| 4 | **`null` is not `0`.** A reader that never acknowledged a cursor has cursor `null` and `events_since_cursor` `null`, never zero | `desk.py:67-72, 273-275`; `src/athena/core/desk_cursors.py:22-27`; `docs/DESK.md:58-61`; `tests/test_desk.py:93` | A zero would tell a fresh agent it was caught up on a trail it never opened | **Since your last visit** on a first visit is "never acknowledged", not "nothing new" (`AGENTS.md:198`) |
| 5 | **The cursor moves only on explicit acknowledgement, only forward, and records no activity event.** Re-ack of the same id is a no-op; a lower id is 409; the database trigger refuses it again for any writer that skips the command | `src/athena/core/migrations/0073_agent_cursors.sql:10-24, 43-48`; `src/athena/aegis/desk_commands.py:60-102`; `docs/DESK.md:69-78`; `tests/test_desk.py:473-503, 633` | "Unsaying an acknowledgement would be a claim about history, not a position" (`0073:16-19`); a read receipt is personal state, not fleet history (`0073:10-14`) | This *is* **Mark caught up** (`docs/BLUEPRINT.md:284-287`, `AGENTS.md:99`): one explicit Operator command writes the cursor; it is monotonic; it is not an Activity event |
| 6 | **Reading never moves the cursor.** `build_desk` reads the cursor (`desk.py:178-180`); the only write is `advance_cursor` | `desk.py:178-180`; `desk_commands.py:3-4, 60-66` | The desk is "the open your inbox habit" (`desk.py:10-12`); a habit that acknowledged on read would erase what it was for | Background refresh and opening the Briefing must be pure reads (`docs/BLUEPRINT.md:285-286`) |
| 7 | **Lease `active` is the clock's verdict at read time, never a stored flag; nothing sweeps expired rows** | `src/athena/aegis/leases.py:3-9, 44-49, 55-60`; `docs/DESK.md:94-96`; `field_guide/02_claiming_work.md:20-23` | A crashed holder's lease simply expires and the work becomes reclaimable with no background sweeper (`leases.py:7-9`) | Assignment "held" is computed at read time (`AGENTS.md:204`); no reaper process defines truth |
| 8 | **Every lease mutation is fenced by an opaque generation.** Renewal must supply the current generation; an omitted token can never turn a delayed acquire into a renewal; a stale generation is refused, not applied | `leases.py:253-266, 293-312`; `lease_commands.py:104-112, 215-233`; `field_guide/02:41-49` | "The fence that stops a paused-then-resumed agent from acting on a world that moved" (`02:46-47`) | An Assignment carries a generation; Handoff resume, complete, yield all name it |
| 9 | **"Held" must be true in aggregate, not just per row.** Items and `total` are active-only; lapsed rows on still-open issues are a separate list with a count taken *after* the filter; lapsed rows on done issues are omitted but stay readable at the owning surface | `desk.py:159-168, 244-264`; `leases.py:146-170, 173-190, 193-240`; `CHANGELOG.md:113-134` | **MWS-29 / PR #402**: `active` was honest per row and false in aggregate; four lapsed rows counted as "held" with no command to clear them (`CHANGELOG.md:113-123`) | Active and lapsed Assignments are structurally distinct lanes (`AGENTS.md:146`); a total never counts rows its list can never show |
| 10 | **Clearing a lapsed row is the holder's, and only the holder's.** Complete now clears the caller's OWN lapsed lease under the fence; an expired row belonging to someone else is "nobody's to complete"; the admin release lever stays ACTIVE-only | `lease_commands.py:508-516, 522-533` | Pre-#402, `complete` refused an expired own lease with 409 — a refusal whose remedy was unreachable (`lease_commands.py:546-549` records the same reasoning for the handoff gate) | Every refusal on an Assignment names a reachable remedy; widening who may delete a possession is a reviewed change, never a convenience |
| 11 | **Four verbs, four meanings.** `complete` releases the lease only; `update status=done` finishes the work; `yield` records honest non-completion with a typed handoff; `decline` refuses the delegation. The delegation inbox drops an item on status category, never on lease release | `field_guide/02:24-39`; `lease_commands.py:1-9, 360-366, 591-596`; `delegations.py:103-106, 319-323`; `desk.py:225-233` | "Silence is the only genuinely unhelpful outcome: an issue you hold and stop working on… looks like progress" (`02:37-39`) | Assignment release, Mission/work completion, yield-with-Handoff and decline are distinct commands with distinct events; the inbox keys on the work's state |
| 12 | **Holding, eligible and accountable are three relations.** The lease is not the assignee and not the contributor set; the claimant gate is narrower than the issue-write gate (a creator may not claim) | `leases.py:3-7`; `lease_commands.py:57-83` | "Creating an issue is not the same as being handed it to work" (`lease_commands.py:76-77`) | `CONTEXT.md:147-149`: Assignment is "the accountable relationship" — Beacon must decide which of the three it names (open question 1) |
| 13 | **A claim requires the exact current revision of the work.** `If-Match` with one strong ETag or 428; read-then-write under `BEGIN IMMEDIATE` so racing claimants serialize | `lease_commands.py:47-49, 199-201, 234-240`; `field_guide/02:9-12` | Two agents must not claim the same snapshot | Assignment creation binds the Mission/work revision it was taken against |
| 14 | **The inbox is a projection that asserts no readiness.** No "ready", "running" or "unblocked" claims; a missing visible blocker does not prove none exists; urgent-first then oldest-first so old work cannot be buried; fail closed on a visibility mismatch | `delegations.py:1-11, 62-66, 159-164, 176-178` | — | The desk packet's blockers are "visible to this actor", stated as such |
| 15 | **Attention is decided in exactly one place; SQL only biases the window.** The ordering predicate fills the bounded window from the attention-prone end and deliberately does not reproduce the decision | `fleet_work.py:40-59, 251-261`; `docs/ACTIVE_WORK.md:57-73` | The previous order put active claims first, so on a busy fleet the expired (attention-bearing) rows were exactly the ones the limit dropped while the summary read "0 need attention" (`fleet_work.py:45-50`) | One predicate defines an Attention Item; a bounded query is ordered so the urgent end is never the clipped end |
| 16 | **Attention is a closed vocabulary of observed signals with a named owner and a labeled next action; ranking is severity-then-oldest, never a priority integer.** `next_action` is `agent-command` only when the caller can actually run it; degraded rows become operator links | `fleet_work.py:329-355, 399-401`; `fleet_attention.py:212-248, 251-260, 316-361, 612-617, 620-647` (main-only: MWS-16) | Athena has an issue `priority` field and orders the inbox by it (`delegations.py:42-49`), but the attention queue ranks by signal severity and age, and `_agent_command_authorized` refuses to label a command the caller cannot execute | Maps onto the three Attention conditions: `attention_reasons` non-empty (unresolved), `owner_id` (named actor), `next_action ≠ none` (concrete action). A `none` row fails condition 3 and is Activity, not attention |
| 17 | **The rollup computes nothing, and every count says what it counts and over what window.** Standing state is unwindowed; event-counted signals are windowed; a quiet fleet "says so, carefully" | `fleet_attention.py:10-18, 44-46, 99-112, 114-127`; `docs/EXCEPTION_SURFACES.md:33-44` | "Someone probed the boundary once, months ago" is not the alarm "someone is probing now" is | Briefing counts carry scope lines; "nothing needs you" is a statement about what was recorded, never a health claim |
| 18 | **Nothing on the desk asserts liveness.** A heartbeat proves a report at an instant; kill state is what the worker last reported; `does_not_assert: process_alive, work_executing, issue_unblocked, approval_granted` | `fleet_work.py:267-276`; `desk.py:141-143`; `docs/WORKERS.md:14-17`; `docs/DESK.md:85-87`; `docs/DISPATCH.md:23-25` | Athena cannot observe processes and never claims to | Worker health in Briefing is "last reported"; Pause Work's "stopping or unconfirmed" (`docs/BLUEPRINT.md:381-382`) is the same honesty |
| 19 | **An approval authorizes an intent against a target, single-use, and the retry re-validates everything** | `src/athena/core/migrations/0063_approval_requests.sql:9-19, 60-66` | Replaying a stored payload "would re-execute a stale payload under authorization evaluated at a different moment" | Covered in depth by `docs/authority.md` (mechanism 3); the Briefing-side lesson is that a pending approval is an ask *of the Operator* with a named requester and target |
| 20 | **The file fence compares declared paths across every active lease with no repo dimension** | `lease_commands.py:159-176`; `0077_lease_declared_paths.sql:4-8` | **MWS-50 (#92)**: a Beacon lease fencing `docs` 409'd an Athena `docs/DESK.md` fence, and the Athena edit had to go unfenced to claim at all | A fence is scoped by repository/Project from the first migration; an over-broad fence that makes seats drop their fence is worse than none |
| 21 | **Notification preferences are projections over the inbox; the inbox and activity log stay the single truth; a broken mute fails open (delivers)** | `0080_notification_priority.sql:4-9, 23-26`; `src/athena/aegis/notification_priority.py:1-7` (main-only: MWS-17) | A solo operator shapes the firehose "without creating a second event authority" | Notification preferences "narrow interruptions further but cannot make a non-actionable event interrupt" (`docs/BLUEPRINT.md:349-350`) — preferences over Attention Items, never a second queue |
| 22 | **There is no cross-reader desk.** Authorization is the identity itself; no parameter names someone else; operators read admin surfaces which stay admin-only | `desk_commands.py:51-55`; `docs/DESK.md:92-93`; `fleet_attention.py:70-76` | — | Agent desk packet is per-Agent; Briefing is the Operator's; the two never share a query that could name the other's reader |

---

## Load-bearing subtleties

**Honest per row, false in aggregate.** Each lapsed row carried `active: false`, and the test that
kept expired rows visible had a good reason — "losing a lease silently is worse than seeing it"
(`tests/test_desk.py:234-237`). The lane still lied, because its *word* ("issues you hold") was a
claim about the set. Beacon's lanes are named claims; a per-item flag does not discharge them.

**Count after the filter, not before.** `lapsed_total` counts what survives the done-issue filter:
"a total that counts rows the list can never show would be the same lie in a smaller font"
(`leases.py:215-220`). `test_lapsed_total_counts_after_the_done_filter_not_rows` guards it.

**Empty and present, not absent.** A fresh agent's desk carries `lapsed: []` and `lapsed_total: 0`
— "two facts, not one missing key" (`tests/test_desk.py:126-129`); `complete` returns `lapsed` on
both branches so absence never becomes a defaulted meaning (`lease_commands.py:575-577`). This is
`AGENTS.md:198` in practice.

**A typed response model silently drops fields.** `/complete` declares a response model that
*filters* the returned dict; `lapsed` would have vanished on the wire with no error until the model
declared it (PR #402 body, "Where the plan and the code disagreed"). Beacon generates TypeScript and
MCP schemas from Rust (`docs/BLUEPRINT.md:585-587`) — the same trap class, so every desk lane field
needs an over-the-wire assertion, not only a unit test.

**One definition of "closed".** Status category is computed by one helper
(`src/athena/aegis/statuses.py:68-84`); `fleet_work.py:30-35` notes there were two hand-written
copies "until the query grammar needed a third", and `leases.py:216-220` filters in Python rather
than duplicate the inbox's SQL. Beacon's closed vocabularies (`AGENTS.md:172-173`) need one owner.

**The sibling lane still has the bug.** `office.py:90-91` calls `leases_held_by(limit=20)` and
filters `active` in Python *after* the SQL limit, so a holder with more than twenty rows can have
an active lease pushed out of the window and read as unseated. Same root cause as MWS-29; noted
open in the review-request handoff, unfixed at the pin. Bounded-then-filter is the shape to test for.

**Capability shipped; adoption did not.** The desk, work-context packets, leases and a scope-filtered
MCP server all existed, and agents (this seat included) did not know
(`docs/AGENT_BOOT.md:3-8`; memory `athena-workspace`). The fix was a byte-identical boot block in
each seat's rules file, sentinel-fenced for drift checks (`AGENT_BOOT.md:44-52, 92-102`). A desk
that is not on the harness's first-read path is a desk nobody sits at.

**The window's order is not the decision.** `fleet_work.py:44-50` is the record of a projection
that truthfully reported "0 need attention" about a page it had filtered clean of them. The repair
was not a better ranking; it was ordering the *fetch* so the clipped end is the quiet end, plus
`examined_count` so the reader can tell the two zeroes apart.

**Priority exists and is deliberately not the ranking key.** Issues carry `priority` and the
inbox orders by it (`delegations.py:42-49`); the attention queue ranks by signal severity then
age (`fleet_attention.py:612-617`), and its severity is per-signal (a webhook with five
consecutive failures is `critical`, `fleet_attention.py:501`). This is the same reasoning as
`docs/authority.md` "Ordering by operator action is allowed. Ordering by authority is not."

**Homonyms, one line.** Athena's "claim" is possession of a work item; Minerva's is a falsifiable
proposition; Athena's "mission" is prose and a display string with no table
(`2026-08-22_minerva-athena-seam-research-claude.md:24-36, 53`). `knowledge.md` owns this finding.

---

## The measured case

`GET /desk` as the Claude seat on 2026-08-29 against the live deploy (eeb8ae3, v1 desk):

| Field | Observed | Meaning |
|---|---|---|
| `schema` | `athena.agent_desk.v1` | pre-MWS-29 desk; `main` serves `v2` (`desk.py:49`) |
| `work.leases.total` | **6** | counts every row (`count_leases_held_by`, removed on main) |
| `work.leases.items[].active` | 2 × `true` (issues 94, 60), **4 × `false`** (issues 34, 35, 36, 37, all claimed 2026-08-22) | four lapsed rows reported as "issues you hold" |
| issues 34/35/36/37 status | `done`, `done`, `done`, `done` (`GET /issues/{id}`) | on `main` all four are omitted (`leases.py:209-213`): items 2, total 2, lapsed `[]`, lapsed_total 0 |
| `GET /issues/36/lease` | row present, `active: false`, generation `5fc888c1…` | the owning surface still answers "who held this last" on both revisions |
| `cursor` / `signals.events_since_cursor` | `null` / `null` | this seat has never acknowledged; not `0` |
| `signals.latest_visible_event_id` | **861** | where a fully drained reader would advance to |
| `signals.unread_notifications` | 46 | |
| `openapi.json` paths | `/desk`, `/desk/cursor`, `/fleet/active-work`, `/notifications…`; **no** `/attention/ranking` | the ranked "Now" queue (`fleet_attention_api.py:19`) is main-only |
| `/fleet/active-work`, `/approvals`, `/automation/rules` | **403** with the member token | operator surfaces stay admin-only (`docs/ACTIVE_WORK.md:49-50`) |
| anonymous `/issues` · `/readyz` · `/desk` | **200 · 200 · 401** | `ANONYMOUS_READS` defaults `True` (`src/athena/config.py:323`); the desk is bearer-only regardless |

Two findings. First, the number that mattered — four phantom "held" — disappears on `main` without
deleting a row: the lane's definition changed, the data did not. Second, the live cursor is `null`
on the seat that reads its desk most: the null/zero distinction is exercised, not theoretical.

The fence collision is also measured: issue #92's body records the exact 409 on 2026-08-29 11:37Z
(`declared path 'docs/DESK.md' overlaps 'docs' on issue 90 held by Codex`). Today's two active
Claude leases fence `docs/*.md` in Beacon (issue 94) and Athena source files (issue 60); they do not
collide only because no prefix is shared, which is luck, not scoping (`lease_commands.py:159-160`).

---

## Dead seams — tried and closed

1. **Athena → Icarus dispatch.** Athena built the complete control-plane half: migration 0067, a
   five-state machine, HMAC callbacks, `dispatch_to_icarus` (`docs/DISPATCH.md:3-10`;
   `docs/ROADMAP.md:84-88`). Icarus has no receiver and by charter never will
   (`2026-08-22_ai-town-cohesion-findings-claude.md:26-31, 37-42`). **MWS-19 (#48)** is an explicit
   HOLD: "do not implement a live dispatch path from this ticket"; the UI must show HOLD and refuse
   dispatch. `RELEASE_READINESS.md:391-393` says the executor contract "has exactly one
   implementation", the example written alongside it. Beacon's Work and Execution share one codebase
   (ADR 0002), so the seam question dissolves — the surviving lesson is `DISPATCH.md:16-25`:
   `accepted` means "the executor said", never "running", and evidence is referenced, never copied.
2. **Buzz → Athena "P2 listening post" poller.** Proposed 2026-08-21
   (`2026-08-21_buzz-athena-integration-analysis.md:75`), never built: at the pin the only Buzz
   code is outbound `src/athena/core/buzz_radio.py`. The requirement that survives is its loop
   guard (skip relay-key, own-key, workflow-tagged messages). Beacon's Conversation and Work share a
   store (`AGENTS.md:88`), so no poller is needed; a mention that creates work still needs the guard.
3. **Minerva ↔ Athena seam.** Eight-agent review verdict "build nothing yet"
   (`2026-08-22_minerva-athena-seam-research-claude.md:9-18`), re-affirmed 2026-08-23
   (`…minerva-adoption-diagnosis-claude.md:48-51`). Not rebuilt here; `knowledge.md` owns it.
4. **Closing anonymous reads.** Pinned by contract (`da67c9e`, PR #393) and still not flipped live:
   anonymous `GET /issues` returns 200 today. Beacon rules this out by construction — ADR 0018
   requires an Operator Account before any Workspace data (`AGENTS.md:30-32`). Athena is the
   counterexample that shows why the default matters.

---

## Consequences for Beacon's Briefing

Each is testable against the cited Athena behavior; none is a design decision.

1. **Composition test.** For every Briefing and desk-packet lane, a test asserts the lane equals the
   owning module query's output for the same actor and the same clock (invariants 1, 3).
2. **Cursor semantics.** `Since your last visit` derives from a per-Operator, forward-only cursor
   that is `null` until first acknowledged; `GET`/refresh never writes it; a lower id is refused;
   the change count is capped and says so (invariants 4-6). A test opens the Briefing twice and
   proves the cursor did not move.
3. **Assignment "held" is computed at read time** against one injected clock; active and lapsed are
   separate lanes; totals count the same instant as their items; lapsed totals count after any
   filter (invariants 7, 9). Mutation test: drop the filter and the total test must fail.
4. **Generation fence on every Assignment mutation**; the holder can clear its own lapsed
   Assignment; nobody else can, including an Operator lever that stays active-only (invariants 8,
   10). Guard test: another actor's lapsed Assignment is never clearable.
5. **Four distinct commands** — release, finish the work, yield-with-Handoff, decline — with
   distinct events; the desk inbox keys on the work's state, not on lease release (invariant 11).
6. **Attention Item = unresolved reason + named owner + executable next action.** Rows whose next
   action is `none` are Activity (invariant 16 ↔ `docs/BLUEPRINT.md:352-358`). `agent-command` is
   labeled only when the caller holds the write capability to run it.
7. **Rank by signal severity then age; never by a priority integer; name the ordering honestly**
   ("needs you first"). Fetch attention-prone rows first so clipping drops the quiet end; report
   `examined` (invariants 15-17).
8. **No liveness claims.** Worker and Run health render as "last reported at"; Briefing's empty
   state is a statement about the record, not about agents (invariant 18).
9. **File/target fences carry a Project or repository dimension** from the first schema (invariant
   20, MWS-50).
10. **Over-the-wire field tests** for every desk lane field, because generated schemas filter
    silently (subtlety 4).
11. **Every refusal names a reachable remedy**; a 409 whose fix requires a command the same state
    forbids is a defect (invariant 10).
12. **Bounded-then-filter is a lint target**: any reader that applies a SQL limit and then filters
    in code is the `office.py:90-91` shape (subtlety 6).
13. **The desk packet sits on the harness's first-read path** (subtlety 7): BLUEPRINT's MCP adapter
    (`docs/BLUEPRINT.md:590-593`) is where an Agent would be told to call the packet first.

---

## Open questions — Kevin's, not ours

1. **Which relation is "Assignment"?** `CONTEXT.md:147-149` defines Assignment as the accountable
   relationship and says to avoid "Lease"; Athena keeps accountable (assignee), eligible
   (contributor) and holding (lease) as three relations, and MWS-29 was a bug in the third. If
   Beacon collapses them, which one does the desk packet's "current Assignment" mean, and where does
   the clock-derived possession live? This page maps lease → Assignment provisionally.
2. **One cursor or one per scope?** Athena has a closed cursor vocabulary with exactly one name
   (`0073_agent_cursors.sql:29-31`). Does **Mark caught up** acknowledge the whole Briefing or the
   current Project scope (`docs/BLUEPRINT.md:277-278` preserves scope across apps)?
3. **Automatic reassignment vs. holder-only yield.** ADR 0010 reassigns after a second failure;
   Athena's yield is deliberately holder-only, never auto-routes, and requires attempted work,
   evidence, a blocking question and resume instructions (`lease_commands.py:360-390`). Should
   Beacon's automatic Handoff require those four fields, and who authors them when the holder is
   the thing that failed?
4. **Does Beacon carry a priority field at all** on Missions or Assignments, given that Athena's is
   used for inbox order but explicitly not for attention ranking (invariant 16)?
5. **Is a Ruling single-use?** Athena's approval is spent inside the retry's own transaction
   (`0063:18-19`); Beacon's Ruling binds a Proposal digest (ADR 0004). These are different objects,
   not a contradiction, but whether one Ruling may authorize one Run or many is a doctrine call.
6. **Freeze vs. deploy.** The live Athena deploy is 30 commits behind `main` with migration 0080 and
   the v2 desk in the gap. Moving it is RED; freezing it leaves the measured defect live for every
   seat until Beacon replaces the desk. Which is intended during the absorption window?
