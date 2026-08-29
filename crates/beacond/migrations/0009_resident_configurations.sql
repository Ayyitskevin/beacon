ALTER TABLE events RENAME TO events_v8;

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
        'resident.retired',
        'resident.configuration_set',
        'resident.configuration_cleared'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v8
ORDER BY sequence;

DROP TABLE events_v8;

CREATE UNIQUE INDEX local_model_activation_decisions_workspace_id_id
ON local_model_activation_decisions (workspace_id, id);

CREATE TABLE resident_configuration_decisions (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    resident_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('configured', 'cleared')),
    predecessor_id TEXT,
    decided_by TEXT NOT NULL CHECK (decided_by = 'operator'),
    decided_at_ms INTEGER NOT NULL CHECK (decided_at_ms >= 0),
    UNIQUE (workspace_id, resident_id, id),
    FOREIGN KEY (workspace_id, resident_id)
        REFERENCES agents(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, resident_id, predecessor_id)
        REFERENCES resident_configuration_decisions(
            workspace_id, resident_id, id
        ) ON DELETE RESTRICT
) STRICT;

CREATE INDEX resident_configuration_decisions_by_resident
ON resident_configuration_decisions (workspace_id, resident_id, sequence DESC);

CREATE UNIQUE INDEX one_initial_resident_configuration_decision
ON resident_configuration_decisions (workspace_id, resident_id)
WHERE predecessor_id IS NULL;

CREATE UNIQUE INDEX one_resident_configuration_successor
ON resident_configuration_decisions (workspace_id, resident_id, predecessor_id)
WHERE predecessor_id IS NOT NULL;

CREATE TABLE resident_configuration_bindings (
    workspace_id TEXT NOT NULL,
    resident_id TEXT NOT NULL,
    configuration_id TEXT NOT NULL,
    activation_id TEXT NOT NULL,
    budget_boundary TEXT NOT NULL CHECK (budget_boundary = 'no_spend'),
    PRIMARY KEY (workspace_id, resident_id, configuration_id),
    FOREIGN KEY (workspace_id, resident_id, configuration_id)
        REFERENCES resident_configuration_decisions(
            workspace_id, resident_id, id
        ) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, activation_id)
        REFERENCES local_model_activation_decisions(workspace_id, id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE resident_configuration_capabilities (
    workspace_id TEXT NOT NULL,
    resident_id TEXT NOT NULL,
    configuration_id TEXT NOT NULL,
    capability TEXT NOT NULL CHECK (capability IN (
        'work.create_mission',
        'work.form_team',
        'work.coordinate_mission',
        'work.assign_work',
        'artifact.produce',
        'artifact.evaluate',
        'evidence.develop'
    )),
    PRIMARY KEY (workspace_id, resident_id, configuration_id, capability),
    FOREIGN KEY (workspace_id, resident_id, configuration_id)
        REFERENCES resident_configuration_bindings(
            workspace_id, resident_id, configuration_id
        ) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER resident_configuration_bindings_require_configured
BEFORE INSERT ON resident_configuration_bindings
BEGIN
    SELECT CASE WHEN (
        SELECT state
        FROM resident_configuration_decisions
        WHERE workspace_id = NEW.workspace_id
          AND resident_id = NEW.resident_id
          AND id = NEW.configuration_id
    ) != 'configured'
    THEN RAISE(ABORT, 'Resident configuration binding requires configured state') END;
END;

CREATE TRIGGER resident_configuration_decisions_no_update
BEFORE UPDATE ON resident_configuration_decisions
BEGIN
    SELECT RAISE(ABORT, 'Resident configuration decisions are append-only');
END;

CREATE TRIGGER resident_configuration_decisions_no_delete
BEFORE DELETE ON resident_configuration_decisions
BEGIN
    SELECT RAISE(ABORT, 'Resident configuration decisions are append-only');
END;

CREATE TRIGGER resident_configuration_bindings_no_update
BEFORE UPDATE ON resident_configuration_bindings
BEGIN
    SELECT RAISE(ABORT, 'Resident configuration bindings are append-only');
END;

CREATE TRIGGER resident_configuration_bindings_no_delete
BEFORE DELETE ON resident_configuration_bindings
BEGIN
    SELECT RAISE(ABORT, 'Resident configuration bindings are append-only');
END;

CREATE TRIGGER resident_configuration_capabilities_no_update
BEFORE UPDATE ON resident_configuration_capabilities
BEGIN
    SELECT RAISE(ABORT, 'Resident configuration capabilities are append-only');
END;

CREATE TRIGGER resident_configuration_capabilities_no_delete
BEFORE DELETE ON resident_configuration_capabilities
BEGIN
    SELECT RAISE(ABORT, 'Resident configuration capabilities are append-only');
END;

UPDATE schema_meta SET version = 9 WHERE singleton = 1 AND version = 8;
