//! Measured spend v1, slice 1: the record's pure types and its canonical bytes.
//!
//! A `MeasuredSpendV1` is the durable, attributable statement of what one Mission actually cost,
//! over a declared window, by a named origin, with its own reference identity and content digest.
//!
//! It is **not** a budget, a ceiling, a reservation, a forecast, a Receipt, or an Invocation
//! ledger. It creates no authority: nothing derives "within budget" from its existence, and this
//! module has deliberately no cross-unit total and no `is_within_budget` -- the reason ADR 0034
//! gives for having no `is_ready` and ADR 0037 for having no `is_in_force`. A single number read
//! off a spend record is exactly the shortcut that lets a consumer skip the check it owes.
//!
//! **Nothing on `main` can produce one yet.** `BudgetEnvelope` has one arm, `NoSpend`; there is no
//! Models module, no Invocation ledger, and no cost field in Receipt v2. So v1's job is the
//! opposite of what a spend record usually does: it makes *"we did not measure"* a first-class,
//! attributable, digest-identified statement rather than an absence.
//!
//! Slice 1 is pure. No database, no service, no event, no migration: those are slice 2. The
//! Outcome Document's `SpendRef` (`crates/beacond/src/outcome.rs`) is the consumer seam -- it cites
//! `(workspace_id, spend_id, version, digest)` per R15 -- and this slice does not touch it. That
//! type still carries a mandatory `amount_minor`, so an `unmeasured` record, which *is* verified,
//! can today only be cited as `not_evaluated`, throwing away its identity and digest. Correcting
//! that is a `beacon.outcome.manifest.v1` change and is fold-round work, not this module's.

use std::cmp::Ordering;
use std::fmt;

use data_encoding::HEXLOWER;
use uuid::Uuid;

use crate::proposal::sha256;

/// The canonicalizer identity. Any future grammar is a new canonicalizer; existing v1 bytes are
/// never reinterpreted under v2 rules.
pub const CANONICALIZER: &str = "beacon.spend.v1";
/// The literal ASCII magic that starts every record, including its terminating NUL: 16 bytes.
const MAGIC: &[u8] = b"beacon.spend.v1\0";
const DIGEST_ALGORITHM: &str = "sha256";

/// The store's envelope for `measured_spend.canonical_bytes`, enforced here first so an over-large
/// record is a fact about the bytes rather than a database error surfacing through a Command.
const MAX_CANONICAL_BYTES: usize = 65_536;

const MAX_LINES: usize = 32;
const MAX_UNCOUNTED: usize = 64;
const MAX_UNIT_BYTES: usize = 16;
const MAX_TEXT_SCALARS: usize = 2_000;
const MAX_TEXT_BYTES: usize = MAX_TEXT_SCALARS * 4;
/// Nine is more than any real currency or metered unit carries, and it keeps the decimal rendering
/// a consumer will do in integer arithmetic.
const MAX_MINOR_UNITS_EXPONENT: u8 = 9;
const DIGEST_HEX_CHARS: usize = 64;

/// The smallest number of bytes one entry of each list can occupy.
///
/// ADR 0034's accepted rule: a declared count is checked against the remaining input at the
/// smallest possible size of one entry, before the first entry is read and before anything reserves
/// capacity for it. Each figure is structural -- every fixed-width field and every length prefix,
/// with zero-length strings.
const MIN_LINE_BYTES: usize = 17;
const MIN_UNCOUNTED_BYTES: usize = 9;

// One-byte tags. Assigned explicitly and never renumbered. `supersedes` is the one position where
// 0x00 is assigned, because absence is a value there and must encode as one.
const TAG_ORIGIN_DAEMON_LEDGER: u8 = 0x01;
const TAG_ORIGIN_OPERATOR_DECLARED: u8 = 0x02;

const TAG_MEASURED: u8 = 0x01;
const TAG_PARTIALLY_MEASURED: u8 = 0x02;
const TAG_UNMEASURED: u8 = 0x03;
const TAG_BROKEN: u8 = 0x04;

