# M6 provisional dispositions — Muse under Kevin's delegation

Source: user-provided “Beacon decisions — M6 provisionals, proceed (Tue 2026-09-29 ~21:15 EDT)” message. Filed on local receipt 2026-09-29; the message's approximate time is its supplied label, not a independently verified decision timestamp.

**PROVISIONAL — reversible by Kevin; binding on documentary discovery only. No implementation may be built on these dispositions until Kevin ratifies or replaces them.** This memo transcribes the supplied dispositions, not a newly inferred ruling. No external/Janus lookup was performed.

## M6(a) — actor grammar for Resident-issued records

1. Actor persisted as a named value via the existing as_str/parse mapping pattern. No new persistence mechanism.
2. New actor variants need explicit mapping entries. Never widen enums silently; never infer authority for a new actor. (Per inventory: "do not widen enums and infer the new actors have authority"; "Reuse existing records/owners".)
3. Denials are explicit denial records (actor + reason), not silent drops — auditable lineage.
4. Compatibility: old mappings must round-trip; any future representation parses old values.

## M6(c) — Invocation admission and nested budgets

1. Admission is an explicit check at Invocation time against the Resident's grant. No implied admission from handle possession.
2. Nested budgets are hierarchical: child budget carved from and bounded by the parent's. A child can never spend what the parent does not hold.
3. Usage metered at each level against the applicable (sub-)budget.

## Not decided here

B1/M1, B7, G3/capacity holds already have dispositions as held — they stand. Q01–Q06 still await authentic owner returns. Final M6 semantics await Kevin.

## Cross-team update supplied with the dispositions

Chronos FIX-28 (#277) is now on main — the clock time-bomb is fixed there. No Beacon action needed; this supersedes its prior open cross-team status. This is the user's update, not a remote status query by this run.

## Citation keys and scope

For draft traceability only: M6-A1 through M6-A4 identify the four M6(a) paragraphs above; M6-C1 through M6-C3 identify the three M6(c) paragraphs above. These labels add no semantics. Every dependent draft assumption must carry PROVISIONAL plus its key(s). Existing accepted requirements remain separately attributed. Uncovered questions must be recorded and parked, never answered by extending these dispositions. Drafts only; nothing lands outside Muse's merge lane. Hosted Actions remain unavailable by policy; local verification only. Preserve every branch/worktree/artifact. No product code, authority, permission, or Tier change.
