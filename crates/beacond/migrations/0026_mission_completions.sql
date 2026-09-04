-- A Mission finished: `MissionCompletionV1`'s store, its Event, and the exactly-once invariant
-- that makes "a result for every declared success condition" a fact the database holds rather than
-- one application code counts.
--
-- PROVISIONAL NUMBER. 0024 and 0025 landed first, so this file took 0026 at its rebase and the
-- ladder steps 25 -> 26. kimi's Artifact store is the remaining lane in this window and renumbers
-- to 0027 when it lands; if it lands first, this file renumbers again. The ADR says the same.
--
-- This is the ONE migration in the Outcome slice-2 plan that rebuilds a closed table. Its kind list
-- was generated from the parent migration -- 0025, not the 0023 this file was first written against
-- -- rather than retyped, because a list copied from an older parent silently drops every kind
-- added since, and the rows that used them. The two Event-keyed roster triggers below are taken
-- from that same parent's text.
DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v25;

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
        'mission.completed'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v25 ORDER BY sequence;

DROP TABLE events_v25;

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

-- One completion record, its canonical bytes, and the identity those bytes hash to.
--
-- Every column below is also in the bytes, and every read re-proves the two agree. `project_id` is
-- the one that is ALSO derivable from the Mission, and the reader compares all three ways: bytes,
-- column, and the Mission's own row.
CREATE TABLE mission_completions (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL
        CHECK (digest_canonicalizer = 'beacon.mission_completion.v1'),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64 AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    -- The upper bound repeats the codec's own `MAX_CANONICAL_BYTES`, which the encoder enforces
    -- first: without it a record every field table accepts would be refused here instead, as a
    -- database error surfacing through a Command rather than as a fact about the bytes.
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 29 AND 65536),
    -- The record's REFERENCE identity (R15): which record a citation names. The digest is its
    -- content identity. A citation carries both and a verified reader refuses a pair that disagrees.
    completion_id TEXT NOT NULL CHECK (length(completion_id) = 36),
    mission_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    accountable_resident_id TEXT NOT NULL,
    accountable_configuration_id TEXT NOT NULL,
    -- The cut this record reports at. Startup proves it is at or after the Mission's admission and
    -- strictly before this completion's own Event.
    boundary_event_seq INTEGER NOT NULL CHECK (boundary_event_seq >= 0),
    disposition TEXT NOT NULL CHECK (disposition IN ('concluded', 'abandoned')),
    completed_at_ms INTEGER NOT NULL CHECK (completed_at_ms >= 0),
    UNIQUE (workspace_id, completion_id),
    FOREIGN KEY (workspace_id, mission_id)
        REFERENCES missions(workspace_id, id) ON DELETE RESTRICT,
    -- The accountable Resident must be an admitted member of THIS Mission under the exact
    -- configuration named. Membership is structural, not asserted.
    FOREIGN KEY (workspace_id, mission_id, accountable_resident_id, accountable_configuration_id)
        REFERENCES mission_admission_members(
            workspace_id, mission_id, resident_id, configuration_id
        ) ON DELETE RESTRICT
) STRICT;

-- Identified within its Workspace AND by its digest, so a relation cannot reach across Workspaces:
-- the primary key alone is the digest, which is not a key a composite foreign key can point at.
CREATE UNIQUE INDEX mission_completions_scoped_identity
ON mission_completions (workspace_id, digest_value);

-- One completion in force per Mission is slice 4's (supersession); v1 stores at most one, and the
-- index is what makes a second one a store refusal rather than a service check.
CREATE UNIQUE INDEX mission_completions_one_per_mission
ON mission_completions (workspace_id, mission_id);

CREATE TRIGGER mission_completions_no_update
BEFORE UPDATE ON mission_completions
BEGIN
    SELECT RAISE(ABORT, 'Mission completions are immutable');
END;

CREATE TRIGGER mission_completions_no_delete
BEFORE DELETE ON mission_completions
BEGIN
    SELECT RAISE(ABORT, 'Mission completions are immutable');
END;

-- One row per declared success condition, and the foreign key is the point: an ordinal past the end
-- of the Mission's declaration cannot be stored, so "the ordinal names a real condition" is the
-- store's rule rather than a comparison the reader has to remember to make.
--
-- The exactly-once invariant is this table's PRIMARY KEY (no duplicate) plus the reader's count
-- comparison against `mission_success_conditions` (no gap, no missing). Two mechanisms for two
-- failure shapes, tested with each other removed.
CREATE TABLE mission_completion_conditions (
    workspace_id TEXT NOT NULL,
    completion_digest TEXT NOT NULL,
    mission_id TEXT NOT NULL,
    condition_ordinal INTEGER NOT NULL CHECK (condition_ordinal >= 0),
    outcome TEXT NOT NULL CHECK (outcome IN (
        'met', 'not_met', 'not_assessed', 'assessment_inconclusive'
    )),
    PRIMARY KEY (workspace_id, completion_digest, condition_ordinal),
    FOREIGN KEY (workspace_id, completion_digest)
        REFERENCES mission_completions(workspace_id, digest_value) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, mission_id, condition_ordinal)
        REFERENCES mission_success_conditions(
            workspace_id, mission_id, ordinal
        ) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER mission_completion_conditions_no_update
BEFORE UPDATE ON mission_completion_conditions
BEGIN
    SELECT RAISE(ABORT, 'Mission completion conditions are immutable');
END;

CREATE TRIGGER mission_completion_conditions_no_delete
BEFORE DELETE ON mission_completion_conditions
BEGIN
    SELECT RAISE(ABORT, 'Mission completion conditions are immutable');
END;

UPDATE schema_meta SET version = 26 WHERE singleton = 1 AND version = 25;
