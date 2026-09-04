-- A Mission's declared success conditions, given the identity they never had (ADR 0041).
--
-- `missions.success_conditions_json` is a JSON array of normalized strings. The array's index is a
-- stable identity in practice -- the row is immutable (`missions_no_update`, `missions_no_delete`),
-- a JSON array preserves order, and the read path re-normalizes -- but nothing NAMES it as one,
-- nothing tests it, and no foreign key can point at it. A consumer that reported a result per
-- condition would be enforcing "exactly once" by counting an array in application code, which is
-- the defect every verified reader in this store exists to prevent.
--
-- So the conditions get rows, and the identity of a condition is
-- `(workspace_id, mission_id, ordinal)` -- the index in the Mission's own declaration order.
--
-- ADDITIVE ONLY. No table is renamed, no Event byte is rewritten, no trigger is dropped, and the
-- Event-kind vocabulary is untouched: this slice adds no Event. The closed-Event-table rebuild
-- arrives with the completion record that needs `mission.completed`, not here.
--
-- `success_conditions_json` STAYS. It is part of the Mission's admission bytes and of the
-- `mission.admitted` Event payload; deleting it would rewrite history. This table is the
-- referenceable projection of it, and startup re-derives one from the other so the two cannot
-- drift.
CREATE TABLE mission_success_conditions (
    workspace_id TEXT NOT NULL,
    mission_id TEXT NOT NULL,
    -- 0-based index into the Mission's declared array, dense and ascending.
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    -- The bounds `mission::normalize_items` already enforces, repeated here so the invariant is in
    -- the store rather than only in Rust.
    text TEXT NOT NULL CHECK (length(trim(text)) BETWEEN 1 AND 300),
    PRIMARY KEY (workspace_id, mission_id, ordinal),
    -- Distinct within a Mission: `normalize_items` refuses a duplicate condition at admission, so a
    -- pair of identical rows is a fact about tampering, not about a Mission.
    UNIQUE (workspace_id, mission_id, text),
    FOREIGN KEY (workspace_id, mission_id)
        REFERENCES missions(workspace_id, id) ON DELETE RESTRICT
) STRICT;

-- Immutable for the same reason the Mission is: the ordinal is only an identity if it cannot move.
CREATE TRIGGER mission_success_conditions_no_update
BEFORE UPDATE ON mission_success_conditions
BEGIN
    SELECT RAISE(ABORT, 'Mission success conditions are immutable');
END;

CREATE TRIGGER mission_success_conditions_no_delete
BEFORE DELETE ON mission_success_conditions
BEGIN
    SELECT RAISE(ABORT, 'Mission success conditions are immutable');
END;

-- INSERT is deliberately NOT refused by a trigger, and the cost is stated rather than hidden.
-- The backfill below is an insert, and so is every future Mission's. A trigger that compared each
-- inserted row against the Mission's JSON would make a disagreeing row unrepresentable -- and would
-- also make the startup re-derivation below untestable, because nothing could produce the row it
-- exists to catch. Two mechanisms where one can be shown to work: the guard is the reconciliation,
-- and its tests create the bad rows directly.
INSERT INTO mission_success_conditions (workspace_id, mission_id, ordinal, text)
SELECT missions.workspace_id, missions.id, condition.key, condition.value
FROM missions, json_each(missions.success_conditions_json) AS condition;

UPDATE schema_meta SET version = 24 WHERE singleton = 1 AND version = 23;
