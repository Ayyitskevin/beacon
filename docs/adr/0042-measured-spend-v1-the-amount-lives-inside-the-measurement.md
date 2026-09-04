---
status: proposed
---

# Measured spend v1: the amount lives inside the measurement

BLUEPRINT requires every Outcome Document to summarise "actual spend" (`docs/BLUEPRINT.md:180-181`),
requires Budget Envelopes whose "actual use rolls up through every ancestor" (`:432-434`), and makes
Models the module that "records every Invocation and enforces the Mission and Agent budgets"
(`:819`).

None of that exists. `BudgetEnvelope` has exactly one arm, `NoSpend`; there is no Models module, no
Invocation ledger, and Receipt v2 (ADR 0031) records no cost at all, though `:796` lists cost among a
Receipt's contents. `currency`, `minor_unit` and `amount_minor` appear nowhere in `crates`.

So this record has no producer, and building it anyway is the point. **Its job in v1 is the opposite
of what a spend record usually does: it makes "we did not measure" a first-class, attributable,
digest-identified statement rather than an absence.** Without it, an Outcome Document has to render
a Mission's cost as a blank, and a blank in a money column reads as zero.

## Decision

Beacon introduces `MeasuredSpendV1` — an immutable, digest-identified record of what one Mission
actually cost, over a declared window, by a named origin. Slice 1 is the pure record: types,
canonical bytes, and refusals. The store, the service, the event, the verified reader and the
Outcome collector's use of it are slice 2 and are not in this decision.

It creates no authority. Nothing derives "within budget" from its existence, and the module has
deliberately no cross-unit total and no `is_within_budget` — ADR 0034's reason for having no
`is_ready` and ADR 0037's for having no `is_in_force`. A single number read off a spend record is
exactly the shortcut that lets a consumer skip the check it owes.

### The amount lives inside the measurement tag

```text
SpendMeasurement {
  Measured(SpendTotals)                              0x01
  PartiallyMeasured(SpendTotals, [UncountedEntry])   0x02   -- at least one named gap
  Unmeasured(reason)                                 0x03   -- no amount field exists on this arm
  Broken(reason)                                     0x04   -- no amount field exists on this arm
}
```

"Absent is never zero" cannot be enforced by a rule that someone has to remember to write. It is
enforced by there being nowhere to put a zero: `Unmeasured` and `Broken` have no amount field, so no
caller can leave one at its default and no reader can find one. This is the same reason
`OutcomeState` wraps a value rather than sitting beside an `Option`.

Five states stay apart, and the fifth is not in this type:

| state | means | must never be spelled as |
|---|---|---|
| no record at all | the spend owner recorded nothing | `unmeasured`, `0` |
| `unmeasured` | a record exists and says nobody counted | absence, `0`, `broken` |
| `broken` | counting was attempted and the ledger is unusable | `unmeasured`, absence |
| `partially_measured` | counted, with at least one named gap | `measured` |
| `measured` | every priced unit in the window was counted; `0` here is a positive claim | `unmeasured` |

The first belongs to the citing document, not to this record. Collapsing it into `unmeasured` would
turn "nobody counted" into "the spend owner said nobody counted", which is a different and stronger
claim.

`counted_units` is not decoration. "Measured 0 over 0 priced units" and "measured 0 over 412" are
different claims, and without the count the second is indistinguishable from the first and from a
bug.

### Lines, not a total

A Mission can spend USD *and* tokens. One amount with one unit forces either a conversion Beacon
does not have and could not freeze, or silently dropping the other unit. So `SpendTotals` carries a
non-empty list of `(unit, amount_minor, minor_units_exponent)` lines and **no total**. A zero always
names its unit: `0 USD` is a claim, a bare `0` is not.

Units are stored exactly as measured (R13.6): `USD` and `usd` are two units. Folding them is the
transformation ADR 0034 refuses for a pinned base revision — a silent normalization of a stored
identity. Lines are unit-unique, because two amounts for one unit is a question rather than a record.

### Who measures, and why there is no third arm

`SpendOrigin` is `daemon_ledger` or `operator_declared`, and there is deliberately **no
`resident_reported`**. A Resident reporting its own spend is an Agent asserting its own budget
consumption; `:434` forbids an Agent to "increase, transfer, borrow, or reinterpret an Envelope", and
an Agent that can under-report its spend can exceed its Envelope while every record says it did not.
That is the same defect class as an Agent approving its own Proposal (`:445-446`) and harder to see,
because a number looks like evidence in a way a self-approval does not.

The two origins never merge. A merged figure is one nobody can reproduce — half in a ledger, half in
an Operator's memory of an invoice. Two records, cited separately, keep both reproducible.

### Reference identity beside content identity

Per R15, `spend_id` is a UUID **inside the canonical bytes**. The digest is *content* identity — what
the bytes are. The UUID is *reference* identity — which record a citation names. The natural key
`(workspace, mission, origin, version)` is a uniqueness constraint in the store and never an identity
in a citation. A citation reads `(workspace_id, spend_id, version, digest)`, and slice 2's verified
reader refuses a pair whose id and digest disagree.

`supersedes` names its predecessor's **digest**, not a version number, because a digest is
self-proving from the bytes alone while a version needs the store to say what that version was. Every
sibling does the same (ADR 0037, ADR 0038). The must-cite-the-current-tail rule and who enforces
contiguity are slice 2's.

## Exact canonical bytes

The ADR 0034/0037 discipline, unchanged: versioned magic, positional fixed-width fields, closed
one-byte tags never renumbered, explicitly length-prefixed UTF-8, and strict decoding that refuses
unknown tags, over-limit or invalid lengths, trailing bytes, invalid UTF-8, a repeated unit, and
non-canonical unit order. Decode, re-encode, and the bytes must be equal.

