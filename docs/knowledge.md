# Knowledge findings inherited by Beacon

**Status:** draft for review · **Produced by:** claude-code harvest agent · **Last verified:** 2026-08-29

Beacon's Knowledge surface is "descended from Minerva" (`docs/BLUEPRINT.md:196@c59dde1`). This page
records what Minerva actually enforces, at a pinned revision, so the single implementing seat does
not re-derive it from taste. Findings, not decisions: anything that would change an accepted Beacon
ADR is listed under open questions and left for Kevin.

---

## How this page was produced

Every rule below was re-derived from committed code or a live read-only probe on 2026-08-29. No
tree was checked out, no SQLite file opened, no Minerva write verb run.

| Source | Tree | Pin | Read with |
|---|---|---|---|
| Minerva code | `~/ai-workspace/claude/minerva` | `origin/main` = `9968a5e` | `git -C <tree> show 9968a5e:<path>`, `git -C <tree> grep -n <pat> 9968a5e -- <paths>` |
| Minerva deploy | `~/deploy/minerva` | `acdca45` (3 commits behind pin) | `git -C ~/deploy/minerva rev-parse HEAD` |
| Athena (homonyms only) | `~/ai-workspace/athena-claude` | `origin/main` = `8ddefc8` | `git grep`, `git show` |
| Vulcan (routing blocker) | `~/ai-workspace/vulcan` | `origin/main` = `56ba96e` | `git show` |
| Icarus (D-3 guard) | `~/ai-workspace/claude/icarus` | `origin/main` = `c919cd2` | `git grep` |
| Beacon | `~/ai-workspace/claude/beacon` | `c59dde1` | read only |

Read at the Minerva pin: `README.md`, all of `docs/adr/`, `docs/STATUS.md`, `docs/ROADMAP.md`,
`docs/PILLAR_SCORECARD.md`, `docs/THREAT_MODEL.md`, `docs/DECISIONS.md`, `docs/VISION.md`,
`CHANGELOG.md`, `docs/evals/2026-08-24-*`, the five migrations,
`core/{types,attribution,db,doctor}.py`, `web/{security,server}.py`, `capabilities.py`,
`claim_search/service.py`, `synthesis/service.py`, `evidence/{service,lens_adoption}.py`,
`assist/adoption.py`, `integrations/{research_packet,ai/anthropic}.py`, and `git log` for
`97ddf05 67d8f81 e1e1f18 1ee6cc9 c80d29b 02a4244`.

Live probes, all GET or read-only CLI, using the venv binary `B=~/deploy/minerva/.venv/bin/minerva`
and `DB=~/data/minerva/research.db` (the `~/.local/bin/minerva` wrapper breaks `capabilities`; see
Dead seams):

| Command | Observed |
|---|---|
| `curl -fsS http://127.0.0.1:8765/healthz` | `{"status":"ok"}` |
| `curl -fsS http://127.0.0.1:8765/api/v1/capabilities` | `schema_version: minerva.capabilities.v3`, `listener.mode: loopback_tcp`, `access_scope: host_local_unauthenticated`, `application_authentication: false` |
| `curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer x' http://127.0.0.1:8765/healthz` | `403`, body `external_identity_rejected`; same for `-H 'X-Minerva-Actor: codex'` on `/` |
| `$B doctor --deep --db $DB` | 12 checks `ok`: 25 snapshots, 56 citations, 29 findings, 0 inferences, 371 audit events, 399 material rows reconciled |
| `$B mission list --db $DB` | 10 missions: 6 `os-user:kevin-lee`, 2 `+seat:claude-code`, 2 `+seat:codex` |
| `$B audit list --db $DB --limit 100 --after-sequence N` (paged to 371) | event-type and actor counts in *The measured case* |
| `$B capabilities` (no `--db`) | v3 document, `listener: null`, 14-item `unavailable` list |
| `$B claim --help` | `{add,show,ledger,review,lineage,status}` — **no `search` on the deployed binary** |
| `systemctl --user cat minerva.service` · `tr '\0' ' ' </proc/1883/cmdline` · `ss -ltnp \| grep 8765` | drop-in `minerva.service.d/10-listener-mode.conf` runs `serve loopback-tcp … --accept-host-local-users`; pid 1883 on `127.0.0.1:8765` |

