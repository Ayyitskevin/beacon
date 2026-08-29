# Prior art: what Beacon should steal from xAI's grok-build

**Status:** findings, not decisions. Nothing here changes a Beacon contract; each item is a claim with a
citation the Product Steward can accept, adapt, or refuse. Companion to
`docs/research/dsh-harness-prior-art-2026-08-29.md` (PR #13) — this page reads a second, independent
harness against the same Beacon surfaces, and where the two agree that agreement is recorded as a
finding in its own right.

**Source:** `github.com/xai-org/grok-build`, a 91-crate Rust monorepo behind a shipped TUI coding
agent. Read at commit **`bc7f02e`** (`SOURCE_REV d5a0335a4722`). All citations are
`path:line@bc7f02e`; paths are relative to `crates/`. **License: Apache-2.0** (204-line standard text,
verified). **There is no NOTICE file** — a precise fact that matters below.

**Last verified:** 2026-08-29 against a blobless clone at that commit.

## How this page was produced

Ninety-one crates is more than one reader should hold, so the tree was partitioned into four disjoint
clusters, three read by fresh agents with the Beacon documents in hand and one read by the author:

| Cluster | Crates | Beacon surface it was read against |
|---|---|---|
| Architecture (author) | `xai-grok-workspace-{types,client,daemon}`, `xai-grok-auth`, `xai-grok-secrets` | the `beacond` ↔ desktop boundary, ADR 0026 |
| Execution substrate | `xai-grok-sandbox`, `xai-fast-worktree`, `xai-crash-handler`, `xai-circuit-breaker`, `xai-interjection-core` | BLUEPRINT §7 sandbox, §4 recovery and Pause Work, ADR 0010/0015/0016, `docs/execution.md` |
| Extensibility | `xai-grok-plugin-marketplace`, `xai-hooks-plugins-types`, `xai-tool-{protocol,types,runtime}`, `xai-grok-mcp` | ADR 0008, BLUEPRINT Extensions |
| State and lifecycle | `xai-agent-lifecycle`, `xai-grok-memory`, `xai-grok-session-events`, `xai-sqlite-journal`, `xai-grok-compaction` | BLUEPRINT §4 memory, ADR 0007/0020, migrations 0011–0013 |

Every load-bearing citation below was then re-opened by the author in the source, not taken from the
agent's notes — the sandbox guard rule, the eleven `KeepReason`s, the abandoned-probe lease, the
marketplace digest pin, the hook reply schema, the `pinned`/`removable` fields, and the memory
consolidation that destroys its source were each confirmed verbatim. Where an agent reported something
was *absent*, that absence was checked too (the sandbox crate was grepped for `RLIMIT`, `setrlimit`,
and timeout machinery: none).

**Two corrections to this page's own board item.** BCN-17 was filed with crate-to-Beacon mappings
derived from crate *names*. Two were wrong: `xai-agent-lifecycle` contains no lifecycle state machine
(it is a turn/session hook registry) and so says nothing about ADR 0020; and `xai-grok-memory` is prior
art in the *negative* direction (see "Where grok-build disagrees"). Both are recorded on the issue.

## The findings, ranked by what they would change in Beacon

### 1. A pure-data wire crate, enforced by its dependency list

`codegen/xai-grok-workspace-types/src/lib.rs:1-7`:

> "This crate is intentionally pure-data and depends on nothing more than `base64`, `serde`,
> `serde_json`, `thiserror`, and `chrono`. There is **no tokio, no async-trait, no I/O**. This makes it
> cheap to depend on from anywhere -- including the eventual WASM browser SDK."

The idea is familiar; the *enforcement* is the finding. The rule is a dependency list, so a violation
is a compile fact, not a review opinion.

**Beacon today:** SQL columns, Rust domain structs, the wire envelope, and TypeScript unions are four
hand-maintained restatements of one vocabulary, reconciled by `hasExactKeys`, `deny_unknown_fields`,
and a reviewer noticing. The Project Policy pattern brief lists "three naming layers must reconcile"
as a standing watch-out.

**Independent agreement:** dsh finding 5 reaches the same conclusion in TypeScript — one type home,
"free of this package's host-side imports", projected to both sides. Two labs, two languages, one
answer.

### 2. Every ergonomic temptation recorded with its reason for rejection

`codegen/xai-grok-workspace-types/src/error.rs:1-35` is a design log disguised as a type. Four
decisions, each stating the tempting-but-wrong alternative first:

- `Io`: "a natural definition would be `Io(#[from] std::io::Error)` … but `std::io::Error` is **not**
  `Serialize`, so the enum cannot be sent over a gRPC stream as-is." Replaced by
  `Io { message, kind: IoKind }`, converted **manually at the boundary**.
- `Tool`: `#[from] ToolError` "would force this crate to depend on `xai-grok-tools`, which would defeat
  the lightweight wire-types-only goal."
- `Vcs`: deliberately `Vcs(String)`, with the forward path written down — it "can promote this to a
  structured `VcsErrorKind` enum **without breaking the wire format** (the JSON shape stays a string)."
- `Internal`: `Box<dyn Error>` is not `Serialize`, so `Internal(String)`.

**The transferable rule:** *a wire error type may not contain a variant that cannot cross the wire.*
That is structural, where Beacon's PR #11 and PR #14 fixes (nine wire codes that existed in the
transport and never reached the desktop) were manual. PR #14 removed the wildcard so the compiler now
holds the line on the desktop side; this finding is the same idea one layer down.

