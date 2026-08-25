-- Per-type integrity: every `evidence.claim` belongs somewhere.
--
-- Lane A made `contains` the canonical relation and DELETED the derived
-- `mission_record_id` column, because a derived column is a second copy that
-- drifts. That left "every evidence.claim has a container" enforced nowhere,
-- and it landed in this lane as a per-type integrity rule.
--
-- WHAT IS ENFORCEABLE HERE, AND WHAT IS NOT. These are different strengths and
-- collapsing them would misrepresent the guarantee:
--
--   AT MOST ONE container   Lane A's `relation_one_container` unique index.
--                           Enforced. Not re-asserted here -- two layers
--                           enforcing one invariant is how they drift.
--
--   AT LEAST ONE, on write  NOT enforceable as a constraint. A relation
--                           references a record, so the record must exist
--                           first; any CHECK at insert time would fire before
--                           the relation it demands could possibly exist.
--                           SQLite defers foreign keys, not arbitrary CHECKs.
--                           So this is a CHECK you RUN (`orphaned_claims`),
--                           not a constraint the database holds.
--
--   AT LEAST ONE, on delete ENFORCEABLE, and enforced below. Removing the last
--                           container from a claim is a mutation the database
--                           can see in full, so it is refused.
--
-- The asymmetry is real rather than an oversight: creation can leave a claim
-- briefly uncontained inside a transaction, which is legitimate; deletion
-- leaves it uncontained permanently, which is not.

CREATE TRIGGER evidence_claim_keeps_its_container
BEFORE DELETE ON relation
WHEN OLD.kind = 'contains'
  AND EXISTS (
      SELECT 1 FROM record
      WHERE record.id = OLD.from_record
        AND record.capability = 'evidence'
        AND record.kind = 'claim'
  )
  AND NOT EXISTS (
      -- Another container already exists, so this delete is a replacement
      -- rather than an orphaning. The unique index makes this impossible
      -- today; it is written anyway so that relaxing that index does not
      -- silently turn a replacement into a refusal.
      SELECT 1 FROM relation r
      WHERE r.from_record = OLD.from_record
        AND r.kind = 'contains'
        AND r.to_reference <> OLD.to_reference
  )
BEGIN
    SELECT RAISE(ABORT, 'an evidence.claim must belong somewhere: this is its only contains relation. Attach the new container first, then remove this one.');
END;

-- The same rule on the other side: retyping a record INTO evidence.claim when
-- it has no container would create an orphan that no write-time check saw.
CREATE TRIGGER evidence_claim_retyped_keeps_its_container
BEFORE UPDATE OF capability, kind ON record
WHEN NEW.capability = 'evidence' AND NEW.kind = 'claim'
  AND NOT (OLD.capability = 'evidence' AND OLD.kind = 'claim')
  AND NOT EXISTS (
      SELECT 1 FROM relation
      WHERE from_record = NEW.id AND kind = 'contains'
  )
BEGIN
    SELECT RAISE(ABORT, 'this record has no contains relation, so retyping it to evidence.claim would create an orphan. Attach a container first.');
END;