Background claims checked before citing: memories `minerva-adoption`, `ai-town-cohesion-lanes`;
handoffs `2026-08-22_ai-town-cohesion-findings-claude`,
`2026-08-22_minerva-athena-seam-research-claude`,
`2026-08-23_minerva-adoption-{diagnosis,baseline-measurements}`,
`2026-08-25_ai-town-live-module-shapes`, `2026-08-25_mosaic-datamodel-synthesis`,
`2026-08-29_minerva-claim-search-and-sigterm-claude`.

---

## What the Blueprint already specifies

- Knowledge: claims separate from support; evidence identifies exact spans or digests; findings
  record support, contradiction, uncertainty, supersession; a model summary never erases the source
  (`docs/BLUEPRINT.md:196-199@c59dde1`). Module table: Evidence owns Claims, Evidence, Findings,
  source spans (`:641`).
- Glossary: Claim `CONTEXT.md:182-184`, Evidence `:186-188`, Finding `:190-192`, Mission `:141-145`,
  Assignment `:147-149` (avoid: Lease, ownership), Private/Shared Memory `:219-227`, Memory
  Promotion `:229-231`, Trace `:326-328`, Provenance `:331-333`.
- Memory model: promotion creates a new provenance-linked record and preserves the source; raw Trace
  is not Memory (`docs/BLUEPRINT.md:335-343`; `docs/adr/0007-private-and-shared-agent-memory.md:7-9`).
- Persistence rules that govern every import: no meaningful nulls, imported source words survive
  every mapping, digests carry algorithm and canonicalizer, references keep their source triple
  (`AGENTS.md:198-201`; `docs/BLUEPRINT.md:681-691`). Migration readers are read-only, one-shot,
  fail-closed against live sources (`AGENTS.md:212`; `docs/adr/0001-read-boundary.md:15-27`).
- Alpha runs without Minerva running at all (`docs/BLUEPRINT.md:834-835`); Minerva is imported in
  Phase 3 preserving vocabulary, provenance, timestamps, digest algorithms (`:1009-1013`).

---

## Corrections to prior claims

1. **"ADR 0013 is not deployed" is half stale.** `docs/adr/0013…md:3@9968a5e` says "implementation
   prepared locally, not merged or deployed"; `CHANGELOG.md:42-43` says "This branch has not been
   deployed"; `docs/VISION.md:39-40,55-56` says the mickey runtime "remains on legacy loopback TCP".
   Live: deploy is `acdca45`, which contains `02a4244`; the systemd drop-in and `/proc/1883/cmdline`
   both show `serve loopback-tcp … --accept-host-local-users`, and the HTTP capability document
   reports `listener.mode: loopback_tcp`. ADR 0013's explicit-mode listener **is** live in its
   acknowledged TCP mode; only `owner-uds` is undeployed. `docs/STATUS.md:17@9968a5e` has this right.
   Note the base unit's own `ExecStart` still uses the legacy implicit syntax, which fails closed at
   `acdca45` (`CHANGELOG.md:38-39`) — the drop-in is load-bearing, not cosmetic.
