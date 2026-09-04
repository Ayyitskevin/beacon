---
status: accepted
---

# Record a non-author Resident's verdict about one exact Proposal digest

Proposal v1 (ADR 0034) records what was intended; Ruling v1 (ADR 0037) records
that a human decided about those exact bytes. Neither records what anyone
*judged* about them. The Blueprint requires that judgment separately, and
requires it from somebody who did not write the thing being judged: "Evidence
can influence a Ruling but cannot grant authority. An Agent cannot approve its
own Proposal through a model judgment. A Ruling record is not assumed favourable
merely because it exists" (`docs/BLUEPRINT.md:445-447`).

## Decision

An **Evaluation** is one non-author Resident's recorded verdict, bound to one
exact Proposal digest, under a rubric the evaluator declares.

**It confers no authority.** Nothing in this record opens a Gate, admits a
dispatch, prepares a worktree, starts a process, invokes a model, or makes a
network request, and no accessor answers a question with a boolean. `meets` is
not approval: approval is a Ruling word, and this vocabulary deliberately does
not contain it. A future dispatch re-verifies its own preconditions and refuses
otherwise; it does not read an Evaluation and proceed.

**A Ruling does not cite an Evaluation in v1.** The citation relation is
deferred until `beacon.ruling.v2` can cover the cited set with the Ruling's own
digest. An append-only relation attached afterwards still rewrites provenance by
addition: a citation attached after the fact is indistinguishable from one made
at ruling time, and a timestamp proves only that the Evaluation does not
postdate the Ruling, never that the citation existed when the Ruling was
recorded. v1 therefore stores Evaluations that no Ruling points at, which is
honest about what the bytes carry.

**The rubric fields are a self-report.** Nothing in Beacon resolves a rubric:
there is no rubric table, no owning module and no foreign key.
`BLUEPRINT:234` puts rubric and fixture management in the Evaluations
application, which does not exist. Recording "the evaluator says they applied
the bytes with this digest" is a truthful claim about the evaluator's *process*;
it is not a verified fact about *content*. In the wording ADR 0034 uses for a
declared readable context: **no surface may present a rubric as applied or
verified on the strength of these fields alone.**

## Canonical bytes

Same canonical style as `beacon.proposal.v1` and `beacon.ruling.v1` — one style
in this crate, not three. Integers unsigned big-endian at the stated width;
strings valid UTF-8 preceded by an unsigned big-endian 64-bit byte length;
SHA-256 values their 32 raw bytes; UUIDs their 16 raw bytes; enums a one-byte
tag that is never renumbered. The magic is the literal ASCII
`beacon.evaluation.v1\0`, **21 bytes** including its terminating NUL.

A list is an unsigned big-endian **`u32` count** followed by that many entries,
and the count is proved against the remaining input before anything is
allocated. This is stated rather than inherited: inheriting a list style
silently is how two incompatible codecs get written from one table.

| # | field | encoding | bound |
|---|---|---|---|
| 1 | magic | 21 literal bytes `beacon.evaluation.v1\0` | fixed; not length-framed |
| 2 | `subject` | tag: `proposal` = `0x01` | the only assigned value in v1 (amended 2026-09-04 — see the amendment) |
| 3 | └ `proposal_digest` | 32 raw bytes | — |
| 4 | └ `proposal_digest_algorithm` | string | `sha256` |
| 5 | └ `proposal_digest_canonicalizer` | string | `beacon.proposal.v1` |
| 6 | `evaluator.resident_id` | uuid | — |
| 7 | `evaluator.resident_configuration_id` | uuid | — |
| 8 | `rubric.id` | string | 1..=64 bytes |
| 9 | `rubric.version` | `u32` | ≥ 1 |
| 10 | `rubric.canonicalizer` | string | 1..=64 bytes |
| 11 | `rubric.sha256` | 32 raw bytes | the exact rubric bytes applied |
| 12 | `inputs.cited_runs` | `list<uuid>` | ≤ 64; a **set**, ascending raw-UUID order, duplicates refused |
| 13 | `verdict` | tag | `meets` `0x01`, `revision_requested` `0x02`, `does_not_meet` `0x03`, `uncertain` `0x04` |
| 14 | `findings` | `list<finding>` | ≤ 64; a **sequence**, order is semantic |
| 15 | &nbsp;&nbsp;└ `severity` | tag | `blocking` `0x01`, `concern` `0x02`, `note` `0x03` |
| 16 | &nbsp;&nbsp;└ `summary` | string | 1..=500 Unicode scalars |
| 17 | `conflicts` | tag: `none` = `0x00`; `declared` = `0x01` + string | ≤ 2,000 scalars |
| 18 | `rationale` | string | mandatory, trimmed, 1..=2,000 scalars |
| 19 | `supersedes` | tag: `none` = `0x00`; `digest` = `0x01` + 32 raw bytes | — |
| 20 | `evaluated_at_ms` | `u64` | **service clock**, never caller input |

