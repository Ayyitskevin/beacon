//! Outcome Document slice 2-B: the verified collector's skeleton.
//!
//! The collector turns a set of citations into the manifest references `outcome.rs` renders from. It
//! is the place where five modules meet, and its whole discipline is one sentence from the design:
//! **a relation or a joined column may identify a candidate, and may never supply a renderable
//! claim.** Every source is re-read through its owning module's verified reader, which proves the
//! Workspace, the exact identity, the stored bytes and the digest before returning a typed value.
//!
//! Three readers do not exist yet — measured spend, the Mission impact classification, the Artifact
//! entity, and `MissionCompletionV1`. They are **typed refusals**, never stubs that return a value:
//! a stub is indistinguishable from a verified read at the call site, and the whole point of this
//! module is that a caller cannot tell the difference between "verified" and "assumed" only by
//! looking at the type. `NotYetAvailable` names which reader is missing.
//!
//! This slice reads; it does not persist. Immutable manifest storage and its reconciliation are
//! slice C, and there is no migration here: 0025 is reserved for the stores in slice A.

use std::collections::BTreeSet;
use std::fmt;

use data_encoding::HEXLOWER;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::outcome::{
    BoundRefusal, MAX_CITATIONS, OutcomeBound, RECEIPT_CHECKSUM_LABEL, ReceiptRef, SourceKind,
};
use crate::proposal::sha256;

/// A reader the collector needs and the store does not have yet.
///
/// Each is a named seam rather than a hole: slice A builds the first three, and `MissionCompletionV1`
/// is C43's record plus its store. A caller that sees one of these knows exactly which lane it is
/// waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingReader {
    MeasuredSpend,
    MissionImpactClassification,
    Artifact,
    MissionCompletion,
}

impl MissingReader {
    /// The lowercase token this seam is named by.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MeasuredSpend => "measured_spend",
            Self::MissionImpactClassification => "mission_impact_classification",
            Self::Artifact => "artifact",
            Self::MissionCompletion => "mission_completion",
        }
    }
}

/// Why a collection did not happen.
///
/// Every arm is a refusal. There is deliberately no arm that carries a partially collected result:
/// the design's §5 rule 6 says a corrupt store, a digest mismatch, a cross-Workspace reference or a
/// broken reader **aborts** rather than becoming an honest domain `broken` row, and a half-built
/// manifest is the shape that would let one through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectorRefusal {
    /// The owning module has no verified reader yet.
    NotYetAvailable(MissingReader),
    /// Outcome's own bound, refused before anything was read.
    Bound(BoundRefusal),
    /// The cited object is not in this Workspace's store at all.
    ///
    /// `kind` is a name rather than a `SourceKind`, because a Receipt is not one: it has no content
    /// digest and is cited by `(Workspace, run UUID, version)`. Borrowing a source tag to say
    /// "receipt" is the R13.8 mistake, and a refusal is a worse place to make it than a census.
    Absent { kind: &'static str, id: String },
    /// The owning reader refused it. The reason is the reader's, repeated rather than reinterpreted.
    Unverified { kind: &'static str, reason: String },
}

impl fmt::Display for CollectorRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotYetAvailable(reader) => {
                write!(formatter, "no verified reader yet: {}", reader.as_str())
            }
            Self::Bound(bound) => write!(
                formatter,
                "bound {} exhausted: configured {}, observed at least {}",
                bound.bound.as_str(),
                bound.configured,
                bound.observed_at_least
            ),
            Self::Absent { kind, id } => {
                write!(formatter, "{kind} {id} is not in this Workspace")
            }
            Self::Unverified { kind, reason } => {
                write!(formatter, "{kind} refused by its owning reader: {reason}")
            }
        }
    }
}

impl std::error::Error for CollectorRefusal {}

/// What a caller asks the collector to resolve.
///
/// Identities only — digests and run UUIDs. Nothing renderable crosses this boundary, because
/// everything renderable is what the collector goes and reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectionRequest {
    pub proposal_digests: Vec<String>,
    pub ruling_digests: Vec<String>,
    pub evaluation_digests: Vec<String>,
    pub receipt_run_uuids: Vec<Uuid>,
}