2. **`claim search` is at the pin, not on the live service.** `README.md:192-193,250@9968a5e` and
   `CHANGELOG.md:11-18` describe it; `git ls-tree acdca45 -- src/minerva/claim_search/` returns
   nothing and live `claim --help` has no `search`. The memory `minerva-adoption` ("claim search
   closed it") is true of `main`, not of `:8765`. Any finding about read-back below depends on
   newer-than-deploy code.
3. **ADR 0011's status line is stale.** `docs/adr/0011…md:3` says "not merged or deployed", but
   `97ddf05` is on `main` and in `acdca45`; the live 403 `external_identity_rejected` on `/healthz`
   and `/` is that commit's behaviour.
4. **"claim is an identifier in BOTH systems" is loose.** `git grep -n claim_id 8ddefc8 -- src/athena`
   returns 0 hits. Athena's word lives in `issue_leases.claimed_at`
   (`src/athena/core/migrations/0054_issue_leases.sql:16@8ddefc8`), the table `issue_claim_handoffs`
   (`0058_issue_claim_handoffs.sql:8`), and the desk lane `asks.claim_handoffs`. The rule survives
   sharpened: see Load-bearing subtleties.
5. **Athena has 80 migrations at `8ddefc8`, not 78**; the mission finding is unchanged (below).
6. **"All events are `os-user:kevin-lee`" (memory, 2026-08-23) is superseded by measurement**: the
   ledger now splits 305 / 38 `+seat:claude-code` / 28 `+seat:codex`.

---

## The invariants

Each row: what Minerva enforces · where · the evidence or incident that produced it · what Beacon
must preserve. Paths are relative to the Minerva tree at `9968a5e` unless marked.

| # | Invariant | Where | Produced by | Beacon must preserve |
|---|---|---|---|---|
| 1 | A claim is a falsifiable proposition; its status is a workflow label, never truth, never a score | `src/minerva/core/migrations/0001_research_core.sql:35-47` (`falsification_criteria NOT NULL` at `:40`), `:54-56`; `docs/adr/0001…md:19-22` | Design decision 2026-07-22 | Claim requires criteria; status is a closed capability vocabulary (`docs/BLUEPRINT.md:663`); no confidence derived from counts |
| 2 | Evidence is one exact byte span of one immutable snapshot with a verbatim quote and one stance | `0001…sql:77-93` (BLOB + sha256 + `CHECK(length(content)=byte_length)`), `:95-115`; `src/minerva/evidence/service.py:162-172` re-checks the quote on create; `capabilities.py` reports `citation_scheme: utf8-byte-offset-v1` | ADR 0001: citations must survive the original file changing | Evidence binds (snapshot digest, algorithm, start, end, quote); a mismatch refuses with a stable code |
| 3 | Nothing is updated or deleted; corrections are new records | triggers `0001…sql:152-195`; withdrawals `:117-128`; retractions `0004_finding_retractions.sql:1-15` | **D-9 incident**: withdrawing evidence made the brief permanently unexportable (`docs/adr/0007…md:16-28`) | Every record kind ships with its negative record in the same migration (`docs/adr/0008…md:67-68`) |
| 4 | Supersession is a pointer on the new evidence card; it does not deactivate the old one | `0001…sql:105,113-114`; `README.md:425-428` | ADR 0001 `:20-21` | Supersede ≠ withdraw ≠ retract: three records, three meanings |
| 5 | Model output is a distinct labeled record — never evidence, never a finding, never counted | `0005_agent_inferences.sql:1-6,14-19,55-67`; `docs/adr/0008…md:86-88` | Retyping a candidate laundered machine text into human assertion (ADR 0008 Context) | Model-drafted statements persist with request/response digests and mandatory citations; promotion is a separate append-only link |
| 6 | A mission is a research scope with a CHECK-enforced `mis_` prefix and composite `(id, mission_id)` FKs on every child | `0001…sql:16-23,44-46`; frozen in packet v2 `src/minerva/integrations/research_packet.py:24,294` | ADR 0002 froze v2 as the fleet-facing contract | Scope is a belonging relation; see homonym |
| 7 | "Who asserted this" = OS principal + optional cooperative seat label; the label is attribution, never authentication | `src/minerva/core/types.py:38-56,59-80`; `cli/main.py:781` "recorded as attribution, not proof"; `actor_kind` frozen at `research_packet.py:251` | 229 events collapsed to one actor made agent contribution invisible (handoff 2026-08-23 diagnosis, cause 2); 22-scenario eval `docs/evals/2026-08-24-seat-attribution-workflow.md:32-55` | Attribution appends to the verified principal, never replaces it; no authorization keyed on a self-declared label |
| 8 | HTTP callers may not declare identity at all | `src/minerva/core/attribution.py:18-52,93-106`; `web/security.py:388-392`; `docs/adr/0011…md:65-77` | Web routes silently ignored headers the API refused (`0011…md:24-25`) | One refusal policy before every route family; live 403 today |
| 9 | Ambient provider/proxy identity is refused; the provider destination is pinned | `src/minerva/integrations/ai/anthropic.py:24,26-35,53-54,77-88` | `e1e1f18` 2026-08-24 | A Route (Beacon) never inherits env-supplied auth, proxies, redirects or retries |
| 10 | Unsafe store inputs fail closed | `src/minerva/core/db.py:322-326` (`database_too_new`), `:374-377` (`database_unmanaged`), `:530-544` (symlink), `:472-503` (`O_NOFOLLOW` + `immutable=1`) | `67d8f81` | Same posture for Beacon's migration reader (`docs/adr/0001-read-boundary.md:21-25@c59dde1` already says so) |
| 11 | Backups are sealed, pinned, owner-read-only artifacts; restore verifies a private staged copy | `db.py:30-31` (`0400`/`0600`), `:242-297` (`backup_integrity_mismatch` at `:295`), `:739-753`; `docs/THREAT_MODEL.md:46` "the seal is not a MAC or signature" | `c80d29b`; drill re-run 2026-08-29 (`docs/STATUS.md:20`) | Safety Snapshot integrity check is explicit about what it is not |
| 12 | Listener scope is explicit; owner-only UDS is the standing mode; loopback needs an acknowledgement flag | `src/minerva/web/server.py:32-33,301-302`; `docs/adr/0013…md:26-40`; 20-case eval `docs/evals/2026-08-24-owner-only-uds.md:31-52` | Every host-local account could reach the unauthenticated API (`0013…md:10-14`) | `beacond` transport states its access scope in its capability document |
| 13 | Reads are query-only and complete-or-refuse; a read mutates nothing and leaves no audit event | `src/minerva/claim_search/service.py:9-10,100`; `README.md:544-546` | Deliberate; cost recorded in handoff 2026-08-29 ("read-back is unmeasurable by construction") | See open question 4 |
| 14 | Every mutation writes its audit event in the same transaction; doctor reconciles rows to history | `0001…sql:130-141`; `synthesis/service.py:268-300`; doctor `material_audit_integrity` (399 rows live) | `docs/BLUEPRINT.md:695-698@c59dde1` already mandates this | Keep |
| 15 | An export is a digest-bound record that cannot outlive a failed write; packet digest excludes its envelope | `0002_findings_and_exports.sql:34-44`; `synthesis/service.py:243-321`; `docs/adr/0002…md:40-46` | ADR 0002 | Outcome Document / Export Package carry `export_digest` plus per-file digests |
| 16 | Capability discovery is database-free and names what is unavailable | `src/minerva/capabilities.py:21,229-258`; live `unavailable` includes `sibling_artifact_exchange`, `orchestration`, `approval_authority`, `mcp` | `02a4244` | A module's self-description must list absences, not only presences |

---

## Load-bearing subtleties

**The "mission" homonym.** Minerva's mission is a real entity: `mis_` enforced by
`CHECK(id GLOB 'mis_…')` (`0001…sql:17@9968a5e`), every child table carries `(id, mission_id)`
(`:44-46,91-92,109-114`), and the id is frozen into `minerva.research-brief.v2`
(`research_packet.py:294`). Athena's "mission" is a display string with no referent: at `8ddefc8`,
80 migrations, `git grep -n -i mission 8ddefc8 -- 'src/athena/core/migrations/*.sql'` yields five
hits, all comments or substrings of *permission/admission*; zero tables, zero columns. "Mission
Control" is Jinja text (`src/athena/templates/base.html:66`, `admin/agent_runs.html:3,9`,
`admin/agents.html:14`, `aegis/partials/fleet_attention.html:67`). Athena's real counterpart to a
Minerva mission is **Project** (`0008_projects.sql:8-14`).