`uncertain` is a first-class outcome, not a soft rejection: an unproved answer
that presents as a negative is a different lie from one that says so.
`conflicts = none` and `supersedes = none` are **values**, because `:630`
requires conflicts to be *recorded* — absence must encode as a statement rather
than as a missing field.

**The record has its own total bound: 65,536 bytes.** The per-field limits
alone admit 145,941 — 64 findings of 500 four-byte scalars is 128,576 of them —
so a grammar with only per-field bounds is not total: a record every row of the
table above accepts could not be stored. The encoder refuses over the bound
before anything reaches SQL, the decoder refuses before it parses, and the
`canonical_bytes` column repeats the number so the two cannot drift apart in
silence.

`cited_runs` is a set so that a permutation of the same inputs cannot produce a
second identity for one evaluation. `findings` is a sequence because the
evaluator wrote them in an order, and re-sorting by severity would be the
interface deciding what mattered.

### The limitation, stated as ADR 0037 states it

This grammar is positional and untagged, so two claims must stay separate. A
reordering the decoder can **see** — a tag where a length belongs, two adjacent
strings swapped — is refused, because the bytes stop parsing or a closed
vocabulary rejects them. A reordering of two **same-width** values is not
detectable by construction: an Evaluation whose `evaluator.resident_id` and
`evaluator.resident_configuration_id` are swapped is byte-for-byte a legitimate
record naming those UUIDs, and no predicate can separate them without an intent
the bytes do not carry. The same is true of `rubric.version` against any other
`u32`.

What holds instead is that such a swap **changes the identity**, because every
field is positional and every byte is covered by the digest. Field order is
fixed by the table above and pinned by the published identity vector below; it
is not, and must not be claimed to be, enforced at runtime.

`subject` and `verdict` are the one pair the caveat does not reach, and the
reason must be stated correctly or it reads as a licence the grammar does not
grant. They are not disjoint: both assign `0x01`. What holds is three separate
facts. `subject` assigns only `0x01` as shipped (amended 2026-09-04), so any other verdict value landing in its
position is an unassigned tag refused by a closed vocabulary. The `0x01`/`0x01`
case is not a swap at all, because the bytes are identical. And they are fields
2 and 13, separated by ten variable-length fields, so a positional transposition
of the pair is not expressible in the first place.

### Published identity vector

Built by a non-author seat (gpt2) from the field table above **before this
encoder existed** — no Beacon encoder, decoder, fixture or constant took part —
and cross-checked through coreutils. That is the only order in which a vector
proves anything: the grammar is written down, an independent reader encodes it,
and the encoder is then measured against what they produced.

- canonical byte count: **439**
- SHA-256: `dab418e0866b19f6536615a9fe8374b88034107ccbbdb769cf07515fbb516683`

```text
YmVhY29uLmV2YWx1YXRpb24udjEAAQABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4fAAAAAAAAAAZzaGEyNTYAAAAAAAAAEmJlYWNvbi5wcm9wb3NhbC52MRAhMkNUZXaHmKm6y9zt/g//7t3Mu6qZiHdmVUQzIhEAAAAAAAAAABJzZWN1cml0eS1yZXZpZXctdjEAAAAHAAAAAAAAABBiZWFjb24ucnVicmljLnYxICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8AAAACABEiM0RVZneImaq7zN3u//7cuph2VDIQASNFZ4mrze8EAAAAAgEAAAAAAAAAE0F1dGhvcnNoaXAgY29uZmxpY3QDAAAAAAAAAA1DaGVja3MgcGFzc2VkAQAAAAAAAAAaU2FtZSBsYWIgbGluZWFnZSBkaXNjbG9zZWQAAAAAAAAANEluZGVwZW5kZW50IGV2YWx1YXRvciByZXZpZXdlZCBleGFjdCBwcm9wb3NhbCBieXRlcy4BQEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl8BAgMEBQYHCA==
```

`a_known_evaluation_fixture_reproduces_its_published_digest` pins all three, and
`the_published_vector_holds_each_tag_where_the_field_table_puts_it` pins the
offsets the refusal tests measure, so a layout change fails a named test instead
of silently moving what those tests look at.

## Non-authorship, in two layers

`BLUEPRINT:445-447` requires that an Agent cannot approve its own Proposal
through a model judgment. That is enforced structurally, not by a caller
remembering a policy check, and it takes **two** layers because neither proves
the whole thing alone:

- **The store** refuses an insert whose projected evaluator Resident is the
  bound Proposal's author. SQL can prove that the *projection* does not name the
  author; it cannot know what the canonical bytes say.
