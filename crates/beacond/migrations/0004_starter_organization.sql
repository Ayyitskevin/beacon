ALTER TABLE events RENAME TO events_v3;

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN (
        'workspace.created',
        'starter_organization.accepted',
        'starter_organization.discarded'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v3
ORDER BY sequence;

DROP TABLE events_v3;

CREATE TABLE starter_organization_decisions (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE RESTRICT,
    proposal_version INTEGER NOT NULL CHECK (proposal_version = 1),
    decision TEXT NOT NULL CHECK (decision IN ('accepted', 'discarded')),
    decided_at_ms INTEGER NOT NULL CHECK (decided_at_ms >= 0)
) STRICT;

CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('resident', 'specialist')),
    admission_source TEXT NOT NULL CHECK (length(trim(admission_source)) BETWEEN 1 AND 64),
    role TEXT NOT NULL CHECK (length(trim(role)) BETWEEN 1 AND 80),
    display_name_state TEXT NOT NULL CHECK (display_name_state IN ('absent', 'set')),
    display_name TEXT NOT NULL CHECK (length(display_name) <= 80),
    lifecycle TEXT NOT NULL CHECK (lifecycle = 'inactive'),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    CHECK (
        (display_name_state = 'absent' AND display_name = '') OR
        (display_name_state = 'set' AND length(trim(display_name)) BETWEEN 1 AND 80)
    )
) STRICT;

CREATE UNIQUE INDEX one_starter_resident_per_role
ON agents (workspace_id, role)
WHERE admission_source = 'starter_organization';

CREATE TABLE starter_organization_admissions (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    role TEXT NOT NULL CHECK (role IN ('lead', 'builder', 'reviewer', 'researcher')),
    display_name_state TEXT NOT NULL CHECK (display_name_state IN ('absent', 'set')),
    display_name TEXT NOT NULL CHECK (length(display_name) <= 80),
    admitted_at_ms INTEGER NOT NULL CHECK (admitted_at_ms >= 0),
    PRIMARY KEY (workspace_id, agent_id),
    UNIQUE (workspace_id, role),
    CHECK (
        (display_name_state = 'absent' AND display_name = '') OR
        (display_name_state = 'set' AND length(trim(display_name)) BETWEEN 1 AND 80)
    )
) STRICT;

UPDATE schema_meta SET version = 4 WHERE singleton = 1 AND version = 3;
