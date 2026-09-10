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

use std::sync::Arc;

use data_encoding::{BASE64, HEXLOWER};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use uuid::Uuid;

use crate::operator_account::{OperatorAccountError, OperatorAccountService};
use crate::proposal::sha256;

/// The canonicalizer identity. Any future grammar is a new canonicalizer; existing v1 bytes are
/// never reinterpreted under v2 rules.
/// The canonicalizer a NEW record is written under.
pub const CANONICALIZER: &str = CANONICALIZER_V2;

/// The two eras, as names. A record carries one of these and is read under the grammar it names.
pub const CANONICALIZER_V1: &str = "beacon.spend.v1";
pub const CANONICALIZER_V2: &str = "beacon.spend.v2";
/// The literal ASCII magic that starts every record, including its terminating NUL: 16 bytes.
const MAGIC_V1: &[u8] = b"beacon.spend.v1\0";
const MAGIC_V2: &[u8] = b"beacon.spend.v2\0";
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
/// The smallest a line can be, per era. MEASURED, not hand-computed: `min_line_bytes_are_measured`
/// encodes the smallest legal line of each era and asserts these figures, because four hand-typed
/// figures in the Outcome Document module were all wrong when they were finally measured.
/// The smallest a v1 line can be: an eight-byte unit length, one byte of unit (empty units are
/// refused), an `i64` amount and a one-byte exponent. Measured in `spend_attribution.rs` rather
/// than argued for here.
const MIN_LINE_BYTES_V1: usize = 18;
const MIN_LINE_BYTES_V2: usize = MIN_LINE_BYTES_V1 + 4;

const fn min_line_bytes(era: SpendEra) -> usize {
    match era {
        SpendEra::V1 => MIN_LINE_BYTES_V1,
        SpendEra::V2 => MIN_LINE_BYTES_V2,
    }
}
const MIN_UNCOUNTED_BYTES: usize = 9;

// One-byte tags. Assigned explicitly and never renumbered. `supersedes` is the one position where
// 0x00 is assigned, because absence is a value there and must encode as one.
const TAG_ATTRIBUTED: u8 = 0x01;
const TAG_UNATTRIBUTED: u8 = 0x02;
const TAG_NOT_APPLICABLE: u8 = 0x03;

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
pub(crate) const MEASURED: &str = "measured";
pub(crate) const PARTIALLY_MEASURED: &str = "partially_measured";
pub(crate) const UNMEASURED: &str = "unmeasured";
pub(crate) const BROKEN: &str = "broken";

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
///
/// `amount_minor` is **never negative** (R17). The wire type stays `i64` because narrowing it would
/// be a canonicalizer change and would buy nothing the domain check does not already give: the sign
/// is refused as a value, not erased from the grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendLine {
    /// The four dimensions this amount is attributed by (v2). A v1 line reads as `Unattributed`
    /// on all four, because its era had no dimensions to carry.
    pub attribution: LineAttribution,
    pub unit: String,
    /// Stored integer minor units. Never binary floating point.
    pub amount_minor: i64,
    /// How many decimal places the minor units carry.
    pub minor_units_exponent: u8,
}

/// Which era's grammar a record was written in, read from its magic.
///
/// R46.7.1's rule, in this module: the era marker is what licenses a reading. A v1 record has no
/// attribution because its grammar had none — it is read as `Unattributed`, never refused, and
/// never silently upgraded, because inventing `NotApplicable` for it would be a fabrication and
/// refusing it would make every stored measurement unreadable after the bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpendEra {
    V1,
    V2,
}

impl SpendEra {
    /// The canonicalizer name a record of this era carries.
    pub fn canonicalizer(self) -> &'static str {
        match self {
            Self::V1 => CANONICALIZER_V1,
            Self::V2 => CANONICALIZER_V2,
        }
    }
}

/// What a measured line says about ONE of its dimensions. THREE answers, never two.
///
/// `Unattributed` says the dimension applies and nobody could say which; `NotApplicable` says the
/// question does not apply to this measurement at all — an operator invoice has no Route. A
/// nullable column would make those one value, and bullet 25's envelopes would then charge an
/// envelope for spend nobody attributed to it. This is the absent/broken separation R46 made for
/// the read layer, at the point where money is attributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendAttribution<T> {
    /// The dimension is known, and this is it.
    Attributed(T),
    /// The dimension applies; the measurement could not say which. Carries why, so a reader can
    /// tell an unpriced provider from an aggregate invoice.
    Unattributed(String),
    /// The dimension does not apply to this measurement.
    NotApplicable,
}

impl<T> SpendAttribution<T> {
    fn tag(&self) -> u8 {
        match self {
            Self::Attributed(_) => TAG_ATTRIBUTED,
            Self::Unattributed(_) => TAG_UNATTRIBUTED,
            Self::NotApplicable => TAG_NOT_APPLICABLE,
        }
    }

    /// The state and the one value it carries, for the store's projection.
    ///
    /// `not_applicable` is the only state with nothing to carry, so a `None` here means exactly
    /// that -- an `unattributed` row always carries the reason it is unattributed, which is the
    /// difference the wire format spends a length prefix on.
    pub fn projection(&self) -> (&'static str, Option<String>)
    where
        T: fmt::Display,
    {
        match self {
            Self::Attributed(value) => ("attributed", Some(value.to_string())),
            Self::Unattributed(reason) => ("unattributed", Some(reason.clone())),
            Self::NotApplicable => ("not_applicable", None),
        }
    }

    /// The state's NAME, for a reader that reports which of the three a line carried.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Attributed(_) => "attributed",
            Self::Unattributed(_) => "unattributed",
            Self::NotApplicable => "not_applicable",
        }
    }
}

/// The four dimensions a line is attributed by, beside the Mission the record already names.
///
/// On the LINE rather than the record, because "reconciles by X" means totals group by X and one
/// record covers an event window that may span many Residents and models. A per-record field could
/// only say "this whole window was one Agent", which is true of narrow measurements and
/// `Unattributed` for the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineAttribution {
    pub agent: SpendAttribution<Uuid>,
    pub route: SpendAttribution<String>,
    pub provider: SpendAttribution<String>,
    pub model: SpendAttribution<String>,
}

impl LineAttribution {
    /// Every dimension unattributed with one reason: what a v1 line reads as under v2.
    pub fn unattributed(reason: &str) -> Self {
        Self {
            agent: SpendAttribution::Unattributed(reason.to_owned()),
            route: SpendAttribution::Unattributed(reason.to_owned()),
            provider: SpendAttribution::Unattributed(reason.to_owned()),
            model: SpendAttribution::Unattributed(reason.to_owned()),
        }
    }

    /// The four states in declaration order, for a reader that must treat them exhaustively.
    pub fn states(&self) -> [(&'static str, &'static str); 4] {
        [
            ("agent", self.agent.name()),
            ("route", self.route.name()),
            ("provider", self.provider.name()),
            ("model", self.model.name()),
        ]
    }
}

/// What a v1 line's dimensions say, in words, when read under v2.
pub const V1_ERA_REASON: &str = "recorded before spend carried dimensions";

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

