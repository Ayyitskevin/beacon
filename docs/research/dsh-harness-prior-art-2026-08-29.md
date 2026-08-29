# Prior art: DeepSeek Harness (`dsh`) — the skill registry and the goal lifecycle

**Status:** research, for review · **Produced by:** claude-code · **Read:** 2026-08-29

Beacon is building an agent office suite. Eight days before this page was written, a different lab
shipped an open-source agent harness that solves several of the same problems — some of them ones
Beacon has not built yet, and one Beacon currently solves by hand. This page records what is worth
taking and what is not.

It is **findings, not decisions.** Nothing here is adopted. Every claim cites a file in a published
package that can be re-read at the pinned version below. Where a finding argues against an accepted
Beacon ADR, it is filed under open questions rather than asserted.

## How this page was produced

| Source | Pin | How it was read |
|---|---|---|
| `@deepseek-ai/dsh` | 0.1.1-rc.2, published 2026-08-21 | `npm view @deepseek-ai/dsh --json` |
| `@deepseek-ai/dsh-skill` | 0.0.1-rc.1 | `npm pack`, then the emitted `lib/types/index.d.ts` |
| `@deepseek-ai/dsh-goal` | 0.0.1-rc.1 | as above, plus `lib/types/{types,domain}.d.ts` |
| `@deepseek-ai/dsh-scope` | 0.0.1-rc.1 | `lib/types/index.d.ts` |
| repository README | `github.com/deepseek-ai/deepseek-harness` | fetched 2026-08-29 |

Citations name the package and the file inside its published tarball, e.g.
`dsh-goal lib/types/types.d.ts`. The packages ship `.d.ts` and compiled `.js`; the doc comments
quoted here are the authors' own words from those declaration files.

**What dsh is:** an "open-source agent harness … built on an everything-is-a-plugin architecture
powered by Cordis" (README). It is a *client*, in the same category as the harnesses that already
drive this fleet — not a model, and not a system Beacon absorbs. **It is in developer preview and
its README warns, in capitals, that there will be compatibility-breaking changes.**

**License and provenance.** dsh is MIT. Design ideas are free to learn from and this page is a
reading, not a copy. If any implementation is ever lifted rather than re-derived, MIT attribution
obligations attach and Beacon — planned open source — needs that recorded deliberately. See the
open questions.

## The findings