const TAG_UNCOUNTED_PROVIDER_DID_NOT_PRICE: u8 = 0x01;
const TAG_UNCOUNTED_INVOCATION_RECORD_MISSING: u8 = 0x02;
const TAG_UNCOUNTED_UNIT_NOT_CONVERTIBLE: u8 = 0x03;
const TAG_UNCOUNTED_OUTSIDE_RETENTION: u8 = 0x04;

const TAG_SUPERSEDES_NONE: u8 = 0x00;
const TAG_SUPERSEDES_RECORD: u8 = 0x01;

// The literal spellings. They are closed-vocabulary tokens, never human text.
const MEASURED: &str = "measured";
const PARTIALLY_MEASURED: &str = "partially_measured";
const UNMEASURED: &str = "unmeasured";
const BROKEN: &str = "broken";

// ---------------------------------------------------------------------------------------------
// The measurement, with the amount inside it
// ---------------------------------------------------------------------------------------------

/// How a figure was produced.
///
/// Two arms and deliberately no `resident_reported`. A Resident reporting its own spend is an Agent
/// asserting its own budget consumption: BLUEPRINT `:819` makes Models the enforcer of Mission and
/// Agent budgets and `:434` forbids an Agent to "increase, transfer, borrow, or reinterpret an
/// Envelope". An Agent that can under-report its spend can exceed its Envelope while every record
/// says it did not -- the same defect class as an Agent approving its own Proposal, and harder to
/// see, because a number looks like evidence in a way a self-approval does not.
///
/// The two origins never merge. A merged figure is one nobody can reproduce: half in a ledger and
/// half in an Operator's memory of an invoice. Two records, cited separately, keep both
/// reproducible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendOrigin {
    /// Beacon totalled its own Invocation records.
    DaemonLedger,
    /// An Operator recorded an out-of-band figure -- an invoice, a provider console.
    OperatorDeclared,
}

impl SpendOrigin {
    fn tag(self) -> u8 {
        match self {
            Self::DaemonLedger => TAG_ORIGIN_DAEMON_LEDGER,
            Self::OperatorDeclared => TAG_ORIGIN_OPERATOR_DECLARED,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedSpend> {
        match tag {
            TAG_ORIGIN_DAEMON_LEDGER => Ok(Self::DaemonLedger),
            TAG_ORIGIN_OPERATOR_DECLARED => Ok(Self::OperatorDeclared),
            _ => Err(MalformedSpend::UnknownTag),
        }
    }

    /// The lowercase token this origin is recorded and rendered as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DaemonLedger => "daemon_ledger",
            Self::OperatorDeclared => "operator_declared",
        }
    }
}

/// One priced line: how much, of what unit.
///
/// The unit is stored exactly as measured -- never case-folded, never normalized (R13.6). `USD` and
/// `usd` are two units, for the reason ADR 0034 refuses to fold a pinned base revision: folding a
/// stored identity is a transformation nobody asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendLine {
    pub unit: String,
    /// Stored integer minor units. Never binary floating point.
    pub amount_minor: i64,
    /// How many decimal places the minor units carry.
    pub minor_units_exponent: u8,
}

/// What was counted, and how much of it.
///
/// `counted_units` is not decoration: "measured 0 over 0 priced units" and "measured 0 over 412
/// priced units" are different claims, and without the count the second is indistinguishable from
/// the first and from a bug.
///
/// `lines` is non-empty, so a zero always names its unit: `0 USD` is a claim, a bare `0` is not.
/// There is deliberately **no cross-unit total**: Beacon holds no rate it could freeze, so summing
/// two units would produce a number no ledger can reproduce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendTotals {
    pub counted_units: u64,
    pub lines: Vec<SpendLine>,
}

/// Why something inside the coverage window was not counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UncountedKind {
    ProviderDidNotPrice,
    InvocationRecordMissing,
    UnitNotConvertible,
    OutsideRetention,
}

