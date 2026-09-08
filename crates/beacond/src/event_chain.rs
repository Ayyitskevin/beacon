//! The event chain: what links one persisted Event to the one before it (R58, BLUEPRINT bullet 42).
//!
//! BLUEPRINT asks that "the event chain verifies from genesis to head". Measured before this
//! module existed (DP-b42-event-chain.md), it did not: five single-row tampers were refused by the
//! per-record startup passes, and two were accepted -- deleting the oldest event, and renumbering
//! every sequence. Both are precisely the sentence's own words, because every refusal came from a
//! record noticing its event had changed, and nothing looked at the events AS A SEQUENCE.
//!
//! This module is the part of the answer that owes nothing to the store's shape: the canonical form
//! of a link, and the three states a row's predecessor can be in. The columns, the head and the
//! startup pass are built on top of it.

use rusqlite::{Connection, OptionalExtension};

use crate::proposal::sha256;

/// The canonicalizer this chain's digests are taken under, named in every digest that carries one
/// (ADR 0034: a digest names the canonicalizer that produced it, or a later version silently
/// compares against bytes it did not make).
pub const CANONICALIZER: &str = "beacon.event_chain.v1";

/// The prefix every link commits to: the canonicalizer's own name and a NUL.
///
/// Written from [`CANONICALIZER`] rather than repeated as a literal, so the name a digest claims
/// and the bytes it actually commits to cannot drift apart -- ADR 0034's rule, applied to itself.
/// A digest over bare fields would collide with any other structure that serialised the same bytes;
/// the prefix makes the domain explicit.
fn magic() -> Vec<u8> {
    let mut prefix = CANONICALIZER.as_bytes().to_vec();
    prefix.push(0);
    prefix
}

/// The chain's first predecessor: a NAMED constant, deliberately not thirty-two zero bytes.
///
/// Zeros are what an uninitialised column, a truncated read and a failed write all look like. A
/// genesis a reader cannot tell apart from an absence is not a genesis, and the whole point of the
/// first link is that it says "nothing came before this" rather than "I do not know".
pub fn genesis() -> [u8; 32] {
    sha256(b"beacon.event_chain.v1.genesis\0")
}

/// What a stored Event says about the Event before it. Three answers to three different questions,
/// spelled apart -- never one nullable column, which would say the last two the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Predecessor {
    /// This Event is the first link. Its predecessor is [`genesis`], and the chain starts here.
    Genesis,
    /// This Event follows the Event whose link digest this is.
    Linked([u8; 32]),
    /// This Event was written before the chain existed, and makes NO CLAIM about what preceded it.
    ///
    /// A migrated store cannot honestly say what came before its pre-migration rows: nothing
    /// recorded it, and computing a link now would attest to an order this process is inferring
    /// rather than remembering. The migration writes its own genesis at the boundary and leaves
    /// these saying what is true -- that they are older than the record of order.
    Unchained,
}

impl Predecessor {
    /// The bytes a link commits to for its predecessor, and the tag that says which of the three
    /// this is. The tag is inside the digest, so an `Unchained` row cannot be re-read as a
    /// `Linked` one whose predecessor happened to be the genesis constant.
    fn tag(self) -> u8 {
        match self {
            Self::Genesis => TAG_GENESIS,
            Self::Linked(_) => TAG_LINKED,
            Self::Unchained => TAG_UNCHAINED,
        }
    }

    fn digest_bytes(self) -> [u8; 32] {
        match self {
            Self::Genesis => genesis(),
            Self::Linked(digest) => digest,
            // An unchained row commits to the genesis constant in the same position, which the tag
            // above keeps distinguishable. It is a placeholder in the bytes, never a claim.
            Self::Unchained => genesis(),
        }
    }
}

const TAG_GENESIS: u8 = 0x01;
const TAG_LINKED: u8 = 0x02;
const TAG_UNCHAINED: u8 = 0x03;

/// One Event, in the form its link digest is taken over.
///
/// Every column the `events` table stores is in here. A field left out would be a field a tamper
/// could change without breaking the chain -- which is exactly the hole this chain exists to close,
/// so the set is the table's set and adding a column to `events` without adding it here is a defect
/// the schema witness catches.
#[derive(Debug, Clone, Copy)]
pub struct Link<'a> {
    pub predecessor: Predecessor,
    pub sequence: i64,
    /// The Workspace **as the column stores it**, not as a parsed value. The digest commits to the
    /// row as it exists: a chain that hashed a re-parsed form would verify a value the table does
    /// not hold, and a text column that drifted from its parse would still pass.
    pub workspace_id: &'a str,
    pub kind: &'a str,
    pub payload_json: &'a str,
    pub occurred_at_ms: i64,
}