| # | Idea | Where it lives | What Beacon has today | Worth taking? |
|---|---|---|---|---|
| 1 | Phase-dependent exact-key validation | `dsh-goal lib/invariant.js`, `types.d.ts` | `deny_unknown_fields` + `hasExactKeys`, keyed on the type | **Yes — highest value per line** |
| 2 | Durable phase vs process-local activation, never persisted | `dsh-goal lib/types/types.d.ts` | Pause Work, in prose (`BLUEPRINT.md` §4) | **Yes** |
| 3 | Typed provenance registry for injected context | `dsh-skill`, `dsh-goal` (`MessageSourceMap`) | Activity as "immutable audit projection"; Progressive Transparency | **Yes** |
| 4 | Stable error codes as one declared union | `dsh-goal lib/types/domain.d.ts` | Wire codes discovered per-slice (see PR #11) | **Yes** |
| 5 | One type home projected to host and client | `dsh-goal lib/types/{types,domain,client}.d.ts` | SQL + Rust + TS reconciled by hand, drift caught by tests | **Yes, but it is a large change** |
| 6 | Ranked multi-root skill registry with per-agent scope | `dsh-skill lib/types/index.d.ts` | ADR 0008 Extensions — unbuilt | **Yes, when Extensions is built** |
| 7 | Escaping at the model-facing framing boundary | `dsh-skill` (`renderSkillContent`, `escapeText`) | Nothing yet — no injected-context surface exists | **Yes, before the first injection ships** |
| 8 | Compare-and-set identity as a pair, not two arguments | `dsh-goal lib/types/types.d.ts` (`GoalRef`) | `project_id` + expected revisions as separate parameters | **Yes — same lesson as ADR 0052** |
| 9 | Full-snapshot versioned events with a clear tombstone | `dsh-goal lib/types/domain.d.ts` | Event payload JSON + startup reconciliation | Partly — Beacon is close |

### 1. Phase-dependent exact-key validation

`GoalSnapshot.blockedReason` is documented as "**Present exactly while `phase` is `blocked`**"
(`dsh-goal lib/types/types.d.ts`) — and the decoder enforces it rather than trusting the comment.
`lib/invariant.js` computes the permitted key set *from the phase* and rejects anything else:

```
const expectedKeys = phase === "blocked"
  ? "blockedReason,id,maxGoalRounds,objective,phase,revision"
  :              "id,maxGoalRounds,objective,phase,revision";
… throw new Error(`goal change goal for phase ${phase} must have exactly ${expectedKeys} fields`)
```

So a blocked goal without a reason is unrepresentable, **and so is a non-blocked goal still carrying
a stale reason.** The second half is the one that is normally missed.

Beacon already believes this — its store rules say no nullable field carries meaning — and it
already validates exact keys, but keyed on the *type*, not on the *state*. Candidates where the
stronger rule would pay: a Repository Binding that no longer applies to the current Charter, a
Mission outcome, and every Run/Receipt state a later slice adds.

### 2. Durable phase versus process-local activation

Two separate types, and the separation is the point:

- `GoalPhase = 'active' | 'paused' | 'blocked' | 'complete'` — "**Durable** continuation phase.
  Activation is process-local and separate."
- `GoalActivation = 'armed' | 'disarmed'` — "Whether this live process may automatically continue an
  active goal … **Process-local continuation eligibility; never persisted.**"

A goal can be durably `active` while this process is `disarmed`. That is precisely Beacon's Pause
Work contract — *"Pause Work does not undo effects, revoke Rulings, or shut down the supervisor"*
and *"Locking … does not pause authorized Agent work"* (`BLUEPRINT.md` §4) — expressed as a type
that cannot accidentally be written to the store. Beacon states the rule in prose today; a type
whose persistence is structurally impossible is a stronger guarantee than a rule a future slice
must remember.

### 3. Provenance rides metadata, never re-parsed prose

Both packages **declare into a shared `MessageSourceMap`**: `dsh-skill` registers
`'skill-invocation'`, `dsh-goal` registers `'goal'` with `{ goalId, revision, round }`. The skill
package states the reasoning outright: on a user-explicit invocation, the user's own words ride a
plain user message and the skill body follows as injected `instructions`-form context carrying a
typed source, "**so transcript consumers present the injection from metadata instead of re-parsing
the model-facing text**."

Beacon's Activity is "an immutable audit projection of domain changes" and Progressive Transparency
promises drill-down into full prompts and tool results. This is the mechanism that makes that
honest: every injected span carries a typed origin, and any new subsystem extends the union rather
than inventing its own convention. It is the same instinct as `docs/authority.md`'s rule that an
adapter supplies its own `meaning` string rather than letting a renderer paraphrase.

### 4. Stable error codes as one declared contract

```
type GoalErrorCode = 'GOAL_AGENT_NOT_LIVE' | 'GOAL_NOT_FOUND' | 'GOAL_ALREADY_EXISTS'
  | 'GOAL_STALE_REVISION' | 'GOAL_INVALID_OBJECTIVE' | 'GOAL_INVALID_MAX_ROUNDS'
  | 'GOAL_INVALID_BLOCK_REASON' | 'GOAL_INVALID_EDIT' | 'GOAL_INVALID_TRANSITION'
```

One exported union, so "is every code handled at every boundary?" is a question the compiler can
answer. Beacon learned the cost of not having this the hard way: four Project Policy wire codes
existed in the transport, were never mapped in the desktop error table, fell through to a generic
"account service unavailable", and left four `session.ts` branches unreachable until PR #11.
A declared code set does not prevent that by itself — but it makes exhaustiveness checkable instead
of relying on a reviewer noticing.

### 5. One type home, projected to both sides

`dsh-goal` splits its vocabulary deliberately:

- `types.ts` — "the **ONE home** of the `goal` projection-key declaration … free of this package's
  host-side imports";
- `domain.ts` — host-coupled shapes (cordis events, agent, llm), "kept separate … because these
  declarations pull dsh-agent, dsh-llm, and cordis into the program — **the one-program-per-side
  layout forbids that on client aggregates**";
- `client.ts` — "a **pure re-export** … **zero content duplication**."

Beacon maintains the same correspondence across three layers by hand — SQL columns and wire envelope
in snake_case, domain structs in camelCase with `deny_unknown_fields`, enum literals repeated
verbatim in SQL `CHECK`s and TypeScript unions — and catches drift with tests. That works, and it
has already cost real review attention. dsh's answer is structural: one declaration, two
projections, and an import discipline that makes the wrong direction impossible rather than merely
tested. Beacon cannot copy it directly (its host is Rust, not TypeScript), but the *shape* — one
source of truth plus a generated or re-exported client projection — is available.

### 6. The skill registry, for when Extensions is built

ADR 0008 accepts permissioned out-of-process plugins; the surface is unbuilt. `dsh-skill` is a
shipped design for the discovery half:

- **Ranked multi-root merge.** `SkillCandidate.rank` — "lower ranks win duplicate skill names
  before provider registration order is considered", with `BUNDLED_SKILL_RANK = 600`. Deterministic
  precedence with a documented tiebreak.
- **Sources are named, and include foreign conventions.** `SkillSource = 'project-dsh' |
  'project-agents' | 'runtime' | 'user-dsh' | 'user-agents' | 'custom' | 'bundled'` — it reads
  `.agents` roots as a first-class source, and the source is "prompt-visible metadata, not
  precedence by itself."
- **Per-agent visibility.** `SkillViewOptions.scope?: ScopeKey` — "Viewing scope (the calling
  agent); omitted reads the global layer alone." Composed with `dsh-scope`'s `scopeChainOf` /
  `bindScopeParent`, this is layered inheritance — the same shape as Beacon's Workspace → Project →
  Mission → Agent nesting, as a kernel primitive.
