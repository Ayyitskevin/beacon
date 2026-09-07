//! Executable absence witnesses for two Alpha acceptance criteria.
//!
//! Both criteria are satisfied today BY CONSTRUCTION and, until these witnesses, by nothing else:
//! no test turned red if the absence stopped being true (DP-alpha-criteria-census-opus2.md,
//! bullets 41 and 30). The shape is `local_transport`'s own witness — compute the surface, assert
//! the forbidden thing is not in it, and keep a control proving the predicate would notice.
//!
//! # Scope, in words, because the names alone cannot carry it
//!
//! **Every workspace member, production code only.** The surface is EVERY member listed in the
//! workspace manifest — read from it, never typed here — so a crate added to the workspace is
//! inside the witness the day it is added. A41.1 widened this after grok's read of #131: the first
//! version walked `crates/beacond/src` alone, so a sender in the desktop crate stayed green while
//! bullet 41 speaks about the product.
//!
//! "Production code only" applies to the pause-timer witness, and only there: a `thread::sleep` in
//! a `#[cfg(test)] mod` is a legitimate timing test, not a scheduler, and flagging it would make
//! the witness fail the first time someone wrote one. The sending witnesses do NOT skip test
//! modules — a test that reaches the network is exactly as interesting as production code that
//! does.

use std::fs;
use std::path::PathBuf;

/// The workspace root: this crate's manifest directory, two levels up (`crates/beacond`).
fn workspace_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("the crate sits two levels under the workspace root")
        .to_path_buf();
    assert!(
        root.join("Cargo.toml").is_file(),
        "no workspace manifest at {}",
        root.display()
    );
    root
}

/// Every workspace member, read from the workspace manifest rather than named here.
///
/// A crate added to the workspace joins these witnesses without anyone remembering to add it —
/// which is the property a hand-typed list cannot have, and the reason the first version of this
/// file had a hole.
fn workspace_members() -> Vec<String> {
    let manifest = fs::read_to_string(workspace_root().join("Cargo.toml"))
        .expect("the workspace manifest is readable");
    let members = manifest
        .split("members = [")
        .nth(1)
        .expect("the workspace manifest lists its members")
        .split(']')
        .next()
        .expect("the members list closes");
    let members: Vec<String> = members
        .split(',')
        .map(|entry| entry.trim().trim_matches('"').to_owned())
        .filter(|entry| !entry.is_empty())
        .collect();
    assert!(
        members.len() >= 2,
        "the workspace lists {} members; this is not reading the manifest",
        members.len()
    );
    members
}

