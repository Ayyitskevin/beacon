---
status: accepted
---

# The Mission impact classification is a record, scoped to one Mission

BLUEPRINT requires that "Policy-classified high-impact Missions include the verification appendix
automatically" (`docs/BLUEPRINT.md:871`, `:990`), and the Outcome Document design removed `omit` from
the appendix API (§6 item 6) so a caller cannot suppress that surface by asking for it.

Nothing on `main` can answer the question. `git grep -ciE 'high_impact|high-impact|impact_class'
origin/main -- crates` returns nothing, `missions` binds a charter revision and names no Policy, and
`project_policy_revisions` carries only Team-Lead selection criteria — with **no digest and no
canonical bytes at all**.

So the appendix rule has no input, and a renderer asked to decide would have to guess.

## Decision

Beacon introduces `MissionImpactClassificationV1` — an immutable, digest-identified record, scoped to
**one Mission**, that says whether that Mission is high-impact and cites the Project Policy revision
it was decided under. Slice 1 is the pure record: types, canonical bytes, and refusals.

### Not a field on Project Policy, and not derived

Three reasons, and the third is the decisive one.

1. **Scope.** `project_policy_revisions` is keyed `(workspace_id, project_id, revision)`. A field
   there cannot express "this one Mission"; it would make every Mission in a Project high-impact or
   none of them.
2. **No digest to cite.** Policy stores no canonical bytes. The Outcome manifest cites its
   classification as a `SourceRef` carrying a digest, and minting one retroactively for a shipped,
   immutable table is exactly what Kevin's ruling 5 refused for Receipts.
3. **Retroactive change.** Re-classifying one Mission would require publishing a new Project-wide
   Policy revision — and that revision would change the answer for Missions that **already
   completed**. Their published Outcomes are immutable and cannot follow, so the record and the
   document would permanently disagree about whether that document should have carried an appendix,
   with nothing to flag it.

Deriving the bit instead fails on both halves: there is nothing to derive from (`BudgetEnvelope` has
one arm, no Run, no Mission-scoped effect classes), and a derived bit is not a record, so the
verification appendix would carry a claim with no digest behind it.

So the classification cites Policy by the identity Policy actually has —
`(project_id, policy_revision, charter_revision)` — and carries its own digest.

### There is no `unclassified` arm

`ImpactLevel` is `high` or `ordinary`. A Mission with no classification has **no record**, and
`prepare` refuses. A default-to-ordinary arm would mean a Mission nobody classified silently gets no
verification appendix: omission achieving what removing `omit` from the API was meant to prevent. An
honest failure — completable but unpublishable until someone classifies — beats a silent one.

The test drives `0x00` specifically, because that is the tag a default would most plausibly take.

### The charter revision travels with the Policy revision

`applies_to_current_charter` on `main` shows a Policy revision can go stale for reasons that have
nothing to do with impact, so the record copies both. Copying is ADR 0037's argument for copying a
Proposal's bindings into a Ruling: a copy makes later drift detectable without trusting the source
row to have stayed put. Both are inside the hashed bytes, so a copy that drifted is detectable rather
than merely wrong.

**What copying does not yet buy** is a read-time drift vocabulary. ADR 0037 pairs a copy with
`RulingForce`; nothing here yet names how a consumer learns the cited Policy revision has moved since
classification. It is deferred rather than forgotten: when a consumer needs it, the shape is
`RulingForce`'s — two facts reported together, `superseded_by` and `policy_drift`, never one bit.

### `policy_rule` has a tag and no path

`ClassificationBasis` is `operator_declared` or `policy_rule`. Only the first can reach the store in
v1; a rule engine that classifies without an Operator is a new authority surface and is Kevin's
ambiguity P-2, not this grammar's.

Defining the tag while refusing it at submission is ADR 0034's `bound`-descriptor pattern, adopted
here deliberately: *"a later module may accept the already-defined tag only after a separate reviewed
decision"*. It is also the correction my own non-author read of the Evaluation Artifact arm asked for
— that arm leans on this precedent while ADR 0038 publishes no reserved tag at all. This ADR does not
repeat that mistake: the arm is in the table below.

`rule_id` is paired with `basis` in both directions and by both the encoder and the decoder. A policy
rule that names no rule cannot be audited; an Operator declaration that names one claims a provenance
it does not have. Absence encodes as a zero-length field, so an empty rule id is not a second
spelling of absence.

### `classified_by` stays, though `measured_by` did not

One arm, `operator`, for the reason `RulingDecider` has one: there is no Operator UUID in the
singleton account model, so v1 records the closed basis rather than inventing an identity.

