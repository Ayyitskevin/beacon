-- Spend attribution dimensions (alpha bullet 39): the canonicalizer CHECK admits both eras, and
-- the projection of a line's four dimensions gets a table of its own.
--
-- TWO CANONICALIZERS, NOT ONE. Migration 0030 faced the same choice for outcome manifests and
-- pinned its CHECK to the single new value. The opposite answer is right here, and the difference
-- is what the codec does with the old bytes: 0030's codec REFUSES `beacon.outcome.manifest.v1` by
-- name, so a store admitting a v1 row would hold a row its own reader cannot read. This codec
-- READS `beacon.spend.v1` -- as unattributed, with the reason naming its era (R46.7.1) -- so a
-- store that refused those rows would be refusing records its reader handles correctly, and the
-- refusal would arrive as a constraint error on a row that was never wrong. The CHECK's job is to
-- equal the codec's truth, and the codec's truth is two values.
--
-- Nothing rewrites a v1 row into v2 bytes. Its digest is its identity, computed over the bytes it
-- has; re-encoding it would mint a different identity for the same measurement and silently claim
-- the record carried dimensions its era could not record.
--
-- SQLite cannot alter a CHECK in place, so `measured_spend` is rebuilt: 0025's DDL copied with the
-- one CHECK widened (the migration test diffs the two definitions so nothing else can drift), rows
-- carried across, both immutability triggers recreated under their 0025 names. Nothing references
-- `measured_spend` by foreign key today -- its `UNIQUE (workspace_id, spend_id)` exists for a
-- citation that has not been built -- so the rename rewrites no other table.
--
-- WHY THE CHILD TABLE IS NOT BACK-FILLED. A line's dimensions live in the canonical bytes, and SQL
-- cannot decode them, so this migration cannot populate `measured_spend_lines` for rows that are
-- already stored. That absence is not ambiguous, because `digest_canonicalizer` answers it: a v1
-- row HAS no dimensions to project, and a v2 row cannot predate this migration because nothing
-- could write v2 bytes into the old CHECK. So "no line rows" means "a v1 record" or "an arm with
-- no lines" -- both readable from columns already here -- and never "we lost them". A projection
-- whose completeness had to be guessed would be the collapse this record exists to refuse.
--
-- NO `events` REBUILD: no Event kind is added. `spend.measured` is unchanged.

DROP TRIGGER measured_spend_no_update;
DROP TRIGGER measured_spend_no_delete;
ALTER TABLE measured_spend RENAME TO measured_spend_v25;

CREATE TABLE measured_spend_v35 (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL CHECK (
        digest_canonicalizer IN ('beacon.spend.v1', 'beacon.spend.v2')),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 16 AND 65536),
    spend_id TEXT NOT NULL CHECK (length(spend_id) = 36),
    mission_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version >= 1),
    origin TEXT NOT NULL CHECK (origin IN ('daemon_ledger', 'operator_declared')),
    measurement TEXT NOT NULL CHECK (
        measurement IN ('measured', 'partially_measured', 'unmeasured', 'broken')),
    coverage_from_event_seq INTEGER NOT NULL CHECK (coverage_from_event_seq >= 0),
    coverage_to_event_seq INTEGER NOT NULL CHECK (
        coverage_to_event_seq >= coverage_from_event_seq),
    measured_at_ms INTEGER NOT NULL CHECK (measured_at_ms >= 0),
    -- The reference identity a citation names, unique within its Workspace: the parent key a
    -- citing foreign key needs.
    UNIQUE (workspace_id, spend_id),
    -- The natural key, as a uniqueness constraint. Never an identity.
    UNIQUE (workspace_id, mission_id, origin, version),
    FOREIGN KEY (workspace_id, mission_id) REFERENCES missions(workspace_id, id) ON DELETE RESTRICT
) STRICT;

INSERT INTO measured_spend_v35 SELECT * FROM measured_spend_v25;
DROP TABLE measured_spend_v25;
ALTER TABLE measured_spend_v35 RENAME TO measured_spend;

