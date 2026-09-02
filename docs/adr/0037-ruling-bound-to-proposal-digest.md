---
status: accepted
---

# Bind a Ruling to one Proposal digest and the policy version it was ruled under

Proposal v1 (ADR 0034) records an immutable, digest-identified description of
intended effects and deliberately creates no authority. Nothing yet records that
a human decided about one. Without that record there is no point at which a
Proposal becomes actionable, and no evidence of who decided what, on which exact
bytes, and when.

The Blueprint requires that a Ruling apply "only to its digest and policy
version", and that "any material change produces a new Proposal and invalidates
the old Ruling" (`docs/BLUEPRINT.md:242-243`). It also requires that evidence
never grant authority: "Evidence can influence a Ruling but cannot grant
authority. An Agent cannot approve its own Proposal through a model judgment. A
Ruling record is not assumed favourable merely because it exists" (`:445-447`).

The Janus gate model this surface descends from adds the rule the fleet has
already paid to learn: a ruling binds bytes rather than names, and the recorder
must not also be the enforcer.

## Decision

Beacon introduces `RulingV1` behind one deep interface:

```text
rule(proposal_digest, decision, reason, supersedes) -> Ruling
get(ruling_digest) -> Ruling
list_for_proposal(proposal_digest) -> [Ruling]
```

A Ruling binds one exact Proposal digest, carries the algorithm and canonicalizer
that name it, and **copies** the Charter, Project Policy, and Repository
revisions and the pinned base revision from that Proposal at the moment of
ruling. Copying is what makes drift detectable later without trusting the
Proposal row to have stayed put, and it is what "one policy version" names.

## Two digest envelopes, both explicit

A Ruling carries two identities and neither is inferred from the other.

The **bound Proposal's** identity is copied whole: its digest value, its
algorithm, and its canonicalizer. A Ruling names the exact bytes it ruled on, and
recording only the value would leave a reader to assume which function produced
it — the assumption this record exists to remove.

The **Ruling's own** identity is the SHA-256 over its canonical bytes under the
canonicalizer `beacon.ruling.v1`.

## Exact canonical bytes

The grammar reuses `beacon.proposal.v1`'s primitives exactly — there is one
canonical style in this crate, not two — and is stated here completely, because
prose that names primitives without assigning an order and tag values admits
several incompatible codecs, each producing permanent valid-looking identities.

The magic is the literal ASCII `beacon.ruling.v1\0`, **17 bytes** including its
terminating NUL. Integers are unsigned big-endian at the field's stated width.
Strings are valid UTF-8 preceded by an unsigned big-endian 64-bit byte length.
SHA-256 values are their 32 raw bytes. Enums use an explicitly assigned one-byte
tag that is never renumbered.

| # | field | encoding |
|---|---|---|
| 1 | magic | 17 literal bytes `beacon.ruling.v1\0` |
| 2 | `proposal_digest` | 32 raw bytes |
| 3 | `proposal_digest_algorithm` | string |
| 4 | `proposal_digest_canonicalizer` | string |
| 5 | `charter_revision` | `u32` |
| 6 | `project_policy_revision` | `u32` |
| 7 | `repository_revision` | `u32` |
| 8 | `pinned_base_revision` | string, exactly 40 lowercase hex characters |
| 9 | `decision` | tag: `approve` = `0x01`, `refuse` = `0x02` |
| 10 | `reason` | string |
| 11 | `decider` | tag: `owner` = `0x01` |
| 12 | `supersedes` | tag: `none` = `0x00`; `digest` = `0x01` followed by 32 raw bytes |
| 13 | `ruled_at_ms` | `u64` |

The decoder consumes the entire input. Trailing bytes, an unknown tag, invalid
UTF-8, and an over-limit length are each refused rather than tolerated.
`supersedes` is the one place a `0x00` tag is assigned, because absence is a
*value* here and must encode as one.

**What "reordering is refused" can and cannot mean here.** This grammar is
positional and untagged, so the two claims must be separated. A reordering the
decoder can *see* — two adjacent strings swapped, a tag where a length belongs —
is refused, because the bytes stop parsing or a closed vocabulary rejects them.
A reordering of two **same-width** values is not detectable by construction: a
record whose `charter_revision` and `project_policy_revision` are swapped is
byte-for-byte a legitimate record whose revisions are those numbers, and no
predicate can tell the two apart without knowing an intent the bytes do not
carry. What holds instead is that such a swap **changes the identity**, because
every field is positional and every byte is covered by the digest. Field order is
fixed by the table above and pinned by the published identity vector below; it is
not, and is not claimed to be, enforced by a runtime check.