/// Every Rust source file under every workspace member's `src`, as `(member/relative path, text)`.
///
/// Shared by both criteria: they ask different questions of one enumeration, so the two cannot
/// drift apart. `tests/` trees are deliberately outside it — this file is one of them.
fn workspace_sources() -> Vec<(String, String)> {
    let root = workspace_root();
    let mut files = Vec::new();
    for member in workspace_members() {
        let source_root = root.join(&member).join("src");
        assert!(
            source_root.is_dir(),
            "workspace member {member} has no src directory"
        );
        let mut pending = vec![source_root.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).expect("a readable member src directory") {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    let relative = path
                        .strip_prefix(&source_root)
                        .expect("every file is under the member's src")
                        .to_string_lossy()
                        .into_owned();
                    files.push((
                        format!("{member}/{relative}"),
                        fs::read_to_string(&path).expect("readable source"),
                    ));
                }
            }
        }
    }
    assert!(
        files.len() > 30,
        "the inventory found {} files; it is not reading the workspace",
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
const NETWORK_CAPABLE_MODULES: &[&str] = &[
    "crates/beacond/resource_discovery.rs",
    "crates/beacond/local_model_activation.rs",
];

/// Client crates whose name in a file means that file can send.
const CLIENT_CRATES: &[&str] = &["ureq", "reqwest", "hyper", "curl", "isahc", "attohttpc"];

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

/// Every manifest that can add a dependency: the workspace root and each member.
fn manifests() -> Vec<(String, String)> {
    let root = workspace_root();
    let mut all = vec![(
        "Cargo.toml".to_owned(),
        fs::read_to_string(root.join("Cargo.toml")).expect("the workspace manifest is readable"),
    )];
    for member in workspace_members() {
        all.push((
            format!("{member}/Cargo.toml"),
            fs::read_to_string(root.join(&member).join("Cargo.toml"))
                .expect("a member manifest is readable"),
        ));
    }
    all
}

/// Names a client crate has been RE-EXPORTED or ALIASED under, anywhere in the workspace.
///
/// grok's finding on #131: `pub use ureq as client;` in an allowed module, then `client::get(..)`
/// in any other, and the second file never mentions a marker. Resolving the alias closes it — the
/// declaration is found in one pass and its name becomes a marker for the next.
fn client_aliases(inventory: &[(String, String)]) -> Vec<String> {
    let mut aliases = Vec::new();
    for (_, text) in inventory {
        for line in text.lines().map(str::trim) {
            let Some(rest) = line
                .strip_prefix("pub use ")
                .or_else(|| line.strip_prefix("use "))
            else {
                continue;
            };
            let Some(crate_name) = CLIENT_CRATES.iter().find(|name| rest.starts_with(**name))
            else {
                continue;
            };
            // `use ureq as client;` and `pub use ureq::Agent as Client;` both rename it.
            if let Some((_, alias)) = rest.rsplit_once(" as ") {
                let alias = alias.trim_end_matches(&[';', ','][..]).trim();
                if !alias.is_empty() && alias != *crate_name {
                    aliases.push(alias.to_owned());
                }
            }
        }
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

/// Which files can send, and by which name — markers plus any resolved alias.
fn network_users(inventory: &[(String, String)]) -> Vec<(String, String)> {
    let aliases = client_aliases(inventory);
    let mut found = Vec::new();
    for (path, text) in inventory {
        for symbol in OUTBOUND_SYMBOLS {
            if text.contains(symbol) {
                found.push((path.clone(), (*symbol).to_owned()));
            }
        }
        for alias in &aliases {
            // The declaration site already matched on the crate's own name; this catches the USE
            // sites, which mention only the alias.
            if text.contains(&format!("{alias}::")) {
                found.push((path.clone(), alias.clone()));
            }
        }
    }
    found
}

#[test]
fn across_every_workspace_member_only_the_local_model_modules_can_reach_the_network() {
    let inventory = workspace_sources();
    let strays: Vec<(String, String)> = network_users(&inventory)
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
fn across_every_workspace_member_no_manifest_admits_a_second_client_or_a_reporter() {
    let manifests = manifests();
    let mut present: Vec<(String, &str)> = Vec::new();
    for (path, text) in &manifests {
        for name in FORBIDDEN_DEPENDENCIES {
            if text.lines().any(|line| line.trim_start().starts_with(name)) {
                present.push((path.clone(), *name));
            }
        }
    }
    assert!(
        present.is_empty(),
        "A41.ONE_CLIENT_NO_REPORTER -- a manifest carries a second client or a reporter: \
         {present:?}"
    );
    assert!(
        manifests.iter().any(|(_, text)| text
            .lines()
            .any(|line| line.trim_start().starts_with("ureq"))),
        "A41.ONE_CLIENT_NO_REPORTER -- `ureq` left every manifest; this witness now guards nothing"
    );
}

/// The control: the predicate notices a stray, exercised against a synthetic inventory.
///
/// Mutating the real crate cannot demonstrate this inside the suite, so the predicate is run
/// against a file listing that is not the crate's — the same device `local_transport`'s witness
/// uses for its own vocabulary.
#[test]
fn the_network_inventory_would_notice_a_stray_client_or_an_alias() {
    // Two shapes in one synthetic listing: a client named outright in a module outside the
    // allow-list, and grok's finding -- a client RE-EXPORTED under another name in an allowed
    // module and then used, by that name only, somewhere else.
    let synthetic = vec![
        (
            "crates/beacond/resource_discovery.rs".to_owned(),
            "pub use ureq as client;".to_owned(),
        ),
        (
            "crates/beacond/telemetry.rs".to_owned(),
            "let stream = TcpStream::connect(host)?;".to_owned(),
        ),
        (
            "apps/desktop/src-tauri/lib.rs".to_owned(),
            "let response = client::get(url).call()?;".to_owned(),
        ),
    ];
    assert_eq!(
        client_aliases(&synthetic),
        vec!["client".to_owned()],
        "the re-export was not resolved to an alias"
    );
    let mut strays: Vec<String> = network_users(&synthetic)
        .into_iter()
        .filter(|(path, _)| !NETWORK_CAPABLE_MODULES.contains(&path.as_str()))
        .map(|(path, symbol)| format!("{path}:{symbol}"))
        .collect();
    strays.sort();
    assert_eq!(
        strays,
        vec![
            "apps/desktop/src-tauri/lib.rs:client".to_owned(),
            "crates/beacond/telemetry.rs:TcpStream::connect".to_owned(),
        ],
        "the predicate missed a stray client, an aliased use, or a sender in another member"
    );
}

// ---------------------------------------------------------------------------------------------
// Bullet 30 — "Elapsed time alone cannot pause work unless the Project Charter or Policy names a
// time gate."
//
// No time gate exists yet (census bullet 30), so the whole criterion rests on the safe half: work
// is paused by an Operator Command and by nothing else. That half is guarded here.
//
// Two assertions, because a scheduler could arrive two ways. It could write the pause state
// itself -- caught by the writer count -- or it could call the existing writer from a timer, which
// no textual inventory can see directly; what it CAN see is a timing primitive appearing in the
// file that owns the writer, which is where such a caller would have to live to reach a private
// service. Neither is a proof; together they are the tripwire the criterion had none of.
// ---------------------------------------------------------------------------------------------

/// The table that says whether work is paused. One writer, by design.
const PAUSE_STATE_TABLE: &str = "UPDATE installation_pause";

/// Primitives that could turn elapsed time into an action.
const TIMING_PRIMITIVES: &[&str] = &[
    "thread::sleep",
    "thread::spawn",
    "Instant::now",
    "SystemTime::now",
];

/// The file with every `#[cfg(test)] mod …` block removed — production code only.
///
/// grok's second finding on #131: a `thread::sleep` inside a test module in the file that owns the
/// pause writer would have tripped the timer witness. That is a legitimate timing test, not a
/// scheduler, and a witness that fails on one would be deleted the first time someone wrote it.
///
/// Brace-matched rather than cut to end-of-file, because `#[cfg(test)]` also sits on individual
/// items (`lib.rs` carries five, one of them a `pub(crate) fn` two thirds of the way up) and
/// cutting from the first would delete most of a production file. Braces inside line comments,
/// block comments, strings, raw strings and char literals do not count — this crate's sources are
/// full of all five.
fn without_test_modules(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut kept_from = 0_usize;
    for (offset, _) in text.match_indices("#[cfg(test)]") {
        if offset < kept_from {
            // Inside a module already removed.
            continue;
        }
        // Look ahead for the `mod` item this attribute decorates. Anything else -- a function, a
        // use -- is left in place; only whole modules are removed.
        let head = &text[offset..text.len().min(offset + 200)];
        let decorates_a_module = head
            .lines()
            .skip(1)
            .find(|line| !line.trim().is_empty())
            .is_some_and(|line| {
                let line = line.trim();
                line.starts_with("mod ") || line.starts_with("pub mod ")
            });
        if !decorates_a_module {
            continue;
        }
        let Some(close) = end_of_block(text, offset) else {
            continue;
        };
        out.push_str(&text[kept_from..offset]);
        kept_from = close;
    }
    out.push_str(&text[kept_from..]);
    out
}

/// The byte offset one past the `}` closing the block that opens after `from`.
///
/// Braces inside line comments, block comments, strings, raw strings and char literals do not
/// count — this crate's sources are full of all five, and a `{` in a doc comment would otherwise
/// end the scan in the wrong place.
fn end_of_block(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = from;
    let mut depth = 0_usize;
    let mut opened = false;
    while index < bytes.len() {
        let here = bytes[index];
        let next = bytes.get(index + 1).copied();
        match here {
            b'/' if next == Some(b'/') => {
                index += text[index..]
                    .find('\n')
                    .map_or(text.len() - index, |at| at + 1);
                continue;
            }
            b'/' if next == Some(b'*') => {
                index += text[index + 2..]
                    .find("*/")
                    .map_or(text.len() - index, |at| at + 4);
                continue;
            }
            b'r' if next == Some(b'"') || next == Some(b'#') => {
                let mut hashes = 0;
                let mut cursor = index + 1;
                while bytes.get(cursor) == Some(&b'#') {
                    hashes += 1;
                    cursor += 1;
                }
                if bytes.get(cursor) != Some(&b'"') {
                    index += 1;
                    continue;
                }
                let closing: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                cursor += 1;
                let at = text[cursor..].find(&closing)?;
                index = cursor + at + closing.len();
                continue;
            }
            b'"' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'"' {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
                index += 1;
                continue;
            }
            b'\'' => {
                // A char literal is short; a lifetime has no closing quote. Scan a bounded window
                // and give up if none is found, which leaves lifetimes alone.
                let limit = (index + 12).min(bytes.len());
                let mut cursor = index + 1;
                while cursor < limit {
                    if bytes[cursor] == b'\'' {
                        break;
                    }
                    cursor += if bytes[cursor] == b'\\' { 2 } else { 1 };
                }
                index = if cursor < limit {
                    cursor + 1
                } else {
                    index + 1
                };
                continue;
            }
            b'{' => {
                depth += 1;
                opened = true;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if opened && depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Every file that writes the pause state.
fn pause_writers(inventory: &[(String, String)]) -> Vec<String> {
    inventory
        .iter()
        .filter(|(_, text)| text.contains(PAUSE_STATE_TABLE))
        .map(|(path, _)| path.clone())
        .collect()
}

#[test]
fn in_production_code_only_an_operator_command_can_change_the_pause_state() {
    let inventory = workspace_sources();

    // One writer. A scheduler that paused work on its own would be a second.
    let writers = pause_writers(&inventory);
    assert_eq!(
        writers,
        vec!["crates/beacond/dispatch.rs".to_owned()],
        "A30.ONE_PAUSE_WRITER -- the pause state is written from somewhere new"
    );

    // ...and that writer's three entry points are the Operator's, named here so a fourth is a
    // failure rather than a surprise. `is_paused` reads and is not one of them.
    let dispatch = &inventory
        .iter()
        .find(|(path, _)| path == "crates/beacond/dispatch.rs")
        .expect("dispatch.rs is in the inventory")
        .1;
    let entry_points = [
        "pub fn pause(",
        "pub fn resume(",
        "pub fn resume_accepting_unresolved(",
    ];
    for entry in entry_points {
        assert!(
            dispatch.contains(entry),
            "A30.PAUSE_ENTRY_POINTS -- `{entry}` is gone; this witness no longer describes the              Operator's surface"
        );
    }

    // No timing primitive in the PRODUCTION half of the file that owns the writer: a timer that
    // reached the private pause service would have to live here, and this is what would notice it
    // arriving. Test modules are stripped first (A41.1, grok): a `thread::sleep` in one is a
    // timing test, not a scheduler.
    let production = without_test_modules(dispatch);

    // THE STRIPPER'S OWN SELF-CHECK, because a stripper that quietly removed everything would make
    // this witness pass forever. It must remove the test module and keep the production code.
    assert!(
        dispatch.contains("fn specimen("),
        "A30.STRIPPER_IS_HONEST -- `specimen` is no longer dispatch.rs's test helper; pick another"
    );
    assert!(
        !production.contains("fn specimen("),
        "A30.STRIPPER_IS_HONEST -- the test module survived stripping"
    );
    assert!(
        production.contains("fn verify_outcome_invariants"),
        "A30.STRIPPER_IS_HONEST -- stripping removed production code"
    );
    assert!(
        production.len() * 3 > dispatch.len(),
        "A30.STRIPPER_IS_HONEST -- stripping removed more than two thirds of the file"
    );

    let timers: Vec<&str> = TIMING_PRIMITIVES
        .iter()
        .copied()
        .filter(|primitive| production.contains(primitive))
        .collect();
    assert!(
        timers.is_empty(),
        "A30.NO_TIMER_BESIDE_THE_WRITER -- elapsed time gained a way to reach the pause state:          {timers:?}"
    );
}

/// The control, against a synthetic inventory: both halves of the predicate notice.
#[test]
fn the_pause_inventory_would_notice_a_scheduler() {
    let synthetic = vec![
        (
            "dispatch.rs".to_owned(),
            "UPDATE installation_pause SET paused".to_owned(),
        ),
        (
            "scheduler.rs".to_owned(),
            "UPDATE installation_pause SET paused = 1".to_owned(),
        ),
    ];
    assert_eq!(
        pause_writers(&synthetic),
        vec!["dispatch.rs".to_owned(), "scheduler.rs".to_owned()],
        "the predicate did not notice a second writer"
    );

    // A timer in PRODUCTION code beside the writer is caught...
    let timed = "fn tick() { std::thread::sleep(d); } UPDATE installation_pause SET paused";
    let timers: Vec<&str> = TIMING_PRIMITIVES
        .iter()
        .copied()
        .filter(|primitive| without_test_modules(timed).contains(primitive))
        .collect();
    assert_eq!(
        timers,
        vec!["thread::sleep"],
        "the predicate did not notice a timer beside the writer"
    );

    // ...and the same sleep inside a test module is NOT, which is the false positive A41.1 removed.
    let in_a_test =
        "fn live() {}\n#[cfg(test)]\nmod tests {\n    fn t() { std::thread::sleep(d); }\n}\n";
    let stripped = without_test_modules(in_a_test);
    assert!(
        stripped.contains("fn live()"),
        "the stripper removed production code around the test module"
    );
    assert!(
        !stripped.contains("thread::sleep"),
        "A30.TEST_MODULES_ARE_NOT_SCHEDULERS -- a sleep in a test module still trips the witness"
    );
}
