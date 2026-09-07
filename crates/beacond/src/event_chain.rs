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

/// The prefix every link commits to. A digest over bare fields would collide with any other
/// structure that happened to serialise the same bytes; the magic makes the domain explicit.
const MAGIC: &[u8] = b"beacon.event_chain.v1\0";

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
        let mut bytes = MAGIC.to_vec();
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
    let head: Option<(Vec<u8>, i64)> = connection
        .query_row(
            "SELECT head_digest, head_sequence FROM event_chain_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let predecessor = match &head {
        // A store that has chained before: this Event follows the head.
        Some((digest, _)) => Predecessor::Linked(as_digest(digest)?),
        // No state row at all: this is the first Event of a store created with the chain.
        None => Predecessor::Genesis,
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

#[cfg(test)]
mod tests {
    use super::*;

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
                    workspace_id: &other,
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