Beacon's Mission (`CONTEXT.md:141-145`; `docs/adr/0023-mission-admission.md:7-10`) is the
Athena-shaped meaning made real: finite outcome, one Team, admission bound to a Charter Revision.
That is **not** Minerva's meaning. A Minerva mission has no team, charter, success conditions or
lifecycle; it is the *belonging* scope of claims (the Mosaic data-model table found claims need
`contains` and `scopes` as two relations — handoff `2026-08-25_mosaic-datamodel-synthesis.md` §3.1;
`src/beacon_legacy/store/enums.py:117@c59dde1` records "claims hang off missions; no scope model").
So the mapping is: Minerva `mis_` → the `contains` parent of a Beacon Claim, which may be a Project
or Mission reference in the shared kernel (`docs/BLUEPRINT.md:657`) — **never a Mission↔Mission
mapping**. Importing `mis_` rows as Beacon Missions would mint Missions with no admission evidence,
which `AGENTS.md:198` (no meaningful nulls) forbids. Where the collision still lurks: Briefing
descends from Athena, whose desk and templates say "Mission Control"; that label must not be
imported as a Mission reference, and a Minerva scope id crossing into Briefing keeps its `mis_`
prefix under a foreign name, shape-checked only, never joined.

**The "claim" homonym.** Minerva `clm_` is a falsifiable proposition (`0001…sql:36-40`). Athena's
claim is possession of a work item: "the delegation claim/lease protocol"
(`0054_issue_leases.sql:1-18@8ddefc8`, `claimed_at` at `:16`), `issue_claim_handoffs`
(`0058…sql:8`), and the desk's `how_to_claim` block. Beacon's glossary already resolves the
meanings: Claim is Minerva's (`CONTEXT.md:182-184`), Athena's claim is **Assignment**
(`CONTEXT.md:147-149`, avoid "Lease, ownership"). The collision re-enters through migration:
`AGENTS.md:200` requires imported source words to survive every mapping, so an imported Athena lease
keeps `claimed_at` and `claim_handoffs` in Beacon's store beside Knowledge Claims. Rules that hold
regardless: a Minerva `clm_` never crosses a module boundary under the bare name `claim_id`; an
imported Athena claim-word is always qualified by its source triple (`docs/BLUEPRINT.md:691`);
Briefing never renders an Athena claim handoff under a Knowledge Claim affordance.