impl UncountedKind {
    fn tag(self) -> u8 {
        match self {
            Self::ProviderDidNotPrice => TAG_UNCOUNTED_PROVIDER_DID_NOT_PRICE,
            Self::InvocationRecordMissing => TAG_UNCOUNTED_INVOCATION_RECORD_MISSING,
            Self::UnitNotConvertible => TAG_UNCOUNTED_UNIT_NOT_CONVERTIBLE,
            Self::OutsideRetention => TAG_UNCOUNTED_OUTSIDE_RETENTION,
        }
    }

    fn parse(tag: u8) -> Result<Self, MalformedSpend> {
        match tag {
            TAG_UNCOUNTED_PROVIDER_DID_NOT_PRICE => Ok(Self::ProviderDidNotPrice),
            TAG_UNCOUNTED_INVOCATION_RECORD_MISSING => Ok(Self::InvocationRecordMissing),
            TAG_UNCOUNTED_UNIT_NOT_CONVERTIBLE => Ok(Self::UnitNotConvertible),
            TAG_UNCOUNTED_OUTSIDE_RETENTION => Ok(Self::OutsideRetention),
            _ => Err(MalformedSpend::UnknownTag),
        }
    }

    /// The lowercase token this gap is recorded as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderDidNotPrice => "provider_did_not_price",
            Self::InvocationRecordMissing => "invocation_record_missing",
            Self::UnitNotConvertible => "unit_not_convertible",
            Self::OutsideRetention => "outside_retention",
        }
    }
}

/// One named gap in a partial measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncountedEntry {
    pub kind: UncountedKind,
    pub detail: String,
}

/// What the measurement established, with the amount **inside** the arm that carries one.
///
/// "Absent is never zero" cannot be enforced by a rule; it is enforced by there being nowhere to
/// put a zero. `Unmeasured` and `Broken` have no amount field, so no caller can leave one at its
/// default and no reader can find one.
///
/// The four stay distinct, and so does a fifth state this type cannot express: **no record at all**.
/// That one belongs to the citing document, and collapsing it into `Unmeasured` would turn "nobody
/// counted" into "the spend owner said nobody counted", which is a different, stronger claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendMeasurement {
    /// Every priced unit inside the coverage window was counted. Zero here is a positive claim: a
    /// ledger was read and totalled nothing.
    Measured(SpendTotals),
    /// Counted, with at least one named gap. Not `Measured`, and not `Broken`: the figure is usable
    /// and incomplete, and it says where.
    PartiallyMeasured(SpendTotals, Vec<UncountedEntry>),
    /// Nothing was counted. There is no amount, and this is not zero.
    Unmeasured(String),
    /// Counting was attempted and the ledger for this window is unusable. Not `Unmeasured`: someone
    /// tried.
    Broken(String),
}

impl SpendMeasurement {
    fn tag(&self) -> u8 {
        match self {
            Self::Measured(_) => TAG_MEASURED,
            Self::PartiallyMeasured(_, _) => TAG_PARTIALLY_MEASURED,
            Self::Unmeasured(_) => TAG_UNMEASURED,
            Self::Broken(_) => TAG_BROKEN,
        }
    }

    /// The literal spelling this measurement is recorded and rendered as. Four distinct words, no
    /// synonyms.
    pub fn literal(&self) -> &'static str {
        match self {
            Self::Measured(_) => MEASURED,
            Self::PartiallyMeasured(_, _) => PARTIALLY_MEASURED,
            Self::Unmeasured(_) => UNMEASURED,
            Self::Broken(_) => BROKEN,
        }
    }

    /// The counted totals, when this arm carries any. `None` is not zero.
    pub fn totals(&self) -> Option<&SpendTotals> {
        match self {
            Self::Measured(totals) | Self::PartiallyMeasured(totals, _) => Some(totals),
            Self::Unmeasured(_) | Self::Broken(_) => None,
        }
    }

    /// Why nothing, or not everything, was counted.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Unmeasured(reason) | Self::Broken(reason) => Some(reason),
            _ => None,
        }
    }
}

