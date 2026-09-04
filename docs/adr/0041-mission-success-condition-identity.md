# Give a Mission's success conditions an identity

- Status: accepted
- Deciders: the Product Steward
- Date: 2026-09-04

## Context

`MissionCompletionV1` must report an outcome for **every declared success condition, exactly once**
(`A-opus-mission-completion-design.md`, Outcome Document prerequisites 1 and 4). Before that record
can be written, a condition has to be something a record can *name*.

Today it is not. `missions.success_conditions_json` is a JSON array of normalized strings
(`0011_missions.sql`), `MissionDraft.success_conditions` is a `Vec<String>`, and
`normalize_items` bounds the list at 20 items of 1..=300 scalars each, trimmed, control-free and
deduplicated by exact string. There is no id and no ordinal column.

The array index *is* stable in practice, but only as an accident of three unrelated mechanisms:
`missions_no_update` and `missions_no_delete` make the row immutable; a JSON array preserves order;
and `load_stored_mission` re-normalizes on read while `verify_mission_event` compares the normalized
Mission against its Event. Nothing names the index as the identity, nothing tests it, and **no
foreign key can point at it**. A completion record that reported "one result per condition" against
a JSON blob would enforce its central invariant by counting an array in application code — the exact
defect every verified reader in this store exists to prevent, and the one the Outcome Document's read
discipline states as a rule: a relation may identify a candidate, never supply a claim.

## Decision

**A condition's identity is `(workspace_id, mission_id, ordinal)`**, where `ordinal` is its 0-based
index in the Mission's own declaration order, and conditions get rows so that identity can be
referenced.

`mission_success_conditions` holds one row per declared condition, with the same bounds
`normalize_items` enforces repeated as store constraints, a composite foreign key to the Mission, and
`UNIQUE (workspace_id, mission_id, text)` — because admission already refuses a duplicate condition,
so a pair of identical rows is a fact about tampering rather than about a Mission. The table refuses
UPDATE and DELETE: the ordinal is an identity only if it cannot move, and the text is evidence only
if it cannot be rewritten.

**`success_conditions_json` stays.** It is part of the Mission's admission bytes and of the
`mission.admitted` Event payload; removing it would rewrite history. The table is the referenceable
projection of it, and startup re-derives one from the other.

**Migration 0024 is additive.** It adds no Event kind, renames no table and drops no trigger, so the
closed-Event-table rebuild that this series will eventually need waits for the completion record that
actually needs `mission.completed`. Rows are written in two places: the migration backfills every
Mission that already exists, and `insert_admission` writes them for every Mission after it, in the
same transaction as the Mission and its Event.

### Two reconciliation checks, deliberately not one

Startup re-derives the table from the Mission's own bytes with **two separate comparisons**:

- the **count**, against the JSON array's length — which catches a row the Mission never declared, a
  shape a per-ordinal walk of the array cannot see, because the extra row sits past its end;
- the **text, per ordinal** — which catches a row rewritten in place, a shape the count cannot see,
  because rewriting one is not adding one.

Deriving both from a single set comparison would be the smaller code and the weaker evidence: both
scenarios would pass through one assertion, and one mutant would kill a pair of tests that are
supposed to be discriminating. The mutation gate asserts the separation in both directions —
deleting either check must leave the *other* scenario passing.

### No INSERT trigger, and why

Nothing in the store compares an inserted row against the Mission's JSON. A trigger that did would
make a disagreeing row unrepresentable — and would also make the reconciliation above **dead code**,
because nothing could produce the row it exists to catch. A check that cannot fail is a sentence
rather than a guard. The guard is the reconciliation, and its tests write the bad rows directly.

If a later slice wants the trigger as well, the reconciliation stays: it is what covers rows written
before the trigger existed.

### What this ADR does not do

No completion record, no Event, no Command, no transport, no per-condition vocabulary. Those are the
later slices of the design this serves. In particular the **per-condition outcome vocabulary is
MissionCompletionV1's** and the Outcome Document imports it (FABLE-RULINGS R13 item 3); nothing on
this branch touches `outcome`.

## Consequences

- A completion record can foreign-key a result to the condition it reports on, and "exactly once"
  becomes an ordinal-set comparison against a store table rather than an array count in Rust.
- The ordinal's stability is now a **stated, tested invariant** rather than an accident of three
  mechanisms, and the reason it holds — the Mission row is immutable — is written down where the next
  reader will look.
- A Mission and its conditions can disagree only by tampering, and startup refuses when they do.
- **Migration numbering.** This slice is **0024**, and the collision the first draft of this line
  predicted is what happened. 0021 was free on `origin/main` when the branch was cut; Candidates then
  merged at 0022 and Evaluation renumbered itself from 0021 to 0023, so this one — last of the three
  to rebase — took 0024. Recorded so the number's history is legible rather than looking arbitrary,
  and because the prediction being right is the argument for naming such things before merge.
- **The ladder gained two arms it never had.** `main` had no arm for schema 22 or 23, so a store at
  either would have refused startup as `UnsupportedSchemaVersion` the moment any later migration
  existed. Every store on `main` tonight is at 23. Both arms are here, and the roll-back guard is
  what proves they work.

## Alternatives rejected

- **Mint a UUID per condition.** A condition already has a natural key inside an immutable row. A
  UUID would be an identity assigned by a consumer to the Mission module's record, and it would not
  appear in the Mission's own bytes, so nothing could verify it against them.
- **Report results by condition text.** Admission deduplicates by exact string, so the text is unique
  within a Mission — but it is up to 300 scalars of operator prose, and keying a durable record on it
  would make every future edit to how text is normalized an identity change.
- **Leave the JSON as the only form and count in code.** This is the status quo, and it is what makes
  the completion record's central invariant unenforceable by the store.