**A seat is provenance, not identity.** `normalize_seat` says it in code: "a seat is a *claim* about
which agent ran the command, never proof of one … Do not build authorization on it"
(`types.py:39-47`). `actor_kind` has two values, `os_user` and `system` (`types.py:19-21`,
`research_packet.py:251`); the seat lives only in free-text `actor_id`. A migration that lifts
`+seat:codex` into a Beacon Agent identity would mint identities from unverifiable labels; Beacon's
Agent identity is Operator-admitted (`docs/adr/0020…`, `0021…`).

**"Findings record supersession" is Beacon's addition, not Minerva's.** Minerva's finding kinds are
`observed_fact, source_assertion, agent_inference, assumption, calculation, recommendation,
unresolved_question` and statuses `supported, contested, unsupported, inconclusive`
(`0002…sql:6-12`). Supersession exists on **evidence cards** (`0001…sql:105`); findings are
corrected by retraction plus a new finding (`docs/adr/0007…md:50-53`). `docs/BLUEPRINT.md:197-198`
puts supersession on Findings — a new kind, to be designed, not inherited.

**Memory Promotion has an exact Minerva precedent, and brief export is not it.** Promotion of an
adopted inference into a human finding is a separate append-only link row, one per inference, the
finding is the human's assertion and the inference remains as provenance
(`0005…sql:55-67`; `assist/adoption.py:294-306`; `docs/adr/0008…md:113-118`). That is the shape
`docs/BLUEPRINT.md:339-340` describes. Brief export (`brief_exports`, `0002…sql:34-44`) is a
digest-bound *projection* of a mission — Beacon's Outcome Document or Export Package, not a
promotion. Minerva has no private/shared distinction at all: one OS user, one store.

