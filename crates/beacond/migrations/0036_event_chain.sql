-- R58 (BLUEPRINT bullet 42): the event chain. Every Event links to the one before it, a stored
-- head names the last link, and startup verifies genesis -> head.
--
-- Measured before this migration (DP-b42-event-chain.md): of seven single-row tampers, five were
-- refused by the per-record startup passes and two opened Ready -- deleting the oldest Event, and
-- renumbering every sequence. Both are the words of the sentence this closes, because every refusal
-- came from a RECORD noticing its Event changed and nothing read the Events as a sequence.
--
-- SQLite cannot add a NOT NULL column without a default, and a default would be a value the chain
-- never computed, so this is 0027's tested rename-create-copy-drop shape for `events`. The `kind`
-- vocabulary, every CHECK and the AUTOINCREMENT key are carried forward unchanged; nothing FOREIGN
-- KEY-references `events`, so no child table moves with it.
--
-- THE GENESIS OF A MIGRATED STORE. A store that existed before this migration cannot honestly say
-- what preceded its rows: nothing recorded the order, and computing links now would attest to an
-- order this process INFERS rather than remembers. So pre-migration Events are written
-- `unchained` -- a state, not a null -- and `event_chain_state` records that this chain's genesis
-- is a MIGRATION rather than a creation, and through which sequence. It records NO digest of the
-- pre-migration table: see `event_chain_state.migrated_through_sequence` below for why one would
-- be a lie. The chain then reads "from the moment Beacon began chaining, nothing has changed",
-- which is what this migration can prove and no more.

-- Two triggers READ `FROM events`, and `ALTER TABLE ... RENAME` rewrites every trigger and view
-- that names the table so it follows the new name. Left alone, both would end up reading
-- `events_v35` -- a table this migration then drops -- and the rosters they close would stop being
-- closed. Found by three version-N rollback fixtures asserting the trigger text, which is the third
-- time this repository has been bitten by a rename rewriting something that was not the subject of
-- the migration (0033's foreign key, 0034's child table, these triggers).
--
-- They are dropped and recreated verbatim from 0015, around the rename, rather than left to be
-- rewritten and patched afterwards.
DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v35;

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN (
        'workspace.created',
        'starter_organization.accepted',
        'starter_organization.discarded',
        'project.created',
        'project.charter_revised',
        'project.policy_published',
        'project.repository_bound',
        'local_model.activated',
        'local_model.deactivated',
        'resident.activated',
        'resident.suspended',
        'resident.reactivated',
        'resident.retired',
        'resident.configuration_set',
        'resident.configuration_cleared',
        'team.formed',
        'mission.admitted',
        'run.preflighted',
        'run.executed',
        'sample_project.installed',
        'proposal.submitted',
        'ruling.recorded',
        'proposal.candidate_recorded',
        'proposal.dispatched',
        'installation.paused',
        'installation.resumed',
        'evaluation.recorded',
        'spend.measured',
        'mission.impact_classified',
        'mission.completed',
        'artifact.recorded'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0),
    -- Which of the three things this row says about the Event before it. Three answers to three
    -- different questions, never one nullable column: `unchained` and `genesis` are not degrees of
    -- each other, and a null would spell them the same way.
    predecessor_kind TEXT NOT NULL CHECK (predecessor_kind IN ('genesis', 'linked', 'unchained')),
    -- The predecessor's link digest, present exactly when there is a predecessor to name.
    predecessor_digest BLOB CHECK (predecessor_digest IS NULL OR length(predecessor_digest) = 32),
    -- This row's own link digest, under `beacon.event_chain.v1`. Present exactly when the row is
    -- IN the chain -- which a pre-migration row is not.
    --
    -- A migrated row could be given a digest computed now, and that digest would attest to its
    -- bytes AS THEY STAND rather than as they were written: a row tampered before this migration
    -- would be blessed by it. A chain that certifies what it never saw is worse than one that says
    -- where it begins, so an `unchained` row carries no digest and `event_chain_state` records the
    -- boundary instead.
    link_digest BLOB CHECK (link_digest IS NULL OR length(link_digest) = 32),
    CHECK ((predecessor_kind = 'linked') = (predecessor_digest IS NOT NULL)),
    CHECK ((predecessor_kind = 'unchained') = (link_digest IS NULL))
) STRICT;

