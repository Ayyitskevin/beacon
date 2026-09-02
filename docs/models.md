# Models findings inherited by Beacon

**Status:** draft for review · **Produced by:** claude-code harvest agent ·
**Last verified:** 2026-08-29

BLUEPRINT §3 says Models is "descended from Vulcan". This page re-derives what that descent has
to carry, from Vulcan's code at a pinned revision and from the live gateway on mickey, so the
single Beacon seat does not re-learn from taste what the fleet already paid to learn. Findings,
not decisions. Every rule cites `path:line@pin`; anything Kevin has to rule on is at the end.

Vocabulary map (Vulcan → Beacon): alias → Route · seat → Agent · provider → provider ·
hosted alias → Cloud Route · `[budgets]` → Budget Envelope · usage ledger → Invocation ledger ·
`keep_alive`/`/api/ps` → Worker health of activated local inference.

---

## How this page was produced

Read-only throughout: no checkout, no chat completion, no secret file opened.

| Tree | Pin | What was read | Command form |
|---|---|---|---|
| Vulcan `~/ai-workspace/vulcan` | `origin/main` = `56ba96e` | README, docs/ARCHITECTURE, docs/ROADMAP, CONTRIBUTING, CHANGELOG, `src/vulcan/{schemas,config,budgets,usage,gateway,errors,api}.py`, `providers/{base,http,ollama,openai_compatible,anthropic}.py`, `deploy/*`, `tests/*`, `.github/workflows/ci.yml`, `docs/evals/2026-08-28-provider-response-bounds.md`, last 30 commit messages | `git -C <tree> show 56ba96e:<path>`, `git -C <tree> grep -n <pat> 56ba96e -- <paths>`, `git log origin/main --oneline -30` |
| Vulcan deploy `~/deploy/vulcan` | `cbc5066` (10 behind main: `git rev-list --count cbc5066..56ba96e` → 10) | HEAD only | `git -C ~/deploy/vulcan rev-parse HEAD` |
| Icarus `~/ai-workspace/claude/icarus` | `origin/main` = `c919cd2` | `packages/core/src/providers.ts`, `live-evidence-profile.ts`, ADR 0056, ADR 0065, `docs/evals/2026-08-27-headless-vulcan-admission.md`, gate2 eval artifacts, `tests/provider/vulcan-chat-completions-gateway.test.ts` | `git show c919cd2:<path>`, `gh pr view 51/69/70` |
| Beacon `~/ai-workspace/claude/beacon` | `c59dde1` | README, CONTEXT, AGENTS, BLUEPRINT §Models (221-229) and §Nested Budget Envelopes (430-436), ADR 0009, docs/authority.md, `crates/beacond/src/{resource_discovery,local_model_activation}.rs`, phase-0 activation checkpoint | `sed -n`, `grep -n` |
| Live roster | `~/deploy/vulcan-data/vulcan.toml` (mtime 2026-08-26 13:23) | whole file; no values resembled a key | `cat` |
| Other consumers | `~/ai-workspace/buzz/run-qwen-agent.sh`; `~/deploy/minerva` @ `acdca45` | lines 1-15; `integrations/ai/anthropic.py` grep | `sed -n`, `grep -n` |

Live probes (all GET / read-only, 2026-08-29 ~10:40 EDT):
`curl -fsS http://127.0.0.1:8140/healthz` · `.../v1/models` · `.../v1/capabilities` ·
`.../v1/usage` · `systemctl status vulcan.service --no-pager` · `systemctl cat vulcan.service` ·
`systemctl show vulcan.service -p DropInPaths -p ProtectHome -p WatchdogUSec -p Type` ·
`diff <(git show 56ba96e:deploy/vulcan.service) /etc/systemd/system/vulcan.service` ·
`ls -l ~/deploy/vulcan-data/` · `curl -s localhost:11434/api/ps` ·
`systemctl show ollama.service -p Environment`.
Background claims were checked against the six memory files and two handoffs named in the brief;
where a memory disagreed with code or the live box, the code wins and the memory is not cited.

---

## What the Blueprint already specifies