Measured spend v1 (ADR 0042) **dropped** its equivalent field, and the difference is worth stating so
the pair does not read as inconsistent. `measured_by` there would have been one-to-one with `origin`,
and two fields that must always agree are two spellings of one fact. Here `basis` and `classified_by`
are not one-to-one: basis says what decided, actor says who recorded, and they separate the moment a
rule engine exists.

### Reference identity beside content identity

Per R15, `classification_id` is a UUID **inside the canonical bytes**. The digest is content identity;
the UUID is reference identity. The Outcome manifest cites `(workspace_id, classification_id, version,
digest)`, and slice 2's verified reader refuses a pair that disagrees.

`supersedes` names its predecessor's digest, not a version number, because a digest is self-proving
from the bytes alone.

### R13.7, and what slice 1 owes it

R13.7's contract is that `prepare` copies the bit into the Outcome manifest **and** the owning
verified reader re-checks it against this record, so the manifest asserts what Policy said at the
boundary rather than what Policy says now. The reader is slice 2's, with the store.

What slice 1 owes it is that the bit cannot be read without the record: `appendix_is_mandatory` hangs
off `MissionImpactClassificationV1`, not off `ImpactLevel`, so a caller holds the identity and digest
the manifest must cite beside it. There is deliberately no `is_high_impact(mission_id)` — a single bit
read off a classification is what `BLUEPRINT:446` warns about, and it is the reason `RulingForce`
exists instead of `is_in_force`.

## Exact canonical bytes

The ADR 0034/0037 discipline, unchanged.

| # | field | encoding | bound |
|---|---|---|---|
| 1 | magic | 25 literal bytes `beacon.mission-impact.v1\0` | fixed; not length-framed |
| 2 | `workspace_id` | uuid, 16 raw bytes | — |
| 3 | `mission_id` | uuid, 16 raw bytes | — |
| 4 | `classification_id` | uuid, 16 raw bytes | — |
| 5 | `version` | `u32` big-endian | — |
| 6 | `impact` | tag | `high` `0x01`, `ordinary` `0x02` |
| 7 | `basis` | tag | `operator_declared` `0x01`, `policy_rule` `0x02` (defined, refused at submission in v1) |
| 8 | `policy.project_id` | uuid, 16 raw bytes | — |
| 9 | `policy.policy_revision` | `u32` big-endian | ≥ 1 |
| 10 | `policy.charter_revision` | `u32` big-endian | ≥ 1 |
| 11 | `rule_id` | `u64` length + UTF-8 | zero length = absent; otherwise 1..=64 bytes, `[a-z0-9._-]`; present iff field 7 is `0x02` |
| 12 | `reason` | `u64` length + UTF-8 | 1..=2,000 scalars |
| 13 | `classified_by` | tag | `operator` `0x01` |
| 14 | `classified_at_ms` | `u64` big-endian | — |
| 15 | `supersedes` | tag | `none` `0x00`, `record` `0x01` |
| 16 | └ predecessor digest | 32 raw bytes | arm `0x01` only |

## Published identity vector

- canonical byte count: **219**
- SHA-256: `6b9f9051849fe42a67c44bfdde487af02082022edff2c1ffa777efba07da44c0`

```text
626561636f6e2e6d697373696f6e2d696d706163742e763100000000000000400080000000000000
01000000000000400080000000000000020000000000004000800000000000000300000002010100
00000000004000800000000000000400000003000000050000000000000000000000000000003a54
6865204d697373696f6e206469737061746368657320696e746f20746865204f70657261746f7227
73206f776e207265706f7369746f72792e01000001a06352292001bbbbbbbbbbbbbbbbbbbbbbbbbb
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
```

(Concatenate the lines, decode as hexadecimal.)

**Provenance, stated honestly.** `crates/beacond/tests/mission_impact.rs` assembles these bytes a
second time from the table above without calling the encoder, and asserts the two agree. That
assembler and this encoder have the **same author**, which ADR 0038's vector did not. A non-author
vector is what this record is missing and is the right thing to ask of the review — the same gap ADR
0042 records.

## What slice 1 deliberately does not do

No store, no service, no `mission.impact_classified` event, no migration, no verified reader, and no
`prepare` integration. Migration numbers are taken at rebase, never reserved.

## Consequences

- A Mission cannot publish an Outcome until someone classifies it. That is the honest failure the
  no-`unclassified` decision buys, and it should surface as an Attention Item rather than as a
  surprise at `prepare` — named as ambiguity P-4.
- Whether the Proposal-level high-impact notion (`:449`, `:723`, `:1181` — the Safety Snapshot
  trigger) is the same classification as this one is **unresolved and is ambiguity P-1**. This record
  deliberately claims only the Mission-level one, and names itself accordingly, so the two cannot be
  merged by accident before that is ruled.
- A copied Policy reference with no drift surface is a binding that can rot quietly. It is written
  down above rather than discovered.