- **The Command, every verified read, and startup reconciliation** re-derive the
  same relation from the canonical **bytes** against the bound Proposal. That is
  the layer proving the projection the trigger saw is the projection the bytes
  describe.

The comparison is on the **Resident**, never on the (Resident, configuration)
pair. Two configurations of one Resident are two evaluators for configuration
purposes and **one author** for this one; keying on the pair would let an author
re-evaluate their own Proposal by changing configuration.

The re-derivation at read and startup is deliberately redundant with the
trigger, exactly as ADR 0037 re-derives two store-forbidden properties:
reconciliation must be complete on its own terms rather than on the order the
store's checks happen to have run, and a store written by an older binary, a
manual repair, or a future migration bug is precisely the case a trigger cannot
speak to.

## Storage

Migration 0023 creates `evaluations`, `evaluation_findings` and
`evaluation_cited_runs`, with update and delete triggers refusing all three, and
adds the `evaluation.recorded` Event kind through the existing closed-Event-table
rebuild — preserving every prior sequence, payload byte and timestamp.

`evaluation_cited_runs` foreign-keys to **`receipts`**, not `runs`: a cited Run
must have recorded a Receipt, and pointing at `runs` would leave that true in
prose and structurally permitted.

`mission_id` on `evaluations` is the one column with **no counterpart in the
canonical bytes** — the grammar carries no Mission. It is copied from the bound
Proposal's verified payload so the evaluator-membership foreign key has a parent,
and it is therefore re-derived against the **Proposal** at every read and at
startup rather than against the bytes. Without that, a wrong Mission would be
invisible to both.

## Supersession and force

An evaluator may change their mind, and the record of the earlier judgment stays
true: it is a true account of what somebody judged about exact bytes, and no
later record makes it untrue. Supersession is therefore a **relation**, not an
edit and not a nullable column — "this Evaluation has no predecessor" is the
absence of a row, and the canonical bytes say `supersedes = none` so
reconciliation can check the two agree.

Each Evaluation cites at most one predecessor and is superseded at most once, so
the graph is a set of disjoint paths: no forks, and "the effective Evaluation is
the last link" always has an answer. A new Evaluation must supersede the
**current end** of its Proposal's chain; superseding an already-superseded link
would fork it. `evaluated_at_ms` is non-decreasing along the chain, and **equal
times are allowed** — two records can land in the same millisecond, and refusing
that would invent a precision the clock does not have.

**Chain order, never timestamp order.** A timestamp is evidence about when; the
chain is what the records say about each other.

`force` is derived at read time and never stored, as two separate facts:
`superseded_by` and `drift`. **There is deliberately no boolean anywhere on it**
— no `is_in_force`, no `is_acceptable`. Drift is about the bound *Proposal's*
revisions, because an Evaluation's own grammar carries no bindings: it judged the
exact bytes a Proposal digest names, and those bytes name the revisions. A
Repository rebind that keeps the same pinned commit reports only the revision as
moved, one drift and not two. Drift is never a startup failure: the world moving
is not a defect in the record.

## What v1 does not do

**It does not satisfy alpha criterion 26.** Criterion 26 requires that Material
Artifacts be evaluated by an Agent other than their author. Evaluation v1 makes
that requirement *checkable for Proposals* — it gives the verdict a shape, an
identity, a store, and a non-author invariant the store enforces — and a
Proposal is not a Material Artifact. There is no Artifact entity, no
Artifact-subject arm on `subject`, and no dispatch that would produce a Material
Artifact to evaluate. Closing criterion 26 needs all three. A later reader must
not mistake this evidence primitive for the criterion it serves.

v1 is complete at slices 1-3. Slice 4 — the
Ruling citation — is deferred, per the decision above. Until then no surface
consumes an Evaluation, which is deliberate: a panel showing `meets` with no
dispatch behind it would be `BLUEPRINT:446` in an interface.

## Amendment (2026-09-04) — the reserved Artifact subject arm (proposed)

Status note: the ADR stays `accepted`; this amendment is **proposed** until the
Evaluation owner's read and merge. It publishes two reservations and one rule so
that a later arm is an assignment, not a reinterpretation. Source: Brief K-2's
design (`beacon-team/A-kimi-evaluation-artifact-arm-design.md`) as folded after
the Evaluation owner's read (`beacon-team/D-opus2-review-eval-arm-design.md`),
and the slice-3 read (`beacon-team/D-kimi-read-slice3.md`).

**The reserved tags, published.** Field 2's tag space is amended to read:
`proposal` = `0x01` (assigned, the v1 arm); `candidate` = `0x02` (**reserved and
refused** until its arm lands); `artifact` = `0x03` (**reserved and refused**
until its arm lands); `0x04` and above unassigned and refused. This row is the
tag registry: a third arm takes the next value here, never by convention.