**Minerva's own sibling-name guard on the other side.** Icarus machine-forbids the words `Athena`,
`Minerva`, `constellation.event.v1` in runtime source (`scripts/security-check.mjs:2006@c919cd2`).
Minerva's `src/` contains zero references to Athena or Icarus (`git grep -i -E 'athena|icarus'
9968a5e -- src/` → 0). Both pillars chose isolation deliberately; Beacon inherits the reason, not
the seam.

---

## The measured case

Ledger paged read-only on 2026-08-29 (`$B audit list --db $DB --limit 100 --after-sequence N`, 371
events, sequences 1-371):

| Event type | Count |
|---|---|
| `research.run.started` | 185 |
| `evidence.card.created` | 56 |
| `research.finding.created` | 29 |
| `source.snapshot.imported` | 25 |
| `research.claim.created` | 23 |
| `database.backup.created` | 19 |
| `research.question.created` | 15 |
| `research.mission.created` | 10 |
| **`synthesis.brief.exported`** | **4** — all on 2026-08-21 (×3) and 2026-08-22 (×1) |
| `research.claim.status_changed` / `lens.candidate.adopted` / `database.initialized` | 3 / 1 / 1 |

Actors: `os-user:kevin-lee` 305 · `+seat:claude-code` 38 · `+seat:codex` 28. Events per day
2026-08-21→29: 171 · 56 · 34 · 44 · 16 · 2 · 2 · 30 · 16. `doctor --deep`: 12/12 checks pass.

Three findings:

1. **Writes turned after the 2026-08-23 discoverability fix** (skill + PATH wrapper; handoff
   `2026-08-23_minerva-adoption-diagnosis-claude.md`): four of the ten missions were filed by a seat
   label, about Chronos, Athena and the review pipeline — not about building Minerva. The pre-fix
   record was 108 of 114 runs from the build loop (`…baseline-measurements.md`).
2. **`brief_exports` is still 4, unchanged for seven days.** This is the gating measurement the
   2026-08-22 seam verdict asked for, re-run. Every reader was a person or the authoring session.
3. **Read-back cannot be measured**: reads emit no audit event (invariant 13). The only surface that
   could answer "did anyone re-check this?" is absent by design, and `claim search` — the fix for
   "a returning session cannot find a claim without knowing its mission" — is not yet deployed.

---

## Dead seams — tried and closed

- **D-2, Athena keypair.** ADRs 0009 and 0010 are Proposed, "not accepted, not implemented"
  (`docs/adr/0009…md:3-14`, `0010…md:3-12`); precondition "Athena can hold a keypair" unmet, five
  gap items (`0010…md:40-50`). Decision 0 keeps it closed (`docs/DECISIONS.md:950`;
  `docs/STATUS.md:62-63`; `docs/NEXT_LEVEL_PLAN.md:50-51`). For Beacon the question dissolves: there
  is no second system to sign for. What survives is ADR 0009's premise that transport identity must
  be authenticated *before* it maps to a local actor (`docs/adr/0002…md:125-128`).
- **D-3, Icarus exchange.** Proposed after D-2 (`docs/ROADMAP.md:409-411`); ADR 0002 already ruled
  "execution output is not automatically evidence" (`0002…md:79-81`) — that rule is the inheritance.
- **The automatic Athena/Icarus seam.** Eight-agent verdict 2026-08-22: build nothing; both designs
  needed a new source kind in Athena's HMAC-before-parse path plus a courier nobody owned, to move
  an unmeasured trickle. Measured 2026-08-23 and again today: 4 exports, human readers. The
  2026-08-23 adoption push built no seam on purpose. `docs/BLUEPRINT.md:834-835` makes it moot.
- **`claude/minerva-next-level`.** One commit `b524901` over `ee4edb6` ("snapshot: codex next-level
  working tree (uncommitted)", 51 files, +4790), **not an ancestor of `main`**; its content landed
  separately as `bfe5a2c`. A review-only snapshot, not a line of development.
- **Vulcan routing.** `anthropic.py:24` pins `https://api.anthropic.com`; `:94-99` sends
  `output_config.format.type = json_schema`. Vulcan's request schema is `extra="forbid"`
  (`src/vulcan/schemas.py:24@56ba96e`) and lists `response_format` as out of scope
  (`docs/ARCHITECTURE.md:419-420@56ba96e`). Blocker stands; Knowledge's assistance path cannot
  route through the fleet gateway until Beacon's Inference module admits structured output.
- **The fleet wrapper.** `~/.local/bin/minerva capabilities` still returns `usage_error` (it injects
  `--db`); the venv binary works. Reported 2026-08-25 as a Janus gate; unfixed. Fleet infra, not
  Minerva, and irrelevant once Beacon owns discovery.