`docs/BLUEPRINT.md:223-229@c59dde1` — Models owns provider connections, Routes, eligibility,
privacy constraints, budgets, live usage, and "the replayable Invocation ledger"; local inference
preferred; discovered ≠ eligible; cloud provider + Route + data scope enabled by Project Policy;
every Invocation attributed to Agent, Project, Mission, Route, provider, model; no silent fallback.
`docs/BLUEPRINT.md:432-436@c59dde1` — Envelopes nest Workspace → Project → Mission → Agent,
children fit parents, use rolls up, reservations may be required before concurrent work.
`docs/BLUEPRINT.md:642@c59dde1` — Inference module owns "Routes, Invocations, usage, provider
health"; interface "invoke, inspect route, read usage". `:896-898`, `:919`, `:1093`, `:1153-1158`
restate the refusal semantics as acceptance criteria and non-goals ("opaque automatic model
fallback"). `docs/adr/0009-local-first-explicit-cloud-routes.md@c59dde1` is the accepted decision.
`AGENTS.md@c59dde1` invariants: "Discovery grants no capability", "Activated local inference is
preferred. Cloud Routes require explicit Project permission", "No model, provider, check, or tool
fallback is silent", "Terminal, absent, broken, and not evaluated are distinct states".
Already built: loopback-only discovery (`crates/beacond/src/resource_discovery.rs:73-80`) and
digest-rechecked activation with an approvable Provider Feature subset
(`crates/beacond/src/local_model_activation.rs:24,57-62,205-260`) — Beacon already models
`tools`/`thinking` as Provider Features, which Vulcan never did (see open question 4).

What the Blueprint does NOT yet say, and Vulcan does: how a refusal is typed; that a budget check
must *reserve* atomically; that token counts arrive after completion; what a ledger line contains
and what it deliberately omits; that provider bodies are a trust boundary with a byte ceiling; and
what "provider health" may cost to compute. Those are the findings below.

---

## Corrections to prior claims

The 2026-08-22 cohesion audit
(`shared/handoffs/2026-08-22_ai-town-cohesion-findings-claude.md:46-55`) found: *Vulcan cannot
serve either of its two natural consumers — Buzz seats need tool-calling,
Minerva needs structured output, `extra="forbid"` rejects both; the ledger holds 20 lines, all
manual probes.* Re-measured today:

1. **Schema claim CONFIRMED, unchanged.** `src/vulcan/schemas.py:23-24@56ba96e` is still
   `ConfigDict(extra="forbid")` and `ChatCompletionRequest` (`:45-53`) admits exactly `model`,
   `messages`, `temperature`, `max_tokens`, `stream`, `seat`. Three commits touched the file since
   08-22 (`7840305` class label, `937e259` derived capabilities, `41f97d4` untracked-seat
   counter); none admitted `tools`, `tool_choice`, or `response_format`. `README.md:278-279` and
   `docs/ARCHITECTURE.md:412,420` still list them as rejected / deferred;
   `tests/test_schemas.py:57-64@56ba96e` pins `tools` → `extra_forbidden`.
2. **"No consumers" claim CORRECTED.** The ledger is 2,992 records, not 20 (`/v1/usage` totals
   `requests: 2992`; `ls -l` 378,733 bytes). Seats today: `oracle-index` 1259, `oracle-query`
   1228, `icarus` 314, `claude-code` 90, plus 10 smaller. Icarus merged a Vulcan provider the day
   after the audit (PR #51, 2026-08-23T02:08Z; #69 and #70 on 08-27) and gets structured output
   *without* `response_format`: the JSON Schema travels in the system message and the reply
   faces Icarus's own validators (`packages/core/src/providers.ts:474-483,512-525@c919cd2`).
   The Icarus adapter always sends `seat: "icarus"` (`providers.ts:472`). The consumer adapted to
   the gateway's strictness; the gateway did not loosen.
3. **Buzz and Minerva bypass CONFIRMED — but one is now a ruling, not a gap.**
   `~/ai-workspace/buzz/run-qwen-agent.sh:4-11` still routes tool loops straight at Ollama and
   records that Kevin ratified this on 2026-08-22 as the "SANCTIONED bypass-with-attribution
   split": tool loops direct, any non-tool seat inference through Vulcan with the seat label.
   Minerva (`~/deploy/minerva` @ `acdca45`, `integrations/ai/anthropic.py:24,94-97`) still
   hardcodes `api.anthropic.com` and sends `output_config.format.type = "json_schema"`.
4. **A fourth consumer the audit could not see:** Icarus's Gate 1 live-evidence profile
   *deliberately refuses* a Vulcan pin
   (`packages/core/src/live-evidence-profile.ts:255-261@c919cd2`, ADR 0056) because hosted
   aliases are metered by Vulcan's own ledger, so an Icarus-computed spend
   ceiling "would claim a bound it cannot observe". ADR 0065 then admits Vulcan to headless
   *proposal-only* work by forcing explicit positive token rates so Icarus can reserve its own
   worst-case cost. That is the seam Beacon dissolves by owning both sides — see consequence 3.

Verdict: **partially holds.** Strictness is intact; "unusable" was false within 24 hours.

---

## The invariants

| # | Invariant | Where (`@56ba96e` unless noted) | Incident / reason that produced it | Beacon must preserve |
|---|---|---|---|---|
| 1 | One alias → exactly one (provider, native model); a provider failure is that provider's typed error, never a retry, fallback, or fabricated reply; at most one upstream call per request | `README.md:15-17,26-29,590-591`; `docs/ARCHITECTURE.md:168`; `docs/ROADMAP.md:66`; `gateway.py:169-170`; `tests/test_routing.py:145,225` | Design invariant from v1; live example: `mid`/`agentic-fast` aliases were *deleted* not repointed when their models left ("no remaining local model honestly fills either role", `vulcan.toml:17-21` live) | A Route resolves to one (provider, model) per Invocation; refusal is typed; no second provider is contacted (`AGENTS.md` "no fallback is silent", ADR 0009) |
| 2 | Hosted calls require a seat and a resolvable budget entry — fail-closed on both; local is never budgeted | `budgets.py:85-110`; `config.py:286-294`; `errors.py:114-141` (400 `seat_required`, 403 `budget_unconfigured`, 429 `budget_exhausted` + reset); `README.md:30-32,525-533`; `tests/test_budgets.py:68-80,339,361` | Gate 5, 2026-08-16; BYOK keys landed 08-21 into an already-armed fail-closed section, so keys "can never be used unbudgeted once exported" (`vulcan.toml:262-266` live) | A Cloud Route Invocation without an attributed Agent *and* a Project grant fails before the upstream call (BLUEPRINT:896) |
| 3 | Budget check and slot reservation are one atomic step; settle on success, release on failure/cancel/disconnect, in one lifecycle; request cap REQUIRED because tokens arrive after completion | `budgets.py:85-136,138-175`; `config.py:277-283`; `gateway.py:321-375` (`_lifecycle`), `:352-356`; `tests/test_budgets.py:115,128,613,633,662` | Gate 5 review rounds (PR #19): final-chunk abandonment used to escape the meter; the copy-pasted scaffold drifted until `ef836ea` (2026-08-28) unified it | Envelope reservation before dispatch (BLUEPRINT:435-436), release on every exit path, settle exactly once — one code path for buffered, streamed, and embedding Invocations |
| 4 | Budgets require the durable ledger; a restart never resets an allowance | `config.py:317-327`; `budgets.py:177-209`; `tests/test_budgets.py:392,470` | "restart-proof allowances are the contract, not a mode" (`ARCHITECTURE.md:293-311`) | Envelope spend is derived from the persisted Invocation ledger, never from process memory |
| 5 | Ledger: append-only, one line per *completed* request, exact key set, never content or native model names, 0600, single writer (flock), replay validates every line, poison → `skipped_lines`, unopenable → startup fails loud | `usage.py:37-46,49-80,158-191,193-241`; `tests/test_ledger.py:128,135,200,327,346` | Gate 4 PR #17 hardened replay post-merge; `2977ba4` (08-28) made new ledgers 0600 | Invocation records are complete on success only; the store is a trust boundary on read as well as write; missing/corrupt history is counted, never guessed |
| 6 | Usage honesty: token totals only meaningful with `requests_with_usage`; counts never invented; reasoning-token breakdowns never double-counted | `usage.py:12-14,99-105`; `schemas.py:235-243`; `README.md:378-398`; `tests/test_reasoning_content.py:157,363` | Providers omit counts; DeepSeek reports reasoning tokens separately | Invocation `usage` is nullable and distinct from "unknown"; Briefing spend never sums nulls as zero silently |
| 7 | Vendor extension fields are dropped: `reasoning_content` never forwarded, never substituted for a missing answer | `README.md:378-398`; `ARCHITECTURE.md:377-383`; `openai_compatible.py:80-81`; `tests/test_reasoning_content.py:143-243` | Surfacing "would be a contract change with its own privacy review" | Trace/Receipt content is what the provider *returned as answer*; hidden reasoning is a separate ruling (open question 1) |
| 8 | Upstream bodies are classification-only; every decoded response is capped at 16 MiB; stalled streams time out; truncated streams are protocol errors, not success | `providers/http.py:35-38,103-118,137-195,241-264,322-338`; `ARCHITECTURE.md:148,216`; `17980d1`, `0222655`, `58ee2c0`; `tests/test_provider_response_bounds.py:71,259,296`; `tests/test_streaming.py:1066` | Codex "Daybreak" hardening 08-28/29: a slow-drip upstream "would otherwise hold an admission slot and budget reservation forever" | Provider adapters are a trust boundary: bounded bytes, bounded idle, status-only error classification, close-on-refuse |
| 9 | Hosted providers are never probed automatically; `unchecked` is a legitimate health state; preflight probes only the routed provider | `ARCHITECTURE.md:174`; `ROADMAP.md:73`; `gateway.py:440-446`; `56ba96e` commit message; `tests/test_routing.py:305` | Rendering `/healthz` must not cost money | "Provider health" (BLUEPRINT:642) has a not-evaluated state that is neither absent nor broken (`AGENTS.md` distinct-states invariant) |
| 10 | Concurrency bound is admission at the door — typed 503, never a queue, retry, or reroute; liveness endpoints never gated | `gateway.py:148-163`; `config.py:43-45`; `ARCHITECTURE.md:313`; `tests/test_concurrency.py:110,205` | 2026-08-25/26: comment claimed excess requests "queue"; corrected — gateway never queues, Ollama does (`vulcan.toml:38-47` live) | Dispatch admission is explicit backpressure; the Worker's own queue is a second layer, not the same one |
| 11 | Config is strict (`extra="forbid"`), no env overrides, unknown key = startup failure; deployed code validates live config before restart | `config.py:26-29`; `ROADMAP.md:75`; `deploy/update.sh:44-54`; `tests/test_deploy_scripts.py:118-143` | 2026-08-23: a 365-char `description` (limit 240, `config.py:207`) took the service down ~3 min; a key landing ahead of code would do the same | Store migrations and config are reconciled by the *deployed* beacond before it restarts; unknown fields are refusals, not warnings |
| 12 | Machine-local unit state lives in a drop-in; the reference unit is replaced wholesale on update | `deploy/vulcan.service.d/local.conf:1-19`; `deploy/README.md` "Hosted BYOK keys" | 2026-08-23: a unit `cp` reverted `EnvironmentFile`, every BYOK alias returned `missing_credential` | Beacon's installer/updater must never clobber operator-local service state (secrets stay in the OS store per `AGENTS.md`) |
| 13 | Seat labels are voluntary metadata: never forwarded upstream, never in logs/errors, capped at 4096 distinct | `schemas.py:51-53`; `budgets.py:1-14,26-28`; `usage.py:43-46,423-436`; `tests/test_seat.py:242-475` | "Guardrail against accidents, not adversaries" — a seat can lie about its name | Agent identity in Beacon is authenticated by beacond, so the Envelope keys on identity, not declaration (load-bearing subtlety B) |
| 14 | Residency is a per-alias knob passed through untouched; rejected on non-Ollama providers at load | `config.py:212-232,382-388`; `providers/ollama.py:145-148,257-259`; `vulcan.toml:143-200` live | Guard-model pinning crash path (dead seam 2) | Worker health of activated local inference includes residency policy per Route |

---

## Load-bearing subtleties

**A. Strictness attracted the consumer it was said to exclude.** The 08-22 audit read
`extra="forbid"` as the reason nothing routed through Vulcan. Icarus routed through it 314 times
by the time of this page, precisely *because* the contract is closed: a loopback gateway that
holds no credential and admits no `tools` is something Icarus could admit to headless
proposal-only work with a bounded threat model (ADR 0065). The lesson is not "forbid everything";
it is that a closed request contract is a *feature consumers can reason about*, and structured
output does not require a `response_format` field when the caller validates downstream.

**B. Vulcan's budget is keyed on self-declaration; Beacon's must not be.** `budgets.py:3-5` says
it plainly: seats are voluntary labels, so the budget "cannot stop a caller that lies about its
name". Vulcan accepts this because loopback + single operator is its whole auth model
(`README.md:38-39`). Beacon has authenticated Agents and one store writer, so the same fail-closed
mechanics can be keyed on identity. Carry the mechanics (atomic reserve, release-on-exit, request
cap bounds token overshoot); do not carry the trust assumption.

**C. Two different ceilings are both real.** Vulcan's `[budgets]` is a *rate* ceiling (per-seat,
per-UTC-day, `budgets.py:25,72-80`). Beacon's Envelope is a *scope* ceiling (per Mission/Agent
lifetime). The GLM lesson (memory `glm-security-lens`: "Rate, not budget, is the binding limit" —
a ~50k-token review hit HTTP 429 with a negligible fraction of the wallet spent) says a scope
ceiling alone will not protect a Cloud Route from provider rate limits. Whether Beacon nests a
daily window inside the Envelope or leaves it to Policy is open question 3.

**D. Local inference capacity is per-model, not per-box, and two of the "facts" were wrong once.**
Measured on mickey (`shared/handoffs/2026-08-25_ollama-vulcan-tuning.md:27-45,82-94`; memory
`ollama-tuning-measured`): Ollama's `-c` is per-slot × `NUM_PARALLEL`, so KV scales *linearly*
with parallelism (muse-glimmer np=2 → 1933.75 MiB, np=4 → 3867.50 MiB — the earlier "free"
claim was falsified on 08-26 by a second data point); KV is per model (qwen3.8:27b ≈ 9.5 GiB
at 262144
ctx incl. the MTP draft, ornith-1.5:35b ≈ 2.7 GiB); and qwen3.8:27b runs `-np 1` regardless of
the env var because MTP speculative decoding forces it — that model serves one request at a time
no matter what the gateway admits. Nothing enforces `max_concurrent_requests` ↔ `NUM_PARALLEL`
coupling (`vulcan.toml:38-48` live; handoff `:93-94`); they were hand-aligned at 4. Beacon's
Worker health must *derive* capacity from the runtime, not carry a constant.

**E. Health that never fails is health that hides.** `/healthz` returns `ok` while a backend is
dead by design (a dead provider is `unavailable`, not a gateway failure), so `ef836ea` added
`provider_availability_changed` log events because "a provider dying went unnoticed unless someone
was polling and diffing". Beacon's Briefing needs the *transition*, not just the state.

**F. The crash was automation reloading into a full box.** 2026-08-17 23:22: an 83.69 GiB tenant
was legitimately resident, the Buzz seats had been stopped, but `ollama-warm-trio.timer` had not —
it tried to load qwen3.8:27b at 262144 ctx into 17.2 GiB free and the failed KV allocation took
the kernel down through amdgpu, not an OOM kill
(`shared/handoffs/2026-08-18_session-wrap-mickey-crash-hardening.md`, "The one-paragraph
version"). The fix class: `inference-preflight --need <GiB> --assert` gates
every large load against a memory floor in `~/.config/inference-limits.env` (present on this box,
`~/.local/bin/inference-preflight`). Four defects that session were "a check that
reported fine while being weaker than it looked" — the durable fix was to make the unknown/failed
state louder than the healthy one. Beacon's Worker health for activated local inference must model
free memory and resident models *before* a Route may dispatch a load, and Pause Work must reach
the warm/reload automation, not only the Runs.

---

## The measured case

All numbers observed 2026-08-29 ~10:40 EDT on mickey; commands are re-runnable as written.

| Measurement | Command | Observed |
|---|---|---|
| Gateway health | `curl -fsS http://127.0.0.1:8140/healthz` | `status: ok`, `api_version: v1`, 5 providers (`local-ollama` **available**; `anthropic`, `moonshot`, `zai`, `xai` **unchecked**), `models_configured: 17` |
| Routes | `curl -fsS .../v1/models` | 17 aliases: 11 local (8 chat, 3 embeddings, all `available`), 6 Cloud Routes (`unchecked`); classes `code`, `code-fast`, `simple`, `utility`, `vision-work`, `medical`, `safety`×2, `embed`×3, `hosted-chat`×6 |
| Capabilities | `curl -fsS .../v1/capabilities` | `callable_capabilities: [chat, embeddings]`; embeddings `max_inputs 64`, `max_input_characters 8192` |
| Ledger | `curl -fsS .../v1/usage` | `scope: ledger`, `requests: 2992`, `requests_with_usage: 2992`, `prompt_tokens: 6,176,643`, `completion_tokens: 749,380`; `replayed_requests: 2734`, `skipped_lines: 0`, `write_failures: 0` |
| Ledger file | `ls -l ~/deploy/vulcan-data/usage-ledger.jsonl` | 378,733 bytes, mode `-rw-rw-r--` (predates the 0600-on-create rule in `2977ba4`; existing files are not re-chmodded) |
| Who spends | `/v1/usage` `by_seat` / `by_provider` | `oracle-index` 1259 · `oracle-query` 1228 · `icarus` 314 · `claude-code` 90 · 10 others; `local-ollama` 2975 vs hosted 17 total (`zai` 8, `moonshot` 5, `anthropic` 2, `xai` 2) |
| Budget state | `/v1/usage` `budgets` | one row today (`glm-preflight`: 0/500 requests, 0/100000 tokens, resets 1788048000); ceilings from `vulcan.toml:269-271` |
| Unit as installed | `systemctl show vulcan.service -p Type -p WatchdogUSec -p ProtectHome -p DropInPaths` | `Type=notify`, `WatchdogUSec=30s`, **`ProtectHome=no`**, `DropInPaths=/etc/systemd/system/vulcan.service.d/local.conf`; active since 2026-08-28 08:47 EDT |
| Unit drift | `diff <(git show 56ba96e:deploy/vulcan.service) /etc/systemd/system/vulcan.service` | main adds `ProtectHome=read-only`, `LockPersonality`, `RestrictRealtime`, `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6` (`deploy/vulcan.service:44-57@56ba96e`); **none installed** — deploy is `cbc5066`, and `deploy/update.sh` never copies the unit |
| Local runtime | `curl -s localhost:11434/api/ps`; `systemctl show ollama -p Environment` | resident: `qwen3.8:27b` 17.1 GiB, `ornith-1.5:35b` 23.1 GiB (both pinned, `expires_at` year 2318); `OLLAMA_NUM_PARALLEL=4`, `OLLAMA_KEEP_ALIVE=-1`, `OLLAMA_MAX_LOADED_MODELS=4`, `OLLAMA_CONTEXT_LENGTH=262144` |

Two readings matter. First, the ledger's largest consumers are automation (ORACLE's semantic
index and Icarus), so "all manual probes" is dead. Second, the hardened unit is a **verified fix
that is unshipped**: it passed review and CI on 08-28 (`2977ba4`), it is on `main`, and the
running service does not have it. Nothing in the deploy flow surfaces that gap.

---

## Dead seams — tried and closed

1. **Hosted-alias metering as the fleet usage digest.** `scripts/usage_reporter.py` +
   `deploy/vulcan-usage-reporter.timer` post a daily diff of `/v1/usage` into Athena's forge
   ingest on standing issue MWS-13 by speaking the `github` forge dialect and naming an issue key
   (`deploy/README.md` "Usage reporter"). Live: `usage-reporter-state.json` updated 2026-08-29
   06:17. It works and it is a courier between two stores. In Beacon the Briefing reads the
   Invocation ledger it already owns; the courier, the forge dialect, and the "seat name ending in
   `-<digits>` looks like an issue key" quirk all die.
2. **Guard-model pinning crash path.** `OLLAMA_KEEP_ALIVE=-1` + `MAX_LOADED_MODELS=4` meant one
   `safety` call parked 12.9-15.5 GiB in the 4th slot forever, putting the box in the 08-17 OOM
   zone. Closed per alias: `keep_alive = "5m"` on `safety`, `safety-second`, `medical`, and all
   three embedders (`vulcan.toml:143-200` live); only the trio inherits the global pin. Vulcan
   honors it on chat and embed paths (`providers/ollama.py:145-148,257-259`).
3. **Retired aliases deleted, not repointed.** `mid` and `agentic-fast` were removed when their
   models left rather than pointed at a weaker model (`vulcan.toml:17-21`). A Route whose model is
   gone becomes absent, not silently different.
4. **Process-scope usage** (`scope: process`) is superseded by ledger scope; kept only as the
   behaviour when `[usage]` is absent (`schemas.py:283-285`).
5. **Icarus Gate 1 ↔ Vulcan money binding.** Closed on the Icarus side (Corrections, item 4):
   a consumer cannot bind a spend ceiling it cannot observe. In Beacon the
   Ruling and the Invocation ledger are in one store, so this seam does not need a courier — but
   only if cost is recorded on the Invocation the Ruling binds.
6. **Buzz tool loops bypass the gateway with attribution** (kind:44200 usage events,
   `run-qwen-agent.sh:10-11`). A sanctioned split for AI Town; in Beacon there is no second path —
   "every Invocation is attributed" (BLUEPRINT:227) means the Inference module must be the
   tool-caller (open question 4).
7. **SIGHUP reload, in-app log streaming, runtime routing mutation, `/metrics` without a scraper,
   automatic ledger rotation** — refused or trigger-gated (`ROADMAP.md` §11 tail). `vulcan
   ledger-truncate` shipped instead (`usage.py:277-344`).

---

## Consequences for Beacon's Models

Each item is testable; the cited Vulcan test is the shape to mirror.

1. **One Route, one (provider, model) per Invocation.** A provider failure yields a typed error
   and zero calls to any other provider. Mirror `tests/test_routing.py:145,225@56ba96e`.
2. **Cloud Route refusal happens before the upstream call** and is keyed on authenticated Agent
   identity + Project grant, not on a caller-supplied label. Mirror `tests/test_budgets.py:74,80`
   with identity supplied by beacond rather than the request body.
3. **Reservation is atomic with the check; every exit path releases; settle happens exactly once
   before the terminal chunk.** One lifecycle for buffered, streamed, and embedding Invocations
   (`gateway.py:321-375`). Mirror `tests/test_budgets.py:115,128,613,633,662`.
4. **Envelopes bound concurrency as well as tokens** — tokens are known only after completion, so
   the in-flight count is what bounds overshoot (`config.py:277-283`). BLUEPRINT:435-436's
   "Policy may require reservations" becomes "the Envelope requires them for Cloud Routes".
5. **Envelope spend is replayed from the persisted Invocation ledger at boot;** a process restart
   can never reset an allowance. Mirror `tests/test_budgets.py:392`.
6. **Invocation records are complete-on-success, content-free, exact-schema on read.** A
   corrupt or foreign record is counted as skipped and never reaches a Briefing number
   (`usage.py:49-80`). Unopenable store → beacond refuses to start. Mirror
   `tests/test_ledger.py:128,327`.
7. **Usage has three states: reported, not reported, and untracked** (`requests_with_usage`,
   `untracked_seat_requests`). Sums over nulls are labelled as partial.
8. **Provider adapters bound decoded bytes (16 MiB) and inter-chunk idle time; both are typed
   errors; truncated streams are failures, not completions** (`http.py:38,241-264`; `17980d1`).
   Mirror `tests/test_provider_response_bounds.py:71,259,296`, `tests/test_streaming.py:1066`.
9. **Upstream error bodies are classified by status only and never surfaced**
   (`http.py:322-338`); prompts, keys, and bodies have leak-sentinel tests on every surface
   (`CONTRIBUTING.md`; `tests/test_seat.py:242-475`).
10. **Provider health never costs money to render.** Cloud Routes report `unchecked` until a real
    Invocation uses them; only an explicit Operator action may probe (`ARCHITECTURE.md:174`;
    `providers/http.py:270-281`). Health transitions are events, not just states (`ef836ea`).
11. **Worker health for activated local inference models capacity:** free memory vs. a floor,
    resident models, per-model KV at the configured context × parallelism, per-model concurrency
    (MTP → 1), and residency policy per Route. A load that would not fit is refused before it is
    attempted (subtleties D and F). Admission at the gateway is derived from the Worker's reported
    parallelism, not a hand-maintained constant.
12. **Admission is backpressure, never a queue;** liveness/discovery endpoints are never gated.
    Mirror `tests/test_concurrency.py:110,205`.
13. **Config and store schema are `deny_unknown_fields`, and the deployed beacond validates them
    before it restarts** (`deploy/update.sh:44-54`; `tests/test_deploy_scripts.py:118-143`).
    Beacon already uses `deny_unknown_fields` on activation records
    (`local_model_activation.rs:56,66,98`).
14. **"Verified but unshipped" is a visible state.** The hardened unit on `main` is not installed
    (measured case). Beacon's update channel (`AGENTS.md`: Stable/Preview, never during active
    Runs) should show the gap between the reviewed revision and the running one.