/// Why a `supersedes` digest cannot be the predecessor it claims to be (sol1's cold read, #9).
///
/// The predecessor is judged by ITS OWN canonical bytes -- the row is looked up by digest alone,
/// with no Workspace predicate, so that "not here" and "somewhere else" stay two different
/// answers -- and never by the successor's word for it. Shared by the spend and classification
/// ladders, which have the same four ways to be wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredecessorFault {
    /// No stored bytes hash to the named digest.
    NotStored,
    /// A row is keyed by the digest, but its bytes do not verify as a record; `reason` is the
    /// predecessor's own reader's word. Distinct from `NotStored`: something is there, and it is
    /// wrong.
    Unverifiable(String),
    /// The predecessor's own bytes name another Workspace.
    InAnotherWorkspace,
    /// The predecessor's own bytes are about another Mission.
    ForAnotherMission,
}

impl fmt::Display for PredecessorFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStored => formatter.write_str("not_stored"),
            // kimi's boundary note (D-kimi-review-r284d): `reason` is NOT a name by construction
            // -- it is built from `SpendStoreError::Malformed(reason).to_string()` and its
            // classification twin (spend.rs:1130, mission_impact.rs:902), which carry a decoder's
            // free text. Since this fault is interpolated into `ClassificationStateFault::Row`'s
            // one-line message, printing the text here puts it in a startup refusal. The NAME is
            // the line; the text is in `Debug`, which this enum derives.
            Self::Unverifiable(_) => formatter.write_str("unverifiable"),
            Self::InAnotherWorkspace => formatter.write_str("in_another_workspace"),
            Self::ForAnotherMission => formatter.write_str("for_another_mission"),
        }
    }
}

/// The immutable v1 payload, exactly as its canonical bytes decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredSpendV1 {
    /// Which grammar these bytes are written in, and therefore what the record is allowed to say.
    ///
    /// It is a FIELD rather than a decode-time detail because the encoder needs it: a record
    /// decoded from `beacon.spend.v1` bytes has to re-encode to those same bytes, and the era is
    /// the only thing that distinguishes them from a v2 record whose dimensions all happen to be
    /// unattributed. Without it the round trip that every verified read performs would refuse
    /// every v1 row -- calling an honest older record malformed (R46.7.1).
    pub era: SpendEra,
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
    /// A negative amount. A credit, refund or reversal is a later record kind with its own
    /// identity, never a sign bit on a measurement (R17).
    NegativeAmount,
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
    /// A record in the v1 era carrying a dimension its grammar has nowhere to put. Refused rather
    /// than dropped: a caller who wrote four answered dimensions and got bytes back that say
    /// nothing would never learn that the record cannot hold them.
    EraCannotCarryDimensions,
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
            Self::NegativeAmount => "negative_amount",
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
            Self::EraCannotCarryDimensions => "era_cannot_carry_dimensions",
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
        // R17: a measurement has no sign. A credit, refund or reversal is a later record kind with
        // its own identity, and putting one here as a negative line would let a Mission render
        // cheaper than free -- and would net against a positive line, which is arithmetic no ledger
        // performed. The check is per line because there is no total to net against.
        if line.amount_minor < 0 {
            return Err(MalformedSpend::NegativeAmount);
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
    /// The grammar being written. Carried here so `encode_totals` can ask, rather than being
    /// threaded through every helper that does not care.
    era: SpendEra,
}