### Published identity vector

The exact bytes and digest of one record, pinned so that an encoder and a
hand-built expected buffer cannot drift together. Constructed independently from
the table above, in Python, by a non-author reviewer, and byte-identical to this
implementation's encoder.

Fields: `proposal_digest` =
`3b1f8c0e5d4a2b697f8e1d0c3a5b7e9d2f4a6c8e0b1d3f5a7c9e1b3d5f7a9c1e`, algorithm
`sha256`, canonicalizer `beacon.proposal.v1`, revisions 1/2/3,
`pinned_base_revision` = `a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2`, decision
`approve`, reason `The declared effects stay inside the Repository and spend
nothing.`, decider `owner`, `supersedes` = `none`, `ruled_at_ms` =
`1756800000000`. **234 bytes.**

```text
record = 626561636f6e2e72756c696e672e7631003b1f8c0e5d4a2b697f8e1d0c3a5b7e9d2f4a6c
         8e0b1d3f5a7c9e1b3d5f7a9c1e0000000000000006736861323536000000000000001262
         6561636f6e2e70726f706f73616c2e763100000001000000020000000300000000000000
         286131623263336434653566366131623263336434653566366131623263336434653566
         3661316232010000000000000042546865206465636c6172656420656666656374732073
         74617920696e7369646520746865205265706f7369746f727920616e64207370656e6420
         6e6f7468696e672e0100000001990970a000
sha256 = 8c9c3367fff1e77524014506a9bf48d0786d105b9011ccc373b79af0ae803b3d
```

The `record` value is wrapped for width; the bytes are the concatenation of its
lines, and `crates/beacond/src/ruling.rs` pins the same value and digest as test
constants.

**The command accepts typed arguments rather than a byte slice.** This is the one
place Ruling v1 does not copy Proposal v1, and the reason is the difference
between the two records. Proposal v1 accepts bytes because the bytes are the
caller's — untrusted, and the whole point is that a parsed object and a digest
can never disagree. A Ruling's bytes are Beacon's own, produced from an
authenticated Owner command over values Beacon already holds. Accepting them from
a caller would create an input surface with nothing to gain, because there is no
second party whose exact encoding must be preserved. Beacon canonicalises,
hashes, and stores; every read re-derives the digest from the stored bytes.

`decision` is the closed vocabulary `approve | refuse`. `decider_basis` is the
closed one-arm `owner`: the singleton account model has no Operator UUID, and v1
will not invent one. `reason` is mandatory, non-empty, arrives trimmed, is at
most 2,000 Unicode scalar values, and contains no NUL or non-whitespace control
character — the same validation a Proposal `summary` receives, from the same
predicate.

The record is append-only. A reversal is a **new** Ruling citing the prior one
through `supersedes`, never an edit, and store triggers refuse update and delete.

**Supersession is a relation, not a nullable column.** `ruling_supersessions`
holds one row per citation, keyed by `(workspace_id, proposal_digest,
ruling_digest)` with both foreign keys scoped over
`(workspace_id, proposal_digest)`. "This Ruling has no predecessor" is the absence
of a row, which carries no meaning a reader must interpret — unlike a NULL, which
would be a meaningful null of exactly the kind this repository forbids. The
canonical bytes say `supersedes = none` and the relation holds no row; every read
checks that the two agree, and startup re-checks it across the whole table. Scope is therefore structural: a link
across Workspaces or across Proposals cannot be stored, rather than being refused
by a Command that something might bypass.

## In force, superseded, and inapplicable are three different states

A Ruling is **in force** only while every revision it copied still equals the
current one. Drift is reported through the readiness vocabulary ADR 0034 already
established — `CharterRevisionMoved`, `ProjectPolicyRevisionMoved`,
`RepositoryRevisionMoved`, `BaseRevisionMoved` — computed at read time and never
stored, so a historical Ruling does not become corrupt because the world moved.

A Ruling is **superseded** when a later Ruling on the same Proposal digest cites
it. The chain is append-only and the effective Ruling for a digest is its last
link — which is only an answer if the graph cannot fork, so:

- the **first** Ruling for a Proposal has no predecessor;
- every **later** Ruling must cite the unique current tail. Citing any other
  Ruling is refused as `NotTheCurrentTail`. The service reads the tail, compares,
  and inserts inside one immediate transaction, so two concurrent rulings cannot
  both believe they extend it;
- the store forbids two shapes of a fork independently: each Ruling cites at most
  one predecessor (primary key) and is superseded at most once (unique index).
  Neither forbids two independent chains under one Proposal, which is why the
  chain is re-derived rather than assumed;
- every read and startup reconciliation re-derive the chain: one first link, one
  path, every Ruling on it.

**Any decision may follow any decision.** An `approve` after an `approve` with a
new reason is a legal re-ruling, not a contradiction; `reason` is mandatory and
non-empty on every link, which is what makes the chain readable as a history
rather than a state machine.

`ruled_at_ms` is non-decreasing along the chain — equal values are allowed,
because two rulings can land in the same millisecond and refusing that would be
inventing a constraint the clock does not support. **The effective order is chain
order, never timestamp order**, and `list_for_proposal` returns chain order,
oldest first. A timestamp is evidence about when; the chain is the record of what
followed what.

## Force is two facts, reported together

`RulingForce` is a struct, not an enum:

```text
{ superseded_by: Option<RulingDigest>, drift: [UnmetPrecondition] }
```

A Ruling is in force exactly when `superseded_by` is `None` **and** `drift` is
empty. Both facts are always reported, so no precedence rule is needed and no
caller has to learn which of "superseded" and "drifted" wins — a Ruling can be
both, and saying so is more useful than choosing.

A Ruling bound to one digest says nothing at all about a different digest. That
is **inapplicability**, not invalidity. Because Proposals are immutable, "the
Proposal changed" always means "there is a different Proposal", so the earlier
Ruling is not broken — it is irrelevant to the new bytes. No surface may report
the second as the first.

The Blueprint's wording — "Any material change produces a new Proposal and
**invalidates** the old Ruling" (`docs/BLUEPRINT.md:242-243`) — is read here as
*no longer applies*, not *the record changes*. Rulings bind bytes; the record is
append-only and never rewritten. A Ruling that has become inapplicable is still a
true account of what a human decided about the bytes it named.

## A Ruling creates no execution authority