**The distinguishing rule (why the name stays `beacon.evaluation.v1`).** A
change that alters the byte layout of records that already exist takes a new
canonicalizer; a change that assigns a value in a published tag position does
not. Ruling v2 (ADR 0037's successor grammar) added a field — every existing
record's layout differs — and took a new name. The Artifact arm assigns a
reserved tag — no existing record's layout moves, every recorded v1 Evaluation
keeps its exact bytes and digest, and the published 439-byte Proposal-subject
vector (`dab418e0…6683`) remains the vector for arm `0x01`, pinned byte-for-byte
by `a_known_evaluation_fixture_reproduces_its_published_digest`. The honest
cost, stated: an older binary reading a store that holds an Artifact-subject
record refuses it as malformed, and startup reconciliation refuses the store —
fail-closed and correct. So the refusal is diagnosable as old-binary rather than
corrupt-record, the arm's build is named here: **arm `0x03` is introduced by the
build that lands migration 0028** (the arm's store migration, provisional
number). `beacon.evaluation.v2` was considered and rejected: the magic is in the
bytes, v1 records stay v1 either way, and v2 buys a second codec, grammar, and
vector set for a difference no reader can observe.

**The arm, once built.** Subject tag `0x03` binds the **Artifact record digest**
(SHA-256 over the `beacon.artifact.v1` canonical bytes, ADR 0043) — never the
bare content digest, because criterion 26 adjudicates author versus evaluator
and the author lives in the record. The arm's envelope carries the digest
envelope (algorithm `sha256`, canonicalizer `beacon.artifact.v1` — closed per
arm: an Artifact arm naming `beacon.proposal.v1` is refused, and the converse)
followed by the subject Artifact's declared `project_id` and `mission_id`,
**copied from the Artifact's canonical bytes**; the Command refuses a mismatch,
and every verified read and startup re-derive the comparison from both records'
bytes. Fields 6-20 of the table above are identical in position, width, and
bound; the arms of the tagged union may differ in width.

**Mission scope (R16/E1).** The evaluator's Mission membership needs a roster,
and an Artifact names no Proposal to derive one from. The subject Artifact
carries the scope: `context: none | mission(project, mission)` is a field of the
Artifact record (ADR 0043, ruled R16/E1), validated at record time against the
Mission's admitted members. An Artifact with `context = none` is not an
Evaluation subject in v1 — the check cannot be performed, so the Command refuses
it. The store's derived `mission_id` column for Artifact subjects is re-derived
against the subject Artifact, one hop through a different record, under the same
column discipline as v1.

**Non-authorship, keyed on the property.** The evaluator's Resident UUID is not
the subject Artifact's accountable Resident UUID — read from both records'
canonical bytes, keyed on the Resident and never on the (Resident,
Configuration) pair (an author changing configuration is still the author). One
exclusion, not two: an Artifact record has exactly one accountable party, and an
Evaluation binds one exact digest — the accountable Residents of *other versions
in the supersession chain* are not excluded, because the Resident who took over
a document at v3 is not the author of v1.

**Supersession: both links, always (R12.10 applied twice).** Evaluation chains
are per subject digest; a new Artifact version is a new subject, and the old
digest's Evaluations stay true of the old bytes and never carry over. A surface
showing an Evaluation of an Artifact shows the cited Evaluation digest with
*its* chain tail beside it, AND the cited Artifact digest with *its*
supersession tail beside it — both derived at read time, both labelled read-time
state, never silently the tail. Drift is empty by construction for Artifact
subjects (the record carries no Charter, Policy, Repository, or base revision),
and subject supersession is a separate read-time field beside `superseded_by`
and `drift`, folded into neither.

**Build rules the amendment pins.** (a) The store migration is ONE migration:
making `proposal_digest` nullable, adding the generalized subject columns, and
rekeying `evaluation_supersessions` and `evaluations_scoped_identity` to
`(workspace_id, subject_digest)` are inseparable — SQLite's MATCH SIMPLE means a
composite foreign key with a NULL component is satisfied, so a split lands
Artifact-subject rows that reference nothing. (b) The arm's frozen identity
vector is built by a non-author seat from the amended field table BEFORE the
arm's encoder exists — the order that proves anything. (c) The pinned refusal
test that flips the subject byte to `0x02` stays green while only arms `0x01`
and `0x03` exist; whichever later arm makes `0x02` valid edits that test, and a
reviewer should expect a pinned test edited by a later change. The arm's own
codec tests owe a `0x04+` refusal: assigning `0x03` is precisely when the
closed-vocabulary guarantee needs re-proving one value up, because the `0x02`
flip then stops being the boundary case.