impl Encoder {
    fn new(era: SpendEra) -> Self {
        Self {
            out: match era {
                SpendEra::V1 => MAGIC_V1.to_vec(),
                SpendEra::V2 => MAGIC_V2.to_vec(),
            },
            era,
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

    /// Read the magic and say WHICH era it names.
    ///
    /// The era is a wire fact (R54.1): it is read from the bytes, never inferred from a column or
    /// passed in by a caller who might be wrong about it.
    fn magic(&mut self) -> Result<SpendEra, MalformedSpend> {
        let magic = self
            .take(MAGIC_V2.len())
            .map_err(|_| MalformedSpend::Magic)?;
        if magic == MAGIC_V2 {
            Ok(SpendEra::V2)
        } else if magic == MAGIC_V1 {
            Ok(SpendEra::V1)
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

/// A Route, provider or model name: the same token shape a unit carries, for the same reason —
/// a stored identity is never folded, and a charset rule is what keeps two spellings from becoming
/// one dimension.
fn require_dimension_token(value: &str) -> Result<(), MalformedSpend> {
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

/// A v1 line can only say one thing about its dimensions: that its era did not record them.
fn require_v1_attribution(attribution: &LineAttribution) -> Result<(), MalformedSpend> {
    let era_reason = |state: &SpendAttribution<String>| matches!(state, SpendAttribution::Unattributed(reason) if reason == V1_ERA_REASON);
    let agent_is_era_reason = matches!(
        &attribution.agent,
        SpendAttribution::Unattributed(reason) if reason == V1_ERA_REASON
    );
    if agent_is_era_reason
        && era_reason(&attribution.route)
        && era_reason(&attribution.provider)
        && era_reason(&attribution.model)
    {
        Ok(())
    } else {
        Err(MalformedSpend::EraCannotCarryDimensions)
    }
}

/// One dimension: its tag, then whatever that arm carries.
fn encode_attribution<T>(
    encoder: &mut Encoder,
    attribution: &SpendAttribution<T>,
    value: impl FnOnce(&mut Encoder, &T) -> Result<(), MalformedSpend>,
) -> Result<(), MalformedSpend> {
    encoder.tag(attribution.tag());
    match attribution {
        SpendAttribution::Attributed(inner) => value(encoder, inner),
        SpendAttribution::Unattributed(reason) => {
            require_text(reason)?;
            encoder.string(reason, MAX_TEXT_BYTES)
        }
        // Carries nothing: "the question does not apply" has no value to give.
        SpendAttribution::NotApplicable => Ok(()),
    }
}

fn encode_line_attribution(
    encoder: &mut Encoder,
    attribution: &LineAttribution,
) -> Result<(), MalformedSpend> {
    encode_attribution(encoder, &attribution.agent, |encoder, id| {
        encoder.uuid(id);
        Ok(())
    })?;
    for token in [
        &attribution.route,
        &attribution.provider,
        &attribution.model,
    ] {
        encode_attribution(encoder, token, |encoder, value| {
            require_dimension_token(value)?;
            encoder.string(value, MAX_UNIT_BYTES)
        })?;
    }
    Ok(())
}

fn encode_totals(encoder: &mut Encoder, totals: &SpendTotals) -> Result<(), MalformedSpend> {
    require_totals(totals)?;
    encoder.u64(totals.counted_units);
    encoder.count(totals.lines.len(), MAX_LINES)?;
    let era = encoder.era;
    for line in &totals.lines {
        match era {
            // The v1 grammar has nowhere to put a dimension. A v1 record therefore carries the one
            // attribution its era can mean -- unattributed, for that reason -- and anything else is
            // refused BY NAME rather than dropped. Dropping it would let a caller write four
            // answered dimensions, get bytes back that say nothing, and never learn.
            SpendEra::V1 => require_v1_attribution(&line.attribution)?,
            SpendEra::V2 => encode_line_attribution(encoder, &line.attribution)?,
        }
        encoder.string(&line.unit, MAX_UNIT_BYTES)?;
        encoder.i64(line.amount_minor);
        encoder.tag(line.minor_units_exponent);
    }
    Ok(())
}

/// One dimension, read back. The tag decides what follows, and an unknown tag is refused rather
/// than read as an absence.
fn decode_attribution<T>(
    decoder: &mut Decoder<'_>,
    value: impl FnOnce(&mut Decoder<'_>) -> Result<T, MalformedSpend>,
) -> Result<SpendAttribution<T>, MalformedSpend> {
    match decoder.tag()? {
        TAG_ATTRIBUTED => Ok(SpendAttribution::Attributed(value(decoder)?)),
        TAG_UNATTRIBUTED => {
            let reason = decoder.string(MAX_TEXT_BYTES)?;
            require_text(&reason)?;
            Ok(SpendAttribution::Unattributed(reason))
        }
        TAG_NOT_APPLICABLE => Ok(SpendAttribution::NotApplicable),
        _ => Err(MalformedSpend::UnknownTag),
    }
}

fn decode_line_attribution(decoder: &mut Decoder<'_>) -> Result<LineAttribution, MalformedSpend> {
    let agent = decode_attribution(decoder, |decoder| decoder.uuid())?;
    let mut token = || {
        decode_attribution(decoder, |decoder| {
            let value = decoder.string(MAX_UNIT_BYTES)?;
            require_dimension_token(&value)?;
            Ok(value)
        })
    };
    Ok(LineAttribution {
        agent,
        route: token()?,
        provider: token()?,
        model: token()?,
    })
}

fn decode_totals(decoder: &mut Decoder<'_>, era: SpendEra) -> Result<SpendTotals, MalformedSpend> {
    let counted_units = decoder.u64()?;
    let count = decoder.count(MAX_LINES, min_line_bytes(era))?;
    let mut lines = Vec::new();
    for _ in 0..count {
        // R46.7.1's era rule: a v1 line carries no dimensions, so it READS as unattributed. Never
        // refused (its era was honest), never `NotApplicable` (that would invent a fact).
        let attribution = match era {
            SpendEra::V1 => LineAttribution::unattributed(V1_ERA_REASON),
            SpendEra::V2 => decode_line_attribution(decoder)?,
        };
        let unit = decoder.string(MAX_UNIT_BYTES)?;
        let amount_minor = decoder.i64()?;
        let minor_units_exponent = decoder.tag()?;
        lines.push(SpendLine {
            attribution,
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

    let mut encoder = Encoder::new(payload.era);
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
    let era = decoder.magic()?;

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
        TAG_MEASURED => SpendMeasurement::Measured(decode_totals(&mut decoder, era)?),
        TAG_PARTIALLY_MEASURED => {
            let totals = decode_totals(&mut decoder, era)?;
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
        era,
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
            canonicalizer: self.era.canonicalizer().to_owned(),
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

// ---------------------------------------------------------------------------------------------
// The store and its verified reader (slice 2)
// ---------------------------------------------------------------------------------------------

/// Which projection disagreed with the canonical bytes, and how.
///
/// Every arm names a different repair. A record column that drifted is one fault; a line projection
/// with the wrong number of rows is another; a single dimension flipped from one SQL-valid state to
/// another is a third, and it is the one an immutability trigger cannot prevent -- a trigger can be
/// dropped, edited around and recreated, leaving a schema that is again complete and a store that
/// holds two answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnDisagreement {
    /// A column on the record's own row.
    Record { column: &'static str },
    /// The line projection holds a different number of rows than the record has lines.
    LineCount { stored: usize, in_bytes: usize },
    /// One field of one projected line, named by both.
    Line {
        line_index: usize,
        field: &'static str,
    },
    /// A v1 record with line rows at all. Its era recorded no dimensions, so it projects none.
    LinesInV1Era { stored: usize },
}

impl fmt::Display for ColumnDisagreement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Record { column } => write!(formatter, "record column {column}"),
            Self::LineCount { stored, in_bytes } => {
                write!(formatter, "{stored} line rows for {in_bytes} lines")
            }
            Self::Line { line_index, field } => write!(formatter, "line {line_index} {field}"),
            Self::LinesInV1Era { stored } => write!(formatter, "{stored} line rows on a v1 record"),
        }
    }
}

/// Why a stored measured-spend record could not be returned.
///
/// Every arm is an integrity failure, not an honest domain state. A digest mismatch, a projection
/// that disagrees with the bytes, a cross-Workspace reference or a citation whose reference identity
/// and content identity disagree all **abort**; none of them becomes a `broken` measurement, because
/// `broken` is a statement about a ledger and these are statements about the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendStoreError {
    /// The stored bytes do not hash to the digest the row is keyed by.
    DigestMismatch,
    /// The stored bytes are not a v1 record.
    Malformed(MalformedSpend),
    /// A projection disagrees with the canonical bytes. The bytes win, and the read refuses: a
    /// column is for querying and never for claiming. The payload says WHICH projection and how --
    /// "a column is wrong" and "line 2's model state is wrong" are two different repairs.
    ColumnsDisagree(ColumnDisagreement),
    /// A citation named this digest with a different `spend_id` or `version` (R15).
    CitedIdentityMismatch,
    /// The row belongs to another Workspace.
    CrossWorkspace,
    /// The record it says it supersedes is not the predecessor it claims (sol1's cold read, #9):
    /// judged by the predecessor's own bytes, never by the successor's word.
    Predecessor {
        digest: String,
        fault: PredecessorFault,
    },
    Store(String),
}

impl fmt::Display for SpendStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DigestMismatch => formatter.write_str("digest_mismatch"),
            Self::Malformed(reason) => write!(formatter, "malformed: {reason}"),
            Self::ColumnsDisagree(detail) => write!(formatter, "columns_disagree: {detail}"),
            Self::CitedIdentityMismatch => formatter.write_str("cited_identity_mismatch"),
            Self::CrossWorkspace => formatter.write_str("cross_workspace"),
            Self::Predecessor { digest, fault } => {
                write!(formatter, "predecessor {digest}: {fault}")
            }
            Self::Store(reason) => write!(formatter, "store: {reason}"),
        }
    }
}

impl std::error::Error for SpendStoreError {}

/// One stored record: its bytes, the identity of those bytes, and what they decode to.
///
/// The fields are private for the reason ADR 0034 gives: a caller-supplied parsed object beside a
/// digest permits the two to disagree. These three were proved equal by the reader that built them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeasuredSpend {
    digest: SpendDigest,
    canonical_bytes: Vec<u8>,
    payload: MeasuredSpendV1,
}

impl MeasuredSpend {
    pub fn digest(&self) -> &SpendDigest {
        &self.digest
    }

    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }

    pub fn payload(&self) -> &MeasuredSpendV1 {
        &self.payload
    }
}

/// Does every projection column say what the bytes say?
///
/// Each column is re-derived rather than compared to a remembered value, so adding a column without
/// adding a line here is a compile-time hole rather than a silent one -- the shape `ruling.rs`
/// established.
/// Every column on the record's own row, against the bytes. Returns the FIRST that disagrees, by
/// name: a caller told only that "columns disagree" has to diff the row by hand to find out which.
fn spend_record_disagreement(
    row: &StoredSpendRow,
    payload: &MeasuredSpendV1,
) -> Option<ColumnDisagreement> {
    // The canonicalizer column is a projection like any other, and now that two are admitted it is
    // one a row can be wrong about. A row claiming v1 over v2 bytes would otherwise be handed back
    // with a digest envelope naming a function that did not produce it.
    let column = if row.digest_canonicalizer != payload.era.canonicalizer() {
        "digest_canonicalizer"
    } else if row.spend_id != payload.spend_id {
        "spend_id"
    } else if row.mission_id != payload.mission_id {
        "mission_id"
    } else if row.version != payload.version {
        "version"
    } else if row.origin != payload.origin.as_str() {
        "origin"
    } else if row.measurement != payload.measurement.literal() {
        "measurement"
    } else if row.coverage_from_event_seq != payload.coverage_from_event_seq {
        "coverage_from_event_seq"
    } else if row.coverage_to_event_seq != payload.coverage_to_event_seq {
        "coverage_to_event_seq"
    } else if row.measured_at_ms != payload.measured_at_ms {
        "measured_at_ms"
    } else {
        return None;
    };
    Some(ColumnDisagreement::Record { column })
}

/// One row of the line projection, as stored.
struct StoredSpendLineRow {
    /// Where the row says it sits. Kept rather than inferred: `ORDER BY line_index` puts the rows
    /// in their stored order, which is not the same as proving each one is at the position the
    /// record gives it (sol1's re-check of the fold). A single line filed at index 1 sorts exactly
    /// as it would at index 0.
    line_index: i64,
    unit: String,
    amount_minor: i64,
    minor_units_exponent: u8,
    states: [(String, Option<String>); 4],
}

/// The complete line projection for one record, in `line_index` order.
///
/// Complete and ordered on purpose: a comparison that stopped at the rows it happened to find could
/// not tell a missing row from a reordered one, and both are ways for the projection to say
/// something the bytes do not.
fn load_spend_line_rows(
    connection: &Connection,
    digest_value: &str,
) -> Result<Vec<StoredSpendLineRow>, SpendStoreError> {
    let store = |error: rusqlite::Error| SpendStoreError::Store(error.to_string());
    let mut statement = connection
        .prepare(
            "SELECT line_index, unit, amount_minor, minor_units_exponent,
                agent_state, agent_value, route_state, route_value,
                provider_state, provider_value, model_state, model_value
         FROM measured_spend_lines WHERE digest_value = ?1 ORDER BY line_index",
        )
        .map_err(store)?;
    let rows = statement
        .query_map(rusqlite::params![digest_value], |row| {
            Ok(StoredSpendLineRow {
                line_index: row.get(0)?,
                unit: row.get(1)?,
                amount_minor: row.get(2)?,
                minor_units_exponent: row.get::<_, i64>(3)? as u8,
                states: [
                    (row.get(4)?, row.get(5)?),
                    (row.get(6)?, row.get(7)?),
                    (row.get(8)?, row.get(9)?),
                    (row.get(10)?, row.get(11)?),
                ],
            })
        })
        .map_err(store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(store)?;
    Ok(rows)
}

/// The line projection, against the bytes it was derived from.
///
/// This is the check an immutability trigger cannot stand in for. A trigger refuses an UPDATE; it
/// does not survive being dropped, edited around and recreated, and a store in that state is again
/// schema-complete while holding two different answers about who spent what. R46: a verified reader
/// refuses a SQL-valid corrupt store rather than returning one of its two answers.
fn spend_line_disagreement(
    connection: &Connection,
    digest_value: &str,
    payload: &MeasuredSpendV1,
) -> Result<Option<ColumnDisagreement>, SpendStoreError> {
    let stored = load_spend_line_rows(connection, digest_value)?;

    // A v1 record recorded no dimensions, so it projects no rows -- in either direction. Rows on a
    // v1 record are not "extra data", they are dimensions its era could not have known.
    if payload.era == SpendEra::V1 {
        return Ok(
            (!stored.is_empty()).then_some(ColumnDisagreement::LinesInV1Era {
                stored: stored.len(),
            }),
        );
    }

    let empty = Vec::new();
    let lines = payload
        .measurement
        .totals()
        .map_or(&empty, |totals| &totals.lines);
    if stored.len() != lines.len() {
        return Ok(Some(ColumnDisagreement::LineCount {
            stored: stored.len(),
            in_bytes: lines.len(),
        }));
    }

    for (line_index, (row, line)) in stored.iter().zip(lines).enumerate() {
        let named = |field| Some(ColumnDisagreement::Line { line_index, field });
        // WHERE the row sits, before WHAT it says. `ORDER BY line_index` returns the rows in
        // stored order; it does not prove any of them is at the position the record gives it, and
        // a row that answered for the wrong line would be compared against the wrong bytes. The
        // index is named by the CANONICAL position -- the line the record has -- because that is
        // the line whose projection is wrong.
        if row.line_index != i64::try_from(line_index).unwrap_or(i64::MAX) {
            return Ok(named("line_index"));
        }
        if row.unit != line.unit {
            return Ok(named("unit"));
        }
        if row.amount_minor != line.amount_minor {
            return Ok(named("amount_minor"));
        }
        if row.minor_units_exponent != line.minor_units_exponent {
            return Ok(named("minor_units_exponent"));
        }
        // Both halves of every dimension: a state that agrees with a value that does not is still
        // two answers, and the pairing CHECK inside the row cannot see the bytes.
        let (agent_state, agent_value) = line.attribution.agent.projection();
        let (route_state, route_value) = line.attribution.route.projection();
        let (provider_state, provider_value) = line.attribution.provider.projection();
        let (model_state, model_value) = line.attribution.model.projection();
        let expected = [
            ("agent", agent_state, agent_value),
            ("route", route_state, route_value),
            ("provider", provider_state, provider_value),
            ("model", model_state, model_value),
        ];
        for (stored_pair, (field, state, value)) in row.states.iter().zip(expected) {
            if stored_pair.0 != state || stored_pair.1 != value {
                return Ok(named(field));
            }
        }
    }
    Ok(None)
}

struct StoredSpendRow {
    digest_value: String,
    digest_canonicalizer: String,
    canonical_bytes: Vec<u8>,
    spend_id: Uuid,
    mission_id: Uuid,
    version: u32,
    origin: String,
    measurement: String,
    coverage_from_event_seq: u64,
    coverage_to_event_seq: u64,
    measured_at_ms: u64,
}

fn load_spend_row(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<StoredSpendRow>, SpendStoreError> {
    connection
        .query_row(
            "SELECT digest_value, canonical_bytes, spend_id, mission_id, version, origin,
                    measurement, coverage_from_event_seq, coverage_to_event_seq, measured_at_ms,
                    digest_canonicalizer
             FROM measured_spend WHERE workspace_id = ?1 AND digest_value = ?2",
            rusqlite::params![workspace_id.to_string(), digest_value],
            |row| {
                Ok(StoredSpendRow {
                    digest_value: row.get(0)?,
                    canonical_bytes: row.get(1)?,
                    spend_id: uuid_column("spend_id", row.get::<_, String>(2)?)?,
                    mission_id: uuid_column("mission_id", row.get::<_, String>(3)?)?,
                    version: row.get::<_, i64>(4)? as u32,
                    origin: row.get(5)?,
                    measurement: row.get(6)?,
                    coverage_from_event_seq: row.get::<_, i64>(7)? as u64,
                    coverage_to_event_seq: row.get::<_, i64>(8)? as u64,
                    measured_at_ms: row.get::<_, i64>(9)? as u64,
                    digest_canonicalizer: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|error| SpendStoreError::Store(error.to_string()))
}

/// A UUID column that cannot parse is a corrupt store, and the reader that follows refuses on the
/// projection comparison rather than here, so one refusal names one fault.
/// A stored identity column, or a typed refusal NAMING the column (opus2's nil lane, ONI1 §4).
///
/// This returned `Uuid::nil()` on a corrupt column and left `spend_columns_match` to refuse on the
/// projection comparison. That works only while no payload can hold a nil id -- and `spend_id` is
/// decoded straight from canonical bytes, where sixteen zero bytes decode to nil. At
/// `payload.spend_id == nil` the corrupt column compared EQUAL and the comparison the design leans
/// on stopped working. A sentinel that can collide with a real value is not a sentinel.
///
/// The raw text rides in `Debug` and never in the one-line message (R28.4).
fn uuid_column(column: &'static str, value: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&value).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(SpendColumnNotUuid { column, raw: value }),
        )
    })
}

/// A stored identity column that is not a UUID. Names the column; the raw text is in `Debug`.
struct SpendColumnNotUuid {
    column: &'static str,
    raw: String,
}

/// Hand-written rather than derived: a derived `Debug` does not count as a READ of `raw`, so the
/// compiler called the field dead -- and it is the whole reason the struct exists.
impl fmt::Debug for SpendColumnNotUuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpendColumnNotUuid")
            .field("column", &self.column)
            .field("raw", &self.raw)
            .finish()
    }
}

impl fmt::Display for SpendColumnNotUuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "the {} column is not a UUID", self.column)
    }
}