- **"Mission Control" → "Fleet Control" rename.** Recommended 2026-08-22; never done — templates at
  `8ddefc8` still say Mission Control. Moot for Beacon, which does not carry Athena's templates.

---

## Consequences for Beacon's Knowledge

Numbered so a reviewer can test each one.

1. A Claim record requires non-empty falsification criteria; creation without them refuses
   (`0001…sql:40`).
2. Evidence stores snapshot digest + algorithm, half-open byte offsets, verbatim quote, one stance;
   the store re-derives the quote from the snapshot on create and on export and refuses a mismatch
   with a stable code (`evidence/service.py:162-172`).
3. Snapshots are immutable, content-addressed rows; Evidence references a snapshot, never a path.
4. Claim, Evidence, Finding tables reject UPDATE and DELETE; a test proves it (`0001…sql:152-195`).
5. Withdrawal, retraction, and supersession are three distinct records; each ships in the same
   migration as the record it corrects (the D-9 lesson, `docs/adr/0008…md:67-68`).
6. A model-drafted statement is its own record kind with request/response digests, prompt version,
   and mandatory citations; it never counts toward Claim status (`0005…sql`).
7. Claim status is a closed, capability-specific vocabulary with no derived confidence.
8. Attribution records the verified principal and the Run; a cooperative label may be appended,
   never substituted, and never authorizes anything (`types.py:59-80`).
9. Knowledge's interface accepts no identity header or seat parameter; identity comes from
   `beacond` (`attribution.py:93-106`).
10. Reads are query-only and complete-or-refuse; whether Shared Memory reads are recorded is open
    question 4.
11. Corpus-wide Claim location exists from the first slice: a claim store whose only entry is
    per-scope is write-only in practice (`claim_search/service.py:1-11`; `CHANGELOG.md:11-18`).
12. The Minerva migration reader opens only a sealed `0400` backup with `immutable=1`, refuses
    unsealed files, and records the backup seal digest as the import's source (`db.py:472-503`).
13. Memory Promotion writes (source id, source digest, promoter, run, time) as a new link row and
    leaves the source untouched (`0005…sql:55-67`).
14. No Mission↔Mission mapping for Minerva imports; no bare `claim_id` crosses Briefing/Knowledge.
15. Knowledge's capability document lists what is unavailable as well as what is
    (`capabilities.py:258`).
16. Import preserves `actor_id` verbatim as a source word with `actor_kind = os_user`; seat labels
    are not resolved to Agents without a Kevin ruling (open question 5).

---

## Open questions — Kevin's, not ours

1. **Visibility of migrated Minerva claims.** Minerva has no visibility model — one OS user, one
   store. Beacon's store gives every record one (`docs/BLUEPRINT.md:688`; Mosaic synthesis §4).
   Migrated claims acquire a Private/Shared status with no source to derive it from. The table's
   recommendation was the most-restrictive default, named as a decision, not inherited.
2. **What a migrated Minerva mission becomes.** A Beacon Mission requires admission evidence
   (`docs/adr/0023…md:7-10`) that never existed. Import as a Project-scoped Knowledge scope, or as
   a Mission with an explicit `legacy` lifecycle value? Either contradicts nothing accepted, but
   `AGENTS.md:198` forbids the silent middle.
3. **Supersession on Findings.** `docs/BLUEPRINT.md:197-198` adds a finding kind Minerva never had.
   Inherit evidence-card supersession only, or design finding supersession with its own negative
   record?
4. **Recording reads.** Minerva's zero-mutation reads made adoption unmeasurable. Beacon already
   records Operator drill-down into Private Memory (`docs/BLUEPRINT.md:336`). Does Shared Memory
   read-back belong in Activity too, and at what granularity?
5. **Seat labels → Agents.** Do `+seat:codex` and `+seat:claude-code` import as Beacon Agent
   identities? ADR 0011 says a seat is not identity; `docs/adr/0020…`/`0021…` make Agents
   Operator-admitted. Importing them as Agents would be a new admission path.
6. **Structured output in Inference.** Knowledge assistance needs a JSON-schema response format
   that Vulcan refuses today. Whether Beacon's Route admits it is an Inference decision with a
   Knowledge dependency.