/// Whether this record replaces an earlier measurement of the same Mission, and which.
///
/// The predecessor is named by its **digest**, not its version number, because a digest is
/// self-proving from the bytes alone while a version needs the store to say what that version was.
/// Every sibling record does the same (ADR 0037, ADR 0038). `None` is an explicit tag, not a
/// missing field: absence is a value here and encodes as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendSupersedes {
    None,
    /// The lowercase 64-character hexadecimal digest of the record this one replaces.
    Record(String),
}

/// The immutable v1 payload, exactly as its canonical bytes decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredSpendV1 {
    pub workspace_id: Uuid,
    pub mission_id: Uuid,
    /// The record's **reference** identity: which record a citation names (R15). The digest is its
    /// **content** identity: what the bytes are. A citation carries both, and a verified reader
    /// refuses a pair that disagrees.
    pub spend_id: Uuid,
    /// Contiguous from 1 per `(workspace, mission, origin)`. A uniqueness constraint in the store,
    /// never an identity in a citation.
    pub version: u32,
    pub origin: SpendOrigin,
    /// A stored UTC millisecond timestamp, never a clock read at read time.
    pub measured_at_ms: u64,
    /// The inclusive event-sequence window this measurement covers. Coverage is declared, never
    /// inferred, and a window that does not span the Mission is the citing collector's refusal --
    /// not this record's, because "partially measured" is the spend owner's word to say.
    pub coverage_from_event_seq: u64,
    pub coverage_to_event_seq: u64,
    pub measurement: SpendMeasurement,
    pub supersedes: SpendSupersedes,
}

// ---------------------------------------------------------------------------------------------
// Refusals -- each names itself
// ---------------------------------------------------------------------------------------------

/// Bytes or field values that cannot be a v1 measured-spend record.
///
/// Every refusal names the rule it broke. "Empty", "too long", "wrong charset" and "out of order"
/// are four different faults, and one name for all four tells a caller nothing about which it hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedSpend {
    Magic,
    Truncated,
    TrailingBytes,
    UnknownTag,
    InvalidUtf8,
    /// The whole record is larger than the envelope the store will hold.
    RecordTooLarge,
    /// A declared length or count larger than its field's limit.
    OverLimitLength,
    EmptyUnit,
    UnitTooLong,
    /// A unit token outside `[A-Za-z0-9._-]`.
    UnitCharset,
    /// Two lines naming one unit. Two amounts for one unit is not a record, it is a question.
    UnitNotUnique,
    /// Lines not in canonical code-point order by unit.
    UnitOrder,
    /// A `measured` or `partially_measured` arm with no lines: a zero that names no unit.
    NoLines,
    TooManyLines,
    MinorUnitsExponent,
    /// A `partially_measured` arm with no named gap. Partial without a gap is `measured`.
    NoUncountedEntry,
    TooManyUncountedEntries,
    EmptyText,
    UntrimmedText,
    TextTooLong,
    /// A control character other than LF or TAB (R13.5's rule, applied to a record's text).
    ControlCharacter,
    /// A coverage window that ends before it starts.
    CoverageRange,
    /// A superseded digest that is not lowercase 64-character hexadecimal.
    SupersedesDigest,
}

impl MalformedSpend {
    /// A stable lowercase reason token. Refusals are named, never spelled differently by two
    /// callers.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Magic => "magic",
            Self::Truncated => "truncated",
            Self::TrailingBytes => "trailing_bytes",
            Self::UnknownTag => "unknown_tag",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::RecordTooLarge => "record_too_large",
            Self::OverLimitLength => "over_limit_length",
            Self::EmptyUnit => "empty_unit",
            Self::UnitTooLong => "unit_too_long",
            Self::UnitCharset => "unit_charset",
            Self::UnitNotUnique => "unit_not_unique",
            Self::UnitOrder => "unit_order",
            Self::NoLines => "no_lines",
            Self::TooManyLines => "too_many_lines",
            Self::MinorUnitsExponent => "minor_units_exponent",
            Self::NoUncountedEntry => "no_uncounted_entry",
            Self::TooManyUncountedEntries => "too_many_uncounted_entries",
            Self::EmptyText => "empty_text",
            Self::UntrimmedText => "untrimmed_text",
            Self::TextTooLong => "text_too_long",
            Self::ControlCharacter => "control_character",
            Self::CoverageRange => "coverage_range",
            Self::SupersedesDigest => "supersedes_digest",
        }
    }
}

