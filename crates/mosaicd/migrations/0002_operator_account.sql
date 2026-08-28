CREATE TABLE operator_accounts (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    role TEXT NOT NULL CHECK (role = 'owner'),
    password_hash TEXT NOT NULL CHECK (length(password_hash) BETWEEN 32 AND 512),
    recovery_key_hash BLOB NOT NULL CHECK (length(recovery_key_hash) = 32),
    password_failed_attempts INTEGER NOT NULL DEFAULT 0 CHECK (password_failed_attempts BETWEEN 0 AND 100),
    password_retry_after_ms INTEGER NOT NULL DEFAULT 0 CHECK (password_retry_after_ms >= 0),
    recovery_failed_attempts INTEGER NOT NULL DEFAULT 0 CHECK (recovery_failed_attempts BETWEEN 0 AND 100),
    recovery_retry_after_ms INTEGER NOT NULL DEFAULT 0 CHECK (recovery_retry_after_ms >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
) STRICT;

CREATE TABLE security_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL CHECK (kind IN (
        'operator.enrolled',
        'authentication.failed',
        'session.unlocked',
        'session.locked',
        'recovery.failed',
        'recovery.used'
    )),
    detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

UPDATE schema_meta SET version = 2 WHERE singleton = 1 AND version = 1;
