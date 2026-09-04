-- Measured spend and the Mission impact classification, given stores (ADR 0042, ADR 0044).
--
-- The number is SETTLED. This lane and the Mission-completion lane both rebased into the same
-- window; completion landed first as 0024, so this one took 0025 at its final rebase, which is what
-- the provisional note said whichever landed second would do. The ladder steps 24 -> 25.
--
-- NOTE FOR THE NEXT LANE THAT REBUILDS THIS TABLE: renumbering is not the whole of a rebase. The
-- kind list below must be REGENERATED FROM THE NEW PARENT, because a list generated once and then
-- carried forward drops every kind added since -- and `verify_schema` cannot catch it, since
-- expected and actual are both built from this file. Only real rows can.
--
-- This migration REBUILDS the closed Event-kind table to add two kinds. That rebuild is the one
-- place a migration can silently delete history: a kind dropped from the CHECK takes every stored
-- row that uses it. The list below is COPIED from 0023's, not retyped, and
-- `the_event_kind_rebuild_drops_no_kind_the_store_already_had` proves it row by row.

DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v24;

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
        'mission.impact_classified'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v24
ORDER BY sequence;

DROP TABLE events_v24;

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



-- One measured-spend record, its canonical bytes, and the identity those bytes hash to.
--
-- `spend_id` is the record's REFERENCE identity and lives inside the canonical bytes (R15); the
-- digest is its CONTENT identity. The natural key below is a uniqueness constraint and never an
-- identity in a citation. Every other column is a PROJECTION for querying: the verified reader
-- re-derives all of them from `canonical_bytes` and refuses any row where the two disagree, so a
-- decoy row with a true column and false bytes cannot become a rendered claim.
CREATE TABLE measured_spend (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL CHECK (digest_canonicalizer = 'beacon.spend.v1'),
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

-- One Mission impact classification. Same discipline, same reasons.
--
-- It cites Project Policy by the identity Policy HAS -- `(project_id, policy_revision,
-- charter_revision)` -- because Policy stores no canonical bytes and no digest, and minting one for
-- a shipped immutable table is what Kevin's ruling 5 refused for Receipts.
CREATE TABLE mission_impact_classifications (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL
        CHECK (digest_canonicalizer = 'beacon.mission-impact.v1'),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 25 AND 65536),
    classification_id TEXT NOT NULL CHECK (length(classification_id) = 36),
    mission_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version >= 1),
    impact TEXT NOT NULL CHECK (impact IN ('high', 'ordinary')),
    basis TEXT NOT NULL CHECK (basis IN ('operator_declared', 'policy_rule')),
    policy_revision INTEGER NOT NULL CHECK (policy_revision >= 1),
    charter_revision INTEGER NOT NULL CHECK (charter_revision >= 1),
    classified_at_ms INTEGER NOT NULL CHECK (classified_at_ms >= 0),
    UNIQUE (workspace_id, classification_id),
    UNIQUE (workspace_id, mission_id, version),
    FOREIGN KEY (workspace_id, mission_id) REFERENCES missions(workspace_id, id) ON DELETE RESTRICT,
    -- The composite key that makes it impossible to classify a Mission under a Policy revision that
    -- does not exist. The check that would otherwise live in prose.
    FOREIGN KEY (workspace_id, project_id, policy_revision)
        REFERENCES project_policy_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER mission_impact_classifications_no_update
BEFORE UPDATE ON mission_impact_classifications
BEGIN
    SELECT RAISE(ABORT, 'Mission impact classifications are immutable');
END;

CREATE TRIGGER mission_impact_classifications_no_delete
BEFORE DELETE ON mission_impact_classifications
BEGIN
    SELECT RAISE(ABORT, 'Mission impact classifications are immutable');
END;

UPDATE schema_meta SET version = 25 WHERE singleton = 1 AND version = 24;
