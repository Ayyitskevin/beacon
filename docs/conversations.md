# Conversations findings inherited by Beacon

**Status:** draft for review · **Produced by:** claude-code harvest agent · **Last verified:** 2026-08-29

BLUEPRINT §3 says Conversations is "descended from Buzz" (`docs/BLUEPRINT.md:187-192`). Buzz is not
Kevin's code: `~/ai-workspace/buzz` is a checkout of upstream `github.com/block/buzz` at `78cbffe`
(what runs), with upstream `origin/main` at `00e61ea`, 221 commits ahead. Kevin's one upstream PR,
block/buzz#6977, was closed unmerged on 2026-08-28; the fix survives only on a fork branch. So what
Beacon inherits here is mostly **how the fleet uses a conversation surface** — the identity model,
the wake mechanism and its failure modes, the liveness lessons, the artifact-first and reply-to
conventions, and the operational scars — plus the few upstream mechanics the fleet depends on
(event kinds, kind-0 profiles, the ACP bridge). Findings with evidence, not decisions. Approval
kinds 46030/46031/46010-46012 are already in `docs/authority.md:94-117` and are not restated.

---

## How this page was produced

Everything below was re-derived on 2026-08-29 from pinned revisions and live probes, never from
memory files alone. Memory and doc claims were treated as claims (R5) and checked.

| Source | Revision / what | What was read | Command form |
|---|---|---|---|
| Buzz upstream | `78cbffe` (runs) · `00e61ea` (upstream main) | `crates/buzz-core/src/kind.rs`, `buzz-acp/src/{lib,acp,config,pool,relay}.rs`, `buzz-cli/src/commands/messages.rs`, `buzz-sdk/src/mentions.rs`, `buzz-relay/src/{api/bridge,handlers/ingest}.rs` | `git -C ~/ai-workspace/buzz show <rev>:<path>`, `git grep -n <pat> <rev> -- <paths>` |
| Kevin's ops layer (untracked in the buzz tree) | working tree, 2026-08-29 | `run-{claude,codex,glm,grok,kimi,qwen,muse}-agent.sh`, `seat-canary.sh`, `backup-relay.sh`, `deploy/compose/{compose.yml,.env}` (.env read with values redacted) | `cat`, `sed -E 's/=.*/=<redacted>/'` |
| Fleet doctrine | `~/.claude/CLAUDE.md` (Workshop section) · `~/.claude/skills/buzz/SKILL.md` | conventions, wedge protocol, mention rules | `grep -n` |
| Athena | `~/ai-workspace/athena-claude` `origin/main` = `8ddefc8` | `src/athena/core/{buzz_radio,forge_events,webhooks}.py`, `aegis/automation.py`, `web/render.py`, `workflows/fleet_assign_commands.py`, `config.py` | `git show origin/main:<path>`, `git grep -n <pat> origin/main -- <paths>` |
| Beacon | `origin/main` = `c59dde1` | `README.md`, `CONTEXT.md`, `AGENTS.md`, `docs/BLUEPRINT.md`, `docs/authority.md`, ADRs 0001-0024 | `sed -n`, `grep -n` |
| Memories | `buzz-relay-mickey`, `review-capacity-fails-silently`, `buzz-athena-integration`, `mickey-quiet-hours` | claims to verify | `cat` |
| Handoffs | `shared/handoffs/2026-08-15_buzz-channel-layout-codex.md`, `2026-08-16_agent-commons-buzz-review-codex.md`, `2026-08-20_buzz-repo-poll-*-codex.md`, `2026-08-21_buzz-athena-integration-analysis.md`, `2026-08-22_athena-deploy-and-roster-claude.md` | provenance of conventions and dead seams | `sed -n`, `grep -n` |

Live probes, all read-only (nothing restarted, nothing sent, no key contents read):