It records that a human decided. Any future dispatch re-verifies independently
that the digest still names those bytes, that every copied revision still
matches, and that readiness reports no unmet precondition, and refuses otherwise
(`docs/BLUEPRINT.md:1160`: "If a Ruling is copied to a changed Proposal, dispatch
refuses it"). Recording and enforcing stay separate surfaces. A Ruling read is
never a permission check, and no accessor returns a boolean.

There is no withdrawal and no expiry in v1, because neither is a ruling: those
are states of a Gate, and v1 models no Gate. An Owner who changes their mind
records a new Ruling that refuses and cites the prior, which leaves the original
decision legible instead of erasing it.

## Persistence, idempotence, and reconciliation

The implementation uses schema version 19 and migration `0019_rulings.sql`. One
immutable Ruling row stores the canonical bytes, the digest envelope, the bound
Proposal digest and its canonicalizer, the four copied revision columns, the
decision, reason, and decider basis, the superseded Ruling if any, and the
ruling time. Composite foreign keys preserve Workspace scope. Store triggers
refuse update and delete.

An authenticated ruling performs one immediate transaction: verify the referenced
Proposal by re-deriving its digest from its stored bytes; copy its bindings;
canonicalise and hash the Ruling; insert the immutable row; and insert exactly
one `ruling.recorded` Event containing the digest envelope, the exact canonical
bytes as canonical RFC 4648 padded base64, actor `owner`, and the ruling time.

Re-ruling with identical content is refused rather than silently duplicated: two
Rulings that differ in nothing would produce one digest, and the second insert
would collide with the first. A deliberate reversal differs by its `supersedes`
link and therefore by its digest.

Startup recomputes each digest from stored bytes, strictly decodes and re-encodes
them, compares every indexed column and the supersession relation to the parsed
value, re-verifies that the bound Proposal exists and that the copied revisions
equal what that Proposal recorded, requires exactly one Event per Ruling whose
decoded byte field exactly equals the stored bytes with no orphan Event, verifies
the Owner account still exists for every `owner` decider, and re-derives every
chain: one first link, one path, every Ruling on it, and ruling times that do not
run backwards along it. Force is derived as part of that read and never refuses
startup by itself — drift fills a list, it does not raise an error.

Two of those are re-derived here even though the store already forbids them,
because reconciliation must be complete on its own terms rather than on the order
`inspect_store` happens to run its checks in. A fork and a cycle are not among
them: nothing in SQL forbids two independent chains under one Proposal, and a
cycle whose bytes agree with the relation is infeasible only because each
Ruling's bytes carry its predecessor's digest — a cycle of them would be a cycle
of SHA-256 preimages. What a store can actually hold is a cycle in the relation
that its Rulings' bytes contradict, and that is refused as the disagreement it
is.

Migration 0019 follows the existing closed-Event-table rebuild pattern. It
preserves every v18 Event sequence, payload byte, and timestamp, adds only
`ruling.recorded`, creates empty Ruling state, and advances the version only
from 18.

## Required evidence before implementation can land

- a known byte fixture independently reproduces its SHA-256 digest;
- every identity-bearing field changes the digest, and one changed byte, a
  reordered field, or a trailing byte is refused or produces a different
  identity;
- ruling on a digest that names no stored Proposal writes neither row nor Event;
- the copied revisions equal the bound Proposal's at ruling time, and a Ruling
  whose copies disagree with its Proposal refuses startup;
- moving any one of the four bound revisions puts the Ruling out of force naming
  that revision and only that one;
- a drifted Ruling stays readable and attributable, and startup still succeeds;
- a second Ruling supersedes the first, both stay readable, and the effective
  Ruling is the last in the chain;
- a `supersedes` naming no Ruling, or one bound to a different Proposal, is
  refused;
- a Ruling that cites two predecessors, or one superseded twice, is refused by
  the store; a chain that forks, or a relation that cycles, refuses startup;
- a ruling time earlier than the link it supersedes is refused when recorded and
  again at startup;
- update and delete against a recorded Ruling are refused by the store;
- an unauthenticated caller receives only the authentication refusal, whatever
  else is wrong with the request;
- identical re-ruling is refused rather than duplicated;
- missing, duplicate, orphaned, altered, or differently encoded Ruling state or
  Event evidence refuses startup;
- a reordered field or a trailing byte is refused rather than tolerated;
- `reason` is mandatory: empty, whitespace-only, or over-long is refused;
- `decision` and `decider` are closed: any tag this ADR does not assign is
  refused;
- a Ruling row with no Event, a duplicate Event, an orphan Event, an altered
  row, and a differently encoded Event payload each refuse startup;
- no accessor reports force as a boolean; and
- recording a Ruling leaves Run, Receipt, worktree, process, model Invocation,
  network, capability, and external-effect counts unchanged.

Each bullet above maps to one named test. The mapping is recorded in
`docs/implementation/phase-3-ruling-v1.md` so that "maps to a test" is a fact a
reviewer can check rather than a claim the author makes.

The full repository gate and a non-author exact-head review are required after
implementation. Applying migration 0019 to live data remains a separate RED
action.

## Consequences

**Accepted.** V1 can record only the Owner's decision, so the rule that an Agent
cannot approve its own Proposal has no code path to exercise yet; the closed
one-arm decider is what keeps that honest, rather than a check that cannot fail.
There is no Gate entity, so nothing yet reports which Proposals await a decision.
There is no withdrawal or expiry. There is no wire or desktop surface: nothing
can consume a Ruling until dispatch exists, and a surface that displayed
"approved" without a dispatch path would be the thing `BLUEPRINT:446` warns
against.

**Rejected.**

- **A mutable ruling status on the Proposal row:** lets a decision drift onto
  different content, which is the defect this record exists to prevent.
- **A caller-supplied `(ruling, digest)` pair:** permits the two to disagree, for
  the same reason ADR 0034 rejects it.
- **Any read that returns a boolean "authorized":** it would invite a consumer to
  skip the verification the Blueprint requires it to perform.
- **Withdrawal or expiry as ruling outcomes:** nobody ruled, so recording them as
  rulings would assert a decision that was never made.
- **Auto-approval, or a risk score that skips the human:** the record's entire
  value is that a person decided.