### 3. Caused state does not go on the observation bus

`codegen/xai-grok-workspace-types/src/events/mod.rs:6-12`:

> "There is **no** `SessionEvent` enum. The EventBus only carries `WorkspaceEvent` — session-scoped
> state (prompt boundaries, tool-call lifecycle, plan-mode transitions, subagent lifecycle, compaction,
> memory flushes, ...) is **sampler-caused** and is reported back to the sampler via the originating
> call's return value or stream chunks. The sampler owns that state."

And `events/workspace.rs:4-9`: the bus carries "only **workspace-observed external state** —
filesystem watcher fires, background subprocess lifecycle (LSP / MCP), background indexing progress."

**The rule:** what the system *observed about the world* goes on the bus; what the system *caused*
returns to the causer through the call that caused it. Beacon's Activity is "an immutable audit
projection of domain changes" and today every surface publishes to it. This is a design decision, not
a cleanup, and it is cheapest before more surfaces publish.

### 4. A destructive action gets an enumerated refusal set that fails toward keep

`xai-fast-worktree/src/git/reason.rs:8-22` — eleven typed `KeepReason`s: `Dirty`, `HiddenFromStatus`,
`IgnoredContent`, `EmbeddedRepo`, `Unpushed`, `OnlyCopy`, `NotInSnapshot`, `WorktreeLocalState`,
`NoRepo`, and critically **`CheckFailed`** and **`GateTimedOut`**. The gate runs on its own thread with
a timeout, a compile-time assertion proves the gate outlives the pass, and the hold is **re-checked
after the slow check returns** (`git/safety.rs:133-168`, `api/gc.rs:99-144`).

"The check timed out" and "the check said no" are different facts and each is its own reason to keep.
This is the fleet's absent/dead/broken lesson as a type, applied to the one operation that cannot be
undone.

**Beacon:** BLUEPRINT §5 lists the actions that always gate; the Execution slice will need the refusal
vocabulary for Receipt verification and landing. `docs/execution.md` already records Icarus's version
of this; grok-build adds the timeout-as-reason and the re-check-after-slow-check.

### 5. Any claim held by a party that may never report back expires on a bounded lease

`xai-circuit-breaker/src/breaker.rs:32-40`:

> "A probe whose owner never reaches `record()` — e.g. its future is dropped on caller cancellation —
> would otherwise hold its slot forever and strand the breaker in `HalfOpen`, shedding all traffic with
> no path back to `Closed`. `try_half_open_probe` treats a claim older than `open_duration` as abandoned
> and lets one caller reclaim it, so a lost probe delays recovery by at most one cool-down."

**Independent agreement:** this is BLUEPRINT §4's rule that a Run "stays stopping or unconfirmed until a
terminal Receipt **or lease expiry** proves its state", arrived at separately. Beacon has it as prose;
grok-build has it as an `AtomicU64` with the failure it prevents written above it.

### 6. Verify the enforcement, and never key a guard on a value the guarded path supplies

`codegen/xai-grok-sandbox/src/read_deny_verify.rs:1-6` exists to confirm a profile's read-deny
*actually took effect in this mount namespace*, and states the trap: the `__GROK_INSIDE_BWRAP` marker
and sentinel "are reproducible by a caller, so **neither proves identity**." And `src/lib.rs:91-99`
arms the network restriction on the resolved config value alone, because "keying on Landlock success
would silently disarm the session-long child-network control exactly in the degraded states … where it
is the only remaining enforcement; arming in those states is the fail-closed direction."

**Independent agreement:** the fleet's own recorded lesson (a control that reads a value the constrained
code supplies is dodgeable — key on capability, not declaration), and Icarus's credential preflight
history. Three sources, one rule; it belongs in the Sandbox slice's review checklist.

### 7. Marketplace metadata is pinned to the same digest as the code

`codegen/xai-grok-plugin-marketplace/src/catalog.rs:39-61`: "when `index_sha` is `Some`, the catalog
entry must carry an equal `sha` or the components are treated as absent." Enrichment degrades to
`None` rather than serve possibly-stale data.

**Beacon:** every field on the Extensions shelf — "version, publisher, compatibility, requested
permissions, integrity" (BLUEPRINT §3) — is a claim about bytes. This stops the shelf describing
version A's permissions beside version B's code. ADR 0008 is silent on it.

### 8. Enumerate the mutate-set; keep the observe-set generous; close the reply schema

`common/xai-tool-protocol/src/turn_hook.rs:164-197`: hooks observe richly but may change exactly two
things — inject a turn (`HookInjection`, `#[serde(deny_unknown_fields)]`) or steer the loop
(`TurnControl`) — with `{}` a no-op; fire-and-forget hooks "MUST NOT block" (`:31-35`) and the ack is
something the caller "never gates its loop on" (`:198-200`).

**Beacon:** this is an implementable reading of "an extension can add capability … it cannot replace
identity, Policy, authority, the event ledger, or Receipt verification" (BLUEPRINT §3) and of the
one-mutation-path rule.

### 9. Two-phase swap with ordered rollback, and refusals declared before they are attempted

`codegen/xai-grok-plugin-marketplace/src/installer.rs:282-288, 372-394, 396-428`: stage into a
nonce-named sibling, rename the old aside, rename the new in; on a registry-save failure roll the
*filesystem* back before reverting the registry — the ordering rationale written down, and the
both-failed error names both paths for a human.

The shelf-side half, `codegen/xai-hooks-plugins-types/src/lib.rs:244-256`, verbatim:

> "Enforced by root-owned managed policy: disable actions are refused and disable state is ignored, so
> surfaces should **show the pinned state up front rather than let a refusal be the first signal**."

and `removable` is "true only for user-registered hook directories, so surfaces don't offer removal
elsewhere." ADR 0008 requires operator review for install and permission expansion but says nothing
about the update transaction or about surfacing a refusal before the click.

### 10. Compaction keeps its source, and says how much survived

`codegen/xai-compaction-transcript/src/lib.rs:14-18, 32-46, 432-436` and
`common/xai-grok-compaction/src/code_compaction/assemble.rs:83-89`: raw turns persist to
`compaction/segment_NNN.md` with an `INDEX.md`; how much survives is a four-level dial with an
always-on statistics floor and whole-turn-boundary truncation; the summary that replaces the
conversation carries a `<transcript_location>` pointer back to its source.

