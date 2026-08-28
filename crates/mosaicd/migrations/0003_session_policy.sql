ALTER TABLE operator_accounts
ADD COLUMN session_timeout_minutes INTEGER NOT NULL DEFAULT 30
CHECK (session_timeout_minutes BETWEEN 1 AND 1440);

ALTER TABLE security_events RENAME TO security_events_v2;

CREATE TABLE security_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL CHECK (kind IN (
        'operator.enrolled',
        'authentication.failed',
        'session.unlocked',
        'session.locked',
        'session.policy_changed',
        'recovery.failed',
        'recovery.used'
    )),
    detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO security_events (sequence, kind, detail_json, occurred_at_ms)
SELECT sequence, kind, detail_json, occurred_at_ms
FROM security_events_v2
ORDER BY sequence;

DROP TABLE security_events_v2;

UPDATE schema_meta SET version = 3 WHERE singleton = 1 AND version = 2;
