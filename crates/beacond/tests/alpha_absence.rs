//! Executable absence witnesses for two Alpha acceptance criteria.
//!
//! Both criteria are satisfied today BY CONSTRUCTION and, until this file, by nothing else: no test
//! turned red if the absence stopped being true (DP-alpha-criteria-census-opus2.md, bullets 41 and
//! 30). The shape is `local_transport`'s own witness — compute the surface, assert the forbidden
//! thing is not in it, and keep a control proving the predicate would notice if it were.
//!
//! The surface is COMPUTED, never typed: `source_inventory` walks the crate's `src` tree at test
//! time, anchored to `CARGO_MANIFEST_DIR`, so a module added tomorrow is inside the witness without
//! anyone remembering to add it. That is the property a hand-written list cannot have, and the
//! reason these criteria were unguarded in the first place.

use std::fs;
use std::path::{Path, PathBuf};

/// Every Rust source file under the crate's `src`, as `(relative path, text)`.
///
/// Shared by both witnesses: they ask different questions of the same surface, so there is one
/// enumeration and two assertions rather than two enumerations that could drift apart.
fn source_inventory() -> Vec<(String, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("the crate's src directory is readable") {
            let path = entry.expect("a readable directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(&root)
                    .expect("every file is under src")
                    .to_string_lossy()
                    .into_owned();
                files.push((
                    relative,
                    fs::read_to_string(&path).expect("readable source"),
                ));
            }
        }
    }
    assert!(
        files.len() > 30,
        "the inventory found {} files; it is not reading the crate",
        files.len()
    );
    files.sort();
    files
}

// ---------------------------------------------------------------------------------------------
// Bullet 41 — "Diagnostic collection defaults off and remains inspectable locally. A scrubbed
// Alpha feedback bundle is previewed and exported manually; Beacon sends nothing automatically."
//
// WHAT THIS WITNESS DOES AND DOES NOT CLAIM. It does not prove nothing is sent automatically --
// no textual inventory can. It proves the far narrower thing that makes that claim checkable: the
// ability to send anything at all is confined to the two local-model modules, and the only client
// in the dependency list is the one they use.
//
// My own census got this wrong and the measurement corrected it: it recorded bullet 41 as "true by
// construction -- no network call sites". There ARE network call sites. `ureq` is a dependency and
// `resource_discovery` calls out over it. What makes the criterion hold is that the endpoint is
// loopback-validated (`LocalModelEndpoint::new` refuses a non-loopback address) and that nothing
// else in the crate can reach the network at all. So that is what is asserted.
// ---------------------------------------------------------------------------------------------

/// The modules allowed to reach the network, and the only ones.
const NETWORK_CAPABLE_MODULES: &[&str] = &["resource_discovery.rs", "local_model_activation.rs"];

/// Symbols that give a module the ability to SEND — to open an outbound connection or use the
/// client that does.
///
/// Deliberately narrower than "anything in `std::net`", and the first run of this witness is why.
/// It flagged two modules that cannot send anything, and re-reading them is what fixed the
/// predicate rather than the crate:
///
///   * `local_transport.rs` speaks over a **UNIX domain socket** (`UnixListener`/`UnixStream`);
///     its only `std::net` reference is `std::net::Shutdown::Write`, an enum the unix-socket API
///     reuses. No network capability at all.
///   * `sandbox/probe.rs` binds a **loopback `TcpListener`** to find out whether the sandbox can
///     deny network access. That is inbound, local, and exists to prove the denial works.
///
/// So `std::net`, `SocketAddr` and `TcpListener` are not markers of sending. `ollama_agent` is
/// listed beside `ureq` because it is the crate's one agent constructor: a module that calls it is
/// reaching the network even though the client's own name never appears in the file.
const OUTBOUND_SYMBOLS: &[&str] = &["ureq", "ollama_agent", "TcpStream::connect", "UdpSocket"];

/// Clients and reporters whose presence in the manifest would mean a second way out.
const FORBIDDEN_DEPENDENCIES: &[&str] = &[
    "reqwest",
    "hyper",
    "curl",
    "surf",
    "isahc",
    "attohttpc",
    "sentry",
    "opentelemetry",
    "tracing-opentelemetry",
    "posthog",
    "segment",
    "datadog",
    "prometheus",
];

fn manifest() -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("the crate manifest is readable")
}

/// Which files use an outbound symbol, and which symbol.
fn network_users(inventory: &[(String, String)]) -> Vec<(String, &'static str)> {
    let mut found = Vec::new();
    for (path, text) in inventory {
        for symbol in OUTBOUND_SYMBOLS {
            if text.contains(symbol) {
                found.push((path.clone(), *symbol));
            }
        }
    }
    found
}

#[test]
fn only_the_local_model_modules_can_reach_the_network() {
    let inventory = source_inventory();
    let strays: Vec<(String, &str)> = network_users(&inventory)
        .into_iter()
        .filter(|(path, _)| !NETWORK_CAPABLE_MODULES.contains(&path.as_str()))
        .collect();
    assert!(
        strays.is_empty(),
        "A41.NETWORK_SURFACE_IS_CONFINED -- a module outside local-model discovery can reach the \
         network: {strays:?}"
    );

    // ...and the two that can are not doing it by accident: the witness is worthless if the
    // allow-list names modules that no longer use the network, because then it is allowing nothing
    // and would keep passing after the real client moved somewhere else.
    let users: Vec<String> = network_users(&inventory)
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    for module in NETWORK_CAPABLE_MODULES {
        assert!(
            users.iter().any(|path| path == module),
            "A41.ALLOWLIST_IS_LIVE -- {module} is allowed to reach the network and does not, so \
             this allow-list is no longer describing the crate"
        );
    }
}

#[test]
fn the_manifest_admits_one_outbound_client_and_no_reporter() {
    let manifest = manifest();
    let present: Vec<&str> = FORBIDDEN_DEPENDENCIES
        .iter()
        .copied()
        .filter(|name| {
            manifest
                .lines()
                .any(|line| line.trim_start().starts_with(name))
        })
        .collect();
    assert!(
        present.is_empty(),
        "A41.ONE_CLIENT_NO_REPORTER -- the manifest carries a second client or a reporter: \
         {present:?}"
    );
    assert!(
        manifest
            .lines()
            .any(|line| line.trim_start().starts_with("ureq")),
        "A41.ONE_CLIENT_NO_REPORTER -- `ureq` left the manifest; this witness now guards nothing"
    );
}

/// The control: the predicate notices a stray, exercised against a synthetic inventory.
///
/// Mutating the real crate cannot demonstrate this inside the suite, so the predicate is run
/// against a file listing that is not the crate's — the same device `local_transport`'s witness
/// uses for its own vocabulary.
#[test]
fn the_network_inventory_would_notice_a_stray_client() {
    let synthetic = vec![
        ("resource_discovery.rs".to_owned(), "ureq::Agent".to_owned()),
        (
            "telemetry.rs".to_owned(),
            "let stream = TcpStream::connect(host)?;".to_owned(),
        ),
    ];
    let strays: Vec<(String, &str)> = network_users(&synthetic)
        .into_iter()
        .filter(|(path, _)| !NETWORK_CAPABLE_MODULES.contains(&path.as_str()))
        .collect();
    assert_eq!(
        strays,
        vec![("telemetry.rs".to_owned(), "TcpStream::connect")],
        "the predicate did not notice a client outside the allow-list"
    );
}
