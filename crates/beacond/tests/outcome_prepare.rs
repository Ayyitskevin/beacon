//! Outcome Document slice 3-A: the immutable manifest store and the candidate Artifact.

use std::fs;
use std::path::PathBuf;

use beacond::{StartupConfig, Supervisor};
use rusqlite::Connection;
use uuid::Uuid;

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-outcome-prepare-{label}-{}.sqlite3",
                Uuid::new_v4()
            )),
        }
    }

    fn connect(&self) -> Connection {
        Connection::open(&self.path).unwrap()
    }
}

impl Drop for TestStore {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

#[test]
fn the_ladder_produces_an_outcome_manifest_store() {
    let store = TestStore::new("ladder");
    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .unwrap();
    let connection = store.connect();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM outcome_manifests", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
