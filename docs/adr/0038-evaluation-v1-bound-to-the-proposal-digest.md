---
status: proposed
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
| 2 | `subject` | tag: `proposal` = `0x01` | the only assigned value in v1 |
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
facts. `subject` assigns only `0x01`, so any other verdict value landing in its
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