impl fmt::Display for MalformedSpend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for MalformedSpend {}

/// A digest that carries its algorithm and canonicalizer identity, never a bare hex string.
///
/// A recorded value alone would leave a reader to assume which function produced it: the assumption
/// these records exist to remove (ADR 0034/0037).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendDigest {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

// ---------------------------------------------------------------------------------------------
// Canonical forms
// ---------------------------------------------------------------------------------------------

/// The comparator every unit token goes through.
///
/// Canonical form compared by Unicode code point, which for UTF-8 is unsigned byte order. Never a
/// locale collation and never a database collation: an ICU primary-strength comparison treats `-`
/// as ignorable and puts `ab` before `a-z`, and this rule puts `a-z` first. A record sorted one way
/// while a store paginates the other is two spellings of one measurement.
///
/// Public because it is the executable form of a rule the ADR states in prose, and a rule stated in
/// two places drifts.
pub fn compare_units(left: &str, right: &str) -> Ordering {
    left.as_bytes().cmp(right.as_bytes())
}

/// A stored currency or metered unit token, exactly as measured.
fn require_unit(value: &str) -> Result<(), MalformedSpend> {
    if value.is_empty() {
        return Err(MalformedSpend::EmptyUnit);
    }
    if value.len() > MAX_UNIT_BYTES {
        return Err(MalformedSpend::UnitTooLong);
    }
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(MalformedSpend::UnitCharset)
    }
}

/// Human text a record may carry.
///
/// LF and TAB are content and are preserved; every other control character is refused by name
/// (R13.5, applied to a record rather than to a document). Whether a *renderer* can carry a line
/// break is that renderer's rule: the Outcome Document refuses one in a table cell, and that
/// constraint belongs to it, not here.
fn require_text(value: &str) -> Result<(), MalformedSpend> {
    if value.is_empty() {
        return Err(MalformedSpend::EmptyText);
    }
    if value.trim() != value {
        return Err(MalformedSpend::UntrimmedText);
    }
    if value.chars().count() > MAX_TEXT_SCALARS {
        return Err(MalformedSpend::TextTooLong);
    }
    if value
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(MalformedSpend::ControlCharacter);
    }
    Ok(())
}

fn digest_bytes(value: &str) -> Result<[u8; 32], MalformedSpend> {
    if value.len() != DIGEST_HEX_CHARS {
        return Err(MalformedSpend::SupersedesDigest);
    }
    let decoded = HEXLOWER
        .decode(value.as_bytes())
        .map_err(|_| MalformedSpend::SupersedesDigest)?;
    let mut bytes = [0_u8; 32];
    if decoded.len() != bytes.len() {
        return Err(MalformedSpend::SupersedesDigest);
    }
    bytes.copy_from_slice(&decoded);
    Ok(bytes)
}

fn require_totals(totals: &SpendTotals) -> Result<(), MalformedSpend> {
    if totals.lines.is_empty() {
        return Err(MalformedSpend::NoLines);
    }
    if totals.lines.len() > MAX_LINES {
        return Err(MalformedSpend::TooManyLines);
    }
    for line in &totals.lines {
        require_unit(&line.unit)?;
        if line.minor_units_exponent > MAX_MINOR_UNITS_EXPONENT {
            return Err(MalformedSpend::MinorUnitsExponent);
        }
    }
    for window in totals.lines.windows(2) {
        match compare_units(&window[0].unit, &window[1].unit) {
            Ordering::Less => {}
            Ordering::Equal => return Err(MalformedSpend::UnitNotUnique),
            Ordering::Greater => return Err(MalformedSpend::UnitOrder),
        }
    }
    Ok(())
}