15. **Route class vocabulary is operator-owned and executable-first.** Vulcan's classes (`code`,
    `code-fast`, `simple`, `utility`, `vision-work`, `medical`, `safety`, `embed`, `hosted-chat`)
    are labels "never used for routing" (`config.py:208-211`); the TOML is edited before the prose
    (`vulcan.toml:15-16`). Beacon's Route is a policy and may route on class, but the class table
    stays a single operator-authored source with the doctrine sentence beside each entry.
16. **Deleting a model is never just a delete** — `shared/roster-check.sh` exists because one
    prune broke a bot, an IDE picker, and three instruction sets (MACHINE BLOCK "Hygiene"). A
    Route whose model is gone is *absent* (distinct from broken), and deactivation must list
    every Route, Resident Configuration, and Team eligibility it invalidates before it commits.

---

## Open questions — Kevin's, not ours

1. **Reasoning content.** Vulcan drops `reasoning_content` and says surfacing it needs "its own
   privacy review" (`README.md:397-398`). BLUEPRINT:315-316 promises Beacon will not *claim*
   hidden reasoning a provider did not return — but when a provider *does* return it (GLM,
   DeepSeek), is it Trace (CONTEXT.md: "Avoid: chain of thought"), Private Memory, or dropped?
   Today it is
   paid for and invisible (GLM spent 572 of 600 tokens on reasoning and returned empty content
   through Vulcan, memory `glm-security-lens`). A contract and privacy ruling, not a parser choice.