CREATE TRIGGER measured_spend_no_update
BEFORE UPDATE ON measured_spend
BEGIN
    SELECT RAISE(ABORT, 'Measured spend records are immutable');
END;

CREATE TRIGGER measured_spend_no_delete
BEFORE DELETE ON measured_spend
BEGIN
    SELECT RAISE(ABORT, 'Measured spend records are immutable');
END;

-- One row per line of one v2 record: the four dimensions as (state, value) pairs, so a roll-up can
-- ask "what did this agent cost" without decoding every record's bytes.
--
-- THE PAIRING RULE IS A CHECK, ONCE PER DIMENSION. `not_applicable` carries nothing and the other
-- two both carry something -- a uuid or token when attributed, the REASON when unattributed -- so
-- `value IS NULL` is exactly `state = 'not_applicable'` and never a second spelling of "unknown".
-- A nullable value column without this rule would let `unattributed` arrive with nothing in it,
-- which is the collapse the wire format refuses by giving `0x02` a length and `0x03` none.
--
-- This is a PROJECTION, not the record. `canonical_bytes` on the parent is the truth; these rows
-- are derived from it at record time and are immutable with it.
CREATE TABLE measured_spend_lines (
    digest_value TEXT NOT NULL REFERENCES measured_spend(digest_value) ON DELETE RESTRICT,
    line_index INTEGER NOT NULL CHECK (line_index >= 0 AND line_index < 32),
    unit TEXT NOT NULL CHECK (length(unit) BETWEEN 1 AND 16),
    amount_minor INTEGER NOT NULL,
    minor_units_exponent INTEGER NOT NULL CHECK (
        minor_units_exponent >= 0 AND minor_units_exponent <= 9),

    agent_state TEXT NOT NULL CHECK (
        agent_state IN ('attributed', 'unattributed', 'not_applicable')),
    agent_value TEXT CHECK ((agent_state = 'not_applicable') = (agent_value IS NULL)),
    route_state TEXT NOT NULL CHECK (
        route_state IN ('attributed', 'unattributed', 'not_applicable')),
    route_value TEXT CHECK ((route_state = 'not_applicable') = (route_value IS NULL)),
    provider_state TEXT NOT NULL CHECK (
        provider_state IN ('attributed', 'unattributed', 'not_applicable')),
    provider_value TEXT CHECK ((provider_state = 'not_applicable') = (provider_value IS NULL)),
    model_state TEXT NOT NULL CHECK (
        model_state IN ('attributed', 'unattributed', 'not_applicable')),
    model_value TEXT CHECK ((model_state = 'not_applicable') = (model_value IS NULL)),

    PRIMARY KEY (digest_value, line_index)
) STRICT;

CREATE TRIGGER measured_spend_lines_no_update
BEFORE UPDATE ON measured_spend_lines
BEGIN
    SELECT RAISE(ABORT, 'Measured spend records are immutable');
END;

CREATE TRIGGER measured_spend_lines_no_delete
BEFORE DELETE ON measured_spend_lines
BEGIN
    SELECT RAISE(ABORT, 'Measured spend records are immutable');
END;

-- A roll-up asks one dimension at a time, and it asks for ATTRIBUTED rows: the state is in the key
-- so an index scan never has to read a row to find out it was unattributed.
CREATE INDEX measured_spend_lines_by_agent
    ON measured_spend_lines (agent_state, agent_value, unit);
CREATE INDEX measured_spend_lines_by_route
    ON measured_spend_lines (route_state, route_value, unit);
CREATE INDEX measured_spend_lines_by_provider
    ON measured_spend_lines (provider_state, provider_value, unit);
CREATE INDEX measured_spend_lines_by_model
    ON measured_spend_lines (model_state, model_value, unit);

UPDATE schema_meta SET version = 35 WHERE singleton = 1 AND version = 34;
