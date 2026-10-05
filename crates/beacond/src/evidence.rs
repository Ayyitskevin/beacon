//! Pure byte binding for an exact source span, not a Knowledge record or admission decision.
//!
//! The caller supplies already acquired bytes. Verification performs no I/O, persistence, logging,
//! authorization, or source lookup. A successful match establishes only that the quoted UTF-8
//! bytes occur at the given byte range in the complete digest-bound snapshot. It says nothing about
//! truth, authorship, stance, access, or whether that span is admissible as Evidence.

use std::fmt;
use std::ops::Range;

use crate::proposal::sha256;

/// Maximum snapshot length accepted by this pure helper, in bytes.
///
/// This bounds the supplied buffer processed here, not acquisition, global Evidence policy, or
/// physical process exit. The length is checked before scanning or hashing the snapshot.
pub const MAX_SOURCE_SNAPSHOT_BYTES: usize = 1_048_576;

/// Maximum quote length accepted by this pure helper, in bytes, checked before UTF-8 validation.
pub const MAX_SOURCE_QUOTE_BYTES: usize = 1_048_576;

/// A supplied digest claim, not a verified source identity or an Artifact record digest.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SourceSpanDigest<'a> {
    pub algorithm: &'a str,
    pub canonicalizer: &'a str,
    pub value: [u8; 32],
}

impl fmt::Debug for SourceSpanDigest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceSpanDigest")
            .finish_non_exhaustive()
    }
}

/// Byte equality established by [`verify_source_span`], borrowing only the snapshot.
///
/// Fields and construction remain private so a caller cannot supply an unverified match. An empty
/// range may establish empty byte equality; it does not decide whether empty Evidence is allowed.
#[derive(Clone, PartialEq, Eq)]
pub struct SourceSpanMatch<'a> {
    digest: [u8; 32],
    range: Range<usize>,
    quote: &'a str,
}

impl<'a> SourceSpanMatch<'a> {
    /// SHA-256 over the complete snapshot bytes, under `beacon.raw.v1` without transformation.
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// The verified half-open byte range within the snapshot.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// The exact UTF-8 slice of the snapshot, never a borrow of the supplied quote argument.
    pub fn quote(&self) -> &'a str {
        self.quote
    }
}

impl fmt::Debug for SourceSpanMatch<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceSpanMatch")
            .finish_non_exhaustive()
    }
}

/// A content-free refusal. Neither formatting surface includes supplied bytes or envelope values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceSpanError {
    SnapshotTooLarge,
    QuoteTooLarge,
    UnsupportedDigestAlgorithm,
    UnsupportedCanonicalizer,
    InvalidSnapshotEncoding,
    InvalidQuoteEncoding,
    DigestMismatch,
    InvalidRange,
    QuoteMismatch,
}

impl fmt::Display for SourceSpanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SnapshotTooLarge => "source snapshot exceeds the byte limit",
            Self::QuoteTooLarge => "source quote exceeds the byte limit",
            Self::UnsupportedDigestAlgorithm => "unsupported source digest algorithm",
            Self::UnsupportedCanonicalizer => "unsupported source canonicalizer",
            Self::InvalidSnapshotEncoding => "source snapshot is not valid UTF-8",
            Self::InvalidQuoteEncoding => "source quote is not valid UTF-8",
            Self::DigestMismatch => "source snapshot digest does not match",
            Self::InvalidRange => "source span is not an in-bounds UTF-8 byte range",
            Self::QuoteMismatch => "source quote does not match the snapshot span",
        })
    }
}

impl std::error::Error for SourceSpanError {}

/// Verify an exact half-open UTF-8 byte span against the complete snapshot's raw SHA-256.
///
/// Refusal order is sizes (snapshot, quote), envelope (algorithm, canonicalizer), encoding
/// (snapshot, quote), complete snapshot digest, range, then exact quote equality. No normalization,
/// fallback, or partial result is permitted. Work is linear in the bounded input lengths; the
/// helper does not copy either input buffer or allocate a source record.
pub fn verify_source_span<'a>(
    snapshot: &'a [u8],
    expected: &SourceSpanDigest<'_>,
    start: u64,
    end: u64,
    quote: &[u8],
) -> Result<SourceSpanMatch<'a>, SourceSpanError> {
    if snapshot.len() > MAX_SOURCE_SNAPSHOT_BYTES {
        return Err(SourceSpanError::SnapshotTooLarge);
    }
    if quote.len() > MAX_SOURCE_QUOTE_BYTES {
        return Err(SourceSpanError::QuoteTooLarge);
    }
    if expected.algorithm != "sha256" {
        return Err(SourceSpanError::UnsupportedDigestAlgorithm);
    }
    if expected.canonicalizer != "beacon.raw.v1" {
        return Err(SourceSpanError::UnsupportedCanonicalizer);
    }

    let snapshot_text =
        std::str::from_utf8(snapshot).map_err(|_| SourceSpanError::InvalidSnapshotEncoding)?;
    std::str::from_utf8(quote).map_err(|_| SourceSpanError::InvalidQuoteEncoding)?;

    let digest = sha256(snapshot);
    if digest != expected.value {
        return Err(SourceSpanError::DigestMismatch);
    }

    let start = usize::try_from(start).map_err(|_| SourceSpanError::InvalidRange)?;
    let end = usize::try_from(end).map_err(|_| SourceSpanError::InvalidRange)?;
    // `str::get` checks ordering, bounds, and both UTF-8 boundaries without indexing panics.
    let matched_quote = snapshot_text
        .get(start..end)
        .ok_or(SourceSpanError::InvalidRange)?;
    if matched_quote.as_bytes() != quote {
        return Err(SourceSpanError::QuoteMismatch);
    }

    Ok(SourceSpanMatch {
        digest,
        range: start..end,
        quote: matched_quote,
    })
}
