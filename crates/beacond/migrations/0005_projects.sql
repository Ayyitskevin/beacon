ALTER TABLE events RENAME TO events_v4;

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN (
        'workspace.created',
        'starter_organization.accepted',
        'starter_organization.discarded',
        'project.created'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v4
ORDER BY sequence;

DROP TABLE events_v4;

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 120),
    current_charter_revision INTEGER NOT NULL CHECK (current_charter_revision >= 1),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    UNIQUE (workspace_id, id)
) STRICT;

CREATE INDEX projects_by_workspace_and_created
ON projects (workspace_id, created_at_ms DESC, id);

CREATE TABLE project_charter_revisions (
    workspace_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    purpose TEXT NOT NULL CHECK (length(trim(purpose)) BETWEEN 1 AND 2000),
    desired_outcomes_json TEXT NOT NULL
        CHECK (json_valid(desired_outcomes_json) AND json_type(desired_outcomes_json) = 'array'),
    constraints_json TEXT NOT NULL
        CHECK (json_valid(constraints_json) AND json_type(constraints_json) = 'array'),
    prohibitions_json TEXT NOT NULL
        CHECK (json_valid(prohibitions_json) AND json_type(prohibitions_json) = 'array'),
    autonomy_profile TEXT NOT NULL CHECK (autonomy_profile IN ('observe', 'supervised')),
    budget_boundary TEXT NOT NULL CHECK (budget_boundary = 'no_spend'),
    external_effects TEXT NOT NULL CHECK (external_effects = 'prohibited'),
    author_kind TEXT NOT NULL CHECK (author_kind = 'owner'),
    authored_at_ms INTEGER NOT NULL CHECK (authored_at_ms >= 0),
    PRIMARY KEY (workspace_id, project_id, revision),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES projects(workspace_id, id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE project_creation_snapshots (
    project_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    initial_name TEXT NOT NULL CHECK (length(trim(initial_name)) BETWEEN 1 AND 120),
    initial_charter_revision INTEGER NOT NULL CHECK (initial_charter_revision = 1),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES projects(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, initial_charter_revision)
        REFERENCES project_charter_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT
) STRICT;

UPDATE schema_meta SET version = 5 WHERE singleton = 1 AND version = 4;