-- Pre-migration rows carry their bytes unchanged and claim nothing: not about order, and not about
-- their own content. They stay covered by the per-record startup passes that already verify every
-- Event against the record it describes -- which is what this store has always had, and is exactly
-- what `event_chain_state` says the chain does NOT extend back over.
INSERT INTO events (
    sequence, workspace_id, kind, payload_json, occurred_at_ms,
    predecessor_kind, predecessor_digest, link_digest
)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms,
       'unchained', NULL, NULL
FROM events_v35
ORDER BY sequence;

DROP TABLE events_v35;

CREATE TRIGGER team_formation_members_no_insert_after_formation
BEFORE INSERT ON team_formation_members
WHEN EXISTS (
    SELECT 1 FROM events
    WHERE workspace_id = NEW.workspace_id
      AND kind = 'team.formed'
      AND json_extract(payload_json, '$.team.id') = NEW.team_id
)
BEGIN
    SELECT RAISE(ABORT, 'Team formation roster is closed');
END;

CREATE TRIGGER mission_admission_members_no_insert_after_admission
BEFORE INSERT ON mission_admission_members
WHEN EXISTS (
    SELECT 1 FROM events
    WHERE workspace_id = NEW.workspace_id
      AND kind = 'mission.admitted'
      AND json_extract(payload_json, '$.mission.id') = NEW.mission_id
)
BEGIN
    SELECT RAISE(ABORT, 'Mission admission roster is closed');
END;

-- One row, and the CHECK makes it one row: a second would be a second head.
CREATE TABLE event_chain_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    -- `created` means this store has chained since its first Event. `migrated` means it has chained
    -- since this migration and says so, which is the only honest claim a migrated store can make.
    genesis_kind TEXT NOT NULL CHECK (genesis_kind IN ('created', 'migrated')),
    -- The last pre-migration sequence: where the chain begins. Present exactly when the genesis is
    -- a migration.
    --
    -- There is deliberately NO digest of the pre-migration table here. The same argument that
    -- leaves pre-migration rows without a link digest applies to the table as a whole: a digest
    -- computed now attests to the bytes AS THEY STAND, so a store tampered before this migration
    -- would be blessed by it. The boundary sequence is what this migration can honestly record.
    migrated_through_sequence INTEGER,
    -- NULL means the chain is EMPTY -- nothing has been appended through the door yet -- and the
    -- next Event is the genesis link. A sentinel digest here would have to be compared against a
    -- constant to mean the same thing, and a sentinel that must be recognised is a value that can
    -- be forged.
    head_digest BLOB CHECK (head_digest IS NULL OR length(head_digest) = 32),
    head_sequence INTEGER NOT NULL,
    CHECK ((genesis_kind = 'migrated') = (migrated_through_sequence IS NOT NULL))
) STRICT;

-- The migrated store's genesis, written here rather than inferred at startup: the chain begins
-- AFTER the last pre-migration Event, it is empty, and this row says both.
-- A store with NO pre-migration Events has nothing to be migrated over: its chain genuinely begins
-- at its creation, and stamping it `migrated` would be a false claim about a history it does not
-- have. Every fresh store runs this ladder from 1, so without the CASE every new store would say
-- it had been migrated. Caught by this lane's own probe.
INSERT INTO event_chain_state (
    id, genesis_kind, migrated_through_sequence, head_digest, head_sequence
)
SELECT 1,
       CASE WHEN count(*) = 0 THEN 'created' ELSE 'migrated' END,
       CASE WHEN count(*) = 0 THEN NULL ELSE max(sequence) END,
       NULL,
       coalesce(max(sequence), 0)
FROM events;

CREATE TRIGGER event_chain_state_no_delete
BEFORE DELETE ON event_chain_state
BEGIN
    SELECT RAISE(ABORT, 'The event chain state is never deleted');
END;

-- The foreign-key guard 0033 established: a rebuild that orphaned a reference would otherwise be
-- found by a later reader rather than by the migration that caused it. `RAISE` is trigger-only, so
-- the CHECK on a temporary table is the shape that fails the transaction here.
CREATE TEMP TABLE migration_0036_foreign_key_guard (dangling INTEGER NOT NULL CHECK (dangling = 0));
INSERT INTO migration_0036_foreign_key_guard SELECT count(*) FROM pragma_foreign_key_check;
DROP TABLE migration_0036_foreign_key_guard;

UPDATE schema_meta SET version = 36 WHERE singleton = 1 AND version = 35;
