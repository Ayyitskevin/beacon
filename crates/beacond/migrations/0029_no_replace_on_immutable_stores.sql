-- Immutability that `INSERT OR REPLACE` cannot walk past (sol1's cold read #13 and pre-read; R20.4).
--
-- Every immutable table on `main` declares it with a `<T>_no_update` / `<T>_no_delete` pair, and
-- SQLite's REPLACE conflict resolution deletes the row it displaces WITHOUT firing that delete
-- trigger unless `PRAGMA recursive_triggers` is on -- which nothing set. A plain
-- `INSERT OR REPLACE` at a stored measured-spend digest replaced the row, projections and all, and
-- returned `Ok(1)`. Measured on SQLite 3.45.1 (sol1's pre-read: error 1811 on the fixed side):
--   recursive_triggers OFF, REPLACE at the primary key ...... replaced (rowid 1 -> 2)
--   recursive_triggers ON,  REPLACE at the primary key ...... refused by the delete trigger
--   recursive_triggers ON,  REPLACE at a UNIQUE key ......... refused by the delete trigger
--   the guard below, recursive_triggers OFF ................. refused; a first insert passes
--   INSERT OR IGNORE at a stored key ........................ was Ok(0), a silent non-refusal; refused
--   ON CONFLICT DO UPDATE ................................... was refused by BEFORE UPDATE; refused at the insert
--
-- Two halves. The pragma at the one place a store connection is opened (`open_store_connection`,
-- lib.rs) is the primary fix and is connection state; this file is the schema half, for any
-- connection that did not come through it -- a shell, a backup tool, a test. Each guard refuses an
-- INSERT whose PRIMARY KEY or any UNIQUE key is already stored: exactly the rows REPLACE would
-- have deleted. A first insert finds nothing and passes; a partial unique index conflicts only
-- between rows its predicate admits, so those branches carry the predicate for both rows; a
-- rowid-alias key compares against `NEW.sequence`, which is -1 when the insert named none.
--
-- GENERATED, not hand-written. The block below is what
--   cargo test -p beacond --lib -- print_the_no_conflicting_insert_triggers --ignored --nocapture
-- prints for a store at 0028: every paired-trigger table (thirty at this head), every key read
-- from `table_info` / `index_list` and every partial predicate from the index's own text, the
-- message copied from the table's delete trigger plus one fixed tail (`IDENTITY_ALREADY_STORED`,
-- lib.rs) that the writers which classify a lost race read back. The test
-- `migration_0029_is_the_generated_insert_guard_for_every_paired_trigger_table` fails if this
-- file and the generator disagree. Edit the generator and regenerate; never the block.
--
-- PROVISIONAL NUMBER. `main` is at 0028 and the completion-store P0 lane wants a number in this
-- window too; whichever lands second renumbers at its final rebase (this file, the `schema_meta`
-- guard at the bottom, `MIGRATION_V29`, `SCHEMA_VERSION` and the generator's parent version move
-- together). The closed `events` table is untouched.
--
-- A store rolled back below 0029 by hand must drop these guards too: startup compares a store's
-- schema with its declared version before it migrates, and a 27 that still carries them is not a 27.

CREATE TRIGGER artifact_supersessions_no_conflicting_insert
BEFORE INSERT ON artifact_supersessions
WHEN EXISTS (
    SELECT 1 FROM artifact_supersessions AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.artifact_digest = NEW.artifact_digest)
       OR (existing.workspace_id = NEW.workspace_id AND existing.supersedes_digest = NEW.supersedes_digest)
)
BEGIN
    SELECT RAISE(ABORT, 'Artifact supersessions are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER artifacts_no_conflicting_insert
BEFORE INSERT ON artifacts
WHEN EXISTS (
    SELECT 1 FROM artifacts AS existing
    WHERE (existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.digest_value = NEW.digest_value)
)
BEGIN
    SELECT RAISE(ABORT, 'Artifacts are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER candidates_no_conflicting_insert
BEFORE INSERT ON candidates
WHEN EXISTS (
    SELECT 1 FROM candidates AS existing
    WHERE (existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.proposal_digest = NEW.proposal_digest)
)
BEGIN
    SELECT RAISE(ABORT, 'Candidates are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER dispatch_preparations_no_conflicting_insert
BEFORE INSERT ON dispatch_preparations
WHEN EXISTS (
    SELECT 1 FROM dispatch_preparations AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.run_id = NEW.run_id)
       OR (existing.workspace_id = NEW.workspace_id AND existing.ruling_digest = NEW.ruling_digest)
)
BEGIN
    SELECT RAISE(ABORT, 'Dispatch preparations are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER evaluation_cited_runs_no_conflicting_insert
BEFORE INSERT ON evaluation_cited_runs
WHEN EXISTS (
    SELECT 1 FROM evaluation_cited_runs AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.evaluation_digest = NEW.evaluation_digest AND existing.run_id = NEW.run_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Evaluation cited Runs are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER evaluation_findings_no_conflicting_insert
BEFORE INSERT ON evaluation_findings
WHEN EXISTS (
    SELECT 1 FROM evaluation_findings AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.evaluation_digest = NEW.evaluation_digest AND existing.ordinal = NEW.ordinal)
)
BEGIN
    SELECT RAISE(ABORT, 'Evaluation findings are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER evaluation_supersessions_no_conflicting_insert
BEFORE INSERT ON evaluation_supersessions
WHEN EXISTS (
    SELECT 1 FROM evaluation_supersessions AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.subject_digest = NEW.subject_digest AND existing.evaluation_digest = NEW.evaluation_digest)
       OR (existing.workspace_id = NEW.workspace_id AND existing.subject_digest = NEW.subject_digest AND existing.supersedes_evaluation_digest = NEW.supersedes_evaluation_digest)
)
BEGIN
    SELECT RAISE(ABORT, 'Evaluation supersessions are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER evaluations_no_conflicting_insert
BEFORE INSERT ON evaluations
WHEN EXISTS (
    SELECT 1 FROM evaluations AS existing
    WHERE (existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.subject_digest = NEW.subject_digest AND existing.digest_value = NEW.digest_value)
)
BEGIN
    SELECT RAISE(ABORT, 'Evaluations are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER local_model_activation_decisions_no_conflicting_insert
BEFORE INSERT ON local_model_activation_decisions
WHEN EXISTS (
    SELECT 1 FROM local_model_activation_decisions AS existing
    WHERE (existing.sequence = NEW.sequence)
       OR (existing.workspace_id = NEW.workspace_id AND existing.id = NEW.id)
       OR (existing.workspace_id = NEW.workspace_id AND existing.provider = NEW.provider AND existing.endpoint = NEW.endpoint AND existing.model_name = NEW.model_name AND existing.id = NEW.id)
       OR (existing.id = NEW.id)
)
BEGIN
    SELECT RAISE(ABORT, 'local model activation decisions are append-only; a row with this identity is already stored');
END;

CREATE TRIGGER measured_spend_no_conflicting_insert
BEFORE INSERT ON measured_spend
WHEN EXISTS (
    SELECT 1 FROM measured_spend AS existing
    WHERE (existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.mission_id = NEW.mission_id AND existing.origin = NEW.origin AND existing.version = NEW.version)
       OR (existing.workspace_id = NEW.workspace_id AND existing.spend_id = NEW.spend_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Measured spend records are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER mission_admission_members_no_conflicting_insert
BEFORE INSERT ON mission_admission_members
WHEN EXISTS (
    SELECT 1 FROM mission_admission_members AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.mission_id = NEW.mission_id AND existing.position = NEW.position)
       OR (existing.workspace_id = NEW.workspace_id AND existing.mission_id = NEW.mission_id AND existing.resident_id = NEW.resident_id AND existing.configuration_id = NEW.configuration_id)
       OR (existing.workspace_id = NEW.workspace_id AND existing.mission_id = NEW.mission_id AND existing.resident_id = NEW.resident_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Mission admission members are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER mission_completion_conditions_no_conflicting_insert
BEFORE INSERT ON mission_completion_conditions
WHEN EXISTS (
    SELECT 1 FROM mission_completion_conditions AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.completion_digest = NEW.completion_digest AND existing.condition_ordinal = NEW.condition_ordinal)
)
BEGIN
    SELECT RAISE(ABORT, 'Mission completion conditions are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER mission_completions_no_conflicting_insert
BEFORE INSERT ON mission_completions
WHEN EXISTS (
    SELECT 1 FROM mission_completions AS existing
    WHERE (existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.mission_id = NEW.mission_id)
       OR (existing.workspace_id = NEW.workspace_id AND existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.completion_id = NEW.completion_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Mission completions are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER mission_impact_classifications_no_conflicting_insert
BEFORE INSERT ON mission_impact_classifications
WHEN EXISTS (
    SELECT 1 FROM mission_impact_classifications AS existing
    WHERE (existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.mission_id = NEW.mission_id AND existing.version = NEW.version)
       OR (existing.workspace_id = NEW.workspace_id AND existing.classification_id = NEW.classification_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Mission impact classifications are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER mission_success_conditions_no_conflicting_insert
BEFORE INSERT ON mission_success_conditions
WHEN EXISTS (
    SELECT 1 FROM mission_success_conditions AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.mission_id = NEW.mission_id AND existing.ordinal = NEW.ordinal)
       OR (existing.workspace_id = NEW.workspace_id AND existing.mission_id = NEW.mission_id AND existing.text = NEW.text)
)
BEGIN
    SELECT RAISE(ABORT, 'Mission success conditions are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER missions_no_conflicting_insert
BEFORE INSERT ON missions
WHEN EXISTS (
    SELECT 1 FROM missions AS existing
    WHERE (existing.id = NEW.id)
       OR (existing.workspace_id = NEW.workspace_id AND existing.project_id = NEW.project_id AND existing.id = NEW.id)
       OR (existing.workspace_id = NEW.workspace_id AND existing.id = NEW.id AND existing.team_id = NEW.team_id)
       OR (existing.workspace_id = NEW.workspace_id AND existing.id = NEW.id)
)
BEGIN
    SELECT RAISE(ABORT, 'Missions are immutable at admission; a row with this identity is already stored');
END;

CREATE TRIGGER project_policy_revisions_no_conflicting_insert
BEFORE INSERT ON project_policy_revisions
WHEN EXISTS (
    SELECT 1 FROM project_policy_revisions AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.project_id = NEW.project_id AND existing.revision = NEW.revision)
       OR (existing.workspace_id = NEW.workspace_id AND existing.project_id = NEW.project_id AND existing.revision = NEW.revision AND existing.charter_revision = NEW.charter_revision)
)
BEGIN
    SELECT RAISE(ABORT, 'Project Policy revisions are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER project_repository_revisions_no_conflicting_insert
BEFORE INSERT ON project_repository_revisions
WHEN EXISTS (
    SELECT 1 FROM project_repository_revisions AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.project_id = NEW.project_id AND existing.revision = NEW.revision)
       OR (existing.workspace_id = NEW.workspace_id AND existing.project_id = NEW.project_id AND existing.revision = NEW.revision AND existing.charter_revision = NEW.charter_revision AND existing.base_revision = NEW.base_revision)
)
BEGIN
    SELECT RAISE(ABORT, 'Project Repository revisions are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER proposals_no_conflicting_insert
BEFORE INSERT ON proposals
WHEN EXISTS (
    SELECT 1 FROM proposals AS existing
    WHERE (existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.digest_value = NEW.digest_value)
)
BEGIN
    SELECT RAISE(ABORT, 'Proposals are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER receipts_no_conflicting_insert
BEFORE INSERT ON receipts
WHEN EXISTS (
    SELECT 1 FROM receipts AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.run_id = NEW.run_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Receipts are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER resident_configuration_bindings_no_conflicting_insert
BEFORE INSERT ON resident_configuration_bindings
WHEN EXISTS (
    SELECT 1 FROM resident_configuration_bindings AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.resident_id = NEW.resident_id AND existing.configuration_id = NEW.configuration_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Resident configuration bindings are append-only; a row with this identity is already stored');
END;

CREATE TRIGGER resident_configuration_capabilities_no_conflicting_insert
BEFORE INSERT ON resident_configuration_capabilities
WHEN EXISTS (
    SELECT 1 FROM resident_configuration_capabilities AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.resident_id = NEW.resident_id AND existing.configuration_id = NEW.configuration_id AND existing.capability = NEW.capability)
)
BEGIN
    SELECT RAISE(ABORT, 'Resident configuration capabilities are append-only; a row with this identity is already stored');
END;

CREATE TRIGGER resident_configuration_decisions_no_conflicting_insert
BEFORE INSERT ON resident_configuration_decisions
WHEN EXISTS (
    SELECT 1 FROM resident_configuration_decisions AS existing
    WHERE (existing.sequence = NEW.sequence)
       OR (existing.workspace_id = NEW.workspace_id AND existing.resident_id = NEW.resident_id AND existing.predecessor_id = NEW.predecessor_id AND (existing.predecessor_id IS NOT NULL) AND (NEW.predecessor_id IS NOT NULL))
       OR (existing.workspace_id = NEW.workspace_id AND existing.resident_id = NEW.resident_id AND (existing.predecessor_id IS NULL) AND (NEW.predecessor_id IS NULL))
       OR (existing.workspace_id = NEW.workspace_id AND existing.resident_id = NEW.resident_id AND existing.id = NEW.id)
       OR (existing.id = NEW.id)
)
BEGIN
    SELECT RAISE(ABORT, 'Resident configuration decisions are append-only; a row with this identity is already stored');
END;

CREATE TRIGGER resident_lifecycle_decisions_no_conflicting_insert
BEFORE INSERT ON resident_lifecycle_decisions
WHEN EXISTS (
    SELECT 1 FROM resident_lifecycle_decisions AS existing
    WHERE (existing.sequence = NEW.sequence)
       OR (existing.workspace_id = NEW.workspace_id AND existing.resident_id = NEW.resident_id AND existing.predecessor_id = NEW.predecessor_id AND (existing.predecessor_id IS NOT NULL) AND (NEW.predecessor_id IS NOT NULL))
       OR (existing.workspace_id = NEW.workspace_id AND existing.resident_id = NEW.resident_id AND (existing.predecessor_id IS NULL) AND (NEW.predecessor_id IS NULL))
       OR (existing.workspace_id = NEW.workspace_id AND existing.resident_id = NEW.resident_id AND existing.id = NEW.id)
       OR (existing.id = NEW.id)
)
BEGIN
    SELECT RAISE(ABORT, 'Resident lifecycle decisions are append-only; a row with this identity is already stored');
END;

CREATE TRIGGER ruling_candidates_no_conflicting_insert
BEFORE INSERT ON ruling_candidates
WHEN EXISTS (
    SELECT 1 FROM ruling_candidates AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.ruling_digest = NEW.ruling_digest)
)
BEGIN
    SELECT RAISE(ABORT, 'Ruling candidate bindings are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER ruling_supersessions_no_conflicting_insert
BEFORE INSERT ON ruling_supersessions
WHEN EXISTS (
    SELECT 1 FROM ruling_supersessions AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.proposal_digest = NEW.proposal_digest AND existing.ruling_digest = NEW.ruling_digest)
       OR (existing.workspace_id = NEW.workspace_id AND existing.proposal_digest = NEW.proposal_digest AND existing.supersedes_ruling_digest = NEW.supersedes_ruling_digest)
)
BEGIN
    SELECT RAISE(ABORT, 'Ruling supersessions are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER rulings_no_conflicting_insert
BEFORE INSERT ON rulings
WHEN EXISTS (
    SELECT 1 FROM rulings AS existing
    WHERE (existing.digest_value = NEW.digest_value)
       OR (existing.workspace_id = NEW.workspace_id AND existing.proposal_digest = NEW.proposal_digest AND existing.digest_value = NEW.digest_value)
)
BEGIN
    SELECT RAISE(ABORT, 'Rulings are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER sample_project_installations_no_conflicting_insert
BEFORE INSERT ON sample_project_installations
WHEN EXISTS (
    SELECT 1 FROM sample_project_installations AS existing
    WHERE (existing.workspace_id = NEW.workspace_id)
       OR (existing.project_id = NEW.project_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Sample Project installations are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER team_formation_members_no_conflicting_insert
BEFORE INSERT ON team_formation_members
WHEN EXISTS (
    SELECT 1 FROM team_formation_members AS existing
    WHERE (existing.workspace_id = NEW.workspace_id AND existing.team_id = NEW.team_id AND existing.position = NEW.position)
       OR (existing.workspace_id = NEW.workspace_id AND existing.team_id = NEW.team_id AND existing.resident_id = NEW.resident_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Team formation members are immutable; a row with this identity is already stored');
END;

CREATE TRIGGER teams_no_conflicting_insert
BEFORE INSERT ON teams
WHEN EXISTS (
    SELECT 1 FROM teams AS existing
    WHERE (existing.id = NEW.id)
       OR (existing.workspace_id = NEW.workspace_id AND existing.project_id = NEW.project_id AND existing.id = NEW.id)
       OR (existing.workspace_id = NEW.workspace_id AND existing.id = NEW.id)
)
BEGIN
    SELECT RAISE(ABORT, 'Teams are immutable; a row with this identity is already stored');
END;

UPDATE schema_meta SET version = 29 WHERE singleton = 1 AND version = 28;