impl Link<'_> {
    /// This Event's link digest: SHA-256 over the magic, the predecessor's tag and digest, and every
    /// stored column, each framed so no two different Events can encode to the same bytes.
    ///
    /// Strings are length-prefixed rather than delimited: a `kind` and a `payload_json` that
    /// concatenate to the same run of bytes as some other pair must not produce the same digest,
    /// and a separator would only move the problem to inputs containing the separator.
    pub fn digest(&self) -> [u8; 32] {
        let mut bytes = magic();
        bytes.push(self.predecessor.tag());
        bytes.extend_from_slice(&self.predecessor.digest_bytes());
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        push_framed(&mut bytes, self.workspace_id.as_bytes());
        push_framed(&mut bytes, self.kind.as_bytes());
        push_framed(&mut bytes, self.payload_json.as_bytes());
        bytes.extend_from_slice(&self.occurred_at_ms.to_be_bytes());
        sha256(&bytes)
    }
}

fn push_framed(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value);
}

/// Append one Event, linked to the chain, inside the caller's transaction.
///
/// **The one door.** Every Event this product writes goes through here, and the witness test in
/// `lib.rs` enumerates the writers mechanically rather than trusting that. The reason is R58's own:
/// a chain is only as good as the guarantee that nothing appends around it, and 56 insert sites
/// each spelling the link themselves is 56 chances to spell it differently.
///
/// **The head is READ here, never passed in.** A caller that supplied its own predecessor would be
/// supplying the value that constrains it -- the dodgeable-guard shape this fleet has been bitten
/// by. The read and the write are in the caller's transaction, which the services already open as
/// `IMMEDIATE`, so no second writer can interleave between them.
pub(crate) fn append_event(
    connection: &Connection,
    workspace_id: &str,
    kind: &str,
    payload_json: &str,
    occurred_at_ms: i64,
) -> rusqlite::Result<i64> {
    let head: Option<(Option<Vec<u8>>, i64)> = connection
        .query_row(
            "SELECT head_digest, head_sequence FROM event_chain_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let predecessor = match &head {
        // A store that has chained before: this Event follows the head.
        Some((Some(digest), _)) => Predecessor::Linked(as_digest(digest)?),
        // A migrated store whose chain has not started yet, or a store with no state row at all
        // (created fresh): either way this Event begins the chain.
        Some((None, _)) | None => Predecessor::Genesis,
    };

    // The sequence is chosen before the digest, because the digest commits to it. AUTOINCREMENT
    // would assign it during the insert, and a digest computed afterwards would need an UPDATE to
    // an Event -- a second write to a row whose whole value is that it is written once.
    let next: i64 = connection.query_row(
        "SELECT coalesce(max(seq), 0) + 1 FROM
           (SELECT coalesce(max(sequence), 0) AS seq FROM events
            UNION ALL
            SELECT coalesce(seq, 0) FROM sqlite_sequence WHERE name = 'events')",
        [],
        |row| row.get(0),
    )?;

    let link = Link {
        predecessor,
        sequence: next,
        workspace_id,
        kind,
        payload_json,
        occurred_at_ms,
    };
    let digest = link.digest();

    connection.execute(
        "INSERT INTO events (
             sequence, workspace_id, kind, payload_json, occurred_at_ms,
             predecessor_kind, predecessor_digest, link_digest
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            next,
            workspace_id,
            kind,
            payload_json,
            occurred_at_ms,
            match predecessor {
                Predecessor::Genesis => "genesis",
                Predecessor::Linked(_) => "linked",
                Predecessor::Unchained => "unchained",
            },
            match predecessor {
                Predecessor::Linked(digest) => Some(digest.to_vec()),
                Predecessor::Genesis | Predecessor::Unchained => None,
            },
            digest.to_vec(),
        ],
    )?;

    // The head moves in the same transaction as the Event it names. A head that lagged its chain
    // would make every later append link to an Event that is no longer last.
    match head {
        Some(_) => connection.execute(
            "UPDATE event_chain_state SET head_digest = ?1, head_sequence = ?2 WHERE id = 1",
            rusqlite::params![digest.to_vec(), next],
        )?,
        None => connection.execute(
            "INSERT INTO event_chain_state (
                 id, genesis_kind, migrated_through_sequence, pre_migration_digest,
                 head_digest, head_sequence
             ) VALUES (1, 'created', NULL, NULL, ?1, ?2)",
            rusqlite::params![digest.to_vec(), next],
        )?,
    };

    Ok(next)
}