impl CollectionRequest {
    /// How many distinct objects this request would cite.
    ///
    /// Counted from the request, before a single read, because the bound exists to prevent the work
    /// and a bound checked afterwards has already paid for it.
    fn citation_count(&self) -> u64 {
        let mut seen: BTreeSet<(u8, String)> = BTreeSet::new();
        for digest in &self.proposal_digests {
            seen.insert((SourceKind::Proposal.tag(), digest.clone()));
        }
        for digest in &self.ruling_digests {
            seen.insert((SourceKind::Ruling.tag(), digest.clone()));
        }
        for digest in &self.evaluation_digests {
            seen.insert((SourceKind::Evaluation.tag(), digest.clone()));
        }
        for run in &self.receipt_run_uuids {
            seen.insert((0xff, run.hyphenated().to_string()));
        }
        seen.len() as u64
    }
}

/// What the collector resolved, all of it verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedSources {
    pub proposals: Vec<CollectedDigest>,
    pub rulings: Vec<CollectedDigest>,
    pub evaluations: Vec<CollectedDigest>,
    pub receipts: Vec<ReceiptRef>,
}

/// One source, named by the identity its owning module actually has.
///
/// Proposal, Ruling and Evaluation are identified by digest and carry no UUID and no version, so
/// this carries what they have. The manifest's `SourceRef` wants a UUID and a schema version, which
/// none of them has -- that gap is `SourceRef`'s, and it is named in the report rather than papered
/// over here with a minted identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedDigest {
    pub kind: SourceKind,
    pub workspace_id: Uuid,
    pub digest_value: String,
}

/// Reads a Workspace's stored sources through their owning verified readers.
pub struct Collector<'a> {
    connection: &'a Connection,
    workspace_id: Uuid,
}

impl<'a> Collector<'a> {
    pub fn new(connection: &'a Connection, workspace_id: Uuid) -> Self {
        Self {
            connection,
            workspace_id,
        }
    }

    /// Resolve a request, or refuse.
    ///
    /// **The bound runs first, before any read.** "Refuses before a partial manifest" is a property
    /// of this ordering and nothing else: once a read has happened, a refusal has already produced
    /// the state the bound existed to prevent.
    pub fn collect(
        &self,
        request: &CollectionRequest,
    ) -> Result<CollectedSources, CollectorRefusal> {
        let observed = request.citation_count();
        if observed > MAX_CITATIONS {
            return Err(CollectorRefusal::Bound(BoundRefusal {
                bound: OutcomeBound::CitationCount,
                configured: MAX_CITATIONS,
                observed_at_least: observed,
            }));
        }

        let mut proposals = Vec::new();
        for digest in &request.proposal_digests {
            proposals.push(self.proposal(digest)?);
        }
        let mut rulings = Vec::new();
        for digest in &request.ruling_digests {
            rulings.push(self.ruling(digest)?);
        }
        let mut evaluations = Vec::new();
        for digest in &request.evaluation_digests {
            evaluations.push(self.evaluation(digest)?);
        }
        let mut receipts = Vec::new();
        for run in &request.receipt_run_uuids {
            receipts.push(self.receipt(*run)?);
        }

        Ok(CollectedSources {
            proposals,
            rulings,
            evaluations,
            receipts,
        })
    }

    /// The Proposal, through `proposal::verified_proposal_within`.
    fn proposal(&self, digest_value: &str) -> Result<CollectedDigest, CollectorRefusal> {
        match crate::proposal::verified_proposal_within(
            self.connection,
            self.workspace_id,
            digest_value,
        ) {
            Ok(Some(proposal)) => Ok(CollectedDigest {
                kind: SourceKind::Proposal,
                workspace_id: self.workspace_id,
                // From the verified value, never from the caller's string: the reader is what
                // proved this digest names these bytes.
                digest_value: proposal.digest().value.clone(),
            }),
            Ok(None) => Err(CollectorRefusal::Absent {
                kind: "proposal",
                id: digest_value.to_owned(),
            }),
            Err(error) => Err(CollectorRefusal::Unverified {
                kind: "proposal",
                reason: error.to_string(),
            }),
        }
    }

    /// The Ruling, through `ruling::verified_ruling_within`.
    fn ruling(&self, digest_value: &str) -> Result<CollectedDigest, CollectorRefusal> {
        match crate::ruling::verified_ruling_within(
            self.connection,
            self.workspace_id,
            digest_value,
        ) {
            Ok(Some(ruling)) => Ok(CollectedDigest {
                kind: SourceKind::Ruling,
                workspace_id: self.workspace_id,
                digest_value: ruling.digest().value.clone(),
            }),
            Ok(None) => Err(CollectorRefusal::Absent {
                kind: "ruling",
                id: digest_value.to_owned(),
            }),
            Err(error) => Err(CollectorRefusal::Unverified {
                kind: "ruling",
                reason: error.to_string(),
            }),
        }
    }