- **Two audiences, independently gated.** `SkillInvocationPolicy { modelInvocable, humanInvocable }`
  with `isModelInvocable()` / `isUserInvocable()`. A capability can be offered to a person and
  withheld from a model, or the reverse.

### 7. Escaping at the framing boundary

`renderSkillContent` produces one canonical `<skill_content>` block "shared verbatim by the `skill`
tool result and the user-explicit invocation injection, so the model sees one canonical shape on
both paths" — a single render path, the same instinct as Beacon's one-mutation-path rule.
`escapeText` exists to "escape model-facing prose embedded inside skill markup so **provider-supplied
text cannot open or close framing tags**", alongside an explicit trust statement: skills are trusted
local content, and user-supplied invocation text stays *outside* the wrapper.

Beacon will inject Charter purpose, Policy criteria, and Repository declarations into model context.
Those strings are Operator-authored, which is a weaker guarantee than it sounds once Residents can
author anything. The escaping boundary and the trust statement both belong in the first slice that
injects domain text, not the slice after the first incident.

### 8. Compare-and-set identity as a pair

`GoalRef { id: GoalId; revision: number }` is "**Compare-and-set identity for one exact goal
revision**" — and it is the type passed around, so an id cannot travel without the revision it was
read at. `GoalId` is `Branded<'GoalId'>`, so it cannot be confused with any other string.

Beacon passes `project_id`, `expected_charter_revision`, and `expected_prior_*_revision` as separate
parameters. That is exactly the shape ADR 0052 fixed one level up, when
`assertLiveEvidenceProfileMatchesManifest` took a parsed object and a digest as independent
arguments and the fix was to make the mismatch *unrepresentable* rather than detected. The same
correction is available here, and it costs one struct.

### 9. Events carry whole snapshots, and absence is recorded

`GoalSnapshotChangeMeta` carries `version: 1`, an `operation`, and `goal: GoalSnapshot` — the
**complete post-mutation state**, not a delta, so replay cannot drift. Clearing is not a deletion
but `GoalClearChangeMeta`, a tombstone carrying `cleared: GoalRef` and `clearedAt`; and the snapshot
variant types its operation as `Exclude<GoalOperation, 'clear'>`, so a clear cannot carry a snapshot.
The decoder "fails replay loudly" on malformed changes.

Beacon is already close: events carry payload JSON and startup reconciliation refuses missing,
mismatched, or non-canonical state. The two details worth borrowing are the explicit `version` on
every payload and the tombstone — recording that something was cleared, rather than inferring it
from absence.

## What Beacon should not take

- **The plugin kernel.** ADR 0003 gives a Rust supervisor sole ownership of SQLite and execution
  authority precisely so agent work does not depend on a webview's lifetime. An in-process
  JavaScript plugin kernel would undo that. Take the *contracts*, not the runtime.
- **The session assumption.** `dsh-goal` is explicitly "**same-session** goal state." Beacon's
  Missions are durable across sessions, restarts, and operators. Its lifecycle vocabulary is worth
  reading; its scope is not Beacon's.
- **A dependency.** Developer preview with breaking changes promised in capitals. Read it; do not
  import it.
- **The unauthenticated UI.** `dsh web` serves `http://127.0.0.1:3080` with no documented
  authentication — defensible for a local developer tool, incompatible with ADR 0018's requirement
  that an authenticated Operator Account precede any Workspace access.

## Open questions — the Product Steward's, not this page's

1. **Is phase-dependent exact-key validation a general store rule or a case-by-case one?** It is the
   highest-value item here and it has a real cost: every state gains a declared key set, and every
   new field touches each state that carries it. Worth it for authority-bearing records; possibly
   not for everything.
2. **Finding 5 is a codebase-wide convention, not a slice.** Adopting one source of truth with a
   generated client projection would touch every surface built so far. Is it a deliberate migration,
   a standing rule applied only to new surfaces, or declined in favour of the current tested-drift
   approach?
3. **Can "never persisted" be enforced in Beacon's split?** dsh gets it from a type that never
   enters a durable payload. Across Rust, SQLite, and TypeScript the equivalent may need a guard or
   a test rather than a type. Worth knowing before Pause Work's activation state is designed.
4. **External prior art needs a license posture.** dsh is MIT, so design is free and this page is a
   reading. If a future slice lifts implementation rather than re-deriving it, attribution attaches
   and Beacon is planned open source. A short rule — what may be read, what must be attributed, who
   decides — would be cheaper to write now than after the first copy.
5. **Ordering.** Findings 7 and 8 are cheapest before the surfaces they protect exist; 1 and 4 are
   cheapest applied to the next slice rather than retrofitted; 5 only gets more expensive. None of
   that is a plan, and sequencing is the Steward's call.