fn require_uncounted(entries: &[UncountedEntry]) -> Result<(), MalformedSpend> {
    if entries.is_empty() {
        return Err(MalformedSpend::NoUncountedEntry);
    }
    if entries.len() > MAX_UNCOUNTED {
        return Err(MalformedSpend::TooManyUncountedEntries);
    }
    for entry in entries {
        require_text(&entry.detail)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The canonical bytes
// ---------------------------------------------------------------------------------------------

/// Appends canonical bytes. Every length is checked against its field's limit here too, so the
/// encoder and the decoder cannot disagree about what fits.
struct Encoder {
    out: Vec<u8>,
}

impl Encoder {
    fn new() -> Self {
        Self {
            out: MAGIC.to_vec(),
        }
    }

    fn tag(&mut self, tag: u8) {
        self.out.push(tag);
    }

    fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    fn i64(&mut self, value: i64) {
        self.out.extend_from_slice(&value.to_be_bytes());
    }

    fn uuid(&mut self, value: &Uuid) {
        self.out.extend_from_slice(value.as_bytes());
    }

    fn sha256(&mut self, value: &[u8; 32]) {
        self.out.extend_from_slice(value);
    }

    fn string(&mut self, value: &str, limit: usize) -> Result<(), MalformedSpend> {
        if value.len() > limit {
            return Err(MalformedSpend::OverLimitLength);
        }
        let length = u64::try_from(value.len()).map_err(|_| MalformedSpend::OverLimitLength)?;
        self.u64(length);
        self.out.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn count(&mut self, length: usize, limit: usize) -> Result<(), MalformedSpend> {
        if length > limit {
            return Err(MalformedSpend::OverLimitLength);
        }
        let count = u32::try_from(length).map_err(|_| MalformedSpend::OverLimitLength)?;
        self.u32(count);
        Ok(())
    }
}

/// Reads canonical bytes. Self-delimiting: every field proves it fits within the remaining input
/// and its own limit before any of it is consumed, and no declared count reserves capacity.
struct Decoder<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.input.len() - self.offset
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], MalformedSpend> {
        if count > self.remaining() {
            return Err(MalformedSpend::Truncated);
        }
        let slice = &self.input[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn magic(&mut self) -> Result<(), MalformedSpend> {
        if self.take(MAGIC.len()).map_err(|_| MalformedSpend::Magic)? == MAGIC {
            Ok(())
        } else {
            Err(MalformedSpend::Magic)
        }
    }

    fn tag(&mut self) -> Result<u8, MalformedSpend> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, MalformedSpend> {
        let mut value = [0_u8; 4];
        value.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(value))
    }

    fn u64(&mut self) -> Result<u64, MalformedSpend> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(value))
    }

    fn i64(&mut self) -> Result<i64, MalformedSpend> {
        let mut value = [0_u8; 8];
        value.copy_from_slice(self.take(8)?);
        Ok(i64::from_be_bytes(value))
    }

    fn uuid(&mut self) -> Result<Uuid, MalformedSpend> {
        let mut value = [0_u8; 16];
        value.copy_from_slice(self.take(16)?);
        Ok(Uuid::from_bytes(value))
    }

    fn sha256_hex(&mut self) -> Result<String, MalformedSpend> {
        let mut value = [0_u8; 32];
        value.copy_from_slice(self.take(32)?);
        Ok(HEXLOWER.encode(&value))
    }

    fn string(&mut self, limit: usize) -> Result<String, MalformedSpend> {
        let length = self.u64()?;
        let limit = u64::try_from(limit).map_err(|_| MalformedSpend::OverLimitLength)?;
        if length > limit {
            return Err(MalformedSpend::OverLimitLength);
        }
        let length = usize::try_from(length).map_err(|_| MalformedSpend::Truncated)?;
        let bytes = self.take(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| MalformedSpend::InvalidUtf8)
    }

    /// Reads a declared entry count and proves the input could hold that many entries, before the
    /// first entry is read and before any `Vec` reserves for it.
    fn count(&mut self, limit: usize, entry_bytes: usize) -> Result<usize, MalformedSpend> {
        let count = usize::try_from(self.u32()?).map_err(|_| MalformedSpend::OverLimitLength)?;
        if count > limit {
            return Err(MalformedSpend::OverLimitLength);
        }
        let smallest = count
            .checked_mul(entry_bytes)
            .ok_or(MalformedSpend::Truncated)?;
        if smallest > self.remaining() {
            return Err(MalformedSpend::Truncated);
        }
        Ok(count)
    }

    fn finish(self) -> Result<(), MalformedSpend> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(MalformedSpend::TrailingBytes)
        }
    }
}

