ALTER TABLE events RENAME TO events_v7;

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN (
        'workspace.created',
        'starter_organization.accepted',
        'starter_organization.discarded',
        'project.created',
        'project.charter_revised',
        'local_model.activated',
        'local_model.deactivated',
        'resident.activated',
        'resident.suspended',
        'resident.reactivated',
        'resident.retired'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v7
ORDER BY sequence;

DROP TABLE events_v7;

DROP INDEX one_starter_resident_per_role;
ALTER TABLE starter_organization_admissions
RENAME TO starter_organization_admissions_v7;
ALTER TABLE agents RENAME TO agents_v7;

CREATE TABLE agents (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN ('resident', 'specialist')),
    admission_source TEXT NOT NULL CHECK (length(trim(admission_source)) BETWEEN 1 AND 64),
    role TEXT NOT NULL CHECK (length(trim(role)) BETWEEN 1 AND 80),
    display_name_state TEXT NOT NULL CHECK (display_name_state IN ('absent', 'set')),
    display_name TEXT NOT NULL CHECK (length(display_name) <= 80),
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('inactive', 'active', 'suspended', 'retired')),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE (workspace_id, id),
    CHECK (
        (display_name_state = 'absent' AND display_name = '') OR
        (display_name_state = 'set' AND length(trim(display_name)) BETWEEN 1 AND 80)
    )
) STRICT;

INSERT INTO agents (
    id, workspace_id, kind, admission_source, role, display_name_state,
    display_name, lifecycle, created_at_ms, updated_at_ms
)
SELECT id, workspace_id, kind, admission_source, role, display_name_state,
       display_name, lifecycle, created_at_ms, updated_at_ms
FROM agents_v7;

CREATE UNIQUE INDEX one_starter_resident_per_role
ON agents (workspace_id, role)
WHERE admission_source = 'starter_organization';

CREATE TABLE starter_organization_admissions (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    agent_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('lead', 'builder', 'reviewer', 'researcher')),
    display_name_state TEXT NOT NULL CHECK (display_name_state IN ('absent', 'set')),
    display_name TEXT NOT NULL CHECK (length(display_name) <= 80),
    admitted_at_ms INTEGER NOT NULL CHECK (admitted_at_ms >= 0),
    PRIMARY KEY (workspace_id, agent_id),
    UNIQUE (workspace_id, role),
    FOREIGN KEY (workspace_id, agent_id)
        REFERENCES agents(workspace_id, id) ON DELETE RESTRICT,
    CHECK (
        (display_name_state = 'absent' AND display_name = '') OR
        (display_name_state = 'set' AND length(trim(display_name)) BETWEEN 1 AND 80)
    )
) STRICT;

INSERT INTO starter_organization_admissions (
    workspace_id, agent_id, role, display_name_state, display_name, admitted_at_ms
)
SELECT workspace_id, agent_id, role, display_name_state, display_name, admitted_at_ms
FROM starter_organization_admissions_v7;

DROP TABLE starter_organization_admissions_v7;
DROP TABLE agents_v7;

CREATE TABLE resident_lifecycle_decisions (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    resident_id TEXT NOT NULL,
    from_lifecycle TEXT NOT NULL CHECK (
        from_lifecycle IN ('inactive', 'active', 'suspended')
    ),
    to_lifecycle TEXT NOT NULL CHECK (
        to_lifecycle IN ('active', 'suspended', 'retired')
    ),
    predecessor_id TEXT,
    decided_by TEXT NOT NULL CHECK (decided_by = 'operator'),
    decided_at_ms INTEGER NOT NULL CHECK (decided_at_ms >= 0),
    UNIQUE (workspace_id, resident_id, id),
    FOREIGN KEY (workspace_id, resident_id)
        REFERENCES agents(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, resident_id, predecessor_id)
        REFERENCES resident_lifecycle_decisions(
            workspace_id, resident_id, id
        ) ON DELETE RESTRICT,
    CHECK (
        (
            predecessor_id IS NULL
            AND from_lifecycle = 'inactive'
            AND to_lifecycle IN ('active', 'retired')
        ) OR (
            predecessor_id IS NOT NULL
            AND (
                (from_lifecycle = 'active' AND to_lifecycle IN ('suspended', 'retired')) OR
                (from_lifecycle = 'suspended' AND to_lifecycle IN ('active', 'retired'))
            )
        )
    )
) STRICT;

CREATE INDEX resident_lifecycle_decisions_by_resident
ON resident_lifecycle_decisions (workspace_id, resident_id, sequence DESC);

CREATE UNIQUE INDEX one_initial_resident_lifecycle_decision
ON resident_lifecycle_decisions (workspace_id, resident_id)
WHERE predecessor_id IS NULL;

CREATE UNIQUE INDEX one_resident_lifecycle_successor
ON resident_lifecycle_decisions (workspace_id, resident_id, predecessor_id)
WHERE predecessor_id IS NOT NULL;

CREATE TRIGGER resident_lifecycle_decisions_no_update
BEFORE UPDATE ON resident_lifecycle_decisions
BEGIN
    SELECT RAISE(ABORT, 'Resident lifecycle decisions are append-only');
END;

CREATE TRIGGER resident_lifecycle_decisions_no_delete
BEFORE DELETE ON resident_lifecycle_decisions
BEGIN
    SELECT RAISE(ABORT, 'Resident lifecycle decisions are append-only');
END;

UPDATE schema_meta SET version = 8 WHERE singleton = 1 AND version = 7;