2. **Overflow to flow.** The MACHINE BLOCK (`~/.claude/CLAUDE.md:59-68`) leaves flow overflow
   "AVAILABLE, NOT YET ENABLED — Kevin's call", needing an SSH tunnel because flow's Ollama is
   deliberately loopback-only. In Beacon terms: is flow a leased Worker with its own activated
   local inference (ADR 0005), and is the tunnel a Worker-lease concern or a Route concern?
3. **Per-Agent Envelope vs. per-seat-per-UTC-day ceiling: replace or nest?** Subtlety C: the
   Envelope is a scope ceiling; Vulcan's window is a rate ceiling; the GLM case shows provider rate
   limits bind before budgets do. If Beacon keeps a daily window, which scope owns it (Workspace
   per provider? Agent?) and does it roll up like BLUEPRINT:433 says Envelopes do?
4. **Does the `tools` exclusion survive the move?** Vulcan refuses `tools` by charter
   (`ROADMAP.md:352`) and Buzz bypasses it under a ratified split. Beacon's Inference module is
   the one *making* tool calls on behalf of Agents, and `local_model_activation.rs:24` already
   treats `tools` and `thinking` as approvable Provider Features. Either Beacon's adapters admit
   tool schemas (and take on the agent-loop stall risk `code-fast` is documented to have,
   `vulcan.toml:93-100`), or Agents cannot use tools through Models at all. This contradicts
   nothing in ADR 0001-0024 but is unaddressed by them.
5. **Is the 16 MiB provider ceiling a constant or a Route property?** The eval
   (`docs/evals/2026-08-28-provider-response-bounds.md`) sized it at 27.4× the largest measured
   fleet response; a provider that legitimately exceeds it "must be admitted by a deliberate,
   measured contract change". Vision Routes (BLUEPRINT names images out of scope for Alpha 1) will
   test this first.
6. **Ship the hardened unit?** `ProtectHome=read-only` etc. are verified on `main` and absent from
   the live service (measured case). This is a Vulcan ops item, not Beacon's, but beacond will be
   a long-running supervisor under systemd with the same key-proxying exposure; the same drop-in
   discipline (`local.conf`) is the pattern to copy.
7. **Who owns the `NUM_PARALLEL` ↔ admission coupling once Beacon exists?** Today it is two
   hand-edited numbers in two files (`vulcan.toml:48`; `ollama.service` Environment). If Beacon's
   Worker derives it (consequence 11), Vulcan's copy becomes stale the day Beacon goes live.