**Beacon:** this is BLUEPRINT §4's "promotion preserves the source" and "Raw Trace is not automatically
Memory" as running code, and the mechanism Progressive transparency promises without one.

### 11. The durability decision is recorded, and the invariant held by construction

`codegen/xai-sqlite-journal/src/lib.rs:1-13, 43-82, 92-107, 338-340`: journal mode chosen by `statfs`,
a kill switch whose *typos* also warn, detection failure degrading to historical behaviour, and a
per-host database filename so the no-WAL invariant cannot be broken by any peer binary.

**Beacon gap, stated plainly:** no journal-mode, `busy_timeout`, or `synchronous` decision for the
managed store is recorded in `docs/adr/` or `crates/beacond/migrations/`. The store has a posture; it
is not written down. grok-build's module doc is a ready template for the ADR. (Note grok-build itself
never sets `synchronous` — `lib.rs:178-211` — so it is a template for the *decision*, not the answer.)

### 12. Dropped events are unignorable at the type level

`codegen/xai-grok-workspace-types/src/events/lag.rs:1-22`: the runtime event stream surfaces lag as
`Result<T, EventLag>`, and `Lagged(u64)` "carries the number of events that were dropped between the
previous successful receive and the current one." A consumer cannot silently miss events. Recorded in
the same file: a **global wire convention** — every wire enum is tagged `tag = "type"`.

**Beacon:** `events_since_cursor` counts what is visible and is capped; a dropped-event signal is a
different fact with no representation today. Relevant the day Beacon adds a live stream.

### 13. One input, orthogonal typed axes — authority never inferred from another axis

`codegen/xai-agent-lifecycle/src/send/contributors/turn_lifecycle.rs:3-8, 26-47, 79-87, 93-115`:
`InputPolicy` splits authority, turn boundary, analytics, compaction class, queue visibility, and
shutdown into six enums, each with exhaustive-`match` predicates and a variant-walking test.

ADR 0020 makes the same separation at the identity layer (lifecycle never grants authority). This
generalizes it to every classified thing, with the test discipline that stops a new variant inheriting
another's policy.

## Independent agreement — where two designs converged

Two independent designs reaching one conclusion is stronger evidence than either alone. Four such
convergences were found:

| Conclusion | grok-build | The other source |
|---|---|---|
| The declaration is what the executor consumes, never free-form caller input | `workspace-types/src/binding.rs:1-16` — "the binding is the *source* of the repos, not free-form client input" | Beacon **ADR 0026**, shipped the same day, independently |
| One pure-data type home, projected to both sides | finding 1 | dsh finding 5 |
| A claim whose holder may never report needs a bounded lease | finding 5 | Beacon BLUEPRINT §4 (Pause Work / unconfirmed Run) |
| Never key a guard on a caller-reproducible value; verify enforcement after applying | finding 6 | fleet lesson; Icarus ADR 0053 |

`binding.rs` also carries two invariants Beacon's Repository Binding does not: every conversation forks
`conv/<id>` off a chosen base and "**never writes the base directly**" (one naming function, one
source), and a conversation binds a *set* of `(remote, branch)` rather than one repository. Both are
candidates for a Repository v2, not for the current revision family.

## Where grok-build disagrees with Beacon

**Memory consolidation destroys its source.** `codegen/xai-grok-memory/src/dream.rs:356`, verbatim:
"Acquires the lock, processes the response, **overwrites workspace MEMORY.md, and cleans up the
processed session files** on success." There is a recency guard (`:298`) and a lock, so it is
deliberate and engineered — and it still deletes the logs it consumed. It has two scopes, both shared,
no private memory, and no promotion record (`storage.rs:10-17`).

