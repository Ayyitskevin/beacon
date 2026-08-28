ALTER TABLE events RENAME TO events_v6;

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
        'local_model.deactivated'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v6
ORDER BY sequence;

DROP TABLE events_v6;

CREATE TABLE local_model_activation_decisions (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    provider TEXT NOT NULL CHECK (provider = 'ollama'),
    endpoint TEXT NOT NULL CHECK (length(trim(endpoint)) BETWEEN 1 AND 128),
    model_name TEXT NOT NULL CHECK (length(trim(model_name)) BETWEEN 1 AND 256),
    model_digest TEXT NOT NULL CHECK (
        length(model_digest) = 64 AND model_digest NOT GLOB '*[^0-9a-f]*'
    ),
    inspection_digest TEXT NOT NULL CHECK (
        length(inspection_digest) = 64 AND inspection_digest NOT GLOB '*[^0-9a-f]*'
    ),
    provider_features_json TEXT NOT NULL CHECK (
        json_valid(provider_features_json)
        AND json_type(provider_features_json) = 'array'
        AND json_array_length(provider_features_json) BETWEEN 1 AND 5
    ),
    unrecognized_features_json TEXT NOT NULL CHECK (
        json_valid(unrecognized_features_json)
        AND json_type(unrecognized_features_json) = 'array'
        AND json_array_length(unrecognized_features_json) BETWEEN 0 AND 64
    ),
    approved_features_json TEXT NOT NULL CHECK (
        json_valid(approved_features_json)
        AND json_type(approved_features_json) = 'array'
        AND json_array_length(approved_features_json) BETWEEN 1 AND 5
    ),
    state TEXT NOT NULL CHECK (state IN ('active', 'inactive')),
    predecessor_id TEXT,
    decided_at_ms INTEGER NOT NULL CHECK (decided_at_ms >= 0),
    CHECK (state = 'active' OR predecessor_id IS NOT NULL),
    UNIQUE (workspace_id, provider, endpoint, model_name, id),
    FOREIGN KEY (workspace_id, provider, endpoint, model_name, predecessor_id)
        REFERENCES local_model_activation_decisions(
            workspace_id, provider, endpoint, model_name, id
        ) ON DELETE RESTRICT
) STRICT;

CREATE INDEX local_model_activation_decisions_by_resource
ON local_model_activation_decisions (
    workspace_id, provider, endpoint, model_name, sequence DESC
);

CREATE TRIGGER local_model_activation_decisions_no_update
BEFORE UPDATE ON local_model_activation_decisions
BEGIN
    SELECT RAISE(ABORT, 'local model activation decisions are append-only');
END;

CREATE TRIGGER local_model_activation_decisions_no_delete
BEFORE DELETE ON local_model_activation_decisions
BEGIN
    SELECT RAISE(ABORT, 'local model activation decisions are append-only');
END;

UPDATE schema_meta SET version = 7 WHERE singleton = 1 AND version = 6;