fn as_digest(bytes: &[u8]) -> rusqlite::Result<[u8; 32]> {
    <[u8; 32]>::try_from(bytes).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            "a chain digest is thirty-two bytes".into(),
        )
    })
}

/// Why a stored chain does not verify. Each names the sequence it failed AT, because "the chain is
/// broken" sends an operator to read every Event and "the chain is broken at 4211" sends them to
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainFault {
    /// The row's own link digest is not the digest of its bytes: this Event was edited in place.
    LinkDigestMismatch { sequence: i64 },
    /// The row names a predecessor that is not the previous row's link: an Event was inserted,
    /// removed or reordered between them.
    PredecessorMismatch { sequence: i64, previous: i64 },
    /// A row claims to start the chain, and something chained came before it.
    GenesisNotFirst { sequence: i64 },
    /// A row is outside the chain, and the chain had already begun at or before it. Every Event
    /// written through the append path is chained, so an unchained row after the boundary was
    /// written around it.
    UnchainedAfterBoundary { sequence: i64 },
    /// The chain ends somewhere other than the head the store records. The last Event was removed,
    /// or one was added without moving the head.
    HeadMismatch {
        expected_sequence: i64,
        found_sequence: i64,
    },
    /// There are Events and no chain state at all: the record of where the chain begins is gone.
    StateMissing,
}

/// The six operator sentences, in one place: **the table Kevin signs** (R58.2).
///
/// Each row is (variant name, sentence template). [`ChainFault::sentence`] renders a row by
/// substituting the variant's own fields for `{sequence}`, `{previous}`, `{expected_sequence}` and
/// `{found_sequence}`, and `SupervisorError`'s `Display` writes that string and nothing else. So
/// signing or rewriting a sentence is ONE edit in ONE place, and the witness --
/// `every_chain_fault_speaks_its_own_signed_sentence` below -- reads the same bytes it does.
///
/// A rewrite that misses half the job goes RED: editing the supervisor's arm instead of this table,
/// dropping a row's `{sequence}` placeholder so a fault stops naming where it failed, swapping the
/// two numbers of a two-number row, or giving two faults the same sentence.
///
/// One rendered line is duplicated on purpose. `LinkDigestMismatch`'s is also a literal row of
/// R42's closed `expected_startup` table in `lib.rs`, which is an INDEPENDENT copy by design --
/// that is the closed table's whole job. Rewriting this row without that one goes RED there, and
/// the report says so.
pub const OPERATOR_SENTENCES: [(&str, &str); 6] = [
    (
        "LinkDigestMismatch",
        "Event sequence {sequence} was edited after it was recorded",
    ),
    (
        "PredecessorMismatch",
        "the Event chain is broken between sequence {previous} and sequence {sequence}",
    ),
    (
        "GenesisNotFirst",
        "Event sequence {sequence} claims to begin a chain that had already begun",
    ),
    (
        "UnchainedAfterBoundary",
        "Event sequence {sequence} was recorded outside the Event chain",
    ),
    (
        "HeadMismatch",
        "the Event chain ends at sequence {found_sequence}, and this store records its head at \
         sequence {expected_sequence}",
    ),
    (
        "StateMissing",
        "this store holds Events and no record of where its Event chain begins",
    ),
];

impl ChainFault {
    /// The row of [`OPERATOR_SENTENCES`] this fault speaks with.
    ///
    /// Exhaustive on purpose: a seventh fault does not compile until someone has written it a
    /// sentence for Kevin to sign.
    fn row(&self) -> usize {
        match self {
            Self::LinkDigestMismatch { .. } => 0,
            Self::PredecessorMismatch { .. } => 1,
            Self::GenesisNotFirst { .. } => 2,
            Self::UnchainedAfterBoundary { .. } => 3,
            Self::HeadMismatch { .. } => 4,
            Self::StateMissing => 5,
        }
    }