fn encode_totals(encoder: &mut Encoder, totals: &SpendTotals) -> Result<(), MalformedSpend> {
    require_totals(totals)?;
    encoder.u64(totals.counted_units);
    encoder.count(totals.lines.len(), MAX_LINES)?;
    for line in &totals.lines {
        encoder.string(&line.unit, MAX_UNIT_BYTES)?;
        encoder.i64(line.amount_minor);
        encoder.tag(line.minor_units_exponent);
    }
    Ok(())
}

fn decode_totals(decoder: &mut Decoder<'_>) -> Result<SpendTotals, MalformedSpend> {
    let counted_units = decoder.u64()?;
    let count = decoder.count(MAX_LINES, MIN_LINE_BYTES)?;
    let mut lines = Vec::new();
    for _ in 0..count {
        let unit = decoder.string(MAX_UNIT_BYTES)?;
        let amount_minor = decoder.i64()?;
        let minor_units_exponent = decoder.tag()?;
        lines.push(SpendLine {
            unit,
            amount_minor,
            minor_units_exponent,
        });
    }
    let totals = SpendTotals {
        counted_units,
        lines,
    };
    require_totals(&totals)?;
    Ok(totals)
}

/// The one canonical encoding of a payload.
///
/// Every accepted byte difference changes the record's content identity, so alternate encodings of
/// one parsed value do not exist: this function is the only producer, and every read proves the
/// stored bytes equal what it returns. The field order, widths, and tag values are ADR 0042's
/// table, and this is the executable form of it.
fn encode(payload: &MeasuredSpendV1) -> Result<Vec<u8>, MalformedSpend> {
    if payload.coverage_to_event_seq < payload.coverage_from_event_seq {
        return Err(MalformedSpend::CoverageRange);
    }

    let mut encoder = Encoder::new();
    encoder.uuid(&payload.workspace_id);
    encoder.uuid(&payload.mission_id);
    encoder.uuid(&payload.spend_id);
    encoder.u32(payload.version);
    encoder.tag(payload.origin.tag());
    encoder.u64(payload.measured_at_ms);
    encoder.u64(payload.coverage_from_event_seq);
    encoder.u64(payload.coverage_to_event_seq);

    encoder.tag(payload.measurement.tag());
    match &payload.measurement {
        SpendMeasurement::Measured(totals) => encode_totals(&mut encoder, totals)?,
        SpendMeasurement::PartiallyMeasured(totals, uncounted) => {
            encode_totals(&mut encoder, totals)?;
            require_uncounted(uncounted)?;
            encoder.count(uncounted.len(), MAX_UNCOUNTED)?;
            for entry in uncounted {
                encoder.tag(entry.kind.tag());
                encoder.string(&entry.detail, MAX_TEXT_BYTES)?;
            }
        }
        SpendMeasurement::Unmeasured(reason) | SpendMeasurement::Broken(reason) => {
            require_text(reason)?;
            encoder.string(reason, MAX_TEXT_BYTES)?;
        }
    }

    match &payload.supersedes {
        SpendSupersedes::None => encoder.tag(TAG_SUPERSEDES_NONE),
        SpendSupersedes::Record(digest) => {
            encoder.tag(TAG_SUPERSEDES_RECORD);
            encoder.sha256(&digest_bytes(digest)?);
        }
    }

    if encoder.out.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedSpend::RecordTooLarge);
    }
    Ok(encoder.out)
}

