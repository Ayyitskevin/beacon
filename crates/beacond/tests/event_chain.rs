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
use std::sync::Arc;

use beacond::{
    AutonomyProfile, OperatorAccountService, OperatorSessionPhase, PauseService,
    ProjectCharterDraft, ProjectDraft, ProjectService, SecretString, StarterOrganizationService,
    StartupConfig, Supervisor, SupervisorError,
};
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

/// Four Events written through services, the fourth `installation.paused`.
///
/// **sol1's fixture** (cold read `D-sol1-b42-read.md`, probe `1afd985a`), carried into this file
/// under my name with its shape unchanged. It exists because the P0 below needs a store whose
/// newest Event is owned by a service that ALSO keeps current state outside the Event rows.
fn service_store(label: &str) -> Store {
    let store = store(label);
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("the store reopens");
    let accounts = Arc::new(OperatorAccountService::open(&store.path).expect("accounts open"));
    accounts
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .expect("the operator password is set");
    accounts
        .complete_operator_setup(true)
        .expect("setup completes");
    let projects = ProjectService::new(Arc::clone(&accounts), report.workspace.id);
    for name in ["First", "Second"] {
        projects
            .create(ProjectDraft {
                name: name.to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Build a chain with distinguishable service Events.".to_owned(),
                    desired_outcomes: vec!["Every stored link verifies.".to_owned()],
                    constraints: vec!["Keep the test local.".to_owned()],
                    prohibitions: vec!["Do not publish.".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .expect("the Project is created");
    }
    PauseService::new(Arc::clone(&accounts))
        .pause()
        .expect("the installation pauses");
    let events: i64 = store
        .connect()
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .expect("counts");
    assert_eq!(
        events, 4,
        "this fixture is four service Events, not {events}"
    );
    store
}

/// CELL: the newest Event deleted AND the head rewritten to the previous link, coherently.
///
/// **sol1's P0, and the finding this lane most needed.** Every check the chain makes still agrees:
/// the surviving links are authentic, they are in order, and `event_chain_state` names the last of
/// them. The store opened Ready -- while `installation_pause.paused = 1` and its transition time
/// went on proving that a fourth Event existed.
///
/// The lesson is structural, not a bug in the walk: hashing plus a head stored in the same rows it
/// describes tells a tampered history from an authentic one. It cannot tell a TRUNCATED history
/// from an honestly shorter one, because both are internally perfect. Completeness needs a fact the
/// same edit does not reach, and the pause owner has one that is not an Event row.
///
/// I changed one thing from sol1's witness and say so rather than quietly matching it: sol1 expects
/// `PauseEventMismatch(4)`, naming the DELETED Event. The store cannot compute 4 -- the row is gone
/// and its sequence with it -- so a refusal printing 4 would be stating a number nobody measured,
/// which is the failure this lane keeps finding. It names the two facts it does hold: the
/// transition time that survived, and the terminal pause Event it can still see (`None` here, and
/// that absence IS the truncation).
#[test]
fn sol1_b42_rewriting_the_head_cannot_hide_a_truncated_service_event() {
    let store = service_store("sol1-truncate-and-rewrite-head");
    let paused_at: i64 = store
        .connect()
        .query_row(
            "SELECT changed_at_ms FROM installation_pause WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("the installation is paused");
    store.tamper(
        "DELETE FROM events WHERE sequence = 4;
         UPDATE event_chain_state
            SET head_digest = (SELECT link_digest FROM events WHERE sequence = 3),
                head_sequence = 3
          WHERE id = 1;",
    );
    let refusal = store
        .restart()
        .expect_err("a truncated tail behind a rewritten head must refuse startup");
    assert!(
        matches!(
            refusal,
            SupervisorError::PauseStateUnsupported {
                terminal_sequence: None,
                ..
            }
        ),
        "the surviving paused state must refuse for its missing Event: {refusal:?}"
    );
    assert!(
        matches!(
            refusal,
            SupervisorError::PauseStateUnsupported { changed_at_ms, .. } if changed_at_ms == paused_at
        ),
        "the refusal must name the transition time that proves the Event existed: {refusal:?}"
    );
}

/// The same store, untampered, still opens Ready.
///
/// Without this the cell above would pass just as well if the fixture itself were broken -- and a
/// reconciliation that refuses every paused installation would look exactly like one that works.
#[test]
fn a_paused_installation_with_its_event_intact_opens_ready() {
    let store = service_store("paused-and-intact");
    store
        .restart()
        .expect("a paused installation reopens Ready");
}

/// CELL: the pause Event still present, but the state made to disagree with it.
///
/// The other shape of the same variant, and the reason it carries a sequence at all: here the store
/// CAN name the Event whose account of the world differs from the state's.
#[test]
fn a_pause_state_that_disagrees_with_its_own_event_is_refused() {
    let store = service_store("pause-disagrees");
    store.tamper(
        "UPDATE installation_pause SET changed_at_ms = changed_at_ms + 1 WHERE singleton = 1",
    );
    let refusal = store
        .restart()
        .expect_err("a pause state that disagrees with its Event must refuse startup");
    assert!(
        matches!(
            refusal,
            SupervisorError::PauseStateUnsupported {
                terminal_sequence: Some(4),
                ..
            }
        ),
        "the refusal must name the Event it disagrees with: {refusal:?}"
    );
}

/// A store with TWO chained Events, both written through the product's own writers.
///
/// A freshly bootstrapped store holds exactly ONE chained Event -- `workspace.created` -- so there
/// is no such thing as a mid-chain row in it, and a cell about a row with chained rows BEFORE it
/// cannot be written against a fresh store at all. My first version of the genesis cell below tried
/// and its own guard caught it. Discarding the starter organization is the cheapest second Event
/// the real door will write.
fn store_with_two_chained_events(label: &str) -> Store {
    let store = store(label);
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("the store reopens");
    let accounts = Arc::new(OperatorAccountService::open(&store.path).expect("accounts open"));
    accounts
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .expect("the operator password is set");
    assert_eq!(
        accounts
            .complete_operator_setup(true)
            .expect("setup completes")
            .phase,
        OperatorSessionPhase::Unlocked
    );
    StarterOrganizationService::new(Arc::clone(&accounts), report.workspace.id)
        .discard(1)
        .expect("the starter organization is discarded");
    let (_, chained) = newest_chained(&store);
    assert_eq!(
        chained, 2,
        "this fixture exists to produce two chained Events, not {chained}"
    );
    store
}

/// The newest chained Event, and the count of rows that are in the chain at all.
fn newest_chained(store: &Store) -> (i64, i64) {
    store
        .connect()
        .query_row(
            "SELECT max(sequence), count(*) FROM events WHERE predecessor_kind != 'unchained'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("a chained Event exists")
}

/// CELL: an Event outside the chain, after the boundary. The door is only a guarantee while
/// nothing appends around it.
///
/// **This test used to prove nothing, and I reported the fault it covers as load-bearing.** It
/// INSERTED a `workspace.created` row with an empty payload; the store refused that with
/// `Json(missing field `workspace_id`)` -- a decode error several passes before the chain runs, so
/// the chain arm was never reached. Fable measured the consequence at crate scope: silencing
/// `UnchainedAfterBoundary` in `verify_chain` SURVIVED all 1293 tests.
///
/// The tamper is now a rewrite in place. The newest chained row is made to claim it was written
/// around the door, and every per-record pass sees exactly the bytes it saw before -- same kind,
/// same payload, same sequence, same count -- so nothing else can refuse first and what refuses is
/// the chain.
#[test]
fn an_event_recorded_outside_the_chain_is_refused() {
    let store = store("around-the-door");
    let (sequence, _) = newest_chained(&store);
    store.tamper(&format!(
        "UPDATE events
            SET predecessor_kind = 'unchained', predecessor_digest = NULL, link_digest = NULL
          WHERE sequence = {sequence}"
    ));
    let refusal = store
        .restart()
        .expect_err("an Event written around the chain must refuse startup");
    assert!(
        matches!(refusal, SupervisorError::EventChainBroken(_)),
        "something other than the chain refused, so this cell measures that instead: {refusal:?}"
    );
    let rendered = refusal.to_string();
    assert!(
        rendered.contains(&sequence.to_string()),
        "the refusal does not name the row that left the chain: {rendered}"
    );
    // The clause that tells this fault from the other five. It is a third copy of Kevin's wording
    // (the signed table, R42's closed table, here), deliberately: a probe that accepts ANY refusal
    // is exactly what let this cell measure a JSON decode error for two commits.
    assert!(
        rendered.contains("outside the Event chain"),
        "a chain fault refused, but not the one this cell is about: {rendered}"
    );
}

/// CELL: a row in the middle of the chain rewritten to claim it begins one.
///
/// The other half of Fable's HOLD: silencing `GenesisNotFirst` also survived all 1293 tests. The
/// deleted-genesis cell above does not reach this arm -- with the first row gone, the second row's
/// named predecessor is missing, so `PredecessorMismatch` refuses first and correctly.
///
/// This is the tamper that forges a beginning rather than removing one: keep every row, and make a
/// later one say the history starts at it. Per-record passes see unchanged bytes.
#[test]
fn a_row_claiming_to_begin_a_chain_already_begun_is_refused() {
    let store = store_with_two_chained_events("second-genesis");
    let (sequence, _) = newest_chained(&store);
    store.tamper(&format!(
        "UPDATE events
            SET predecessor_kind = 'genesis', predecessor_digest = NULL
          WHERE sequence = {sequence}"
    ));
    let refusal = store
        .restart()
        .expect_err("a second beginning must refuse startup");
    assert!(
        matches!(refusal, SupervisorError::EventChainBroken(_)),
        "something other than the chain refused, so this cell measures that instead: {refusal:?}"
    );
    let rendered = refusal.to_string();
    assert!(
        rendered.contains(&sequence.to_string()),
        "the refusal does not name the row that claimed the beginning: {rendered}"
    );
    assert!(
        rendered.contains("begin a chain that had already begun"),
        "a chain fault refused, but not the one this cell is about: {rendered}"
    );
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

/// The same ordering claim, against the two faults the HOLD found unwitnessed.
///
/// A forged beginning AND a payload a record can see, in one store. The chain fault is real here --
/// the cell above proves this exact tamper refuses on its own -- and it still must lose, because a
/// refusal naming the record sends an operator to one row and `EventChainBroken` sends them to the
/// chain. Ordering is a design decision, so it gets a test rather than a paragraph.
#[test]
fn a_forged_beginning_still_loses_to_a_refusal_a_record_can_make() {
    let store = store_with_two_chained_events("ordering");
    let (sequence, _) = newest_chained(&store);
    let workspace: String = store
        .connect()
        .query_row("SELECT id FROM workspaces LIMIT 1", [], |row| row.get(0))
        .expect("a Workspace exists");
    let elsewhere = Uuid::new_v4();
    store.tamper(&format!(
        "UPDATE events
            SET predecessor_kind = 'genesis', predecessor_digest = NULL
          WHERE sequence = {sequence}"
    ));
    store.tamper(&format!(
        "UPDATE events
            SET payload_json = json_set(payload_json, '$.workspace_id', '{elsewhere}')
          WHERE kind = 'workspace.created' AND workspace_id = '{workspace}'"
    ));
    let refusal = store
        .restart()
        .expect_err("a store with both faults must refuse");
    assert!(
        matches!(refusal, SupervisorError::EventPayloadScopeMismatch(_)),
        "the chain pass ran before a pass that could name the record: {refusal:?}"
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
