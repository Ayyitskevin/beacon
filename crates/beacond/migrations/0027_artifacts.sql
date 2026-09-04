-- Artifact v1 slice 2 (ADR 0043): the Artifact store, its Event kind, and the supersession
-- relation.
--
-- An Artifact is a versioned work product with content identity and provenance. Recording one
-- creates no authority: it opens no Gate, admits no dispatch, completes no Mission, and publishes
-- no Outcome. v1 is INLINE-ONLY (R16/A7): the content lives in `content_inline`, bounded by the
-- module's 1 MiB ceiling, and the content-addressed blob tier arrives with the first kind that
-- needs it.
--
-- NUMBERING: this migration held 0026 provisionally on `team/kimi-artifact-store`; the completion
-- store landed first and took 0026, so this one renumbered to 0027 at its final rebase, exactly as
-- the provisional note said the second lane would. It advances schema_meta from 26.
--
-- The `events` rebuild carries every kind main has after 0026 PLUS 'artifact.recorded' -- the #50
-- trap is a kind list copied from the wrong parent. This list was regenerated against 0026's list
-- (which added spend.measured, mission.impact_classified, and mission.completed after 0023), and a
-- test diffs the two lists so a drifted copy cannot pass.

DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v26;

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
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v26
ORDER BY sequence;

DROP TABLE events_v26;

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

-- One Artifact: its canonical bytes, the identity those bytes hash to, and its content, inline.
--
-- The columns past `canonical_bytes` are PROJECTIONS FOR QUERYING ONLY: every claim is re-derived
-- from the bytes on every verified read and at startup, and a row that disagrees with its own
-- bytes is invalid state, not a different record. `creation_event_seq` is the one column with no
-- counterpart in the canonical bytes: it is copied from the record's own `artifact.recorded` Event
-- at insert and re-derived against that Event at every read and at startup -- the
-- `evaluations.mission_id` discipline, applied to a cursor.
CREATE TABLE artifacts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL CHECK (digest_canonicalizer = 'beacon.artifact.v1'),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 19 AND 4096),
    -- v1 is inline-only (R16/A7): the module's 1 MiB content ceiling is enforced here as well as
    -- in the codec, so the two cannot drift apart in silence.
    content_inline BLOB NOT NULL CHECK (length(content_inline) <= 1048576),
    kind TEXT NOT NULL CHECK (kind = 'outcome_document'),
    media_type TEXT NOT NULL CHECK (media_type = 'text/markdown; charset=utf-8'),
    content_bytes INTEGER NOT NULL CHECK (content_bytes BETWEEN 0 AND 1048576),
    accountable_resident_id TEXT NOT NULL,
    accountable_configuration_id TEXT NOT NULL,
    -- Absence is a value: `none` carries no Project or Mission, and the two identity columns are
    -- present exactly when the context is a Mission. A zero-length string is not a second
    -- spelling of absence.
    context_kind TEXT NOT NULL CHECK (context_kind IN ('none', 'mission')),
    context_project_id TEXT,
    context_mission_id TEXT,
    has_predecessor INTEGER NOT NULL CHECK (has_predecessor IN (0, 1)),
    supersedes_digest TEXT,
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0),
    creation_event_seq INTEGER NOT NULL CHECK (creation_event_seq >= 1),
    -- Table constraints follow the columns (SQLite admits no column after a table constraint).
    CHECK (
        (context_kind = 'none'
            AND context_project_id IS NULL AND context_mission_id IS NULL)
        OR (context_kind = 'mission'
            AND context_project_id IS NOT NULL AND context_mission_id IS NOT NULL)
    ),
    CHECK (
        (has_predecessor = 0 AND supersedes_digest IS NULL)
        OR (has_predecessor = 1 AND length(supersedes_digest) = 64
            AND supersedes_digest NOT GLOB '*[^0-9a-f]*')
    ),
    UNIQUE (workspace_id, digest_value),
    -- The accountable Resident always exists in this Workspace. For a Mission-scoped Artifact the
    -- Configuration is the one that Mission's admission roster pinned (the composite FK below);
    -- for a Workspace-scoped one it is a link in the Resident's configuration chain -- *a*
    -- configuration of that Resident, with no admission to name (opus's E1).
    FOREIGN KEY (workspace_id, accountable_resident_id)
        REFERENCES agents(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, accountable_resident_id, accountable_configuration_id)
        REFERENCES resident_configuration_decisions(workspace_id, resident_id, id)
        ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, context_mission_id)
        REFERENCES missions(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, context_mission_id, accountable_resident_id,
                 accountable_configuration_id)
        REFERENCES mission_admission_members(
            workspace_id, mission_id, resident_id, configuration_id
        ) ON DELETE RESTRICT
) STRICT;

-- Supersession is a relation, not a nullable column. "This Artifact has no predecessor" is the
-- ABSENCE OF A ROW, and the canonical bytes say `supersedes = none`, so reconciliation can check
-- the two agree. Each Artifact cites at most one predecessor (primary key) and is superseded at
-- most once (unique constraint), so the graph is a set of disjoint paths and "the current tail"
-- always has an answer. The `ruling_supersessions` shape, minus the Proposal scope: Artifact
-- chains are independent lineages, and several may exist under one Mission or none.
CREATE TABLE artifact_supersessions (
    workspace_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    supersedes_digest TEXT NOT NULL,
    PRIMARY KEY (workspace_id, artifact_digest),
    UNIQUE (workspace_id, supersedes_digest),
    CHECK (artifact_digest <> supersedes_digest),
    FOREIGN KEY (workspace_id, artifact_digest)
        REFERENCES artifacts(workspace_id, digest_value) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, supersedes_digest)
        REFERENCES artifacts(workspace_id, digest_value) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER artifacts_no_update
BEFORE UPDATE ON artifacts
BEGIN
    SELECT RAISE(ABORT, 'Artifacts are immutable');
END;

CREATE TRIGGER artifacts_no_delete
BEFORE DELETE ON artifacts
BEGIN
    SELECT RAISE(ABORT, 'Artifacts are immutable');
END;

CREATE TRIGGER artifact_supersessions_no_update
BEFORE UPDATE ON artifact_supersessions
BEGIN
    SELECT RAISE(ABORT, 'Artifact supersessions are immutable');
END;

CREATE TRIGGER artifact_supersessions_no_delete
BEFORE DELETE ON artifact_supersessions
BEGIN
    SELECT RAISE(ABORT, 'Artifact supersessions are immutable');
END;

UPDATE schema_meta SET version = 27 WHERE singleton = 1 AND version = 26;