```
docker ps --filter name=buzz --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}'
docker inspect buzz-prod-relay-1 --format '{{.Config.Image}} {{.Created}}'
docker logs --since 24h buzz-prod-relay-1 2>&1 | grep 'status":403'          # then parsed with python
systemctl --user list-units 'buzz-acp-*' --all; systemctl list-units 'buzz-*' --all
systemctl --user list-unit-files 'buzz-*' 'ollama-*'; systemctl --user list-timers --all | grep buzz
journalctl --user -u buzz-acp-<seat> -n 40 -o short-iso   (seat ∈ claude codex glm grok kimi)
journalctl --user -u buzz-acp-kimi --since '2026-08-29 00:47' --until '2026-08-29 08:10' -o cat | wc -l
ls -la ~/backups/buzz-relay/; tail -5 ~/backups/buzz-relay/backup.log
ssh flow 'ls -1 ~/backups/mickey-offhost/; find ~/backups/mickey-offhost -name "buzz-pg*" | wc -l'
source ~/.buzz-keys/owner.key; export BUZZ_PRIVATE_KEY="$SECKEY"; export BUZZ_RELAY_URL=http://100.125.80.91:3000
buzz users get --name <handle>; buzz channels list; buzz channels members --channel $FLEET
buzz messages get --channel $FLEET --since $(date -d '2026-08-29 04:00:00 UTC' +%s) --limit 60
gh pr view 6977 --repo block/buzz --json state,closedAt,mergedAt
gh pr view 377 --repo Ayyitskevin/athena --json state,mergedAt   (also 378)
```

---

## What the Blueprint already specifies

- `docs/BLUEPRINT.md:187-192` — Threads attach to Projects, Missions, Claims, Artifacts, Proposals,
  or Runs; messages carry actor identity, reply structure, timestamps, optional signatures; a
  conversation produces an Artifact or Mission only through an explicit command; "nothing becomes
  authoritative merely because it was said".
- `docs/BLUEPRINT.md:639` — the Conversation module owns "Threads, messages, mentions, directed
  inbox" with the interface "post, reply, read thread".
- `docs/BLUEPRINT.md:345-366` — Attention Item semantics: a condition unresolved, a named actor
  eligible, a stateable next action; everything else stays in Activity.
- `docs/BLUEPRINT.md:745` and `AGENTS.md:149` — secrets never enter Threads or logs.
- `docs/BLUEPRINT.md:830` — after restart the operator traces every step "without relying on a chat
  transcript"; `:834` — Alpha runs with no Buzz process at all.
- `CONTEXT.md:178-180` — Thread, avoid "chat, channel"; `:58` — Display Name never serves as
  identity; `:299` — Attention Item; `:151` — Handoff is a durable packet, avoid "message".

Vocabulary map used below: Buzz channel → Thread; Buzz mention → directed message (an Attention
Item only when the three conditions hold); owner key → Operator (`CONTEXT.md:10`); cloud seat →
Resident Agent (`:62`); local worker (qwen, muse) → Resident Agent on an isolated local Worker
(`:81`) — not a Specialist (`:77`), because they persist across Missions; kind-0 profile name →
Display Name; buzz-acp harness → Worker runtime.

---

## Corrections to prior claims

1. **"NO backups configured (open risk)"** — the Buzz row of the CLAUDE.md fleet table said this
   from deploy until 2026-08-29. It was false from 2026-08-16: `backup-relay.sh` (header dated
   2026-08-16) plus `buzz-relay-backup.timer` have produced a verified `pg_dump` nightly since then
   (`~/backups/buzz-relay/backup.log`: 15 `backup ok` lines; first dump
   `buzz-pg.20260816T175127Z.sql.gz`). The row now reads correctly (`~/.claude/CLAUDE.md:384`).
   The memory `buzz-relay-mickey` had it right all along — the doc was the stale copy.
2. **"The parser grabs EVERY `@word`"** (`skills/buzz/SKILL.md:30`) overstates it. At `78cbffe`
   `resolve_content_mentions` strips fenced blocks and inline code spans before scanning
   (`crates/buzz-cli/src/commands/messages.rs:163`; `crates/buzz-sdk/src/mentions.rs:244`), and an
   `@` only counts at start-of-string or after ASCII whitespace (`mentions.rs:57-59`), so
   `user@host` is exempt. The rule holds for bare `@fastify/cookie` in prose; the cheaper fix than
   `--mention <pubkey>` is backticks. The fail-closed part is exact: an unresolved name aborts the
   whole send (`messages.rs:137`), an ambiguous one too (`:143`).
3. **"Off-host copy: buzz-relay dumps land in `flow:~/backups/mickey-offhost/`"** (memory) —
   true (15 dumps under `mickey-offhost/buzz-relay/`), but at probe time the newest there was
   `20260828T033052Z` while mickey already held `20260829T033015Z`: the off-host copy lags the
   local one by a day. Not diagnosed further (read-only); `pull.log` exists there.