impl std::error::Error for SpendColumnNotUuid {}

/// One measured-spend record, verified, by digest within a Workspace.
///
/// The shape `verified_proposal_within` and `verified_ruling_within` already have. `Ok(None)` is
/// "no such record in this Workspace" and is a different answer from a refusal -- and it is also
/// what a cross-Workspace digest returns, because the Workspace is a predicate of the query rather
/// than a field compared afterwards: a row that exists in another Workspace is not visible here at
/// all.
pub fn verified_spend_within(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<MeasuredSpend>, SpendStoreError> {
    let Some(row) = load_spend_row(connection, workspace_id, digest_value)? else {
        return Ok(None);
    };
    // 1. The bytes hash to the identity the row is keyed by.
    if HEXLOWER.encode(&sha256(&row.canonical_bytes)) != row.digest_value {
        return Err(SpendStoreError::DigestMismatch);
    }
    // 2. They strictly decode, and re-encode to exactly themselves.
    let payload = decode(&row.canonical_bytes).map_err(SpendStoreError::Malformed)?;
    if encode(&payload).map_err(SpendStoreError::Malformed)? != row.canonical_bytes {
        return Err(SpendStoreError::Malformed(MalformedSpend::TrailingBytes));
    }
    // 3. Every projection says what the bytes say.
    // A nil identity is not a value any honest record carries, and it is exactly what the old
    // column sentinel decoded to -- so refuse it BY NAME here, in the reader, before the
    // projection comparison that used to be the only thing standing between the two.
    if payload.spend_id.is_nil() || payload.mission_id.is_nil() {
        return Err(SpendStoreError::CitedIdentityMismatch);
    }
    if let Some(detail) = spend_record_disagreement(&row, &payload) {
        return Err(SpendStoreError::ColumnsDisagree(detail));
    }
    // ...and the line projection, which is derived from these same bytes and is just as capable of
    // drifting from them (sol1's cold read of B39).
    if let Some(detail) = spend_line_disagreement(connection, &row.digest_value, &payload)? {
        return Err(SpendStoreError::ColumnsDisagree(detail));
    }
    // 4. And the record's own Workspace is the one that was asked for -- keyed on the field in the
    //    bytes, not on the column the query matched.
    if payload.workspace_id != workspace_id {
        return Err(SpendStoreError::CrossWorkspace);
    }
    // 5. The record it says it supersedes is a record of this Mission, in this Workspace -- by the
    //    predecessor's OWN bytes (sol1's cold read, #9). On every read, like the checks above: a
    //    row written past the Command is still refused.
    if let Some((digest, fault)) = predecessor_fault(connection, &payload)? {
        return Err(SpendStoreError::Predecessor { digest, fault });
    }
    Ok(MeasuredSpend {
        digest: SpendDigest {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            // The era in the BYTES, not the build's newest name: a digest reported under a
            // canonicalizer that did not produce it names nothing (ADR 0034/0037).
            canonicalizer: payload.era.canonicalizer().to_owned(),
            value: row.digest_value,
        },
        canonical_bytes: row.canonical_bytes,
        payload,
    }
    .into())
}

/// The fault, if any, in a record's `supersedes` link, judged by the predecessor's own bytes.
///
/// Looked up by digest ALONE: a Workspace predicate would make a foreign predecessor look absent,
/// and "absent" and "somewhere else" are two different faults. The predecessor's bytes are hashed
/// and decoded here rather than trusted, so a corrupt predecessor row is named as such and never
/// read as a scope.
fn predecessor_fault(
    connection: &Connection,
    payload: &MeasuredSpendV1,
) -> Result<Option<(String, PredecessorFault)>, SpendStoreError> {
    let SpendSupersedes::Record(digest_value) = &payload.supersedes else {
        return Ok(None);
    };
    let stored: Option<Vec<u8>> = connection
        .query_row(
            "SELECT canonical_bytes FROM measured_spend WHERE digest_value = ?1",
            rusqlite::params![digest_value],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| SpendStoreError::Store(error.to_string()))?;
    let fault = match stored {
        None => PredecessorFault::NotStored,
        Some(bytes) if HEXLOWER.encode(&sha256(&bytes)) != *digest_value => {
            PredecessorFault::Unverifiable(SpendStoreError::DigestMismatch.to_string())
        }
        Some(bytes) => match decode(&bytes) {
            Err(reason) => {
                PredecessorFault::Unverifiable(SpendStoreError::Malformed(reason).to_string())
            }
            Ok(predecessor) if predecessor.workspace_id != payload.workspace_id => {
                PredecessorFault::InAnotherWorkspace
            }
            Ok(predecessor) if predecessor.mission_id != payload.mission_id => {
                PredecessorFault::ForAnotherMission
            }
            Ok(_) => return Ok(None),
        },
    };
    Ok(Some((digest_value.clone(), fault)))
}

/// The reader a citation uses: `(workspace_id, spend_id, version, digest)`.
///
/// R15: the digest is content identity and the UUID is reference identity, and **a citation whose
/// pair disagrees is refused**. The comparison reads both from the decoded canonical bytes, never
/// from the projection columns -- a decoy row with the right id and the wrong bytes fails at the
/// digest, and one with the right bytes and a lying column fails at `columns_disagree` before this
/// check is reached.
pub fn verified_spend_for_citation(
    connection: &Connection,
    workspace_id: Uuid,
    spend_id: Uuid,
    version: u32,
    digest_value: &str,
) -> Result<Option<MeasuredSpend>, SpendStoreError> {
    let Some(record) = verified_spend_within(connection, workspace_id, digest_value)? else {
        return Ok(None);
    };
    if record.payload.spend_id != spend_id || record.payload.version != version {
        return Err(SpendStoreError::CitedIdentityMismatch);
    }
    Ok(Some(record))
}

// ---------------------------------------------------------------------------------------------
// The write path: one Command, one record, one Event
// ---------------------------------------------------------------------------------------------

/// Why a measured-spend record could not be recorded.
///
/// The account refusal comes first and alone, so an unauthenticated caller learns nothing about the
/// bytes it supplied. Everything after it names one fault: bytes that are not a v1 record, bytes
/// that decode but do not re-encode to themselves, a record addressed at another Workspace, a
/// Mission that does not exist here, and a record that would take an identity already taken are
/// five different mistakes.
#[derive(Debug)]
pub enum SpendRecordError {
    Account(OperatorAccountError),
    /// The supplied bytes are not a v1 record.
    Malformed(MalformedSpend),
    /// The bytes decode, but re-encoding the decoded value does not reproduce them. ADR 0034's
    /// submit contract: a caller submits bytes, and only bytes that are already canonical are
    /// stored -- otherwise the digest names something the caller never sent.
    NotCanonical,
    /// The record's own `workspace_id` is not this service's Workspace. Read from the bytes, never
    /// from the caller's argument, because the bytes are what the digest covers.
    CrossWorkspace,
    /// `(workspace, mission, origin, version)` is already taken, or `spend_id` is. A correction is
    /// a new record at the next version, never an edit.
    ///
    /// A19.1: this means a TAKEN IDENTITY and nothing else. It used to mean any constraint the
    /// store refused, which made it the answer for a Mission that does not exist -- and a caller
    /// that believed it recorded correction after correction against a Mission that was never
    /// there.
    IdentityTaken,
    /// The record names a Mission this Workspace does not have.
    ///
    /// The store refused the insert on its `(workspace_id, mission_id)` foreign key. That is the
    /// only foreign key this insert can fail: the record's own Workspace is compared against the
    /// service's before any statement runs (`CrossWorkspace` above), and a service exists only
    /// inside a Workspace the store holds. So an unknown Mission is what SQLite reported, and it is
    /// what this says.
    ///
    /// It is not a correctable state. A spend record measures a Mission; naming one that does not
    /// exist is a different record, never a later version of this one.
    MissionUnknown {
        workspace_id: Uuid,
        mission_id: Uuid,
    },
    /// The declared coverage window ends past the last Event this Workspace has. A window is a
    /// claim about a stretch of the store's own history, and the store is the only thing that can
    /// confirm it -- at record time, where `max(sequence)` is a fact. At read time the same question
    /// cannot be answered without re-deriving a past fact from a world that has moved on.
    CoverageBeyondStore {
        claimed_to_event_seq: u64,
        last_event_seq: u64,
    },
    /// The record it says it supersedes is not the predecessor it claims. Refused by the verified
    /// read-back inside the recording transaction -- the row never commits -- and by the reader on
    /// every read after, for the same reason in the same words. There is deliberately no separate
    /// Command-side check: the read-back IS the write-time refusal, and a second copy of it would
    /// be a check no test could fail.
    Predecessor {
        digest: String,
        fault: PredecessorFault,
    },
    Store(String),
}

impl From<OperatorAccountError> for SpendRecordError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

/// A19.1: which constraint the store refused, from SQLite's EXTENDED code.
///
/// `ConstraintViolation` is every constraint -- UNIQUE, PRIMARY KEY, CHECK, NOT NULL and FOREIGN
/// KEY alike -- so mapping the base code to one variant made the owner answer a question it had not
/// asked. The extended code is the one that distinguishes them, and there is no default arm to
/// `IdentityTaken`: a constraint this owner does not recognise is a store fault carrying what
/// SQLite said, never a claim about an identity nobody checked.
///
/// The shape is the crate's own, from the classification owner (`mission_impact.rs`), which drew
/// this distinction when it was written.
fn constraint_refusal(
    error: &rusqlite::Error,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Option<SpendRecordError> {
    let rusqlite::Error::SqliteFailure(inner, _) = error else {
        return None;
    };
    if inner.code != rusqlite::ErrorCode::ConstraintViolation {
        return None;
    }
    match inner.extended_code {
        rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE | rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY => {
            Some(SpendRecordError::IdentityTaken)
        }
        rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY => Some(SpendRecordError::MissionUnknown {
            workspace_id,
            mission_id,
        }),
        _ => None,
    }
}

impl From<rusqlite::Error> for SpendRecordError {
    /// Without the record in hand this cannot name the Mission, so it never guesses one: the
    /// identity refusals still map here, and everything else is a store fault. The insert path
    /// calls [`constraint_refusal`] with the record it is inserting, which is where the foreign-key
    /// arm becomes reachable.
    fn from(error: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(inner, _) = &error
            && inner.code == rusqlite::ErrorCode::ConstraintViolation
            && matches!(
                inner.extended_code,
                rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                    | rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
            )
        {
            return Self::IdentityTaken;
        }
        Self::Store(error.to_string())
    }
}

impl From<SpendStoreError> for SpendRecordError {
    fn from(error: SpendStoreError) -> Self {
        match error {
            SpendStoreError::Malformed(reason) => Self::Malformed(reason),
            SpendStoreError::Predecessor { digest, fault } => Self::Predecessor { digest, fault },
            other => Self::Store(other.to_string()),
        }
    }
}

impl fmt::Display for SpendRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "account: {error:?}"),
            Self::Malformed(reason) => write!(formatter, "malformed: {reason}"),
            Self::NotCanonical => formatter.write_str("not_canonical"),
            Self::CrossWorkspace => formatter.write_str("cross_workspace"),
            Self::IdentityTaken => formatter.write_str("identity_taken"),
            Self::MissionUnknown {
                workspace_id,
                mission_id,
            } => write!(
                formatter,
                "mission_unknown: workspace {workspace_id} has no mission {mission_id}"
            ),
            Self::CoverageBeyondStore {
                claimed_to_event_seq,
                last_event_seq,
            } => write!(
                formatter,
                "coverage_beyond_store: window ends at {claimed_to_event_seq}, \
                 the last event in this workspace is {last_event_seq}"
            ),
            Self::Predecessor { digest, fault } => {
                write!(formatter, "predecessor {digest}: {fault}")
            }
            Self::Store(reason) => write!(formatter, "store: {reason}"),
        }
    }
}

