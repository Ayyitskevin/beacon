-- TEST ONLY. Explicit fixture-only event-kind admission; not a numbered migration.
-- This fixture is loaded only by cfg(test) code on an owned disposable v37 store.
DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_i1_pre;

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
        'artifact.recorded',
        'invocation.admitted',
        'invocation.observed',
        'pilot.workspace_published', 'pilot.authority_published', 'pilot.authority_revoked',
        'pilot.operation_claimed', 'pilot.operation_observed', 'pilot.uncertainty_reviewed'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0),
    predecessor_kind TEXT NOT NULL CHECK (predecessor_kind IN ('genesis', 'linked', 'unchained')),
    predecessor_digest BLOB CHECK (predecessor_digest IS NULL OR length(predecessor_digest) = 32),
    link_digest BLOB CHECK (link_digest IS NULL OR length(link_digest) = 32),
    CHECK ((predecessor_kind = 'linked') = (predecessor_digest IS NOT NULL)),
    CHECK ((predecessor_kind = 'unchained') = (link_digest IS NULL))
) STRICT;

INSERT INTO events (
    sequence, workspace_id, kind, payload_json, occurred_at_ms,
    predecessor_kind, predecessor_digest, link_digest
)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms,
       predecessor_kind, predecessor_digest, link_digest
FROM events_i1_pre
ORDER BY sequence;

DROP TABLE events_i1_pre;

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
CREATE TABLE pilot_records (
 digest BLOB PRIMARY KEY CHECK(length(digest)=32),
 digest_algorithm TEXT NOT NULL CHECK(digest_algorithm='sha256'),
 digest_canonicalizer TEXT NOT NULL CHECK(digest_canonicalizer IN ('beacon.pilot.workspace.v1','beacon.pilot.authority.v1','beacon.pilot.operation.v1','beacon.pilot.revocation.v1','beacon.pilot.observation.v1','beacon.pilot.uncertainty.v1')),
 kind TEXT NOT NULL CHECK(kind IN ('workspace','authority','revocation','claim','observation','review')),
 workspace_id TEXT NOT NULL REFERENCES workspaces(id),
 scope_json TEXT NOT NULL CHECK(json_valid(scope_json)),
 canonical_bytes BLOB NOT NULL CHECK(length(canonical_bytes) BETWEEN 1 AND 65536),
 event_sequence INTEGER NOT NULL UNIQUE CHECK(event_sequence>0),
 occurred_at_ms INTEGER NOT NULL CHECK(occurred_at_ms>=0),
 owner_basis TEXT NOT NULL CHECK(owner_basis='owner')
) STRICT;
CREATE TABLE pilot_heads (
 scope TEXT PRIMARY KEY,
 digest BLOB NOT NULL REFERENCES pilot_records(digest)
) STRICT;