4. **"44 HTTP 403s in 24h"** (this task's brief) — 46 at probe time: 44 from the claude seat key
   `c55e7028…`, 2 from the codex key `c9226992…`. The window slides; the shape is what matters
   (see the measured case).
5. **"Kimi wedged, zero journal lines for 7h"** (memory) — confirmed exactly: 0 lines between
   04:47 and 12:10 UTC. The memory's inference "steer fold = wedge" is only half right — the same
   fold at 12:12Z completed normally (see subtleties).

---

## The invariants

| Invariant | Where it lives | The incident that produced it | What Beacon must preserve |
|---|---|---|---|
| Actor identity is a signing key; the relay refuses an event whose pubkey is not the authenticated identity | `crates/buzz-relay/src/handlers/ingest.rs:2025-2028@78cbffe`; closed relay `deploy/compose/.env:15-16` (`REQUIRE_AUTH_TOKEN`, `REQUIRE_RELAY_MEMBERSHIP` both true); kind 0 profile `crates/buzz-core/src/kind.rs:9` | none — inherited from Nostr; it is why the fleet could give each seat a key and each key a systemd unit | Message authorship is a verified key, and Display Name is separable from it (`CONTEXT.md:58`) |
| A mention is the wake; a message that mentions nobody wakes nobody | `crates/buzz-acp/src/config.rs:51-52` (`SubscribeMode::Mentions`); kimi journal `subscribe=Mentions` | 2026-08-20 repo poll got zero replies until every bot was mentioned (`shared/handoffs/2026-08-20_buzz-repo-poll-{diagnosis,followup}-codex.md`) | A directed message is a distinct object from a post; "posted" and "delivered to an Agent" are different states |
| Only the Operator's key wakes a seat; a non-owner mention is dropped silently, including the agent's own posts | `crates/buzz-acp/src/lib.rs:236-262@78cbffe` (`author_allowed`, `OwnerOnly`); `:2719-2722` (`ignore_self`); every `run-*-agent.sh` sets `BUZZ_ACP_RESPOND_TO="owner-only"` | 2026-08-17: posting as `claude-agent.key` returned `accepted:true` and total silence, "indistinguishable from a wedged fleet" (memory `buzz-relay-mickey`) | The surface says before send whether a message can wake its target; "accepted, will wake nobody" is a visible state, never a silent drop |
| Mentions resolve by published profile name, case-insensitively; unresolved or ambiguous names fail closed before send | `messages.rs:137,143,152-157,205-216@78cbffe`; `--mention <pubkey>` downgrades unresolved text to presentation-only (`:592`, `crates/buzz-cli/src/lib.rs:395`) | 2026-08-23: most identities had no kind-0 profile so every handle mention was rejected; profiles published for 6 identities, qwen/muse left nameless on purpose (`skills/buzz/SKILL.md:23-37`) | Resolution binds an Agent identity, never a name; a deliberately unnamed Agent is reachable by identity |
| A mention to a busy seat is folded into the in-flight turn ("steer"); when the agent cannot steer, the fold is cancel+merge and the sender is never told | `crates/buzz-acp/src/acp.rs:184-202@78cbffe`; `config.rs:75` (`MultipleEventHandling::Steer`); kimi journal `steering_supported=false` | 2026-08-29 04:46:40Z: `steer: no active_run_id … falling back to cancel+merge`, then 7h24m of silence, unit `active` throughout | Delivery to an Agent mid-Run is a recorded, visible outcome (queued / folded / refused), not a harness log line |
| Silence is not thinking; `active` is not alive; presence is a process fact; the only proof is a reply | `~/.claude/CLAUDE.md:527-534` (WEDGE PROTOCOL); `seat-canary.sh:401-416` (reply-based, restart on 2nd miss); `crates/buzz-acp/src/lib.rs:69-80@78cbffe` (presence kind 20001 published at startup) | 2026-08-16: 3 of 5 seats missed a consultation; 2026-08-17: 4 needless restarts from misreading journal silence; 2026-08-29: five seats unavailable ~7h with nothing surfacing it | Liveness is "last acknowledged reply", shown per Agent; Worker "online" is labelled as runtime presence, not Agent health |
| Provider quota / billing / rate-limit failures live in the per-seat journal and nowhere else | grok journal: `status=402 Payment Required … Grok Build usage balance exhausted` hourly 05:14Z→14:14Z; channel-visible notice said only "Internal error" (#fleet 05:13Z, `f6f7c025`) | 2026-08-29 outage (memory `review-capacity-fails-silently:12-19`) | A failed Run caused by a directed message carries the provider's reason to the surface; an Attention Item without a stateable next action is forbidden (`BLUEPRINT.md:352-356`) |
| Artifact-first: produce the file, post the pointer; a message body is not a record | `~/.claude/CLAUDE.md:493,516`; `skills/buzz/SKILL.md:70`; seat prompt `run-glm-agent.sh:160` | Set with the seats 2026-08-16 (GLM's rule); the record is `shared/handoffs/` with the schema, never the channel | "Nothing becomes authoritative merely because it was said" (`BLUEPRINT.md:191-192`); Handoff (`CONTEXT.md:151`) is the record |
| Reply-to discipline: reply in-thread; top-level is for milestones and blockers; mention only who must act | `~/.claude/CLAUDE.md:518-519`; `skills/buzz/SKILL.md:71` | Codex's rule, 2026-08-16 consultation | Reply structure is first-class (`BLUEPRINT.md:190`); failure notices reply to the message they failed (Buzz's do not — see subtleties) |
| Local workers are Buzz-only: isolated unix users who cannot read the vault or handoffs; the requester copies their durable output into the record under its own name | `~/.claude/CLAUDE.md:520-526`; hardened system units `buzz-seat-{qwen,muse}.service`; `run-qwen-agent.sh:309-312` (one MCP slot, replies via `buzz-dev-mcp`) | Grok's catch, 2026-08-16 | An Agent reachable only through a Thread is a supported isolation pattern; its outputs are Artifacts under its identity, promoted by the requester (Memory Promotion, `CONTEXT.md`) |
| A refusal must carry its reason on both sides | relay Err path logs status only (`crates/buzz-relay/src/api/bridge.rs:697-702@78cbffe`); seat side logs the likely failing publish at `debug`/off-target (`pool.rs:4167-4171,4360`) | 46 refusals in 24h whose reason nobody recorded (measured case) | Every post/reply refusal is typed and lands in Activity with the reason |

---

## Load-bearing subtleties

**The fold is not the failure; the missing completion is.** Two identical `steer: … falling back
to cancel+merge` lines today. The 04:46:40Z one (event `0210bce0…`) was followed by nothing for
7h24m; the 12:12:44Z one (event `45148fac…`, Beacon PR #6) was answered with PASS at 12:16Z. The
harness acknowledged both to itself ("control signal sent to in-flight task … mode=Steer") and to
nobody else. The fleet's compensating rule — "never mention a seat mid-turn"
(`review-capacity-fails-silently:24-25`) — is a human rule papering over a runtime default
(`meh=Steer`), and it costs the ability to ever queue a second ask.

**Presence lies in exactly the case that matters.** `presence set to online` (kind 20001) is
published when the harness process connects (`lib.rs:69-80`). Kimi was "online" for the whole
outage. Beacon's Worker presence and Agent liveness must be different fields.

**Failure notices lose their thread and their reason.** After ten backoff attempts (`max=10`,
claude journal) the harness posts a top-level ⚠️ kind-9 with no `e` (reply) tag and no mention
(#fleet 05:12Z `c55e7028`, 05:13Z `f6f7c025`). Claude's carried the quota text; grok's carried
"Internal error" while the 402 stayed in the journal. One is an Attention Item candidate, the
other fails the third condition (`BLUEPRINT.md:352-356`) — and they look the same in the channel.

**Owner-signed automation loses "who said it".** Athena's `buzz_radio` shells out to the CLI with
whatever key `ATHENA_BUZZ_KEY_FILE` names (`src/athena/core/buzz_radio.py:3-5,25-31@8ddefc8`;
`config.py:222`), and the live service points it at Kevin's `owner.key`
(`shared/handoffs/2026-08-21_buzz-athena-integration-analysis.md:13`). Every assignment radio is
signed as the Operator. Athena had to harden the frame so an issue title could not forge a second
`ATHENA_ASSIGN` directive under that signature (`buzz_radio.py:45-60`, `_one_line`). The Operator
identity was chosen because seats respond to owner only — the wake rule forced the impersonation.

**The relay enforces membership on mentions, not on resolution.** After name resolution the CLI
checks that every mentioned pubkey is a channel member and refuses otherwise
(`messages.rs:601-605`); the review-gate room therefore needs explicit `channels add-member` before
a seat can be mentioned there (memory `buzz-athena-integration`). Reachability is per-Thread.

**Display Name is the fleet's only resolution key, and Kevin deliberately left two Agents without
one.** qwen and muse "exist to be notified when needed" (Kevin, 2026-08-23) and are woken by
pubkey. `buzz users get --name` today: kevin, claude, codex, glm, grok, kimi resolve (one pubkey
each); the two local pubkeys `a6466cb7…`/`7c29f9b3…` resolve to nothing and the relay does not
label which is which — "identify by the reply" (`SKILL.md:35-38`).

**An owner can stop an agent from inside the conversation.** A kind-9 `!shutdown` from the owner
mentioning the agent exits the harness gracefully (`lib.rs:2724-2740`). Seats also carry hard
ceilings: `IDLE_TIMEOUT=300`, `MAX_TURN_DURATION=900` for cloud seats, `600`/`1800` for local
(`run-*-agent.sh`). These are the closest Buzz analogue to Pause Work.

**Redis and MinIO are not in the dump window.** `backup-relay.sh:35-43` dumps Postgres (the real
store, 59 tables per the memory) and tars the git-conformance volume (190-227 bytes — empty);
`compose.yml` declares `buzz-redis-data` (presence, read state) and `buzz-minio-data` (media)
with no backup path. Whether that matters is marked "unverified" in `CLAUDE.md:384`.

---

## The measured case

**The 2026-08-29 outage, from #fleet events and journals (all UTC).**

| Time | Evidence | What it shows |
|---|---|---|
| 04:34–04:38 | kimi `537d7a47` posts PASS twice and an acknowledgement | seat alive |
| 04:45:56 | kimi unit stopped/started (owner restart); 04:45:57 `presence set to online` | presence ≠ liveness begins |
| 04:46:40 | `steer: no active_run_id … cancel+merge` on Kevin's second ask | fold into in-flight turn |
| 04:47, 04:48 | Kevin mentions `@grok`, `@claude` for the same review | three seats asked |
| 05:12, 05:13 | top-level ⚠️ notices from claude (quota text) and grok ("Internal error") | one actionable, one not |
| 05:14→14:14 | grok journal: `status=402 Payment Required` once per hour, 10 times | reason only in journal |
| 04:47→12:10 | kimi journal: **0 lines**; `systemctl --user list-units` shows `active running` | silence with a green unit |
| 12:10:16 | kimi restarted by owner; 12:11 acknowledgement (55 s after re-ping) | restart + re-ping recovers |
| 12:12:44 | second steer fold (event `45148fac…`); 12:16 PASS | same log line, opposite outcome |
| 12:13 | Kevin posts a review request with **no** mention, "every reviewer is busy or degraded, and a mention mid-turn is what wedges them" | the doctrine, live |

glm's journal shows `429 Too Many Requests` retries on 2026-08-27 05:19–05:28Z, a restart on
2026-08-28 12:47Z, and nothing since; its rate-limited state came from `glm-preflight` (memory),
not re-run here. All five `buzz-acp-*` units read `active running` at probe time; the seat canary
that would have caught this is `disabled` (`systemctl --user list-unit-files`), since 2026-08-20
(memory `mickey-quiet-hours`). Its four days of history: 93 log lines, 30 `WEDGED` restarts —
kimi 26, claude 1, qwen 1, muse 1 — with kimi restarted every ~2 h through the night of
2026-08-18/19 (`grep canary shared/logs/mickey-actions.log`).

**46 refusals nobody can explain from logs.** `docker logs --since 24h buzz-prod-relay-1 | grep
'status":403'` → 46 lines, all `POST /events` via `buzz_relay::api::bridge`, `accepted:false`,
no `kind`, no reason (the Err arm logs status only, `bridge.rs:697-702`). 44 are the claude key,
2 the codex key. For the claude key over 24h: 279 bridge requests — 135 accepted with no kind,
48 kind-7 reactions accepted, 48 kind-5 deletions accepted, 4 kind-9 messages, 44 refused. Every
one of the 44 came 0.2–2.3 s after an accepted kind-7 from the same key, and the first at
`19:10:04.232656Z` is 0.24 ms before the seat's own `requeueing failed batch … attempt=1` line
(`journalctl --user -u buzz-acp-claude --since '2026-08-28 15:05' --until '2026-08-28 15:20'`).
So the refused event is something the harness submits at the instant an agent turn errors, after
its "working" reaction and before the reaction removals. The per-turn NIP-AM metric (kind 44200,
`pool.rs:4143-4165`, `p`-tagged to the owner) fits: the relay refuses it unless the `p` tag is "the
registered owner of this agent" (`ingest.rs:2542-2545`), the seats were admitted by membership,
not registration, and no kind-44200 event was accepted from anyone in 24h. The seat-side warning
uses target `pool::metrics` and never appears in the journal. **Not proven** — the point is that
neither party recorded enough to prove it.

**Backups, counted.** `ls ~/backups/buzz-relay/`: 14 pg dumps retained (KEEP=14), 370 KB on
2026-08-16 → 3.0 MB on 2026-08-29, 26 MB total; timer next fires 2026-08-30 03:30 EDT; off-host
15 dumps on flow, newest one day behind. Relay image pinned by digest
`sha256:d4a5a9fe…` (`deploy/compose/.env:5`), container created 2026-08-15T23:15Z — no upstream
change has reached the running relay since deploy.

**Upstream drift, measured.** `git rev-list --count 78cbffe..00e61ea` = 221. Commits touching
paths the fleet depends on: `crates/buzz-acp` 16, `buzz-cli/src/commands/messages.rs` 3,
`buzz-relay/src/api/bridge.rs` 4, `buzz-sdk/src/mentions.rs` 0, `buzz-core/src/kind.rs` **0** —
`git diff 78cbffe 00e61ea -- crates/buzz-core/src/kind.rs | grep '^[-+]pub const'` is empty, so
no kind number the fleet or authority.md cites has moved yet.

---

## Dead seams — tried and closed

- **Buzz → Athena poller and the "buzz" forge dialect (P2).** Never built. `git grep -i
  'buzz.*poll' origin/main -- src/` returns nothing; `forge_events.py:40-44@8ddefc8` registers a
  closed dialect set of exactly `("github",)`. Reason recorded in the analysis handoff (line 16):
  Buzz's webhook egress blocks CGNAT `100.64.0.0/10` (`crates/buzz-core/src/network.rs:32,55`),
  so the relay can never push to a tailnet Athena; a poller with a loop guard was the only path and
  was deferred.
- **Athena's own keypair.** Not done; `buzz_radio` still signs with the key file it is pointed
  at, which is the owner key. `~/.buzz-keys/` holds `owner.key`, `claude-agent.key`,
  `codex-agent.key`, `glm-agent.key` (names only) — no Athena key exists.
- **What did ship (verified at `8ddefc8`):** `buzz_message` automation action, non-schedulable
  because retries would re-send (`aegis/automation.py:46-55,260-267`; PR #377 merged
  2026-08-22T04:38Z); `buzz://` permalinks linkified only in text nodes to keep author-controlled
  hrefs out (`web/render.py:69-170`; PR #378 merged 2026-08-22T04:38Z); radio receipts as
  `radioed_assignment` activity rows (`workflows/fleet_assign_commands.py:97-132`); the
  `#athena-feed` channel (`e29bb951-d272-4822-a8e5-ffac2f9462f2`) exists — **and carried 0
  messages in the last 7 days.** Wired-but-idle and wired-but-broken are indistinguishable from
  the channel; not diagnosed here.
- **GLM's Athena account.** Still undecided: "glm has a declared seat and a live unit but no
  Athena account" (`shared/handoffs/2026-08-22_athena-deploy-and-roster-claude.md:72`).
- **block/buzz#6977 — `messages get` fails closed on an unreadable channel.** Closed unmerged
  2026-08-28T13:05:39Z (`gh pr view`). The relay silently filters events the caller cannot read,
  so `[]`/exit 0 meant "empty", "nonexistent", or "not a member" alike; the fix probes kind-39000
  metadata first (commit `421333485`, on `fork/kimi/buzz-msg-get-fail-closed`). The installed
  `/usr/local/bin/buzz` (root, 2026-08-16) predates it and has no `--version`; whether it carries
  the fix is unverified. This is authority.md's open question 3 made concrete: a fleet correctness
  property that depends on someone else's roadmap.
- **The seat canary.** Built 2026-08-16 (reply-based, two-strike, grace window), fixed 2026-08-18
  (its sudo `show` gap silently skipped the local seats' grace), disabled 2026-08-20 for quiet
  hours. Nothing automated watches seat liveness today.
- **Kimi cannot be steered.** `steering_supported=false` at every start; every mid-turn mention is
  a cancel+merge. Upstream's adapter roadmap, not the fleet's.

---

## Consequences for Beacon's Conversations

1. A directed message binds a target **Agent identity**, never a Display Name. Test: two Residents
   with the same Display Name → a directed message by name is refused before it is stored, with
   both candidates named (`messages.rs:143` behaviour, made a contract).
2. A directed message has a delivery state separate from "posted": posted → delivered to a Worker
   runtime → acknowledged by the Agent (a reply or explicit ack) → or refused/folded/queued. Test:
   deliver to an Agent mid-Run and assert the sender can read the outcome without a log.
3. Agent liveness is derived from the last acknowledged reply, and Worker presence is a different
   field with a different label. Test: a Worker reports online while its Agent has not replied for
   longer than Policy allows → Briefing shows the stall, not "online".
4. When a Run triggered by a directed message fails, the provider reason (quota, billing,
   rate limit, timeout) is part of the failure record and the notice replies **to the message it
   failed**. Test: simulated 402 → the failure record contains the provider text; an Attention Item
   is created only if the next action is stateable (top up, wait until `<time>`, reassign).
5. A message that cannot wake its target (sender not eligible, target suspended or retired,
   target not in the Thread) is refused or marked "will not wake" at post time. No `accepted:true`
   followed by silence.
6. Every refusal on the post/reply interface returns a typed reason and writes it to Activity.
   Test: force each refusal class and assert Activity has one row each with the reason.
7. Message bodies are never records. A message may point at an Artifact; creating an Artifact,
   Mission, or Handoff from a Thread is an explicit Command (`BLUEPRINT.md:191-192`) that records
   who issued it.
8. Automation never posts under the Operator's identity. If a component posts on the Operator's
   behalf, the message carries its own actor identity plus an "acting for Operator" relation.
9. An Agent that can only be reached through Threads (no store access) is a supported
   configuration; its outputs are Artifacts under its identity, promoted into Shared Memory by the
   requester with provenance preserved.
10. The conversation store is part of the Safety Snapshot; ephemeral runtime state (presence,
    typing) is not, and its absence after restore is not a fault.
11. If Beacon ever reads Buzz data (import or adapter), it records the observed kind number and
    the source revision per event (authority.md:369), because 221 commits of drift already sit
    between the running relay and upstream.
12. Owner-level stop from inside a Thread (`!shutdown`) maps to Pause Work / a suspend Command,
    never to a message convention.

---

## Open questions — Kevin's, not ours

1. **Is deliberate namelessness a Beacon concept?** Kevin left qwen/muse without profile names so
   they are woken only by key. `CONTEXT.md:58` already says Display Name is not identity; whether
   an Agent may have *no* Display Name and still be first-class is undecided.
2. **Queue, steer, or refuse?** Buzz seats run `meh=Steer`; the fleet compensates with "never
   mention a seat mid-turn". ADR 0010 governs Runs, not messages. Which default Beacon adopts is a
   doctrine call, and each has a cost (lost asks vs. cancelled work vs. blocked senders).
3. **Can Residents wake each other?** ADR 0006 and `BLUEPRINT.md:126` have Residents assigning
   work and coordinating through Threads; every fleet seat responds to the Operator only
   (`lib.rs:252`). The fleet's practice contradicts the accepted ADR's intent, not its text.
4. **Is provider quota/billing exhaustion Worker health or Route health?** `BLUEPRINT.md:359-363`
   lists "Worker health degrades" as Attention-worthy; today's outage was three provider accounts,
   not three Workers. Where the Attention Item hangs determines who the "named actor" is.
5. **Acting-for-Operator identity.** ADR 0018 makes the Operator Account the only human identity;
   Athena's radio shows automation will sign as the Operator whenever wake rules demand it. Whether
   Beacon needs an explicit relation, or forbids the pattern outright, is his to rule.
6. **Does Beacon import Buzz history?** Alpha runs without Buzz (`BLUEPRINT.md:834`); the 14
   nightly dumps are the only durable record of who asked whom since 2026-08-15. Import is a
   migration decision with a real dataset behind it.
7. **Redis/MinIO in the backup window** — carried over from `CLAUDE.md:384` as "unverified whether
   that matters"; still unverified here.
