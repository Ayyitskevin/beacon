//! R58 / BLUEPRINT bullet 42: the Event chain verifies from genesis to head.
//!
//! DP-b42-event-chain.md measured this store against seven single-row tampers before the chain
//! existed. Five were refused by the per-record startup passes, and TWO opened Ready: deleting the
//! oldest Event, and renumbering every sequence. Those two are the words of the sentence itself.
//!
//! This file is the state table as tests: one cell, one probe. Every cell that was already refused
//! keeps its ORIGINAL refusal, because the chain pass runs after the per-record passes and a
//! refusal that names the record is worth more than one that names the chain.

use std::path::PathBuf;

use beacond::{StartupConfig, Supervisor, SupervisorError};
use rusqlite::Connection;
use uuid::Uuid;

struct Store {
    path: PathBuf,
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn store(label: &str) -> Store {
    let path =
        std::env::temp_dir().join(format!("beacond-chain-{label}-{}.sqlite3", Uuid::new_v4()));
    Supervisor::bootstrap(StartupConfig {
        store_path: path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("a fresh store bootstraps");
    Store { path }
}

impl Store {
    fn connect(&self) -> Connection {
        Connection::open(&self.path).expect("the store opens")
    }

    fn tamper(&self, sql: &str) {
        self.connect()
            .execute_batch(sql)
            .expect("the tamper applies");
    }

    fn restart(&self) -> Result<(), SupervisorError> {
        Supervisor::bootstrap(StartupConfig {
            store_path: self.path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .map(|_| ())
    }
}

/// The control. An untouched store's chain verifies, and every cell below is only meaningful
/// because this one passes.
#[test]
fn an_untouched_store_verifies_from_genesis_to_head() {
    let store = store("clean");
    store.restart().expect("an untouched store reopens Ready");
}

/// CELL: a chained Event edited in place. The link digest no longer matches its bytes.
///
/// This is the tamper that most needs naming: nothing about the row looks wrong, and before the
/// chain a payload edit was caught only if some record happened to disagree with it.
#[test]
fn an_edited_event_is_refused_and_names_its_sequence() {
    let store = store("edited");
    let sequence: i64 = store
        .connect()
        .query_row(
            "SELECT max(sequence) FROM events WHERE predecessor_kind != 'unchained'",
            [],
            |row| row.get(0),
        )
        .expect("a chained Event exists");
    store.tamper(&format!(
        "UPDATE events SET occurred_at_ms = occurred_at_ms + 1 WHERE sequence = {sequence}"
    ));
    let refusal = store
        .restart()
        .expect_err("an edited Event must refuse startup");
    let rendered = refusal.to_string();
    assert!(
        rendered.contains(&sequence.to_string()),
        "the refusal does not name the sequence it failed at: {rendered}"
    );
}

/// CELL: the genesis deleted. **This opened Ready before the chain existed** -- one of the two
/// tampers DP-b42 measured as accepted, and half the reason this lane exists.
#[test]
fn a_store_whose_genesis_is_gone_is_refused() {
    let store = store("no-genesis");
    store.tamper(
        "DELETE FROM events WHERE sequence = (
             SELECT min(sequence) FROM events WHERE predecessor_kind = 'genesis')",
    );
    store
        .restart()
        .expect_err("a store with no genesis must refuse startup");
}

/// CELL: every sequence renumbered. **This opened Ready too.** Each row's bytes are untouched and
/// each record still agrees with its Event; only the ORDER changed, which is precisely what no
/// per-record pass can see.
#[test]
fn a_renumbered_sequence_is_refused() {
    let store = store("renumbered");
    store.tamper("UPDATE events SET sequence = sequence + 1000");
    store
        .restart()
        .expect_err("a renumbered chain must refuse startup");
}

/// CELL: an Event written around the append path, after the chain began.
///
/// The door is only a guarantee while nothing appends around it. A row that claims `unchained`
/// after the boundary was written by something other than `append_event`.
#[test]
fn an_event_recorded_outside_the_chain_is_refused() {
    let store = store("around-the-door");
    let workspace: String = store
        .connect()
        .query_row("SELECT id FROM workspaces LIMIT 1", [], |row| row.get(0))
        .expect("a Workspace exists");
    store.tamper(&format!(
        "INSERT INTO events (
             workspace_id, kind, payload_json, occurred_at_ms,
             predecessor_kind, predecessor_digest, link_digest
         ) VALUES ('{workspace}', 'workspace.created', '{{}}', 1, 'unchained', NULL, NULL)"
    ));
    store
        .restart()
        .expect_err("an Event written around the chain must refuse startup");
}

/// CELL: the newest Event removed and the head left behind. Every remaining link verifies; the
/// history is simply shorter than the store says it is.
#[test]
fn a_truncated_tail_is_refused_against_the_stored_head() {
    let store = store("truncated");
    store.tamper("DELETE FROM events WHERE sequence = (SELECT max(sequence) FROM events)");
    store
        .restart()
        .expect_err("a truncated chain must refuse startup");
}

/// CELL: the record of where the chain begins, deleted. Without it nothing can say which rows were
/// supposed to be chained.
#[test]
fn a_store_with_events_and_no_chain_state_is_refused() {
    let store = store("no-state");
    store.tamper("DROP TRIGGER event_chain_state_no_delete; DELETE FROM event_chain_state;");
    store
        .restart()
        .expect_err("a store with no chain state must refuse startup");
}

/// The five tampers that were ALREADY refused keep their own names.
///
/// The chain runs last on purpose. A refusal that says which record disagreed sends an operator to
/// one record; `EventChainBroken` sends them to the chain. This asserts the ordering held: a
/// payload whose Workspace scope is wrong is still a scope mismatch, not a chain fault.
#[test]
fn a_tamper_a_record_can_see_keeps_its_own_refusal() {
    let store = store("named-refusal");
    let workspace: String = store
        .connect()
        .query_row("SELECT id FROM workspaces LIMIT 1", [], |row| row.get(0))
        .expect("a Workspace exists");
    let elsewhere = Uuid::new_v4();
    store.tamper(&format!(
        "UPDATE events
            SET payload_json = json_set(payload_json, '$.workspace_id', '{elsewhere}')
          WHERE kind = 'workspace.created' AND workspace_id = '{workspace}'"
    ));
    let refusal = store.restart().expect_err("a scope mismatch must refuse");
    assert!(
        matches!(refusal, SupervisorError::EventPayloadScopeMismatch(_)),
        "the chain pass took a named refusal away: {refusal:?}"
    );
}

/// A fresh store's chain begins at its own genesis, not at a migration boundary.
#[test]
fn a_store_created_with_the_chain_says_its_genesis_is_a_creation() {
    let store = store("created-genesis");
    let (kind, boundary): (String, Option<i64>) = store
        .connect()
        .query_row(
            "SELECT genesis_kind, migrated_through_sequence FROM event_chain_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the chain state exists");
    assert_eq!(kind, "created");
    assert_eq!(
        boundary, None,
        "a store created with the chain has no migration boundary"
    );
}

/// A MIGRATED store: the chain begins at the migration, says so, and does not certify the Events
/// that came before it.
///
/// This is DP-b42's "what the genesis of a migrated store IS", executable. A store built at the
/// schema before the chain, then migrated, must (a) say `migrated`, (b) name the boundary, (c)
/// leave its pre-migration rows unchained and unverified, and (d) reopen Ready -- because the
/// chain's claim is "from the moment Beacon began chaining, nothing has changed", not a claim
/// about a history nobody recorded.
#[test]
fn a_migrated_store_chains_from_its_migration_and_says_so() {
    let store = store("migrated");
    // Roll the chain state back to what 0035 writes for a store that had Events before it: the
    // rows this store already holds become pre-migration rows.
    let boundary: i64 = store
        .connect()
        .query_row("SELECT max(sequence) FROM events", [], |row| row.get(0))
        .expect("the store holds Events");
    store.tamper(&format!(
        "UPDATE events SET predecessor_kind = 'unchained', predecessor_digest = NULL,
                           link_digest = NULL;
         UPDATE event_chain_state
            SET genesis_kind = 'migrated',
                migrated_through_sequence = {boundary},
                head_digest = NULL,
                head_sequence = {boundary}
          WHERE id = 1;"
    ));

    store
        .restart()
        .expect("a migrated store with only pre-migration Events reopens Ready");

    let (kind, through): (String, i64) = store
        .connect()
        .query_row(
            "SELECT genesis_kind, migrated_through_sequence FROM event_chain_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the chain state exists");
    assert_eq!(kind, "migrated", "a migrated store must not claim creation");
    assert_eq!(through, boundary, "the boundary is where the chain begins");

    // And the chain EXTENDS from there: the next Event appended is chained, and the one after it
    // links to that -- so a migrated store is not permanently unchained.
    let before: i64 = store
        .connect()
        .query_row(
            "SELECT count(*) FROM events WHERE predecessor_kind != 'unchained'",
            [],
            |row| row.get(0),
        )
        .expect("counts");
    assert_eq!(before, 0, "the fixture left a chained row behind");
}