| # | field | encoding | bound |
|---|---|---|---|
| 1 | magic | 16 literal bytes `beacon.spend.v1\0` | fixed; not length-framed |
| 2 | `workspace_id` | uuid, 16 raw bytes | — |
| 3 | `mission_id` | uuid, 16 raw bytes | — |
| 4 | `spend_id` | uuid, 16 raw bytes | — |
| 5 | `version` | `u32` big-endian | — |
| 6 | `origin` | tag | `daemon_ledger` `0x01`, `operator_declared` `0x02` |
| 7 | `measured_at_ms` | `u64` big-endian | — |
| 8 | `coverage_from_event_seq` | `u64` big-endian | — |
| 9 | `coverage_to_event_seq` | `u64` big-endian | ≥ field 8 |
| 10 | `measurement` | tag | `0x01` measured, `0x02` partially, `0x03` unmeasured, `0x04` broken |
| 11 | └ `counted_units` (arms `0x01`,`0x02`) | `u64` big-endian | — |
| 12 | └ line count | `u32` big-endian | 1..=32 |
| 13 | └ `line.unit` | `u64` length + UTF-8 | 1..=16 bytes, `[A-Za-z0-9._-]`, ascending, unique |
| 14 | └ `line.amount_minor` | `i64` big-endian | — |
| 15 | └ `line.minor_units_exponent` | one byte | ≤ 9 |
| 16 | └ uncounted count (arm `0x02`) | `u32` big-endian | 1..=64 |
| 17 | └ `uncounted.kind` | tag | `0x01`..`0x04` |
| 18 | └ `uncounted.detail` | `u64` length + UTF-8 | 1..=2,000 scalars |
| 19 | └ `reason` (arms `0x03`,`0x04`) | `u64` length + UTF-8 | 1..=2,000 scalars |
| 20 | `supersedes` | tag | `none` `0x00`, `record` `0x01` |
| 21 | └ predecessor digest | 32 raw bytes | arm `0x01` only |

Every string field is `u64` length prefix then bytes, matching `beacon.proposal.v1` and
`beacon.ruling.v1`. Every declared count is checked against the remaining input at the smallest
possible size of one entry before the first entry is read, and no declared count reserves capacity.

### The record envelope is load-bearing here

`MAX_CANONICAL_BYTES` is 64 KiB, the store's envelope, enforced by the encoder and by the decoder
before the input is split. Unlike Ruling v1 — where nothing the grammar accepts comes near it — the
per-field limits here admit roughly **eight times** that: 64 uncounted entries at 2,000 scalars each.
So this bound refuses records the field table alone would accept, and it is tested rather than
assumed.

### Lines are refused out of order, not sorted

`beacon.outcome.manifest.v1` sorts its arrays before hashing, because a manifest is assembled from
several collections by a collector. This record is different: its bytes are the caller's, submitted
whole and proved by an exact decode/re-encode round trip. An encoder that silently re-sorted would
make a caller's unsorted bytes fail that equality check with a confusing error; refusing at both ends
with `unit_order` says which rule was broken. Two modules, two answers, one reason.

### Text carries LF and TAB

R13.5's rule applied to a record: LF and TAB are content and are preserved, every other control
character is refused by name, and empty, padded and over-long are three more names rather than one.
Whether a *renderer* can carry a line break is that renderer's rule — the Outcome Document refuses one
in a table cell (`line_break_in_label`) — and that constraint belongs there, not here. **The
consequence is a seam:** a valid spend reason spanning lines cannot render in the Outcome's current
spend section. Either that section moves reasons out of table cells, or spend reasons are single-line
by convention. Named here rather than discovered at slice 2.

## Published identity vector

- canonical byte count: **182**
- SHA-256: `9461d5deeba593e027dae523f3040515feea0ab5cb7b35cbf120a119ff7e774b`

```text
626561636f6e2e7370656e642e7631000000000000004000800000000000000100000000000040008000
00000000000200000000000040008000000000000003000000020100000
1a0635229200000000000000001000000000000107201000000000000019c000000020000000000000003
555344000000000001e240020000000000000006746f6b656e73000000000000000000
01aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
```

(Concatenate the lines, decode as hexadecimal.)

**Provenance, stated honestly.** `crates/beacond/tests/spend.rs` assembles these bytes a second time
from the table above without calling the encoder, and asserts the two agree — so a layout change
fails a named test. But that assembler and this encoder have the **same author**, which ADR 0038's
vector did not: gpt2 built that one from the field table before the encoder existed. A non-author
vector is the strongest thing this record is missing, and it is the right thing to ask of the review.

## What slice 1 deliberately does not do

No store, no service, no `spend.measured` event, no migration, no verified reader, and no Outcome
collector integration. Migration numbers are taken at rebase, never reserved: `main` is at 0020 and
several lanes are queued behind it.

`measured_by` is **not** a field, and the design that proposed it was wrong. With no Operator UUID in
the singleton account model — the reason `RulingDecider` has one arm — a `measured_by` would carry no
information `origin` does not already carry, and two fields that must always agree are two spellings
of one fact and eventually two answers. When an Operator identity exists, it is a new field and a new
canonicalizer version.

## Consequences

- A Mission's spend cannot be recorded until Models exists to measure it. Until then every honest
  record is `unmeasured`, which is the answer this record exists to make sayable.
- Byte-identical measurements taken twice are two records, because `spend_id` and `measured_at_ms`
  differ. That is the accountability event differing, not waste.
- Beacon will never sum two units. A consumer that wants one number has to choose a rate, and
  choosing a rate is a decision no ledger can reproduce afterwards.
- A multi-line spend reason is valid here and unrenderable in the Outcome's current spend section.
  The seam is named above and is one of the two open questions this slice hands on.
