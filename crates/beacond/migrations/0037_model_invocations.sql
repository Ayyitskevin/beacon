-- Invocation storage (DP13 S1): one admitted Mission-plan request, its exact prompt and request
-- bytes, and the bounded transport observations C1's grammar already closes.
--
-- THIS MIGRATION CREATES NO AUTHORITY. A retained authority reference is a name and a claimed
-- digest, nothing behind it is verified here, and no row in these tables permits a provider call.
-- The structural ceilings the owner enforces are PARSER bounds, not a request/response envelope:
-- the envelope is one of the reserved product decisions and is retained as evidence inside the
-- admission's own canonical bytes, never spelled as a SQL literal.
--
-- Two Event kinds are added, so `events` is rebuilt on 0036's own rename-create-copy-drop shape.
-- Every schema-36 kind, every column, every CHECK and the AUTOINCREMENT key are carried forward
-- unchanged, and the two triggers that READ `FROM events` are dropped before the rename and
-- recreated verbatim afterwards -- the trap 0036 documents, which has now bitten this repository
-- four times (0033's foreign key, 0034's child table, 0036's triggers, and this one if left alone).
--
-- NO EVENT ROW CHANGES. `predecessor_kind`, `predecessor_digest` and `link_digest` copy across as
-- they stand: this migration adds a vocabulary, not a genesis, and re-deriving a link digest now
-- would attest to bytes as they stand rather than as they were written.

DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v36;

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
        'invocation.observed'
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
FROM events_v36
ORDER BY sequence;

DROP TABLE events_v36;

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

-- One admitted Mission-plan Invocation.
--
-- `canonical_bytes` is the identity's source; every projected column beside it is a candidate key
-- for a join and is re-derived from those bytes at every owning read. `prompt_inline` and
-- `request_inline` are the exact bytes the admission committed to by digest AND length, following
-- ArtifactV1's content discipline (`0027_artifacts.sql`, `artifact.rs`): a digest never substitutes
-- for the bytes, and the bytes are proved against the digest on every read.
--
-- NO SQL SIZE CEILING on the two blobs, deliberately, and this is the one place the Artifact
-- precedent cannot be copied. `artifacts.content_inline` may spell `<= 1048576` because R16/A7 fixed
-- that number; here the request/response envelope is a reserved product decision, so a literal would
-- freeze it. The owner enforces `length == claimed length` from the canonical bytes and refuses
-- anything past its own parser ceiling before the blob is transferred.
CREATE TABLE model_invocations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL CHECK (
        digest_canonicalizer = 'beacon.model_invocation.admission.v1'),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 300 AND 4096),
    invocation_id TEXT NOT NULL CHECK (length(invocation_id) = 36),
    project_id TEXT NOT NULL,
    mission_id TEXT NOT NULL,
    resident_id TEXT NOT NULL,
    admitted_configuration_id TEXT NOT NULL,
    activation_id TEXT NOT NULL,
    prompt_inline BLOB NOT NULL,
    request_inline BLOB NOT NULL,
    admitted_at_ms INTEGER NOT NULL CHECK (admitted_at_ms >= 0),
    creation_event_seq INTEGER NOT NULL CHECK (creation_event_seq >= 1),
    UNIQUE (workspace_id, invocation_id),
    UNIQUE (workspace_id, digest_value),
    FOREIGN KEY (workspace_id, mission_id)
        REFERENCES missions(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, resident_id)
        REFERENCES agents(workspace_id, id) ON DELETE RESTRICT,
    -- The four-column admission-members key, as ruled: `0018_proposals.sql` carries the exact
    -- four-column UNIQUE this references, and `0027_artifacts.sql` already depends on it the same
    -- way. It is not weakened to three columns.
    FOREIGN KEY (workspace_id, mission_id, resident_id, admitted_configuration_id)
        REFERENCES mission_admission_members(
            workspace_id, mission_id, resident_id, configuration_id
        ) ON DELETE RESTRICT
) STRICT;

-- One exact thing the local transport observed, sequenced within its Invocation.
--
-- The uniqueness constraints give one `headers_observed`, one `body_prefix_observed`, one
-- `body_complete` and one `body_incomplete` per Invocation. They do NOT encode `C xor I`, `P`
-- requires `H`, or the terminal rule: that is the owner's language, and C1 already owns it. A
-- projection CHECK that tried would be a second, drifting copy of a grammar.
CREATE TABLE model_invocation_observations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    invocation_id TEXT NOT NULL CHECK (length(invocation_id) = 36),
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    kind TEXT NOT NULL CHECK (kind IN (
        'headers_observed', 'body_prefix_observed', 'body_complete', 'body_incomplete')),
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL CHECK (
        digest_canonicalizer = 'beacon.model_invocation.observation.v1'),
    digest_value TEXT NOT NULL CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 60 AND 1052672),
    observed_event_seq INTEGER NOT NULL CHECK (observed_event_seq >= 1),
    PRIMARY KEY (workspace_id, invocation_id, sequence),
    UNIQUE (workspace_id, invocation_id, kind),
    UNIQUE (workspace_id, invocation_id, digest_value),
    FOREIGN KEY (workspace_id, invocation_id)
        REFERENCES model_invocations(workspace_id, invocation_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX model_invocations_by_mission
ON model_invocations (workspace_id, mission_id, admitted_at_ms, invocation_id);

CREATE TRIGGER model_invocations_no_update
BEFORE UPDATE ON model_invocations
BEGIN
    SELECT RAISE(ABORT, 'Model Invocations are immutable');
END;

CREATE TRIGGER model_invocations_no_delete
BEFORE DELETE ON model_invocations
BEGIN
    SELECT RAISE(ABORT, 'Model Invocations are immutable');
END;

CREATE TRIGGER model_invocation_observations_no_update
BEFORE UPDATE ON model_invocation_observations
BEGIN
    SELECT RAISE(ABORT, 'Model Invocation observations are immutable');
END;

CREATE TRIGGER model_invocation_observations_no_delete
BEFORE DELETE ON model_invocation_observations
BEGIN
    SELECT RAISE(ABORT, 'Model Invocation observations are immutable');
END;

-- 0033's foreign-key guard, for the same reason: a rebuild that orphaned a reference would
-- otherwise be found by a later reader rather than by the migration that caused it. `RAISE` is
-- trigger-only, so the CHECK on a temporary table is the shape that fails the transaction here.
CREATE TEMP TABLE migration_0037_foreign_key_guard (dangling INTEGER NOT NULL CHECK (dangling = 0));
INSERT INTO migration_0037_foreign_key_guard SELECT count(*) FROM pragma_foreign_key_check;
DROP TABLE migration_0037_foreign_key_guard;

UPDATE schema_meta SET version = 37 WHERE singleton = 1 AND version = 36;