    /// The Evaluation, through `evaluation::verified_evaluation_within`.
    fn evaluation(&self, digest_value: &str) -> Result<CollectedDigest, CollectorRefusal> {
        match crate::evaluation::verified_evaluation_within(
            self.connection,
            self.workspace_id,
            digest_value,
        ) {
            Ok(Some(evaluation)) => Ok(CollectedDigest {
                kind: SourceKind::Evaluation,
                workspace_id: self.workspace_id,
                digest_value: evaluation.digest().value.clone(),
            }),
            Ok(None) => Err(CollectorRefusal::Absent {
                kind: "evaluation",
                id: digest_value.to_owned(),
            }),
            Err(error) => Err(CollectorRefusal::Unverified {
                kind: "evaluation",
                reason: error.to_string(),
            }),
        }
    }

    /// One Receipt, cited by the identity a Receipt actually has, with an Outcome checksum over its
    /// exact stored bytes.
    ///
    /// **The `outcome` column is not read.** It is a projection of a value inside `receipt_json`,
    /// and a joined column may identify a candidate but may never supply a renderable claim. The
    /// checksum covers the stored bytes and nothing else, so a store whose column disagrees with its
    /// bytes produces the same citation as one whose column agrees -- the projection cannot reach
    /// the document.
    ///
    /// The bytes are read as bytes -- `CAST(receipt_json AS BLOB)` -- rather than as a `String`,
    /// because "exact stored bytes" is the claim the checksum makes, and a round trip through Rust's
    /// UTF-8 validation is a transformation nobody asked it to prove.
    fn receipt(&self, run_uuid: Uuid) -> Result<ReceiptRef, CollectorRefusal> {
        let row: Option<(i64, Vec<u8>)> = self
            .connection
            .query_row(
                "SELECT version, CAST(receipt_json AS BLOB) FROM receipts
                 WHERE workspace_id = ?1 AND run_id = ?2",
                params![self.workspace_id.to_string(), run_uuid.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| CollectorRefusal::Unverified {
                kind: "receipt",
                reason: error.to_string(),
            })?;

        let Some((version, receipt_json)) = row else {
            return Err(CollectorRefusal::Absent {
                kind: "receipt",
                id: run_uuid.hyphenated().to_string(),
            });
        };
        let version = u32::try_from(version).map_err(|_| CollectorRefusal::Unverified {
            kind: "receipt",
            reason: "receipt version is not a u32".to_owned(),
        })?;

        Ok(ReceiptRef {
            workspace_id: self.workspace_id,
            run_uuid,
            receipt_version: version,
            outcome_checksum: HEXLOWER.encode(&sha256(&receipt_json)),
        })
    }

    /// The measured-spend reader, which does not exist yet.
    pub fn spend(&self) -> Result<CollectedDigest, CollectorRefusal> {
        Err(CollectorRefusal::NotYetAvailable(
            MissingReader::MeasuredSpend,
        ))
    }

    /// The Mission impact classification reader, which does not exist yet.
    pub fn classification(&self) -> Result<CollectedDigest, CollectorRefusal> {
        Err(CollectorRefusal::NotYetAvailable(
            MissingReader::MissionImpactClassification,
        ))
    }

    /// The Artifact reader, which does not exist yet.
    pub fn artifact(&self) -> Result<CollectedDigest, CollectorRefusal> {
        Err(CollectorRefusal::NotYetAvailable(MissingReader::Artifact))
    }

    /// The `MissionCompletionV1` reader, whose record is not built either.
    pub fn completion(&self) -> Result<CollectedDigest, CollectorRefusal> {
        Err(CollectorRefusal::NotYetAvailable(
            MissingReader::MissionCompletion,
        ))
    }
}

/// The label every Outcome Receipt checksum carries.
///
/// Re-exported here so a consumer of the collector never has to reach into the manifest module to
/// learn what the checksum it just received is. It is an **Outcome verification checksum** over the
/// exact stored `receipt_json` bytes, not a Receipt content digest: ADR 0027 and Receipt v2 define
/// none, and minting one would contradict them (Kevin's ruling 5).
pub const OUTCOME_RECEIPT_CHECKSUM_LABEL: &str = RECEIPT_CHECKSUM_LABEL;