This is the exact tradeoff ADR 0007 and BLUEPRINT §4 refuse ("Memory Promotion … creates a new,
provenance-linked record and **preserves the source**"). Beacon's rule now has a competent
counter-example proving the tradeoff is real rather than hypothetical. Validate, do not copy.

**String identifiers, not UUIDs.** `workspace-types/src/identity.rs:1-12` uses newtypes over `String`
"so callers can pick whatever id scheme they like … and so the wire format stays human-readable", with
the inner field private so "callers [cannot] poke arbitrary strings into the newtype and bypass
whatever invariants we add later." The encapsulation half agrees with dsh (`Branded<'GoalId'>`) and
bites on Beacon's TypeScript side, which passes plain strings. The String-over-`Uuid` half runs
*against* Beacon's `uuid::Uuid` and is a tradeoff, not a verdict.

## Negative findings, stated so nobody re-covers them

- `xai-agent-lifecycle` has **no** resident-style state machine — no inactive/active/suspended/retired,
  no transition table (`src/lib.rs:1-5`). It neither confirms nor contradicts ADR 0020.
- `xai-grok-sandbox` has **no** resource limits — no `RLIMIT`/`setrlimit`, no memory, CPU, or timeout
  machinery (grepped across the crate). BLUEPRINT §7 requires "time, output, memory, and cost are
  bounded"; Beacon's spec is stronger here and grok-build is not prior art for it.
- `xai-grok-session-events` is `Serialize`-only with no reader (`src/types.rs:5-7`); there is no replay
  path, so "what happens on a malformed event" has no answer there.
- `xai-sqlite-journal` never sets `synchronous`; its crash-safety reasoning is entirely about journal
  mode on network filesystems, not fsync.
- Who cancels an in-flight turn lives outside `xai-interjection-core`, in crates not read.

## Absorption ledger — both harnesses, every finding, what form it takes

A findings page improves nothing by existing; the fleet has measured that. A finding improves Beacon
only when it takes one of three forms, in rising strength: **context** a reader might consult, a
**binding** the next slice plan must cite, or an **executable** check that fails a build or a gate.
This ledger assigns each finding from both pages its strongest honest form.

| # | Finding | Source | Form | Lands in | Status |
|---|---|---|---|---|---|
| E1 | Declared error-code set, exhaustive at the wire boundary | dsh 4 | **executable** — no wildcard; compiler | desktop `CommandError` table | **shipped, PR #14** |
| E2 | Pure-data wire crate with a dependency allowlist | dsh 5 + gb 1 | **executable** — crate + gate check | Phase 1 kernel, before more surfaces | ADR + slice, next |
| E3 | Durability posture recorded | gb 11 | **executable** — pragmas asserted at open | managed store | ADR now (cheap), assert in the store slice |
| B1 | Phase-dependent exact-key validation | dsh 1 | binding | every new revision family | next slice plan |
| B2 | Compare-and-set identity as a typed pair | dsh 8 | binding | next mutation command | next slice plan |
| B3 | Full-snapshot events, explicit `version`, tombstone for clears | dsh 9 | binding | next event family | next slice plan |
| B4 | Escape at the framing boundary; trust statement | dsh 7 | binding | first slice that injects Charter/Policy text into a prompt | that slice |
| B5 | Enumerated fails-toward-keep refusal set; timeout is its own reason | gb 4 | binding | Execution (Receipt, landing) | Execution slice |
| B6 | Bounded lease on any unreported claim | gb 5 | binding (already prose in BLUEPRINT §4) | Pause Work / Run state | Execution slice |
| B7 | Verify enforcement after applying; never key a guard on caller-supplied value | gb 6 | binding — review checklist | Sandbox | Sandbox slice |
| B8 | Metadata pinned to the code digest | gb 7 | binding | Extensions | ADR 0008 amendment |
| B9 | Enumerated mutate-set, closed reply schema, non-blocking hooks | gb 8 | binding | Extensions | ADR 0008 amendment |
| B10 | Two-phase swap with ordered rollback; refusals surfaced before the attempt | gb 9 | binding | Extensions | ADR 0008 amendment |
| B11 | Compaction keeps its source; detail dial; transcript pointer | gb 10 | binding | Knowledge / Progressive transparency | Knowledge slice |
| A1 | Caused vs observed state on the event bus | gb 3 | **ADR** | Activity / events | before the next publisher |
| A2 | Wire envelope carries only what survives a hop; `deadline` | gb 2 | ADR | command envelope | before the first long Run |
| A3 | Dropped-events signal unignorable | gb 12 | ADR | live event stream | when a stream exists |
| A4 | Orthogonal typed axes; authority never inferred | gb 13 | ADR — generalizes 0020 | classification enums | opportunistic |
| C1 | Provenance rides typed metadata | dsh 3 | context | Conversations | — |
| C2 | Ranked multi-root skill registry with per-agent scope | dsh 6 (gb agrees) | context | Extensions | — |
| C3 | Typed ids, private inner field | gb / dsh 5 | context | TS side | — |
| C4 | Never-write-the-base; bind a set of `(remote, branch)` | gb binding.rs | context | Repository v2 | — |
| X1 | Consolidation that destroys its source | gb memory | **counter-example** | validates ADR 0007 | do not copy |
| X2 | Sandbox without resource limits | gb sandbox | counter-example | BLUEPRINT §7 is stronger | do not copy |

Three observations the ledger makes visible. First, **only one finding was executable immediately, and
it shipped the same day it was measured** — the others become executable only when the surface they
guard exists. Second, most of the harvest lands in three not-yet-built surfaces (Execution, Extensions,
Knowledge), which is why the pages carry pinned revisions: they will be read months from now. Third,
the mechanism that made a binding actually bind today was a **slice plan that named its inputs** — the
Project Repository slice cited the PR #6 pattern brief and inherited none of its defects. The cheapest
way to make this ledger real is one line in the slice recipe: *state which prior-art findings apply.*

## What Beacon should not take

- **The TUI, the plugin marketplace as a product, the coding-agent loop.** grok-build is a single-user
  terminal agent; Beacon is a governed multi-agent office. The substrate transfers; the product does not.
- **The memory design** — see "Where grok-build disagrees".
- **String ids** — a tradeoff Beacon already made the other way.
- **Anything from `xai-grok-auth`** without its own read: it was in the author's cluster and was not
  reached. It is unexamined, not cleared.

## Licensing — a Steward decision, stated precisely

grok-build is **Apache-2.0 with no NOTICE file**. Consequences for Beacon (MIT-or-whatever it chooses):

- *Ideas* are not licensed. Everything on this page is an idea and carries no obligation.
- *Copied code* must carry the Apache LICENSE text and a copyright notice, and modified files must
  say they were modified (Apache §4). Because there is no NOTICE file, §4(d) imposes nothing further —
  there is nothing to propagate. Apache-2.0 code inside an MIT project is permitted; the reverse
  direction is the one with friction.
- *The patent grant* (§3) is a benefit of copying rather than re-deriving — worth knowing if Beacon
  ever ports the sandbox verification or the circuit breaker.

The dsh page raised the same question for an MIT source. The answer wants to be one policy, not two:
**does Beacon copy code from prior art, or only ideas?** Until that is decided, this page is
ideas-only, and nothing in it was copied.

## Open questions — the Product Steward's, not this page's

1. Copy code or ideas only? (Licensing, above.) One ruling covers both harnesses.
2. Finding 3 — draw the caused/observed line on Activity now, before the next surface publishes, or
   accept that Activity is a catch-all and say so in an ADR?
3. E2 — is the pure-data wire crate a Phase 1 item or a Phase 2 refactor? It gets more expensive with
   every surface added.
4. E3 — record the managed store's durability posture as an ADR this week? It is a page, and the
   absence is a gap either way.
5. The slice-recipe line — add "prior-art findings applied" as a required field of every slice plan?