impl std::error::Error for SpendRecordError {}

/// The `spend.measured` Event payload.
///
/// It carries the record's two identities and its exact bytes, and **no amount**: a consumer that
/// wants the amount reads the record through its verified reader, where the bytes are proved. An
/// amount copied into an Event is a second source that can disagree with the ledger, which is the
/// same reason `MissionCompletionV1` cites spend rather than restating it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpendMeasuredPayload {
    pub workspace_id: String,
    pub spend_id: String,
    pub mission_id: String,
    pub version: u32,
    pub origin: String,
    pub measurement: String,
    pub digest: SpendDigestPayload,
    pub canonical_bytes_base64: String,
    pub measured_at_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpendDigestPayload {
    pub algorithm: String,
    pub canonicalizer: String,
    pub value: String,
}

/// Records measured spend, and reads it back verified.
///
/// There is deliberately **no** `record(payload: MeasuredSpendV1)` and no
/// `record(payload, digest)`: a parsed object beside a digest permits the two to disagree, and the
/// disagreement is invisible at the call site. The only way in is bytes.
pub struct MeasuredSpendService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl MeasuredSpendService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    /// Records one immutable measured-spend record from the exact bytes supplied.
    ///
    /// The value returned is **read back through the verified reader**, never the caller's parse:
    /// what a caller receives is what a later reader will see, or the call fails.
    pub fn record(&self, canonical_bytes: &[u8]) -> Result<MeasuredSpend, SpendRecordError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let payload = decode(canonical_bytes).map_err(SpendRecordError::Malformed)?;
            // Decoded, then re-encoded, then compared to the caller's slice. A decoder alone
            // accepts more than one byte sequence for some values; this is the check that makes the
            // digest an identity for exactly one of them.
            if encode(&payload).map_err(SpendRecordError::Malformed)? != canonical_bytes {
                return Err(SpendRecordError::NotCanonical);
            }
            if payload.workspace_id != self.workspace_id {
                return Err(SpendRecordError::CrossWorkspace);
            }
            let digest_value = HEXLOWER.encode(&sha256(canonical_bytes));

            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            insert_spend(&transaction, &digest_value, canonical_bytes, &payload)?;
            let recorded = verified_spend_within(&transaction, self.workspace_id, &digest_value)?
                .ok_or_else(|| SpendRecordError::Store("record vanished".to_owned()))?;
            transaction.commit()?;
            Ok(recorded)
        })
    }

    /// One record by its content identity, verified.
    pub fn get(&self, digest_value: &str) -> Result<Option<MeasuredSpend>, SpendRecordError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            Ok(verified_spend_within(
                connection,
                self.workspace_id,
                digest_value,
            )?)
        })
    }

    /// Every record for one Mission, each verified, oldest first.
    ///
    /// Daemon and operator records coexist: they are separate `(origin, version)` ladders and are
    /// never merged into one number here or anywhere else.
    pub fn for_mission(&self, mission_id: Uuid) -> Result<Vec<MeasuredSpend>, SpendRecordError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let mut statement = connection.prepare(
                "SELECT digest_value FROM measured_spend
                 WHERE workspace_id = ?1 AND mission_id = ?2
                 ORDER BY origin, version",
            )?;
            let digests = statement
                .query_map(
                    rusqlite::params![self.workspace_id.to_string(), mission_id.to_string()],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let mut records = Vec::with_capacity(digests.len());
            for digest_value in digests {
                // The joined row identified a candidate; the record itself is read verified.
                let record =
                    verified_spend_within(connection, self.workspace_id, &digest_value)?
                        .ok_or_else(|| SpendRecordError::Store("record vanished".to_owned()))?;
                records.push(record);
            }
            Ok(records)
        })
    }
}