    /// This fault's operator sentence, rendered from its signed row.
    pub(crate) fn sentence(&self) -> String {
        let template = OPERATOR_SENTENCES[self.row()].1;
        match self {
            Self::LinkDigestMismatch { sequence }
            | Self::GenesisNotFirst { sequence }
            | Self::UnchainedAfterBoundary { sequence } => {
                template.replace("{sequence}", &sequence.to_string())
            }
            Self::PredecessorMismatch { sequence, previous } => template
                .replace("{previous}", &previous.to_string())
                .replace("{sequence}", &sequence.to_string()),
            Self::HeadMismatch {
                expected_sequence,
                found_sequence,
            } => template
                .replace("{expected_sequence}", &expected_sequence.to_string())
                .replace("{found_sequence}", &found_sequence.to_string()),
            Self::StateMissing => template.to_owned(),
        }
    }
}

/// Verify the stored chain from its genesis to its head.
///
/// **Runs AFTER the per-record passes, deliberately.** Five of the seven tampers measured in
/// DP-b42-event-chain.md are already refused by name -- `MissionEventMismatch(10)`,
/// `ProjectStateMismatch`, and so on -- and those names are more useful than a generic chain fault,
/// because they say which RECORD disagreed. The chain's job is the two nobody else catches: a
/// deleted genesis and a renumbered sequence. Running it first would replace good refusals with a
/// vaguer one.
///
/// Reads only. It runs inside the caller's transaction, which is ADR 0045's single startup
/// snapshot, so it judges the same store every other pass judged.
pub(crate) fn verify_chain(connection: &Connection) -> Result<Option<ChainFault>, rusqlite::Error> {
    let state: Option<(Option<Vec<u8>>, i64, Option<i64>)> = connection
        .query_row(
            "SELECT head_digest, head_sequence, migrated_through_sequence
             FROM event_chain_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    let mut statement = connection.prepare(
        "SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms,
                predecessor_kind, predecessor_digest, link_digest
         FROM events ORDER BY sequence",
    )?;
    let mut rows = statement.query([])?;

    let Some((head_digest, head_sequence, migrated_through)) = state else {
        // No state and no Events is a store that has never appended: a valid empty chain.
        return Ok(if rows.next()?.is_some() {
            Some(ChainFault::StateMissing)
        } else {
            None
        });
    };

    let boundary = migrated_through.unwrap_or(0);
    let mut previous: Option<(i64, [u8; 32])> = None;

    while let Some(row) = rows.next()? {
        let sequence: i64 = row.get(0)?;
        let workspace_id: String = row.get(1)?;
        let kind: String = row.get(2)?;
        let payload_json: String = row.get(3)?;
        let occurred_at_ms: i64 = row.get(4)?;
        let predecessor_kind: String = row.get(5)?;
        let predecessor_digest: Option<Vec<u8>> = row.get(6)?;
        let link_digest: Option<Vec<u8>> = row.get(7)?;

        let predecessor = match predecessor_kind.as_str() {
            "unchained" => {
                // Pre-migration rows are outside the chain by construction, and the boundary says
                // how far back that goes. One after the boundary was written around the door.
                debug_assert_eq!(Predecessor::Unchained.tag(), TAG_UNCHAINED);
                if sequence > boundary {
                    return Ok(Some(ChainFault::UnchainedAfterBoundary { sequence }));
                }
                // Nothing to recompute: an unchained row carries no link digest to check, which is
                // the whole of what `Predecessor::Unchained` means.
                continue;
            }
            "genesis" => {
                if previous.is_some() {
                    return Ok(Some(ChainFault::GenesisNotFirst { sequence }));
                }
                Predecessor::Genesis
            }
            _ => {
                let digest = as_digest(&predecessor_digest.unwrap_or_default())?;
                match previous {
                    Some((_, link)) if link == digest => Predecessor::Linked(digest),
                    Some((earlier, _)) => {
                        return Ok(Some(ChainFault::PredecessorMismatch {
                            sequence,
                            previous: earlier,
                        }));
                    }
                    // A linked row with nothing chained before it: whatever it followed is gone.
                    None => {
                        return Ok(Some(ChainFault::PredecessorMismatch {
                            sequence,
                            previous: boundary,
                        }));
                    }
                }
            }
        };

        let recomputed = Link {
            predecessor,
            sequence,
            workspace_id: &workspace_id,
            kind: &kind,
            payload_json: &payload_json,
            occurred_at_ms,
        }
        .digest();
        if as_digest(&link_digest.unwrap_or_default())? != recomputed {
            return Ok(Some(ChainFault::LinkDigestMismatch { sequence }));
        }
        previous = Some((sequence, recomputed));
    }

    // The walk has to END where the head says it does. Without this, truncating the newest Events
    // and leaving the head behind would verify link by link and still be a shorter history.
    match (previous, &head_digest) {
        // The walk ended where the head says, on the digest the head names.
        (Some((sequence, digest)), Some(head)) => {
            if sequence != head_sequence || digest.as_slice() != head.as_slice() {
                return Ok(Some(ChainFault::HeadMismatch {
                    expected_sequence: head_sequence,
                    found_sequence: sequence,
                }));
            }
        }
        // An empty chain and nothing chained: a migrated store nobody has appended to yet, or a
        // fresh one. Valid, and the commonest state a migrated store is in.
        (None, None) => {}
        // A head that names a digest with no chained rows behind it, or chained rows under a head
        // that says the chain is empty. Either way the two disagree.
        (None, Some(_)) => {
            return Ok(Some(ChainFault::HeadMismatch {
                expected_sequence: head_sequence,
                found_sequence: boundary,
            }));
        }
        (Some((sequence, _)), None) => {
            return Ok(Some(ChainFault::HeadMismatch {
                expected_sequence: head_sequence,
                found_sequence: sequence,
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R58.2: every one of the six sentences Kevin signs, rendered through the supervisor an
    /// operator actually reads, and compared back to the row it claims to speak with.
    ///
    /// The gap this closes, in my own report's words: five of the six faults had no witness, so a
    /// rewrite of five of these sentences would have failed nothing. The rows below give each
    /// fault its own sentinel numbers, so the pins are not tautologies on the table --
    ///
    ///   * a fault wired to another fault's row goes RED (`row` is checked against the name),
    ///   * a dropped `{sequence}` goes RED (every number the variant carries must be named),
    ///   * `PredecessorMismatch`'s two numbers swapped goes RED (the test substitutes them by its
    ///     own mapping, not by `sentence`'s),
    ///   * a leftover `{placeholder}` goes RED,
    ///   * two faults given the same sentence goes RED,
    ///   * and a supervisor arm that writes anything but this table goes RED for all six.
    #[test]
    fn every_chain_fault_speaks_its_own_signed_sentence() {
        let rows: [(ChainFault, &str, String, &[i64]); 6] = [
            (
                ChainFault::LinkDigestMismatch { sequence: 4211 },
                "LinkDigestMismatch",
                OPERATOR_SENTENCES[0].1.replace("{sequence}", "4211"),
                &[4211],
            ),
            (
                ChainFault::PredecessorMismatch {
                    sequence: 4213,
                    previous: 4212,
                },
                "PredecessorMismatch",
                OPERATOR_SENTENCES[1]
                    .1
                    .replace("{previous}", "4212")
                    .replace("{sequence}", "4213"),
                &[4212, 4213],
            ),
            (
                ChainFault::GenesisNotFirst { sequence: 4214 },
                "GenesisNotFirst",
                OPERATOR_SENTENCES[2].1.replace("{sequence}", "4214"),
                &[4214],
            ),
            (
                ChainFault::UnchainedAfterBoundary { sequence: 4215 },
                "UnchainedAfterBoundary",
                OPERATOR_SENTENCES[3].1.replace("{sequence}", "4215"),
                &[4215],
            ),
            (
                ChainFault::HeadMismatch {
                    expected_sequence: 4216,
                    found_sequence: 4217,
                },
                "HeadMismatch",
                OPERATOR_SENTENCES[4]
                    .1
                    .replace("{expected_sequence}", "4216")
                    .replace("{found_sequence}", "4217"),
                &[4216, 4217],
            ),
            (
                ChainFault::StateMissing,
                "StateMissing",
                OPERATOR_SENTENCES[5].1.to_owned(),
                &[],
            ),
        ];

        let mut seen: Vec<String> = Vec::new();
        for (index, (fault, name, expected, must_name)) in rows.into_iter().enumerate() {
            assert_eq!(
                OPERATOR_SENTENCES[index].0, name,
                "row {index} of the signed table is not {name}'s row"
            );
            assert_eq!(
                fault.row(),
                index,
                "{name} speaks with row {} of the signed table, not its own",
                fault.row()
            );

            let rendered = crate::SupervisorError::EventChainBroken(fault).to_string();
            assert_eq!(rendered, expected, "{name} does not render its signed row");
            assert!(
                !rendered.contains('{') && !rendered.contains('}'),
                "{name} left a placeholder unfilled: {rendered}"
            );
            for number in must_name {
                assert!(
                    rendered.contains(&number.to_string()),
                    "{name} does not name {number}, so it does not say where the chain failed: \
                     {rendered}"
                );
            }
            assert!(
                !seen.contains(&rendered),
                "{name} says exactly what another fault says, so the sentence cannot tell an \
                 operator which fault it is: {rendered}"
            );
            seen.push(rendered);
        }

        assert_eq!(
            seen.len(),
            OPERATOR_SENTENCES.len(),
            "the table and the faults are different sizes"
        );

        // On the TEMPLATES, not the rendered lines. My first version of this check compared the
        // rendered sentences, and a mutant that signed two faults the same template SURVIVED it:
        // the sentinel numbers differ, so the rendered strings differ however identical the
        // sentences are. A guard has to read the thing it constrains.
        for (left, (name, template)) in OPERATOR_SENTENCES.iter().enumerate() {
            for (other_name, other_template) in OPERATOR_SENTENCES.iter().skip(left + 1) {
                assert_ne!(name, other_name, "{name} appears twice in the signed table");
                assert_ne!(
                    template, other_template,
                    "{name} and {other_name} are signed the same sentence, so the line an \
                     operator reads cannot tell them apart"
                );
            }
        }
    }

    /// The chain's cost, measured rather than asserted: 100k links, the same shape
    /// DP-b42-event-chain.md measured before the build.
    ///
    /// The packet's number was a Python/OpenSSL proxy (58 ms marginal per 100k). This runs the
    /// real `Link::digest` and prints the figure the report quotes, so the claim in the report is
    /// this code's, not an estimate about it. It asserts only an upper bound wide enough not to be
    /// a flake on a loaded box -- the point is the measurement, and a bound that fails on a busy
    /// machine would be a worse test than none.
    #[test]
    fn one_hundred_thousand_links_cost_what_the_packet_said() {
        let payload = format!(
            r#"{{"workspaceId":"{WS}","missionId":"{WS}","digestValue":"{}","version":4}}"#,
            "a".repeat(64)
        );
        let start = std::time::Instant::now();
        let mut previous = Predecessor::Genesis;
        for sequence in 1..=100_000_i64 {
            let digest = Link {
                predecessor: previous,
                sequence,
                workspace_id: WS,
                kind: "proposal.dispatched",
                payload_json: &payload,
                occurred_at_ms: 1_788_400_000_000 + sequence,
            }
            .digest();
            previous = Predecessor::Linked(digest);
        }
        let elapsed = start.elapsed();
        println!(
            "R58.COST: 100k links in {:?} ({:.2} us/link)",
            elapsed,
            elapsed.as_secs_f64() * 1e6 / 100_000.0
        );
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "100k links took {elapsed:?}, which is not a chain cost but a defect"
        );
    }

    /// R58: **no product code writes an Event around the door.**
    ///
    /// The single append path is only a guarantee while it is the only path, and 27 call sites
    /// spread over 22 modules is 27 chances for the 28th to be written by hand. This reads the
    /// crate's own sources and refuses any raw `INSERT INTO events` outside a `#[cfg(test)]`
    /// module.
    ///
    /// Test modules are deliberately exempt and that exemption is the point: a probe that plants a
    /// tampered, duplicated or orphaned row MUST be able to write around the chain, or the tamper
    /// cells could not be built at all. Those rows say `unchained`, which is what a row outside the
    /// chain honestly is.
    #[test]
    fn no_product_code_writes_an_event_around_the_door() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut files = 0_usize;
        for entry in std::fs::read_dir(&src).expect("the crate's source directory reads") {
            let path = entry.expect("a directory entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            files += 1;
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).expect("a source file reads");
            // Everything from the file's own test module onward is exempt.
            let cut = text.find("#[cfg(test)]\nmod tests").unwrap_or(text.len());
            let product = &text[..cut];
            if name == "event_chain.rs" {
                continue; // the door itself
            }
            for (number, line) in product.lines().enumerate() {
                if line.contains("INSERT INTO events") {
                    offenders.push(format!("{name}:{}", number + 1));
                }
            }
        }
        // The census has to have READ something, or an empty result passes for a clean one.
        assert!(
            files >= 20,
            "the source census found only {files} files: it has stopped matching"
        );
        assert!(
            offenders.is_empty(),
            "product code writes Events around `append_event`: {offenders:?}"
        );
    }

    const WS: &str = "00000000-0000-4000-8000-000000000001";

    fn link(predecessor: Predecessor) -> Link<'static> {
        Link {
            predecessor,
            sequence: 7,
            workspace_id: WS,
            kind: "workspace.created",
            payload_json: r#"{"workspaceId":"x","name":"Beacon"}"#,
            occurred_at_ms: 1_788_400_000_000,
        }
    }

    /// The genesis is a value, not an absence: thirty-two zero bytes are what an uninitialised
    /// column, a truncated read and a failed write all look like, and a first link a reader cannot
    /// tell apart from those says nothing.
    #[test]
    fn the_genesis_is_a_named_constant_and_never_zeroes() {
        assert_ne!(genesis(), [0_u8; 32]);
        assert_eq!(genesis(), sha256(b"beacon.event_chain.v1.genesis\0"));
        // Stable across calls: a genesis derived from anything that moves would re-key the chain.
        assert_eq!(genesis(), genesis());
    }

    /// Every stored column is inside the digest. A field left out is a field a tamper can change
    /// while the chain still verifies -- the hole this chain exists to close.
    #[test]
    fn every_stored_column_changes_the_link() {
        let base = link(Predecessor::Genesis);
        let other = "00000000-0000-4000-8000-000000000002";
        let mutations: Vec<(&str, Link<'_>)> = vec![
            (
                "sequence",
                Link {
                    sequence: 8,
                    ..base
                },
            ),
            (
                "workspace_id",
                Link {
                    workspace_id: other,
                    ..base
                },
            ),
            (
                "kind",
                Link {
                    kind: "project.created",
                    ..base
                },
            ),
            (
                "payload_json",
                Link {
                    payload_json: "{}",
                    ..base
                },
            ),
            (
                "occurred_at_ms",
                Link {
                    occurred_at_ms: 1_788_400_000_001,
                    ..base
                },
            ),
            (
                "predecessor",
                Link {
                    predecessor: Predecessor::Linked([9_u8; 32]),
                    ..base
                },
            ),
        ];
        let mut seen = vec![base.digest()];
        for (label, mutated) in mutations {
            let digest = mutated.digest();
            assert!(
                !seen.contains(&digest),
                "changing the {label} did not change the link"
            );
            seen.push(digest);
        }
    }

    /// The three predecessor states are three digests, because the tag is inside the bytes. Without
    /// it, an `Unchained` row and a `Genesis` row would hash identically -- and a migrated store's
    /// "I make no claim" would be readable as "nothing came before me", which is a claim.
    #[test]
    fn the_three_predecessor_states_are_never_one_value() {
        let genesis_link = link(Predecessor::Genesis).digest();
        let unchained = link(Predecessor::Unchained).digest();
        let linked = link(Predecessor::Linked(genesis())).digest();
        assert_ne!(genesis_link, unchained, "genesis and unchained hash alike");
        assert_ne!(genesis_link, linked, "genesis and a link to it hash alike");
        assert_ne!(linked, unchained, "a real link and unchained hash alike");
    }

    /// Strings are length-prefixed, so no two different Events encode to the same bytes. Delimiting
    /// instead would only move the problem to inputs containing the delimiter -- and `payload_json`
    /// is arbitrary caller-shaped text.
    #[test]
    fn a_shifted_boundary_between_two_strings_is_a_different_link() {
        let base = link(Predecessor::Genesis);
        let left = Link {
            kind: "ab",
            payload_json: "cd",
            ..base
        };
        let right = Link {
            kind: "abc",
            payload_json: "d",
            ..base
        };
        assert_ne!(
            left.digest(),
            right.digest(),
            "two Events whose text concatenates alike produced one link"
        );
    }
}
