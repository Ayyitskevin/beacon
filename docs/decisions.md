# Decisions findings inherited by Beacon

**Status:** draft for review · **Produced by:** claude-code harvest agent · **Last verified:** 2026-08-29

BLUEPRINT §3 says Decisions is "the authority surface descended from Janus"
(`docs/BLUEPRINT.md:240@c59dde1`). Janus was frozen today by Kevin's ruling that Beacon absorbs it
(PR #4, `a25abf2`, open). Janus is also the fleet's proof that a gate model derived from taste is
wrong: its author recorded leanings on three open questions and a real twelve-gate corpus overruled
two of them (`docs/adr/0001-the-gate-model.md:117-119@6b62110`). This page re-derives the **gate
model** from the pinned code and the live ledger so Beacon inherits the evidence, not the taste.
It states findings, not decisions.

**Boundary.** `docs/authority.md` already owns Janus's *binding* mechanics — file/git/text
bindings, the tri-state `verify_binding`, reported-not-enforced, the measured `unbound` case, and
the operator-action table (`docs/authority.md:99,112-115,141-166,227-297@c59dde1`). Nothing there
is restated. This page owns what a gate *is*: why a human is needed, options, decay, horizon,
consumer, the five terminal states, resource gates, the board's ordering, the scorecard's honesty
rules, and the corpus all of it was fitted to.

---

## How this page was produced

Read on 2026-08-29 with no checkout, switch, pull, stash, or write in any tree. The Janus clone's
working tree is on a docs branch and was ignored; every Janus line is from the pin.

| Source | Tree | Revision | Read with |
|---|---|---|---|
| Janus code + docs | `/example/checkouts/janus` | `origin/main` = **6b62110** | `git -C /example/checkouts/janus show 6b62110:<path>` |
| Janus PR #3 (open) | same | `1a49124` on `claude/unverifiable-binding-is-not-fine` | `git show 1a49124 -- src/janus/cli.py` — cited as **pending**, not main |
| Janus PR #4 (open) | same | `a25abf2` on `claude/freeze-as-beacon-absorbs` | `git show a25abf2 -- README.md docs/ROADMAP.md` — pending |
| Janus LIVE ledger | `~/.janus/janus.db` via the installed CLI only | installed copy `cli.py`/`core.py` **byte-identical to 6b62110** (`diff` empty) | read-only verbs below; never `raise/decide/withdraw/expire/supersede/revise-check`; `--check` was NOT confirmed |
| Beacon | `/example/checkouts/beacon` | **c59dde1** | `README.md`, `CONTEXT.md`, `AGENTS.md`, `docs/BLUEPRINT.md`, `docs/authority.md`, `docs/adapters.md`, ADRs 0001/0002/0004/0016/0018/0020/0023 |
| Prior claims | memories `janus-pillar`, `janus-gate-ledger`, `review-capacity-fails-silently`, `absent-dead-broken-collapse`; handoffs `2026-08-29_janus-pr3-unverifiable-binding-claude`, `2026-08-25_ai-town-live-module-shapes` §3 | — | read as claims (R5), checked against the pin and the live ledger |

Every citation is `path:line@6b62110` for Janus and `path:line@c59dde1` for Beacon unless another
pin is named. Live probes, all read-only, run 2026-08-29 14:46-14:48Z:

```bash
janus stats --json          # the scorecard; text form: janus stats
janus list --state all      # 29 gates — `janus list` alone defaults to open (5)
janus list --state all --json | python3 -c '…'   # tallies of kind / state / binding_kind / ruled_by
janus board                 # 5 waiting; PROMISED section
janus board --check         # exit 2: "refusing to run stored commands unattended" — nothing ran
janus doctor                # append-only proof, drift; NO unverifiable line at 6b62110
janus show g3410c2cff0f g4deae14ca3d ge477ad893e9 gf430f95f410 g55daf244a78 g1a603e736cb g43344426bdb
```

---

## What the Blueprint already specifies

- **Decisions** shows what needs a Ruling, who may decide, the bound Proposal, whether the Binding
  holds, and whether the Ruling is in force; a material change makes a new Proposal and invalidates
  the old Ruling (`docs/BLUEPRINT.md:240-243`; design test `:1159`).
- **Invariant 3 is already Beacon doctrine, on both sides.** "Evidence can influence a Ruling but
  cannot grant authority … A Ruling record is not assumed favourable merely because it exists"
  (`:445-447`); "Evidence informs authority but never grants it" (`AGENTS.md:144`).
- **The six axes** (`:455-467`) encode Janus findings via `authority.md`: `binding`/`check`/
  `enforcement` are the binding mechanics; **`lifecycle` already carries all five Janus terminal
  states** (`authorized`≈`approved`, `refused`, `withdrawn`, `expired`, `superseded`, `:463-464`);
  `force` is `pending / in force / spent / indeterminate` (`:465`); "never compressed into a
  generic Approved state" (`:467`).
- **Attention Item** exists only when a condition is unresolved, a named actor is eligible, and the
  product can state the concrete next action (`:352-356`; `CONTEXT.md:299-302`; test `:1161`).
  Enforced refusals and not-yet-run checks stay in Activity (`:358-359`).
- **Time never gates by itself**: "Routine work does not pause merely because time has passed"
  (`:361`); "Elapsed time alone cannot pause work unless … Policy names a time gate" (`:904`).
- **Persistence**: no meaningful nulls (`AGENTS.md:198`); derived verdicts computed at read time
  (`:204`); "Terminal, absent, broken, and not evaluated are distinct states" (`:146`).
- **Migration readers** are read-only, one-shot, fail-closed (`AGENTS.md:212-213`); direct database
  reads require verified-quiescent snapshots (`docs/BLUEPRINT.md:1013`; ADR 0001 banner `:15-27`).

Not yet in the Blueprint, at c59dde1: `kind` (why a human), ordered options on a Gate, decay with a
re-runnable check, a per-outcome consumer, resource gates and delivery, a no-priority rule, a
question-length rule, and the scorecard's three honesty rules. Each is a finding below.

---

## Corrections to prior claims

1. **"Beacon `AGENTS.md` forbids a priority integer" — not at c59dde1.** `grep -n -i priority`
   over `AGENTS.md`, `CONTEXT.md`, `docs/BLUEPRINT.md`, `docs/adr/*.md`, `GOVERNANCE.md` returns
   nothing. The prohibition exists only in Janus: `AGENTS.md:17-19@6b62110` ("Never add … a
   priority integer") and ADR 0001 `:111` ("It measures the raiser's feelings"). Beacon's nearest
   text is `AGENTS.md:61` (Track Records are "never composite scores") and `docs/BLUEPRINT.md:275`
   (attention drawer: named actor + next action). The rule is inherited, not yet written.
2. **Binding-kind and state counts moved.** `docs/authority.md:269-276` and `docs/adapters.md:
   88-92` record 13 gates, `git 8 · none 4 · file 1 · text 0`, "all four unbound are approved".
   Live 29: `git 19 · file 6 · none 4 · text 0`; of the four unbound, **three** are approved
   (`g4deae14ca3d`, `g6b5df553432`, `g43344426bdb`) and one is superseded (`g1a603e736cb`).
   `2026-08-25_ai-town-live-module-shapes.md:88-91` says `approved 8 · superseded 5 · open 2`;
   live is `17 · 7 · 5`. The two stable findings (no `text`, four unbound) still hold.
3. **"A pillar nobody but its author uses" no longer holds.** ROADMAP M4 `:109-115` and memory
   `janus-gate-ledger:29` record "8 gates, all 8 by one seat" then "9 gates, 8 mine, 1 codex".
   Live: `kevin-lee+codex 15 · kevin-lee+claude-code 13 · kevin-lee 1`. Codex is now the majority
   raiser. M2's exit — the *operator* uses the board — is still recorded NOT MET (`:68-72`) and
   PR #4 keeps it so; this harvest did not re-ask.
4. **Memory `janus-pillar.md:20,52`** ("ZERO CODE", "BLOCKED: M1 must not start") is stale: M1,
   M2, M4 shipped 2026-08-24 (`docs/ROADMAP.md:26,48,83`). `janus-gate-ledger` is current.
5. **"`raise` accepts relative paths"** (morning survey, per the PR #3 handoff `:13`) was fixed by
   PR #1: `src/janus/core.py:110-118` absolutizes at raise time, and its comment names the gate
   that taught it. The live defect is one layer up (PR #3, below).
6. **Horizon out of sample.** The corpus said 6 of 12 gates had an honest deadline
   (`docs/evidence/2026-08-24-gate-corpus.md:47-49`). Live: **0 of 29** across three seats; the
   overdue tier has never fired (`janus stats`: "horizon 0 of 29 ← the overdue marker has never
   fired"). Optional was right; the field is also unused.

---

## The invariants

| Invariant | Where (`@6b62110`) / test | The case that produced it | What Beacon must preserve |
|---|---|---|---|
| **1. A gate is open or closed, never both.** State is derived from one terminal event, not a column | `docs/VISION.md:38-40`; `migrations/0001_initial.sql:8-11,69-70` (rulings `gate_id PRIMARY KEY`); `core.py:297-303`; `tests/test_invariants.py:45,53` | Designed in from the append-only correction discipline: "a column you can UPDATE … can hold two truths" (`0001_initial.sql:9-11`) | A second terminal row must fail at the store, not in application code; `AGENTS.md:204` (read-time verdicts) already fits |
| **2. A ruling binds a digest, not a name.** Digest re-derived at ruling time and stored on the ruling | `VISION.md:41-43`; `core.py:304-311`; `core.py:110-135` (absolute path, pinned commit); tests `:86,206,247,286` | `g55daf244a78` bound `<repo>@HEAD` and went void one unrelated commit later (`core.py:129-132`); `g3410c2cff0f` bound a relative path nobody else can resolve (`:112-116`) | Mechanics in `docs/authority.md`. Add: **both** the raise-time and ruling-time digests are kept (`0001_initial.sql:77-79`), and a ruling written with no digest is a defect (PR #3) |
| **3. Reading a ruling is not authority.** No verb answers "may I act" | `VISION.md:44-46`; `core.py:1-7`; `cli.py:151-152,458`; `README.md:111-114`; Janus `AGENTS.md:23-25`; tests `:99` (no `authorize/permit/is_authorized` in `core`), `:394` (board says so) | The threat model's trust boundary: any same-user process can write the ledger, so a Janus read must never be grounds to act (`docs/THREAT_MODEL.md:8-17`); PR #4 names it "the one to carry hardest" | `docs/BLUEPRINT.md:445-447` and `AGENTS.md:144` are this rule. Keep the Authority interface at "submit, rule, verify binding" (`:644`); dispatch re-derives (`:1159`) |
| **4. (operational) A gate names its consumer, or it is a note.** `question`, `kind`, `decay`, `consumer` refused at the schema | `VISION.md:48-51`; ADR 0001 `:52-55`; `0001_initial.sql:32-39`; `core.py:250-261` (280 chars) | Orphan gates: "the human answers into a void" (ADR 0001 `:30-31`) | This is Beacon's Attention Item rule (`docs/BLUEPRINT.md:352-356`) — see the mapping below |

---

## The corpus

Twelve real pending-human items from one session, 2026-08-23/24, recorded *before* the schema
existed (`docs/evidence/2026-08-24-gate-corpus.md:1-23`). What each forced:

| Finding | Corpus | Model decision it forced | Against the author's leaning? |
|---|---|---|---|
| The RED irreversibility list covers **3 of 12** | `:33-38` (#1, #8 only; #2 is `resource`) | `kind` = *why a human is needed*: `irreversible / authority / taste / resource`, no `other` (ADR 0001 `:121-137`; `core.py:22,250-255`; test `:155`) | yes — the obvious enum was the RED list |
| Multi-option is the **majority** shape, 6 of 12, incl. both highest-stakes gates (#4, #5) | `:27-31` | Ordered `options`; approval must name exactly one offered id, refused at write (ADR `:169-179`; `0001_initial.sql:148-158`; tests `:133,140`); raiser must recommend one (`cli.py:54-59`) | yes — approve/refuse "would flatten half the corpus into prose" |
| 6 of 12 carry an honest deadline, all blocking in-flight work | `:47-49` | `horizon` optional; invented dates "teach an operator to ignore the field" (ADR `:181-186`) | no |
| Gate #1: review requested 18:17, PR merged 22:10 without it — neither decided, refused, nor expired | `:57-70` | Fifth terminal state **`superseded`**, "the most common decay mode" (ADR `:188-199`; `core.py:24`; test `:147`) | not anticipated at all (`:188`) |
| Gate #2: mint a credential — a question, consumer, decay, and "no ruling makes a token exist" | `:51-53` | `kind: resource`; an approval is a **promise, not a delivery**; optional `delivery.check` (ADR `:201-207,234-264`) | not anticipated |

**The stated limitation Beacon must carry with it.** The M0 non-author review (codex) returned
HOLD on sampling bias: twelve gates, one session, one operator, three of them Janus's own design
questions, `resource` resting on one example. Kevin kept the enum at four and wrote the objection
into the ADR; the revisit trigger is *an observed classification failure*, not another argument
(ADR 0001 `:139-167`).

**Out of sample, today.** Live `kind`: `authority 23 · resource 3 · taste 2 · irreversible 1`.
Codex's 15 gates are all `authority`; 12 of them begin "Approve/May/Merge … at exact commit" —
the corpus's gate #1 shape, which the corpus filed under `irreversible` (`:42`). The same shape
carries two values from two seats. Nothing failed to classify, so ADR 0001's trigger did not fire
— but the enum's *discriminating power* did: 79% of live gates land on one value once a second
seat used it. Finding, not decision: a `kind` vocabulary without a worked example per value drifts
to its broadest member.

---

## Load-bearing subtleties

**1. A supersede is not a ruling — NOBODY RULED.** Only `approved`/`refused` are rulings
(`core.py:23`); the CLI prints a different closing sentence per terminal state and a test pins that
the three non-rulings say `NOBODY RULED` (`cli.py:161-181`; tests `:591,597,614`). The scorecard
never counts a supersede as a ruling or in time-to-ruling (`cli.py:587-588`; test `:673`;
`docs/ROADMAP.md:99-102`). It shipped wrong once — "This records that a human ruled" printed over
all five states (`cli.py:163-166`). `superseded` takes a required reason and an *optional*
binding, because a digest proves what overtook a gate, never that it did (ADR 0001 `:211-232`);
it is the only terminal state settable by neither raiser nor ruler (`:230-232`). Live: 7 of 24
closures (29%); 5 of the 7 set by someone other than the raiser; one narrows scope rather than
ends a question — `gec9db9e0b61` "Kevin authorized merge only; deployment was not authorized by
the one-word reply", re-raised as `g7b64c3a2e49` (merge only). One gate, one decision.

**2. An approved resource gate is a promise.** `_delivery_status` treats an unrun check as
`unchecked`, never delivered (`cli.py:274-287`); the board's second heading is **PROMISED, NOT
DELIVERED** (`:365-383,436`); an approved resource gate *without* a check is counted in a sentence,
never listed, because "a row nothing can clear teaches the reader to skip the section"
(`:372-377`; tests `:487,513,523,534`). Live: `g4deae14ca3d` approved 2026-08-24T23:58Z, delivery
`exit=1` on 08-25, still PROMISED 4 days later. `g43344426bdb` was delivered on 08-24 and read as
undelivered until its check was revised (`migrations/0002_check_revisions.sql:8-14`).

**3. `decay` / `consumer` / `horizon` are the Attention Item's three conditions, stated by the
raiser.** ADR 0001 `:50-55`; `docs/BLUEPRINT.md:352-356`:

| Janus | Beacon | Gap |
|---|---|---|
| `decay` — what worsens while it waits, with an optional check whose exit 0 = the cost arrived (`0001_initial.sql:22-23`; `cli.py:259-271`) | condition 1 "unresolved"; Briefing refreshes "when risk changes … work stalls" (`:361-365`) | Beacon observes risk; Janus makes the raiser *declare* it and ship the probe. Unmeasured is printed, not blank |
| `consumer` — who acts, and what they do with **each** outcome (`:24`) | condition 2 named eligible actor + condition 3 concrete next action (`:355-356`) | Janus branches the next action per outcome; the Attention Item states one |
| `horizon` — the raiser's own deadline, observed by the clock, never automatic expiry (`cli.py:300-304`; ROADMAP `:65-66`) | `:361`, `:904` — time gates only where Policy says so | Consistent; live use is 0 of 29 |
| `kind` — why a human (`:22`) | no field; Policy classifies *what triggers* a Gate (`:417-428`) | Different axes: risk of the action vs. what kind of answer only a human can give |
| `options` (`0001_initial.sql:47-59`) | none; Ruling is "allow or deny" (`CONTEXT.md:288`) | 17 of 29 live gates, 6 of 12 corpus gates carry options |

**4. The board sorts by *observed* decay; there is no priority field.** Sort key: decay rank →
passed horizon → longest wait (`cli.py:363`; docs at `:237-256`; test `:330`). Rank: `landed`
(0) → `broken`/`unchecked`/`unmeasured` (1) → `not yet` (2): "evidence of slack demotes a gate;
absence of evidence does not promote it to safe" (`:245-249`, `docs/ROADMAP.md:55-57`). A
timed-out (124) or missing (127) check is `broken`, never `not yet`, "the most expensive possible
way to be wrong" (`:250-256`; test `:860`). Unmeasured ≠ unchecked ≠ not yet ≠ broken: four
values (`:259-271`; test `:348`). The tier *order* is the author's judgment call, not a corpus finding
(memory `janus-gate-ledger:17`). Live: all 5 open gates are `unmeasured`/`unchecked`; 13 of 29
carry a check; 8 observations on 5 gates — the sort has had almost nothing to sort on.
`docs/authority.md:317-349` already bans authority-strength ordering and allows operator-action
ordering; the same test applies here.

**5. Scorecard honesty, three rules with tests.** Every measure prints a number including zero;
every rate prints its denominator ("a percentage over n=2 is a lie with a decimal point"); nothing
is extrapolated — a rate over a window shorter than its definition is **refused by name** so the
absence reads as a decision (`cli.py:462-473,484-488,568-572`; tests `:688,704,712,736`).
"Consumer acted" cannot be measured directly, so it is split measurable/confirmed/unknown and
**unknown is never counted as acted** (`:516-523`; test `:724`). A subtlety the rules do not
catch: time-to-ruling median 18m, fastest 2s and 4s — those are agents recording a decision Kevin
gave in-session before the gate was raised, so TTR measures *recording lag* when raiser and
recorder coincide. Honest per the rules, and not what its name says; the caveat style at
`cli.py:587-588` is the pattern for saying so.

**6. Seat attribution is a claim appended to a shared OS user.** `seat_actor` returns
`<os_user>` or `<os_user>+<seat>`, never the seat alone, never from a remote caller
(`core.py:39-56`; tests `:166,172`). Live approved rulings: `ruled_by` = `kevin-lee` 8 ·
`kevin-lee+claude-code` 6 · `kevin-lee+codex` 3; nine agent-recorded approvals cite Kevin in the
reason. `ge477ad893e9` was ruled by bare `kevin-lee` **4 seconds** after codex raised it, reason
"Kevin explicitly said 'yeah lets fix it' in the active Codex session" — a bare OS user is not
proof a human ran the CLI. `ruled_by` is the **recorder**; the decider is prose. THREAT_MODEL
`:29-33` calls rulings "evidence about a person"; with agent-recorded rulings that evidence is
hearsay. Beacon's authenticated Operator Command (ADR 0018) is the thing Janus could not have.

**7. A check can be revised; a binding cannot.** Migration 0002: a check is "executable text
written once, at raise time, by someone guessing at a future they have not seen"; a correction is a
NEW ROW, the original stays visible, allowed on a closed gate (`0002_check_revisions.sql:1-28`;
`core.py:398-431`; tests `:932,954,981`; `docs/THREAT_MODEL.md:73-85`). A binding is the thing
ruled *on*: revising it would let a ruling drift onto new bytes (invariant 2). Consequence today:
`g3410c2cff0f` is permanently unverifiable (relative locator, `CANNOT VERIFY` in `show`); the
remedy is supersede + re-raise with an absolute binding (PR #3 handoff `:34`). PR #3 (`1a49124`,
pending) found that `doctor`, `decide`, and `show` all flattened `verify_binding`'s `None` into
silence — `digest_of_live` swallows the read error (`core.py:325-334`) so a ruling can be written
with `bound_sha256 = NULL`. The fix warns and still writes; whether `decide` should demand `--yes`
there is a human call (commit message).

**8. Append-only by trigger, proven on a real row.** Every ledger table refuses UPDATE/DELETE
(`0001_initial.sql:117-143`; test `:113`); `doctor` mutates a probe row it just wrote, because an
UPDATE matching zero rows proves nothing (`cli.py:673-697`). A reversal is a new gate with `cites`
(`0001_initial.sql:30,44`; live: 6 gates cite a predecessor). An edited applied migration is
refused (`core.py:186-219`; test `:180`).

**9. The question is the disclosure surface.** A title can leak an incident, a negotiation, or a
person "in the one place designed to be scanned quickly" — never put the finding in the question
(`docs/THREAT_MODEL.md:21-27`); one sentence ≤ 280 chars (`core.py:256-261`; test `:160`). The
catastrophic failure is not a leak: it is a future version that infers consent, scores risk to skip
a human, or auto-approves (`:92-96`; `docs/VISION.md:67-68`; Janus `AGENTS.md:17-19`).

**10. Checks are executable text.** Never run on write, never on list, printed in full before
running; unattended callers must pass `--yes`; a hung check is recorded as exit 124, not raised
(`docs/THREAT_MODEL.md:35-57`; `cli.py:321-356`; `core.py:455-466`; tests `:814,837,846`). The
first `--check` broke the visibility clause within a day (`:42-45`). Live: `janus board --check`
from this harness exited 2 after listing 4 commands and running none.

---

## The measured case

`janus stats` at 2026-08-29T14:48:10Z, window 2026-08-24T12:56:06Z → 2026-08-29T13:12:45Z (5d):

| Measure | Value |
|---|---|
| raised | **29** — `kevin-lee+codex` 15 of 29 (52%) · `kevin-lee+claude-code` 13 (45%) · `kevin-lee` 1 (3%); per week **not reported** — "the ledger is 5d old, shorter than the week this measure is defined over" |
| closed | 24 of 29 (83%) · open 5 — `approved` 17 of 24 (71%) "← a human actually ruled" · `refused` 0 · `expired` 0 · `withdrawn` 0 · `superseded` 7 of 24 (29%) |
| time to ruling | n=17 · median 18m · fastest 0m (2s) · slowest 11h — rulings only |
| consumer acted | measurable 3 of 29 (10%) · confirmed 1 · unknown 26 — "Unknown is never counted as acted" |
| fields | decay check 13 of 29 (45%) "← the board reads 'unmeasured' for the other 16" · delivery check 3 · binding 25 (86%) · options 17 (59%) · horizon **0** "← the overdue marker has never fired" |
| checks run | 8 observations across 5 of 29 (17%) gates |

`janus list --state all --json`, tallied: `kind` authority 23 · resource 3 · taste 2 ·
irreversible 1 · `binding_kind` git 19 · file 6 · none 4 · text 0 · states approved 17 ·
superseded 7 · open 5 · no approval on an option-bearing gate lacks an `option_id` · every
approved bound gate's ruling digest equals its raise digest (14 of 14).

`janus board`: "5 waiting on a human — longest wait 5d · 5 whose decay has never been checked";
top row `unmeasured 5d g3410c2cff0f authority`; **PROMISED, NOT DELIVERED — 1** (`g4deae14ca3d`,
`not landed 4d`); footer "Reading this board is not authority to act."

`janus doctor`: installed copy; migrations `0001_initial, 0002_check_revisions`; append-only
enforced on UPDATE and DELETE; open gates 5; `drift ge477ad893e9 (approved) — bound artifact no
longer matches` (the `reported + bytes-drifted` row of `docs/authority.md:244`: nothing stops
anyone; codex should re-raise). **No line for `g3410c2cff0f`** although `show` prints `CANNOT
VERIFY` — the flattening PR #3 fixes, confirmed live against a CLI byte-identical to the pin.

---

## Dead seams — frozen or closed

- **M3 read-only surfaces will not be built** (PR #4 `a25abf2`, `docs/ROADMAP.md` diff). Its one
  named consumer was Mosaic's migration reader, and Mosaic was superseded by Beacon, which reads
  the SQLite file directly. The roadmap's own warning applied to itself: "every sibling that built
  a seam before it had a measured consumer closed it again" (`docs/ROADMAP.md:122-123@6b62110`;
  ADR 0001 `:112-113`; `docs/VISION.md:72-73`).
- **M2's exit stays NOT MET** — "not yet — haven't used it" (`docs/ROADMAP.md:68-72`), kept unmet
  by the freeze. The board's consumer never measured using it; Beacon's Briefing and Attention
  drawer inherit the same exit question, and the same instruction: re-ask, do not assume.
- **Relay pointer on raise** — never built; "the seam most likely to turn Janus into a notifier"
  (`:119-121`). Beacon's notification path is the Attention Item alone (`AGENTS.md:74`).
- **Loopback HTTP** — never built. THREAT_MODEL `:50-57`: the moment a lower-trust caller can
  write a gate, its check is remote code execution; checks from a remote origin must be refused at
  write time. Beacon Plugins have no raw store access (`AGENTS.md:139`).
- **The Beacon-side migration reader must open a quiesced copy.** PR #4 note: copy, no
  `-wal`/`-shm`, `immutable=1`; Beacon ADR 0001 banner `:15-27`; `docs/BLUEPRINT.md:1013`. Janus
  opens in WAL mode (`core.py:181`), so companions are the live-connection tell. At probe time
  `~/.janus/` held `janus.db` and a `.bak-20260824-220929`, no companions. The legacy Python
  adapter's `janus list --json` path (Beacon ADR 0001 `:84`) is the other legal read; pick one.

---

## Consequences for Beacon's Decisions

1. Gate state is derived from one terminal event; a test inserting a second terminal row expects
   a store-level refusal (`tests/test_invariants.py:45,53@6b62110`).
2. The Authority interface stays "submit, rule, verify binding" (`docs/BLUEPRINT.md:644`); a test
   asserts no `authorize`/`permit`/`is_authorized` verb exists on it (`:99`).
3. Every surface that renders a Ruling says reading it is not authority (`cli.py:151-152,458`;
   test `:394`), and dispatch re-derives the digest (`docs/BLUEPRINT.md:1159`).
4. Five terminal `lifecycle` values, two of which are Rulings; each has its own closing copy, and a
   sixth value fails a test rather than inheriting a sentence (`cli.py:170-181`; tests `:591,597`).
5. No Briefing metric counts a supersede, withdrawal, or expiry as a Ruling or in time-to-ruling
   (`:673`); the share of supersedes is itself a printed number.
6. A Gate may carry ordered options with exactly one recommended; an allow Ruling names exactly one
   offered option, refused at write (`0001_initial.sql:148-158`; tests `:133,140`). `CONTEXT.md:
   287-289` ("allow or deny") needs a home for this.
7. `kind` is a closed vocabulary of *why a human is needed*, distinct from Policy's risk classes;
   a gate that fits none is a finding, not `other` (`core.py:250-255`; test `:155`). Carry the
   stated limitation (ADR 0001 `:139-167`) and the live 23-of-29 `authority` measurement beside it.
8. No priority integer in Decisions or Briefing; attention orders by observed facts — a check that
   fired, a passed deadline, wait — and names the ordering honestly (`docs/authority.md:339-349`).
9. `unmeasured`, `unchecked`, `not yet`, and `broken` are four distinct values, and a broken probe
   never reads as slack (`cli.py:249-271`; test `:860`; `AGENTS.md:146`).
10. Resource-class Gates carry a delivery check; allowed-but-undelivered has its own heading;
    allowed-with-no-check is counted, never listed (`cli.py:365-383`; tests `:487,534`).
11. Every scorecard measure prints a number including zero; every rate its denominator; a rate over
    a window shorter than its definition is refused by name (tests `:688,704,712`).
12. A check is never run on write or on list, is printed before it runs, and needs explicit consent
    unattended (tests `:814,837`); in Beacon a check is a Proposal under Execution, never free text.
13. Checks are revisable append-only; Bindings never are; the remedy for an unverifiable Binding is
    a new Gate citing the old (`core.py:398-431`; test `:954`).
14. `decider` is the authenticated Operator's Command (ADR 0018), never inferred from who wrote the
    row; the migration reader preserves Janus `ruled_by` verbatim as *recorder* and leaves decider
    unresolved rather than mapping it to `operator` (`AGENTS.md:146,205`).
15. The migration reader opens a verified-quiesced copy, fail-closed (Beacon ADR 0001 `:15-27`).
16. A Gate's question is one sentence ≤ 280 characters and never carries the finding
    (`core.py:256-261`; `docs/THREAT_MODEL.md:21-27`).

---

## Open questions — Kevin's, not ours

1. **Should an allow Ruling on an unverifiable Binding require explicit confirmation?** PR #3 left
   it a warning, stating "whether `decide` should require `--yes` here too is a human call"
   (`1a49124`, commit message).
2. **Does Beacon's Gate absorb `taste` and `resource`, or only `irreversible`/`authority`?**
   `CONTEXT.md:283-284` defines a Gate as a Proposal that cannot proceed; 9 of 12 corpus gates had
   no Proposal with intended effects (a licence, an enum, a process). Either the Gate definition
   widens or those become Attention Items — a domain-language change (GOVERNANCE decision process).
3. **What is `force` for a Gate with no Ruling?** `superseded`/`withdrawn`/`expired` have a
   `lifecycle` value (`docs/BLUEPRINT.md:463-464`) but no Ruling to be `pending / in force / spent
   / indeterminate` (`:465`), and `AGENTS.md:198` forbids a meaningful null. Structurally, `force`
   on the Ruling rather than the Gate makes the question disappear; that is a schema call.
4. **Where does allowed-but-undelivered sit on the six axes?** `lifecycle: authorized`, `force: in
   force`, yet nothing carries "promised, not delivered"; `spent` is wrong (nothing consumed) and
   `indeterminate` is wrong (the check answered: not landed).
5. **Does `kind` survive, or does Policy classification replace it?** The RED list covers 3 of 12
   corpus gates; the four-value enum collapses to `authority` in 23 of 29 live gates. Both facts are
   real and they pull in different directions.
6. **`gf430f95f410`** (rename `janus-authorized` → `authorized` + decider) is open 4 days, bound and
   verified, at the top of the board. `docs/BLUEPRINT.md:463` already says `authorized`. Is the
   Blueprint the ruling, or does the gate still need one so the board stops carrying it?
7. **M2's exit, re-asked for Beacon:** will the Operator read the Attention drawer instead of
   grepping handoffs? Janus never got a yes. Measure by asking.
8. **`g3410c2cff0f`** — supersede + re-raise with an absolute binding is the claude-code seat's to
   do, but it is Kevin's five-day-old question (PR #3 handoff `:34`). Same for the drifted
   `ge477ad893e9`, which is codex's to re-raise.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