/// The row and its Event, written together in one transaction.
///
/// Every projection column is derived from `payload` here, in the one place, so a column cannot be
/// supplied separately from the bytes it is supposed to project.
fn insert_spend(
    connection: &Connection,
    digest_value: &str,
    canonical_bytes: &[u8],
    payload: &MeasuredSpendV1,
) -> Result<(), SpendRecordError> {
    // The window is checked against the store BEFORE the row is written, and against this
    // Workspace's own Events. The record's own `spend.measured` Event is written after this, so a
    // window can never include the moment the measurement was recorded -- it covers work that had
    // already happened, which is the only thing a measurement can honestly cover.
    let last_event_seq: i64 = connection
        .query_row(
            "SELECT coalesce(max(sequence), 0) FROM events WHERE workspace_id = ?1",
            rusqlite::params![payload.workspace_id.to_string()],
            |row| row.get(0),
        )
        .map_err(|error| SpendRecordError::Store(error.to_string()))?;
    let last_event_seq = u64::try_from(last_event_seq).unwrap_or(0);
    if payload.coverage_to_event_seq > last_event_seq {
        return Err(SpendRecordError::CoverageBeyondStore {
            claimed_to_event_seq: payload.coverage_to_event_seq,
            last_event_seq,
        });
    }
    connection
        .execute(
            "INSERT INTO measured_spend (
             workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
             spend_id, mission_id, version, origin, measurement,
             coverage_from_event_seq, coverage_to_event_seq, measured_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                payload.workspace_id.to_string(),
                DIGEST_ALGORITHM,
                payload.era.canonicalizer(),
                digest_value,
                canonical_bytes,
                payload.spend_id.to_string(),
                payload.mission_id.to_string(),
                i64::from(payload.version),
                payload.origin.as_str(),
                payload.measurement.literal(),
                i64::try_from(payload.coverage_from_event_seq)
                    .map_err(|_| SpendRecordError::Store("coverage out of range".to_owned()))?,
                i64::try_from(payload.coverage_to_event_seq)
                    .map_err(|_| SpendRecordError::Store("coverage out of range".to_owned()))?,
                i64::try_from(payload.measured_at_ms)
                    .map_err(|_| SpendRecordError::Store("timestamp out of range".to_owned()))?,
            ],
        )
        // A19.1: this is the one statement whose constraint refusal can name a Mission, because the
        // payload is here. A bare `?` would take the `From` impl, which has no record to name and
        // would report an unknown Mission as a store fault.
        .map_err(|error| {
            constraint_refusal(&error, payload.workspace_id, payload.mission_id)
                .unwrap_or_else(|| SpendRecordError::from(error))
        })?;

    // The projection. Only the two arms that HAVE lines write rows here, and only in the v2 era: a
    // v1 line carries no dimensions, so projecting it would file four rows whose every state is the
    // record's era rather than a fact about spend. A record whose measurement carries no lines
    // writes none either -- and both absences are explained by columns already on the parent row
    // (`digest_canonicalizer`, `measurement`), so an empty child set is never ambiguous.
    if let Some(totals) = payload
        .measurement
        .totals()
        .filter(|_| payload.era == SpendEra::V2)
    {
        for (index, line) in totals.lines.iter().enumerate() {
            let (agent_state, agent_value) = line.attribution.agent.projection();
            let (route_state, route_value) = line.attribution.route.projection();
            let (provider_state, provider_value) = line.attribution.provider.projection();
            let (model_state, model_value) = line.attribution.model.projection();
            connection
                .execute(
                    "INSERT INTO measured_spend_lines (
                     digest_value, line_index, unit, amount_minor, minor_units_exponent,
                     agent_state, agent_value, route_state, route_value,
                     provider_state, provider_value, model_state, model_value
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    rusqlite::params![
                        digest_value,
                        i64::try_from(index).unwrap_or(i64::MAX),
                        line.unit,
                        line.amount_minor,
                        i64::from(line.minor_units_exponent),
                        agent_state,
                        agent_value,
                        route_state,
                        route_value,
                        provider_state,
                        provider_value,
                        model_state,
                        model_value,
                    ],
                )
                .map_err(|error| SpendRecordError::Store(error.to_string()))?;
        }
    }
    let event = serde_json::to_string(&SpendMeasuredPayload {
        workspace_id: payload.workspace_id.to_string(),
        spend_id: payload.spend_id.to_string(),
        mission_id: payload.mission_id.to_string(),
        version: payload.version,
        origin: payload.origin.as_str().to_owned(),
        measurement: payload.measurement.literal().to_owned(),
        digest: SpendDigestPayload {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalizer: payload.era.canonicalizer().to_owned(),
            value: digest_value.to_owned(),
        },
        canonical_bytes_base64: BASE64.encode(canonical_bytes),
        measured_at_ms: payload.measured_at_ms,
    })
    .map_err(|error| SpendRecordError::Store(error.to_string()))?;
    crate::event_chain::append_event(
        connection,
        &payload.workspace_id.to_string(),
        "spend.measured",
        &event,
        i64::try_from(payload.measured_at_ms)
            .map_err(|_| SpendRecordError::Store("timestamp out of range".to_owned()))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod nil_sentinel {
    use super::*;

    /// opus2's ONI1 §4: the nil sentinel could collide with a real payload id.
    ///
    /// `parse_uuid_column` returned `Uuid::nil()` for a corrupt column and relied on
    /// `spend_columns_match` to refuse. `spend_id` is decoded from canonical bytes, sixteen zero
    /// bytes decode to nil, and nothing rejected a nil payload id -- so at `payload.spend_id ==
    /// nil` the corrupt column compared EQUAL and the guard the design leans on stopped working.
    ///
    /// Two independent refusals now stand where one sentinel used to: the column no longer parses
    /// silently, and a nil identity is refused by name in the reader.
    #[test]
    fn a_corrupt_identity_column_refuses_by_name_instead_of_becoming_nil() {
        let refusal = uuid_column("spend_id", "not-a-uuid-at-all".to_owned())
            .expect_err("a corrupt column must not become a value");
        let line = refusal.to_string();
        assert!(
            line.contains("spend_id") || format!("{refusal:?}").contains("spend_id"),
            "the refusal does not name the column: {line} / {refusal:?}"
        );
        assert!(
            !line.contains("not-a-uuid-at-all"),
            "the raw column text reached the one-line message: {line}"
        );
        assert!(
            format!("{refusal:?}").contains("not-a-uuid-at-all"),
            "the raw column text is not reachable through Debug: {refusal:?}"
        );

        // And the honest case still parses.
        let good = Uuid::new_v4();
        assert_eq!(
            uuid_column("spend_id", good.to_string()).expect("an honest column parses"),
            good
        );

        // The collision the sentinel allowed: a nil id is no longer a value either side can hold.
        assert!(
            uuid_column("spend_id", Uuid::nil().to_string()).is_ok(),
            "nil is still a syntactically valid UUID -- the reader is what refuses it"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// R68/H1: the recording Event, proved
// ---------------------------------------------------------------------------------------------

/// Why a spend record did not prove itself WITH its recording Event.
///
/// A distinct type rather than a new arm on [`SpendStoreError`], for the same reason the
/// classification reader has one: a failure mode only this reader can raise belongs beside this
/// reader, not on an enum matched exhaustively across the crate.
#[derive(Debug)]
pub(crate) enum SpendEventProof {
    Store(SpendStoreError),
    /// No `spend.measured` Event in this Workspace names this digest, or more than one does.
    RecordingEventUnaccounted,
    /// The Event exists, and what it carries disagrees with the verified record.
    DivergentEvidence,
}

impl SpendEventProof {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Store(_) => "Store",
            Self::RecordingEventUnaccounted => "RecordingEventUnaccounted",
            Self::DivergentEvidence => "DivergentEvidence",
        }
    }
}

/// The verified spend record AND the `spend.measured` Event that recorded it.
///
/// `verified_spend_within` proves bytes, columns, lines, scope and predecessor and returns without
/// proving the ledger ever recorded it. A packet calling itself independent owner evidence needs
/// the Event too: a digest is never sufficient provenance (R68/H1).
pub(crate) fn verified_spend_with_event_within(
    connection: &Connection,
    workspace_id: Uuid,
    digest_value: &str,
) -> Result<Option<MeasuredSpend>, SpendEventProof> {
    let Some(record) = verified_spend_within(connection, workspace_id, digest_value)
        .map_err(SpendEventProof::Store)?
    else {
        return Ok(None);
    };

    let mut statement = connection
        .prepare(
            "SELECT payload_json FROM events
             WHERE workspace_id = ?1 AND kind = 'spend.measured'
               AND json_extract(payload_json, '$.digest.value') = ?2",
        )
        .map_err(|_| SpendEventProof::RecordingEventUnaccounted)?;
    let payloads: Vec<String> = statement
        .query_map(
            rusqlite::params![workspace_id.to_string(), digest_value],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| SpendEventProof::RecordingEventUnaccounted)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| SpendEventProof::RecordingEventUnaccounted)?;
    let [payload_json] = payloads.as_slice() else {
        return Err(SpendEventProof::RecordingEventUnaccounted);
    };

    let payload: SpendMeasuredPayload =
        serde_json::from_str(payload_json).map_err(|_| SpendEventProof::DivergentEvidence)?;
    // The era decides which canonicalizer these bytes are written in, so the Event's claim is
    // checked against the RECORD's era rather than against whichever spelling is current.
    if payload.workspace_id != workspace_id.to_string()
        || payload.digest.value != *digest_value
        || payload.digest.canonicalizer != record.payload().era.canonicalizer()
        || payload.spend_id != record.payload().spend_id.to_string()
        || payload.mission_id != record.payload().mission_id.to_string()
        || payload.version != record.payload().version
        || payload.canonical_bytes_base64 != BASE64.encode(record.canonical_bytes())
    {
        return Err(SpendEventProof::DivergentEvidence);
    }
    Ok(Some(record))
}
