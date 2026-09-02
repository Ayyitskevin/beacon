-- Briefing reads recent Receipts for the whole Workspace, newest first, under a
-- bound. The shipped ordering index is
-- `runs_project_started(workspace_id, project_id, started_at_ms DESC)`, which
-- puts `project_id` between the Workspace and the order key: SQLite can satisfy
-- the WHERE from it but must sort every matching Run before LIMIT helps
-- (`USE TEMP B-TREE FOR ORDER BY`). A Query whose cost grows with the Workspace
-- is not bounded; it merely returns a bounded answer.
--
-- Additive only: no table is renamed, no row is copied, no trigger is dropped,
-- and no Receipt byte is rewritten.
CREATE INDEX runs_workspace_started
ON runs (workspace_id, started_at_ms DESC, id DESC);

UPDATE schema_meta SET version = 20 WHERE singleton = 1 AND version = 19;