fn decode(input: &[u8]) -> Result<MeasuredSpendV1, MalformedSpend> {
    // Measured before the input is split: a decoder that measures afterwards has already done the
    // work it meant to refuse.
    if input.len() > MAX_CANONICAL_BYTES {
        return Err(MalformedSpend::RecordTooLarge);
    }
    let mut decoder = Decoder::new(input);
    decoder.magic()?;

    let workspace_id = decoder.uuid()?;
    let mission_id = decoder.uuid()?;
    let spend_id = decoder.uuid()?;
    let version = decoder.u32()?;
    let origin = SpendOrigin::parse(decoder.tag()?)?;
    let measured_at_ms = decoder.u64()?;
    let coverage_from_event_seq = decoder.u64()?;
    let coverage_to_event_seq = decoder.u64()?;
    if coverage_to_event_seq < coverage_from_event_seq {
        return Err(MalformedSpend::CoverageRange);
    }

    let measurement = match decoder.tag()? {
        TAG_MEASURED => SpendMeasurement::Measured(decode_totals(&mut decoder)?),
        TAG_PARTIALLY_MEASURED => {
            let totals = decode_totals(&mut decoder)?;
            let count = decoder.count(MAX_UNCOUNTED, MIN_UNCOUNTED_BYTES)?;
            let mut uncounted = Vec::new();
            for _ in 0..count {
                let kind = UncountedKind::parse(decoder.tag()?)?;
                let detail = decoder.string(MAX_TEXT_BYTES)?;
                uncounted.push(UncountedEntry { kind, detail });
            }
            require_uncounted(&uncounted)?;
            SpendMeasurement::PartiallyMeasured(totals, uncounted)
        }
        tag @ (TAG_UNMEASURED | TAG_BROKEN) => {
            let reason = decoder.string(MAX_TEXT_BYTES)?;
            require_text(&reason)?;
            if tag == TAG_UNMEASURED {
                SpendMeasurement::Unmeasured(reason)
            } else {
                SpendMeasurement::Broken(reason)
            }
        }
        _ => return Err(MalformedSpend::UnknownTag),
    };

    let supersedes = match decoder.tag()? {
        TAG_SUPERSEDES_NONE => SpendSupersedes::None,
        TAG_SUPERSEDES_RECORD => SpendSupersedes::Record(decoder.sha256_hex()?),
        _ => return Err(MalformedSpend::UnknownTag),
    };
    decoder.finish()?;

    Ok(MeasuredSpendV1 {
        workspace_id,
        mission_id,
        spend_id,
        version,
        origin,
        measured_at_ms,
        coverage_from_event_seq,
        coverage_to_event_seq,
        measurement,
        supersedes,
    })
}

impl MeasuredSpendV1 {
    /// The one canonical byte encoding of this record.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MalformedSpend> {
        encode(self)
    }

    /// The record's content identity: SHA-256 over its canonical bytes, carrying its own envelope.
    pub fn digest(&self) -> Result<SpendDigest, MalformedSpend> {
        Ok(SpendDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: CANONICALIZER.to_owned(),
            value: HEXLOWER.encode(&sha256(&self.canonical_bytes()?)),
        })
    }

    /// Decode canonical bytes back into a record. Strict: unknown tags, over-limit or invalid
    /// lengths, trailing bytes, invalid UTF-8, non-canonical unit order, and a repeated unit are
    /// all refused.
    pub fn from_canonical_bytes(input: &[u8]) -> Result<Self, MalformedSpend> {
        decode(input)
    }
}
