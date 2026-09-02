//! Operating-system disk-encryption status for the volume holding live data.
//!
//! ADR 0017 relies on operating-system disk encryption for the open live store
//! instead of adding an application-level database cipher. That trade is only
//! honest if Beacon can say which of three things is true — the volume is
//! encrypted by the operating system, it is not, or Beacon could not tell — and
//! can show what it read to reach that answer.
//!
//! The asymmetry below is the whole safety property: nothing reports `Enabled`
//! except a device-mapper crypt UUID this module read itself. Every failure to
//! read reports `Unknown` with its reason. No read failure can ever produce
//! `Enabled` or `Disabled`.
//!
//! Nothing here spawns a process, opens a socket, or writes anything. Every
//! observation is a bounded read of `/proc/self/mountinfo` and `/sys/dev/block`
//! plus one `stat`. Nothing is persisted: the answer is about the host right
//! now, and a stored copy would outlive the condition it describes.
//!
//! Sources. The mount-table format is `proc_pid_mountinfo(5)`: six fixed
//! fields, a variable run of optional fields, a `-` separator, then filesystem
//! type, mount source, and super options. The device-mapper UUID convention
//! (`CRYPT-<TYPE>-<uuid>-<name>` for crypt targets) is written by cryptsetup;
//! `dmsetup(8)` on this host documents that device-mapper devices carry a UUID
//! and that UUIDs are read as subsystem-prefixed constituents, but it does not
//! spell the crypt prefix out. cryptsetup is not installed here, so that prefix
//! is recorded as observed convention rather than a locally citable format —
//! which is why the match is on the generic `CRYPT-` prefix with the type token
//! derived from the UUID, not on an allowlist of known types.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{OperatorAccountError, OperatorAccountService};

const MAX_MOUNTINFO_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MOUNTINFO_ENTRIES: usize = 16_384;
const MAX_MOUNTINFO_OPTIONAL_FIELDS: usize = 64;
const MAX_SYSFS_FILE_BYTES: u64 = 4 * 1024;
const MAX_STACK_DEPTH: usize = 8;
const MAX_STACK_NODES: usize = 64;
const MAX_EVIDENCE_ENTRIES: usize = 32;
const MAX_OBSERVATION_CHARS: usize = 200;
const MAX_CRYPT_TYPE_CHARS: usize = 16;
/// Path components walked upward when deciding absence from brokenness.
const MAX_PATH_ANCESTORS: usize = 32;
/// A path inside a one-sentence conclusion keeps its tail, which is the part
/// that says which device it is.
/// A device or mapping name inside a conclusion. Small enough that the sentence
/// around it always fits, with the count of anything omitted.
/// A hexadecimal run at least this long is treated as an identifier, not text.
const MAX_IDENTIFIER_RUN_CHARS: usize = 12;
/// A subject inside a conclusion, and how many one conclusion may carry. Small
/// enough that the longest template plus the most subjects stays inside the
/// adapter's bound -- asserted by a property test, not assumed.
const MAX_SUBJECT_CHARS: usize = 24;
const MAX_SUBJECTS: usize = 3;
pub const MAX_RENDERED_CONCLUSION_CHARS: usize = 200;

const NOT_VERIFIED_OPAL: &str = "drive-firmware encryption (OPAL or another self-encrypting drive), which Beacon cannot observe";
const NOT_VERIFIED_FSCRYPT: &str = "filesystem-level encryption such as fscrypt or ext4 encryption";
const NOT_VERIFIED_BELOW_HOST: &str =
    "encryption applied beneath this host, such as by a hypervisor or a storage array";
const NOT_VERIFIED_KEY: &str = "the strength, custody, or current lock state of any key";

/// What Beacon proved about operating-system disk encryption for the volume
/// holding this Installation's live data.
///
/// Three states, closed.
///
/// `Enabled`: every branch of the stack was read to a physical leaf and every
/// one of those paths passed through a crypt layer.
///
/// `Disabled`: the same completeness, and at least one of those paths had no
/// crypt layer. It does NOT mean the stack contained no crypt layer — a volume
/// over one encrypted member and one plain one is `Disabled` and its reason
/// names both. It is a proven negative about at least one path to disk, not an
/// absence of proof.
///
/// `Unknown`: anything unproved — a read that failed, a branch that terminated
/// at no physical device, a budget spent. `mechanism` carries the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveDataEncryption {
    Enabled,
    Disabled,
    Unknown,
}

/// One thing Beacon read, and what was there. A later reader re-runs `source`
/// and compares. Observations are bounded and carry no key material: a crypt
/// UUID contributes its type prefix only, never the full identifier.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptionEvidence {
    pub source: String,
    pub observation: String,
    /// True when either field was cut. The interface says "(shortened)" from
    /// this flag, never by looking for an ellipsis -- a redaction marker in one
    /// field made a silent truncation in the other look guarded.
    pub truncated: bool,
}

/// The result of one live reading. Never stored, so every field describes the
/// host at `observed_at_ms` and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveDataEncryptionReport {
    /// The answer. The sentence an operator reads is rendered from this, never
    /// carried as free text something later has to cut.
    pub conclusion: LiveDataEncryptionConclusion,
    /// The volume this report is about, or absent when none was identified.
    pub mount_point: Option<Subject>,
    pub evidence: Vec<EncryptionEvidence>,
    pub not_verified: Vec<String>,
    pub observed_at_ms: u64,
}

/// A value that appears inside a rendered conclusion: a device number, a
/// mapping name, a path tail, an error kind.
///
/// Bounded when it is CONSTRUCTED, not when it is printed. Five rounds of
/// review found the same class in a new shape each time -- a sentence composed
/// from unbounded parts and then cut -- because every fix bounded the strings
/// that had overflowed rather than the type that let them in. A conclusion now
/// holds no strings of its own, so there is nothing left to overflow.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Subject {
    pub role: SubjectRole,
    pub value: String,
    /// True when the value was cut. The interface says so from this flag, never
    /// by looking for an ellipsis in the text.
    pub shortened: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectRole {
    /// A block device, always `major:minor`.
    Device,
    /// A device-mapper mapping name.
    Mapping,
    /// A crypt mechanism, e.g. `dm-crypt (LUKS2)`.
    Mechanism,
    /// A filesystem path, mount point, or filesystem type.
    Location,
    /// A short technical detail, e.g. an I/O error kind.
    Detail,
}

impl Subject {
    fn new(role: SubjectRole, value: &str) -> Self {
        let cleaned: String = redact_identifiers(value)
            .chars()
            .map(|character| {
                if character.is_control() {
                    ' '
                } else {
                    character
                }
            })
            .collect();
        let cleaned = cleaned.trim();
        let shortened = cleaned.chars().count() > MAX_SUBJECT_CHARS;
        let value = if shortened {
            cleaned.chars().take(MAX_SUBJECT_CHARS).collect()
        } else {
            cleaned.to_owned()
        };
        Self {
            role,
            value,
            shortened,
        }
    }

    fn device(device: DeviceNumber) -> Self {
        Self::new(SubjectRole::Device, &device.to_string())
    }

    /// A path keeps its TAIL, which is the part that says which device it is.
    fn path(path: &Path) -> Self {
        let text = redact_identifiers(&path.display().to_string());
        let count = text.chars().count();
        if count <= MAX_SUBJECT_CHARS {
            return Self::new(SubjectRole::Location, &text);
        }
        Self {
            role: SubjectRole::Location,
            value: text.chars().skip(count - MAX_SUBJECT_CHARS).collect(),
            shortened: true,
        }
    }
}

/// Every reason this reader can produce. Closed, so the sentence an operator
/// sees is chosen from a fixed set rather than assembled from whatever the
/// kernel handed back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    CryptCoversEveryPath,
    NoCryptOnAnyPath,
    SomePathsUncovered,
    StorePathUnresolved,
    StoreNotInspectable,
    MountTableUnreadable,
    MountTableUnparsed,
    MountTableEmpty,
    NoMountForStore,
    MountAndStatDisagree,
    MemoryBackedFilesystem,
    OverlayFilesystem,
    UserspaceFilesystem,
    NetworkFilesystem,
    NoBlockDeviceForFilesystem,
    LinkResolvesToNothing,
    DeviceUnreadable,
    ReadOverBudget,
    StackOverBudget,
    NoPhysicalLeaf,
    MappingWithoutBackingDevice,
    LoopbackImage,
    PlatformNotSupported,
}

/// The answer, and only the answer: a closed state, a closed reason, and a
/// bounded list of what the reason is about. No free text.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveDataEncryptionConclusion {
    pub state: LiveDataEncryption,
    pub reason: ReasonCode,
    pub subjects: Vec<Subject>,
    /// How many subjects did not fit. A capped list that does not say it was
    /// capped is a partial answer wearing a complete one -- and a count is
    /// bounded by construction where "and N more" spliced into a sentence was
    /// not.
    pub omitted_subjects: u32,
}

impl LiveDataEncryptionConclusion {
    fn new(state: LiveDataEncryption, reason: ReasonCode, mut subjects: Vec<Subject>) -> Self {
        let omitted_subjects =
            u32::try_from(subjects.len().saturating_sub(MAX_SUBJECTS)).unwrap_or(u32::MAX);
        subjects.truncate(MAX_SUBJECTS);
        Self {
            state,
            reason,
            subjects,
            omitted_subjects,
        }
    }
}

/// The human sentence, rendered from the conclusion by a total function.
///
/// It cannot overflow because its inputs cannot: one fixed template per reason,
/// at most `MAX_SUBJECTS` subjects, each at most `MAX_SUBJECT_CHARS`. The
/// desktop renders the same sentence from the same fields; this one is for logs
/// and for anything that reads a report without an interface.
pub fn render_conclusion(conclusion: &LiveDataEncryptionConclusion) -> String {
    let of = |role: SubjectRole| -> String {
        let named: Vec<String> = conclusion
            .subjects
            .iter()
            .filter(|subject| subject.role == role)
            .map(|subject| {
                if subject.shortened {
                    format!("{}\u{2026}", subject.value)
                } else {
                    subject.value.clone()
                }
            })
            .collect();
        if named.is_empty() {
            "an unnamed subject".to_owned()
        } else {
            named.join(", ")
        }
    };
    let more = match conclusion.omitted_subjects {
        0 => String::new(),
        1 => " and 1 more".to_owned(),
        count => format!(" and {count} more"),
    };
    let sentence = match conclusion.reason {
        ReasonCode::CryptCoversEveryPath => format!(
            "every path to disk passes through {}",
            of(SubjectRole::Mechanism)
        ),
        ReasonCode::NoCryptOnAnyPath => format!(
            "at least one path to disk has no crypt layer: {}",
            of(SubjectRole::Device)
        ),
        ReasonCode::SomePathsUncovered => format!(
            "{} is encrypted; {} is not",
            of(SubjectRole::Mapping),
            of(SubjectRole::Device)
        ),
        ReasonCode::StorePathUnresolved => format!(
            "the live store path could not be resolved: {}",
            of(SubjectRole::Detail)
        ),
        ReasonCode::StoreNotInspectable => "the live store could not be inspected".to_owned(),
        ReasonCode::MountTableUnreadable => "the mount table could not be read".to_owned(),
        ReasonCode::MountTableUnparsed => "the mount table could not be parsed".to_owned(),
        ReasonCode::MountTableEmpty => "the mount table was empty".to_owned(),
        ReasonCode::NoMountForStore => "no mount table entry covers the live store".to_owned(),
        ReasonCode::MountAndStatDisagree => {
            "the mount table and the live store disagree about which volume holds it".to_owned()
        }
        ReasonCode::MemoryBackedFilesystem => {
            "the live store is on a memory-backed filesystem".to_owned()
        }
        ReasonCode::OverlayFilesystem => {
            "the live store is on an overlay filesystem; its layers are decided outside this mount"
                .to_owned()
        }
        ReasonCode::UserspaceFilesystem => "the live store is on a userspace filesystem".to_owned(),
        ReasonCode::NetworkFilesystem => {
            "the live store is on a network filesystem; its encryption is decided off this host"
                .to_owned()
        }
        ReasonCode::NoBlockDeviceForFilesystem => format!(
            "this filesystem ({}) exposes no block device Beacon can inspect",
            of(SubjectRole::Location)
        ),
        ReasonCode::LinkResolvesToNothing => format!(
            "{} is a link that resolves to nothing",
            of(SubjectRole::Location)
        ),
        ReasonCode::DeviceUnreadable => format!(
            "a device in the volume's stack could not be read: {}",
            of(SubjectRole::Location)
        ),
        ReasonCode::ReadOverBudget => format!(
            "{} exceeded Beacon's read budget",
            of(SubjectRole::Location)
        ),
        ReasonCode::StackOverBudget => {
            "the volume's device stack exceeded Beacon's read budget".to_owned()
        }
        ReasonCode::NoPhysicalLeaf => "no physical device terminated the volume's stack".to_owned(),
        ReasonCode::MappingWithoutBackingDevice => format!(
            "mapping {} has no readable backing device",
            of(SubjectRole::Mapping)
        ),
        ReasonCode::LoopbackImage => format!(
            "the volume is a loopback image backed by {}; Beacon does not follow the backing file",
            of(SubjectRole::Location)
        ),
        ReasonCode::PlatformNotSupported => "not detectable on this platform".to_owned(),
    };
    format!("{sentence}{more}")
}

/// One reason with the things it is about, before it becomes a conclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    reason: ReasonCode,
    subjects: Vec<Subject>,
}

impl Verdict {
    fn bare(reason: ReasonCode) -> Self {
        Self {
            reason,
            subjects: Vec::new(),
        }
    }

    fn about(reason: ReasonCode, subjects: Vec<Subject>) -> Self {
        Self { reason, subjects }
    }
}

/// Where the host's mount table and block-device tree live.
///
/// Injected rather than hardcoded so a test can present an unreadable sysfs, a
/// truncated mount table, or a device stack with a cycle — the readings whose
/// wrong answers matter most.
#[derive(Debug, Clone)]
pub struct SystemView {
    mountinfo: PathBuf,
    sys_dev_block: PathBuf,
}

impl SystemView {
    pub fn host() -> Self {
        Self {
            mountinfo: PathBuf::from("/proc/self/mountinfo"),
            sys_dev_block: PathBuf::from("/sys/dev/block"),
        }
    }

    pub fn new(mountinfo: PathBuf, sys_dev_block: PathBuf) -> Self {
        Self {
            mountinfo,
            sys_dev_block,
        }
    }
}

/// Reports operating-system disk-encryption status without changing any
/// operating-system setting, persisting anything, or emitting an event.
pub struct DiskEncryptionService {
    accounts: Arc<OperatorAccountService>,
    store_path: PathBuf,
    system: SystemView,
}

impl DiskEncryptionService {
    pub fn new(accounts: Arc<OperatorAccountService>, store_path: PathBuf) -> Self {
        Self {
            accounts,
            store_path,
            system: SystemView::host(),
        }
    }

    pub fn with_system_view(
        accounts: Arc<OperatorAccountService>,
        store_path: PathBuf,
        system: SystemView,
    ) -> Self {
        Self {
            accounts,
            store_path,
            system,
        }
    }

    /// Reads live state on every call. There is no cache, so "recheck" is this
    /// same Query — a second command would be a second thing to keep honest.
    pub fn status(&self) -> Result<LiveDataEncryptionReport, OperatorAccountError> {
        self.accounts.require_authenticated()?;
        let observed_at_ms = u64::try_from(
            crate::unix_time_ms().map_err(|_| OperatorAccountError::ClockBeforeUnixEpoch)?,
        )
        .map_err(|_| OperatorAccountError::ClockBeforeUnixEpoch)?;
        let report = inspect(&self.store_path, &self.system, observed_at_ms);
        // The session is rechecked after the reads, as local-model discovery
        // does: a result gathered under a session that has since closed is
        // withheld rather than returned.
        self.accounts.require_authenticated()?;
        Ok(report)
    }
}

/// Public test-facing entry for one reading. It calls the same reader
/// `DiskEncryptionService::status` calls, without the account session, so a
/// fixture can present the states a live host will not produce on demand.
pub fn live_data_encryption_for_test(
    store_path: &Path,
    system: &SystemView,
    observed_at_ms: u64,
) -> LiveDataEncryptionReport {
    inspect(store_path, system, observed_at_ms)
}

fn inspect(
    store_path: &Path,
    system: &SystemView,
    observed_at_ms: u64,
) -> LiveDataEncryptionReport {
    if !cfg!(target_os = "linux") {
        return unknown(
            Vec::new(),
            None,
            Verdict::bare(ReasonCode::PlatformNotSupported),
            observed_at_ms,
        );
    }
    inspect_linux(store_path, system, observed_at_ms)
}

fn inspect_linux(
    store_path: &Path,
    system: &SystemView,
    observed_at_ms: u64,
) -> LiveDataEncryptionReport {
    let mut evidence = Vec::new();

    let target = match resolve_target(store_path) {
        Ok(target) => target,
        Err(verdict) => return unknown(evidence, None, verdict, observed_at_ms),
    };
    let Some(stat_device) = device_of(&target) else {
        return unknown(
            evidence,
            None,
            Verdict::bare(ReasonCode::StoreNotInspectable),
            observed_at_ms,
        );
    };
    // The path itself is deliberately not recorded: the subject of this report
    // is a volume, and the store path names the operator's home directory.
    record(
        &mut evidence,
        "stat(live store)".to_owned(),
        format!("st_dev = {stat_device}"),
    );

    let mounts = match read_mountinfo(&system.mountinfo) {
        Ok(mounts) => mounts,
        Err(verdict) => return unknown(evidence, None, verdict, observed_at_ms),
    };
    let Some(mount) = select_mount(&mounts, &target) else {
        return unknown(
            evidence,
            None,
            Verdict::bare(ReasonCode::NoMountForStore),
            observed_at_ms,
        );
    };
    record(
        &mut evidence,
        system.mountinfo.display().to_string(),
        format!(
            "{} {} {} at {}",
            mount.device, mount.fstype, mount.source, mount.mount_point
        ),
    );
    // A path keeps its tail: the leaf directory is what names the volume.
    let mount_point = Some(Subject::path(Path::new(&mount.mount_point)));

    // A longest-prefix selection that picked the wrong entry would answer
    // confidently about a volume the store does not live on. An anonymous
    // st_dev (major 0) cannot corroborate, so it is not asked to.
    if stat_device.major != 0 && stat_device != mount.device {
        return unknown(
            evidence,
            mount_point,
            Verdict::bare(ReasonCode::MountAndStatDisagree),
            observed_at_ms,
        );
    }

    if let Some(reason) = unsupported_filesystem(&mount.fstype) {
        return unknown(evidence, mount_point, Verdict::bare(reason), observed_at_ms);
    }

    let root = if mount.device.major != 0 {
        mount.device
    } else {
        // btrfs and ZFS report an anonymous device number, so the block device
        // has to come from the mount source instead.
        match match block_device_of_source(&mount.source) {
            Ok(found) => found,
            Err(verdict) => return unknown(evidence, mount_point, verdict, observed_at_ms),
        } {
            Some(device) => {
                record(
                    &mut evidence,
                    format!("stat({})", mount.source),
                    format!("anonymous mount device; block device is {device}"),
                );
                device
            }
            None => {
                return unknown(
                    evidence,
                    mount_point,
                    Verdict::about(
                        ReasonCode::NoBlockDeviceForFilesystem,
                        vec![Subject::new(SubjectRole::Location, &mount.fstype)],
                    ),
                    observed_at_ms,
                );
            }
        }
    };

    match walk_block_stack(system, root, &mut evidence) {
        Walk::Encrypted { mechanisms } => LiveDataEncryptionReport {
            conclusion: LiveDataEncryptionConclusion::new(
                LiveDataEncryption::Enabled,
                ReasonCode::CryptCoversEveryPath,
                mechanisms
                    .iter()
                    .map(|kind| Subject::new(SubjectRole::Mechanism, kind))
                    .collect(),
            ),
            mount_point,
            evidence,
            not_verified: vec![NOT_VERIFIED_KEY.to_owned()],
            observed_at_ms,
        },
        Walk::UncoveredPath {
            uncovered,
            crypt_seen,
        } => {
            let mut subjects: Vec<Subject> = crypt_seen
                .iter()
                .take(1)
                .map(|name| Subject::new(SubjectRole::Mapping, name))
                .collect();
            // Every uncovered device, not a pre-trimmed few: the conclusion
            // caps the list and counts what it dropped, so the count is real.
            subjects.extend(uncovered.iter().map(|device| Subject::device(*device)));
            let conclusion = LiveDataEncryptionConclusion::new(
                LiveDataEncryption::Disabled,
                if crypt_seen.is_empty() {
                    ReasonCode::NoCryptOnAnyPath
                } else {
                    ReasonCode::SomePathsUncovered
                },
                subjects,
            );
            // Name the branches that are not covered in evidence too; a
            // negative nobody can locate is not much better than no answer.
            record(
                &mut evidence,
                system.sys_dev_block.display().to_string(),
                render_conclusion(&conclusion),
            );
            LiveDataEncryptionReport {
                conclusion,
                mount_point,
                evidence,
                not_verified: vec![
                    NOT_VERIFIED_OPAL.to_owned(),
                    NOT_VERIFIED_FSCRYPT.to_owned(),
                    NOT_VERIFIED_BELOW_HOST.to_owned(),
                ],
                observed_at_ms,
            }
        }
        Walk::Undetermined { verdict } => unknown(evidence, mount_point, verdict, observed_at_ms),
    }
}

fn unknown(
    evidence: Vec<EncryptionEvidence>,
    mount_point: Option<Subject>,
    verdict: Verdict,
    observed_at_ms: u64,
) -> LiveDataEncryptionReport {
    LiveDataEncryptionReport {
        conclusion: LiveDataEncryptionConclusion::new(
            LiveDataEncryption::Unknown,
            verdict.reason,
            verdict.subjects,
        ),
        mount_point,
        evidence,
        not_verified: vec![
            NOT_VERIFIED_OPAL.to_owned(),
            NOT_VERIFIED_FSCRYPT.to_owned(),
            NOT_VERIFIED_BELOW_HOST.to_owned(),
            NOT_VERIFIED_KEY.to_owned(),
        ],
        observed_at_ms,
    }
}

fn record(evidence: &mut Vec<EncryptionEvidence>, source: String, observation: String) {
    if evidence.len() >= MAX_EVIDENCE_ENTRIES {
        return;
    }
    let (source, source_cut) = bounded(source);
    let (observation, observation_cut) = bounded(observation);
    evidence.push(EncryptionEvidence {
        source,
        observation,
        truncated: source_cut || observation_cut,
    });
}

/// Evidence: redacted, control-stripped, and shortened WITH a marker. A
/// silently truncated observation reads as the whole value it is a prefix of,
/// which is the same lie in a smaller place than a truncated conclusion.
fn bounded(value: String) -> (String, bool) {
    let cleaned: String = redact_identifiers(&value)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.chars().count() <= MAX_OBSERVATION_CHARS {
        return (cleaned.to_owned(), false);
    }
    (cleaned.chars().take(MAX_OBSERVATION_CHARS).collect(), true)
}

/// Replaces any long hexadecimal run with an ellipsis before it is recorded.
///
/// A crypt UUID is not a secret, but it is a stable identifier for this host's
/// volume, and it reaches evidence by more doors than the obvious one: many
/// distributions name the mapping `luks-<uuid>`, so `dm/name` and the mount
/// source carry it too. Redacting on the way into the record closes every door
/// at once rather than one per source.
fn redact_identifiers(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut run = String::new();
    for character in value.chars() {
        if character.is_ascii_hexdigit() {
            run.push(character);
            continue;
        }
        push_run(&mut out, &mut run);
        out.push(character);
    }
    push_run(&mut out, &mut run);
    out
}

fn push_run(out: &mut String, run: &mut String) {
    if run.chars().count() >= MAX_IDENTIFIER_RUN_CHARS {
        out.push('\u{2026}');
    } else {
        out.push_str(run);
    }
    run.clear();
}

/// A block device as the kernel numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DeviceNumber {
    major: u32,
    minor: u32,
}

impl DeviceNumber {
    /// Decodes the kernel's 64-bit `dev_t`. Beacon uses `libc::major` and
    /// `libc::minor` rather than an open-coded shift so the encoding stays the
    /// platform's, not this module's opinion of it.
    fn from_raw(raw: u64) -> Self {
        Self {
            major: libc::major(raw) as u32,
            minor: libc::minor(raw) as u32,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        let (major, minor) = value.trim().split_once(':')?;
        Some(Self {
            major: major.parse().ok()?,
            minor: minor.parse().ok()?,
        })
    }
}

impl std::fmt::Display for DeviceNumber {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.major, self.minor)
    }
}

/// Resolves the volume's subject: the store itself, or -- only when the store
/// does not exist yet -- the directory that will hold it.
///
/// Every other error means a store that IS there and could not be resolved: a
/// permission failure, an I/O error, a symlink loop. Swapping in the parent
/// there would answer confidently about a different subject, so it is reported
/// rather than papered over.
fn resolve_target(store_path: &Path) -> Result<PathBuf, Verdict> {
    match fs::canonicalize(store_path) {
        Ok(resolved) => Ok(resolved),
        // NotFound from canonicalize does not prove the store is absent: a link
        // whose target is gone answers the same way, and its parent is a
        // different subject. Ask whether the entry itself is there.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::symlink_metadata(store_path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let parent = store_path
                        .parent()
                        .ok_or_else(|| Verdict::bare(ReasonCode::StorePathUnresolved))?;
                    fs::canonicalize(parent)
                        .map_err(|error| unresolved(&format!("{:?}", error.kind())))
                }
                Ok(_) => Err(dangling(store_path)),
                Err(error) => Err(unresolved(&format!("{:?}", error.kind()))),
            }
        }
        Err(error) => Err(unresolved(&format!("{:?}", error.kind()))),
    }
}

fn unresolved(detail: &str) -> Verdict {
    Verdict::about(
        ReasonCode::StorePathUnresolved,
        vec![Subject::new(SubjectRole::Detail, detail)],
    )
}

fn device_of(path: &Path) -> Option<DeviceNumber> {
    Some(DeviceNumber::from_raw(fs::metadata(path).ok()?.dev()))
}

/// The block device behind a mount source, for filesystems whose own device
/// number is anonymous. Routed through `presence` like every other link the
/// walk follows: a `/dev/mapper/...` entry pointing at nothing is broken, not
/// a filesystem that exposes no block device.
fn block_device_of_source(source: &str) -> Result<Option<DeviceNumber>, Verdict> {
    let path = Path::new(source);
    if !path.is_absolute() {
        return Ok(None);
    }
    match presence(path)? {
        Presence::Missing => Ok(None),
        Presence::Present => match fs::metadata(path) {
            Ok(metadata) if metadata.file_type().is_block_device() => {
                Ok(Some(DeviceNumber::from_raw(metadata.rdev())))
            }
            Ok(_) => Ok(None),
            Err(error) => Err(unreadable_reason(path, error.kind())),
        },
    }
}

/// Filesystems whose encryption is not decided by a block device under this
/// host. Each is `Unknown` with its own reason rather than a guess.
fn unsupported_filesystem(fstype: &str) -> Option<ReasonCode> {
    match fstype {
        "tmpfs" | "ramfs" => Some(ReasonCode::MemoryBackedFilesystem),
        "overlay" | "overlayfs" => Some(ReasonCode::OverlayFilesystem),
        _ if fstype.starts_with("fuse") => Some(ReasonCode::UserspaceFilesystem),
        "nfs" | "nfs4" | "cifs" | "smb3" | "9p" | "virtiofs" | "ceph" | "afs" => {
            Some(ReasonCode::NetworkFilesystem)
        }
        _ => None,
    }
}

#[derive(Debug)]
struct MountEntry {
    device: DeviceNumber,
    mount_point: String,
    fstype: String,
    source: String,
}

fn read_mountinfo(path: &Path) -> Result<Vec<MountEntry>, Verdict> {
    let unreadable = || Verdict::bare(ReasonCode::MountTableUnreadable);
    let text = match read_bounded(path, MAX_MOUNTINFO_BYTES) {
        Ok(text) => text,
        Err(BoundedRead::Failed(_)) => return Err(unreadable()),
        Err(BoundedRead::OverBudget) => return Err(over_budget(path)),
    };
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if entries.len() >= MAX_MOUNTINFO_ENTRIES {
            return Err(over_budget(path));
        }
        // A line Beacon cannot parse is a line it does not understand, and a
        // misunderstood mount table is exactly how a confident wrong volume
        // gets chosen. The format is stable kernel ABI, so this is strict.
        entries.push(
            parse_mount_line(line).ok_or_else(|| Verdict::bare(ReasonCode::MountTableUnparsed))?,
        );
    }
    if entries.is_empty() {
        return Err(Verdict::bare(ReasonCode::MountTableEmpty));
    }
    Ok(entries)
}

fn parse_mount_line(line: &str) -> Option<MountEntry> {
    let mut fields = line.split(' ');
    let _mount_id = fields.next()?;
    let _parent_id = fields.next()?;
    let device = DeviceNumber::parse(fields.next()?)?;
    let _root = fields.next()?;
    let mount_point = unescape_octal(fields.next()?);
    let _options = fields.next()?;
    let mut optional = 0;
    loop {
        if optional > MAX_MOUNTINFO_OPTIONAL_FIELDS {
            return None;
        }
        if fields.next()? == "-" {
            break;
        }
        optional += 1;
    }
    let fstype = fields.next()?.to_owned();
    let source = unescape_octal(fields.next()?);
    Some(MountEntry {
        device,
        mount_point,
        fstype,
        source,
    })
}

/// mountinfo escapes space, tab, newline, and backslash as three-digit octal.
fn unescape_octal(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        let digits: String = chars.clone().take(3).collect();
        let decoded = (digits.len() == 3)
            .then(|| u8::from_str_radix(&digits, 8).ok())
            .flatten();
        match decoded {
            Some(byte) => {
                out.push(byte as char);
                chars.nth(2);
            }
            None => out.push(character),
        }
    }
    out
}

/// The mount whose point is the longest path-component prefix of the target.
/// On a tie the last entry wins, because a later mount shadows an earlier one
/// at the same point.
fn select_mount<'a>(entries: &'a [MountEntry], target: &Path) -> Option<&'a MountEntry> {
    let mut best: Option<&MountEntry> = None;
    for entry in entries {
        if !target.starts_with(Path::new(&entry.mount_point)) {
            continue;
        }
        let longer_than_best = best.is_none_or(|current| {
            Path::new(&entry.mount_point).components().count()
                >= Path::new(&current.mount_point).components().count()
        });
        if longer_than_best {
            best = Some(entry);
        }
    }
    best
}

/// What the walk proved about the whole stack, not about one branch of it.
/// What the walk proved about the whole stack, not about one branch of it.
enum Walk {
    /// Every branch terminated at a physical leaf and every one of those paths
    /// passed through a crypt layer.
    Encrypted {
        mechanisms: Vec<String>,
    },
    /// Every branch terminated at a physical leaf and at least one of those
    /// paths had no crypt layer. `uncovered` names them; `crypt_seen` names the
    /// mappings that WERE encrypted, so the report can say both.
    UncoveredPath {
        uncovered: Vec<DeviceNumber>,
        crypt_seen: Vec<String>,
    },
    Undetermined {
        verdict: Verdict,
    },
}

/// What one branch proved. `Terminated` means every path below this node
/// reached a physical leaf; nothing else counts as an answer.
enum Branch {
    Terminated { uncovered: Vec<DeviceNumber> },
    Undetermined { verdict: Verdict },
}

/// A physical leaf: a real disk or partition. It has no `dm/` directory and no
/// parents. A device-mapper node with no parents is not a leaf — it is a
/// mapping whose backing device Beacon never saw, which is an unread stack
/// rather than an encrypted one.
struct Walker<'a> {
    system: &'a SystemView,
    nodes: usize,
    crypt_kinds: BTreeSet<String>,
    crypt_mappings: Vec<String>,
    /// Branches proved terminated, keyed on the device AND whether a crypt
    /// layer covered the path that reached it: the same disk can be reached by
    /// a covered and an uncovered path, and they are different answers.
    settled: HashMap<(DeviceNumber, bool), Vec<DeviceNumber>>,
    /// Devices on the path currently being explored. A device reached again
    /// from below itself is a cycle, and a cycle terminates nothing.
    on_path: HashSet<(DeviceNumber, bool)>,
}

fn walk_block_stack(
    system: &SystemView,
    root: DeviceNumber,
    evidence: &mut Vec<EncryptionEvidence>,
) -> Walk {
    let mut walker = Walker {
        system,
        nodes: 0,
        crypt_kinds: BTreeSet::new(),
        crypt_mappings: Vec::new(),
        settled: HashMap::new(),
        on_path: HashSet::new(),
    };
    match walker.explore(root, 0, false, evidence) {
        Branch::Undetermined { verdict } => Walk::Undetermined { verdict },
        Branch::Terminated { uncovered } if uncovered.is_empty() => Walk::Encrypted {
            // Every component of a conclusion is bounded, including this one:
            // a stack can carry more crypt types than a sentence can name.
            mechanisms: walker.crypt_kinds.into_iter().collect(),
        },
        Branch::Terminated { uncovered } => Walk::UncoveredPath {
            uncovered,
            crypt_seen: walker.crypt_mappings,
        },
    }
}

impl Walker<'_> {
    /// Explores one branch to its physical leaves.
    ///
    /// Returns `Terminated` only when every path below `device` reached a
    /// physical leaf. Any branch that ends in a cycle, a budget stop, an
    /// unreadable node, or a mapping with no backing device returns
    /// `Undetermined`, and one such branch is the whole answer: a sibling that
    /// finished, encrypted, proves nothing about the branch that did not.
    fn explore(
        &mut self,
        device: DeviceNumber,
        depth: usize,
        crypt_above: bool,
        evidence: &mut Vec<EncryptionEvidence>,
    ) -> Branch {
        let key = (device, crypt_above);
        // Already proved terminated through a readable path: a diamond, not a
        // cycle. Its answer is reusable exactly because it was fully explored.
        if let Some(uncovered) = self.settled.get(&key) {
            return Branch::Terminated {
                uncovered: uncovered.clone(),
            };
        }
        if self.on_path.contains(&key) {
            return Branch::Undetermined {
                verdict: Verdict::about(ReasonCode::NoPhysicalLeaf, vec![Subject::device(device)]),
            };
        }
        // Checked before the node is counted, not after it is explored.
        if self.nodes >= MAX_STACK_NODES || depth > MAX_STACK_DEPTH {
            return Branch::Undetermined {
                verdict: over_stack_budget(),
            };
        }
        self.nodes += 1;
        self.on_path.insert(key);
        let outcome = self.explore_node(device, depth, crypt_above, evidence);
        self.on_path.remove(&key);
        if let Branch::Terminated { uncovered } = &outcome {
            self.settled.insert(key, uncovered.clone());
        }
        outcome
    }

    fn explore_node(
        &mut self,
        device: DeviceNumber,
        depth: usize,
        crypt_above: bool,
        evidence: &mut Vec<EncryptionEvidence>,
    ) -> Branch {
        let node = self.system.sys_dev_block.join(device.to_string());
        match directory_presence(&node) {
            Ok(Presence::Present) => {}
            Ok(Presence::Missing) => {
                return Branch::Undetermined {
                    verdict: unreadable_reason(&node, std::io::ErrorKind::NotFound),
                };
            }
            Err(verdict) => return Branch::Undetermined { verdict },
        }

        let backing = node.join("loop/backing_file");
        match presence(&backing) {
            Ok(Presence::Present) => {
                let path = match read_sysfs(&backing) {
                    Ok(path) => path,
                    Err(verdict) => return Branch::Undetermined { verdict },
                };
                return Branch::Undetermined {
                    verdict: Verdict::about(
                        ReasonCode::LoopbackImage,
                        vec![Subject::new(SubjectRole::Location, &path)],
                    ),
                };
            }
            Ok(Presence::Missing) => {}
            Err(verdict) => return Branch::Undetermined { verdict },
        }

        // A `dm/` directory is what makes this a mapping rather than a disk.
        let mapping = match directory_presence(&node.join("dm")) {
            Ok(Presence::Present) => match self.read_mapping(&node, evidence) {
                Ok(mapping) => Some(mapping),
                Err(verdict) => return Branch::Undetermined { verdict },
            },
            Ok(Presence::Missing) => None,
            Err(verdict) => return Branch::Undetermined { verdict },
        };
        let crypt_on_path = crypt_above || mapping.as_ref().is_some_and(|found| found.is_crypt);

        // A missing slaves directory means this device has no parents. A
        // DANGLING one answers NotFound from read_dir too, so presence is asked
        // first: a link to nothing is an error, not a device with no parents.
        let slaves = node.join("slaves");
        let slaves_present = match presence(&slaves) {
            Ok(found) => found == Presence::Present,
            Err(verdict) => return Branch::Undetermined { verdict },
        };
        let mut parents = Vec::new();
        if slaves_present {
            let children = match fs::read_dir(&slaves) {
                Ok(children) => children,
                Err(error) => {
                    return Branch::Undetermined {
                        verdict: unreadable_reason(&slaves, error.kind()),
                    };
                }
            };
            // The iterator is bounded BEFORE it is consumed, not while it is.
            // A pre-child budget check is only as good as the loop that honours
            // it: collecting every DirEntry first and then checking still walks
            // the whole directory, and no fixture can see that from outside --
            // enumerating costs no open, so there is nothing to poison. Bounding
            // the iterator makes the property structural instead.
            let remaining = MAX_STACK_NODES.saturating_sub(self.nodes) + 1;
            for child in children.take(remaining) {
                // The budget bounds the work, so it is spent before the work:
                // one more child means one more device to read and explore, and
                // asking after the fan-out is enumerated lets a single wide node
                // perform arbitrarily many reads before the ceiling is reported.
                if self.nodes + parents.len() >= MAX_STACK_NODES {
                    return Branch::Undetermined {
                        verdict: over_stack_budget(),
                    };
                }
                let child = match child {
                    Ok(child) => child,
                    Err(error) => {
                        return Branch::Undetermined {
                            verdict: unreadable_reason(&slaves, error.kind()),
                        };
                    }
                };
                let dev_file = child.path().join("dev");
                match read_required(&dev_file) {
                    Ok(value) => match DeviceNumber::parse(&value) {
                        Some(parent) => parents.push(parent),
                        None => {
                            return Branch::Undetermined {
                                verdict: unreadable_reason(
                                    &dev_file,
                                    std::io::ErrorKind::InvalidData,
                                ),
                            };
                        }
                    },
                    Err(verdict) => return Branch::Undetermined { verdict },
                }
            }
        }

        if parents.is_empty() {
            return match mapping {
                // A mapping with no backing device is an unread stack.
                Some(found) => Branch::Undetermined {
                    verdict: Verdict::about(
                        ReasonCode::MappingWithoutBackingDevice,
                        vec![Subject::new(SubjectRole::Mapping, &found.name)],
                    ),
                },
                // A real disk or partition: the only thing that terminates a branch.
                None => Branch::Terminated {
                    uncovered: if crypt_on_path {
                        Vec::new()
                    } else {
                        vec![device]
                    },
                },
            };
        }

        let mut uncovered = Vec::new();
        for parent in parents {
            match self.explore(parent, depth + 1, crypt_on_path, evidence) {
                Branch::Terminated { uncovered: below } => uncovered.extend(below),
                // One unterminated branch settles the whole answer.
                undetermined => return undetermined,
            }
        }
        Branch::Terminated { uncovered }
    }

    fn read_mapping(
        &mut self,
        node: &Path,
        evidence: &mut Vec<EncryptionEvidence>,
    ) -> Result<Mapping, Verdict> {
        let uuid_path = node.join("dm/uuid");
        let uuid = read_required(&uuid_path)?;
        // A mapping with a uuid always has a name beside it. Substituting a
        // placeholder for a failed read would let a read failure reach a
        // confident state, which is the one thing this reader must never do.
        let name = read_required(&node.join("dm/name"))?;
        match crypt_type(&uuid) {
            Some(kind) => {
                // The type prefix only. The remainder of a crypt UUID is a
                // stable identifier for this host's volume and has no place in
                // a report an operator may export.
                record(
                    evidence,
                    uuid_path.display().to_string(),
                    format!("device-mapper uuid prefix {}, mapping {name}", label(&kind)),
                );
                self.crypt_kinds.insert(mechanism_label(&kind));
                if !self.crypt_mappings.contains(&name) {
                    self.crypt_mappings.push(name.clone());
                }
                Ok(Mapping {
                    name,
                    is_crypt: true,
                })
            }
            None => {
                record(
                    evidence,
                    uuid_path.display().to_string(),
                    format!(
                        "device-mapper uuid prefix {}, mapping {name}; not a crypt target",
                        subsystem_prefix(&uuid)
                    ),
                );
                Ok(Mapping {
                    name,
                    is_crypt: false,
                })
            }
        }
    }
}

struct Mapping {
    name: String,
    is_crypt: bool,
}

/// The type token of a device-mapper crypt UUID, or `None` when the UUID does
/// not name a crypt target. Matching the generic `CRYPT-` prefix rather than an
/// allowlist means an unfamiliar crypt type still reports `Enabled` with an
/// honest label instead of being silently missed.
fn crypt_type(uuid: &str) -> Option<String> {
    let rest = uuid.strip_prefix("CRYPT-")?;
    Some(
        rest.split('-')
            .next()
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(MAX_CRYPT_TYPE_CHARS)
            .collect(),
    )
}

fn label(kind: &str) -> String {
    if kind.is_empty() {
        "CRYPT".to_owned()
    } else {
        format!("CRYPT-{kind}")
    }
}

fn mechanism_label(kind: &str) -> String {
    if kind.is_empty() {
        "dm-crypt".to_owned()
    } else {
        format!("dm-crypt ({kind})")
    }
}

fn subsystem_prefix(uuid: &str) -> String {
    let prefix: String = uuid
        .split('-')
        .next()
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(MAX_CRYPT_TYPE_CHARS)
        .collect();
    if prefix.is_empty() {
        "none".to_owned()
    } else {
        prefix
    }
}

/// Whether a path is there, keeping the difference between "absent" and
/// "Beacon could not tell". A boolean probe collapses those into one answer,
/// and the collapsed answer is the one that lets an unread stack look proven.
#[derive(Debug, PartialEq, Eq)]
enum Presence {
    Present,
    Missing,
}

/// The one place this module asks the filesystem whether something is there.
///
/// Every step of the walk follows links -- the store path, `slaves/*`, the
/// `dev` file behind each slave entry, `dm/*`, `loop/backing_file` -- and a
/// link that resolves to nothing answers `NotFound`, exactly as a name that was
/// never created does. Believing that answer is how an unread stack becomes a
/// proven negative, so `NotFound` is only absence once every component of the
/// path is known to resolve.
fn presence(path: &Path) -> Result<Presence, Verdict> {
    match fs::symlink_metadata(path) {
        // The name is there. Following it is a separate question: a dangling
        // link has metadata of its own and none at its target.
        Ok(_) => match fs::metadata(path) {
            Ok(_) => Ok(Presence::Present),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(dangling(path)),
            Err(error) => Err(unreadable_reason(path, error.kind())),
        },
        // Not there -- or a parent component is a link to nothing, which is a
        // different fact and reaches this arm with the same error.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            for ancestor in path.ancestors().skip(1).take(MAX_PATH_ANCESTORS) {
                if ancestor.as_os_str().is_empty() {
                    break;
                }
                match fs::symlink_metadata(ancestor) {
                    Ok(_) => match fs::metadata(ancestor) {
                        // This component resolves, so the leaf really is absent.
                        Ok(_) => return Ok(Presence::Missing),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            return Err(dangling(ancestor));
                        }
                        Err(error) => return Err(unreadable_reason(ancestor, error.kind())),
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(unreadable_reason(ancestor, error.kind())),
                }
            }
            Ok(Presence::Missing)
        }
        Err(error) => Err(unreadable_reason(path, error.kind())),
    }
}

fn over_stack_budget() -> Verdict {
    Verdict::bare(ReasonCode::StackOverBudget)
}

fn dangling(path: &Path) -> Verdict {
    Verdict::about(ReasonCode::LinkResolvesToNothing, vec![Subject::path(path)])
}

/// Reads a value the walk requires. Absence is a broken stack, not a value.
fn read_required(path: &Path) -> Result<String, Verdict> {
    match presence(path)? {
        Presence::Present => read_sysfs(path),
        Presence::Missing => Err(unreadable_reason(path, std::io::ErrorKind::NotFound)),
    }
}

/// The same question for a directory. Presence already followed the link --
/// every entry under /sys/dev/block is a symlink to the device's real
/// directory -- so this only adds "and it is a directory".
fn directory_presence(path: &Path) -> Result<Presence, Verdict> {
    match presence(path)? {
        Presence::Missing => Ok(Presence::Missing),
        Presence::Present => match fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => Ok(Presence::Present),
            Ok(_) => Err(unreadable_reason(path, std::io::ErrorKind::NotADirectory)),
            Err(error) => Err(unreadable_reason(path, error.kind())),
        },
    }
}

fn unreadable_reason(path: &Path, kind: std::io::ErrorKind) -> Verdict {
    Verdict::about(
        ReasonCode::DeviceUnreadable,
        vec![
            Subject::path(path),
            Subject::new(SubjectRole::Detail, &format!("{kind:?}")),
        ],
    )
}

fn over_budget(path: &Path) -> Verdict {
    Verdict::about(ReasonCode::ReadOverBudget, vec![Subject::path(path)])
}

/// Reads one bounded value, and can tell a complete read from a truncated one.
///
/// `take(budget)` alone cannot: it returns the first `budget` bytes of a larger
/// file and looks exactly like a whole small file. Reading one byte past the
/// budget makes the overflow visible, and an overflowing read is no answer.
/// Why a bounded read did not produce a value. One shape, two vocabularies:
/// callers phrase it for the thing they were reading.
enum BoundedRead {
    Failed(std::io::ErrorKind),
    OverBudget,
}

/// Reads at most `budget` bytes, and can tell a complete read from a truncated
/// one.
///
/// `take(budget)` alone cannot: it returns the first `budget` bytes of a larger
/// file and looks exactly like a whole small file. Reading one byte past the
/// budget makes the overflow visible. Both call sites had that defect and both
/// were repaired at once, which is the drift this shared helper prevents.
fn read_bounded(path: &Path, budget: u64) -> Result<String, BoundedRead> {
    let file = File::open(path).map_err(|error| BoundedRead::Failed(error.kind()))?;
    let mut bytes = Vec::new();
    file.take(budget + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| BoundedRead::Failed(error.kind()))?;
    if bytes.len() as u64 > budget {
        return Err(BoundedRead::OverBudget);
    }
    String::from_utf8(bytes).map_err(|_| BoundedRead::Failed(std::io::ErrorKind::InvalidData))
}

fn read_sysfs(path: &Path) -> Result<String, Verdict> {
    match read_bounded(path, MAX_SYSFS_FILE_BYTES) {
        Ok(value) => Ok(value.trim().to_owned()),
        Err(BoundedRead::Failed(kind)) => Err(unreadable_reason(path, kind)),
        Err(BoundedRead::OverBudget) => Err(over_budget(path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_kernels_device_number_encoding() {
        // A live reading from this host: `stat` reported st_dev 66306 for a
        // path the mount table places on 259:2. An open-coded 8-bit shift
        // decodes the same value as 3:2, which would select the wrong device.
        let device = DeviceNumber::from_raw(66306);
        assert_eq!(device.major, 259);
        assert_eq!(device.minor, 2);
        assert_eq!(device.to_string(), "259:2");
    }

    #[test]
    fn parses_a_mount_line_past_its_variable_optional_fields() {
        let line = "32 2 259:2 / / rw,relatime shared:1 master:3 - ext4 /dev/nvme0n1p2 rw";
        let entry = parse_mount_line(line).expect("the documented format parses");
        assert_eq!(
            entry.device,
            DeviceNumber {
                major: 259,
                minor: 2
            }
        );
        assert_eq!(entry.mount_point, "/");
        assert_eq!(entry.fstype, "ext4");
        assert_eq!(entry.source, "/dev/nvme0n1p2");
    }

    #[test]
    fn unescapes_the_octal_mount_point_encoding() {
        assert_eq!(unescape_octal("/mnt/my\\040disk"), "/mnt/my disk");
        assert_eq!(unescape_octal("/mnt/back\\134slash"), "/mnt/back\\slash");
        assert_eq!(unescape_octal("/plain"), "/plain");
    }

    #[test]
    fn selects_the_longest_mount_point_and_the_last_of_a_tie() {
        let entries = vec![
            MountEntry {
                device: DeviceNumber { major: 1, minor: 1 },
                mount_point: "/".to_owned(),
                fstype: "ext4".to_owned(),
                source: "/dev/one".to_owned(),
            },
            MountEntry {
                device: DeviceNumber { major: 2, minor: 2 },
                mount_point: "/home".to_owned(),
                fstype: "ext4".to_owned(),
                source: "/dev/two".to_owned(),
            },
            MountEntry {
                device: DeviceNumber { major: 3, minor: 3 },
                mount_point: "/home".to_owned(),
                fstype: "ext4".to_owned(),
                source: "/dev/three".to_owned(),
            },
        ];
        let selected = select_mount(&entries, Path::new("/home/operator/store"))
            .expect("a covering mount exists");
        assert_eq!(selected.device, DeviceNumber { major: 3, minor: 3 });
        // "/homeless" must not match "/home": the comparison is by path
        // component, not by string prefix.
        let shallow =
            select_mount(&entries, Path::new("/homeless")).expect("a covering mount exists");
        assert_eq!(shallow.mount_point, "/");
    }

    #[test]
    fn reads_a_crypt_type_from_any_crypt_uuid_without_an_allowlist() {
        assert_eq!(crypt_type("CRYPT-LUKS2-abc-name").as_deref(), Some("LUKS2"));
        assert_eq!(crypt_type("CRYPT-BITLK-abc-name").as_deref(), Some("BITLK"));
        assert_eq!(mechanism_label("LUKS2"), "dm-crypt (LUKS2)");
        assert_eq!(mechanism_label(""), "dm-crypt");
        assert_eq!(crypt_type("LVM-abcdef"), None);
        assert_eq!(subsystem_prefix("LVM-abcdef"), "LVM");
    }

    #[test]
    fn redacts_long_identifiers_before_they_reach_evidence() {
        let (recorded, truncated) = bounded(
            "device-mapper uuid prefix CRYPT-LUKS2, mapping luks-4f8c1d2e9a7b6c5d4e3f2a1b0c9d8e7f"
                .to_owned(),
        );
        assert!(recorded.contains("CRYPT-LUKS2"));
        assert!(!recorded.contains("4f8c1d2e9a7b6c5d4e3f2a1b0c9d8e7f"));
        assert!(recorded.contains('\u{2026}'));
        // Redaction is not truncation: the flag says only whether text was cut.
        assert!(!truncated);
        // Short hexadecimal text is ordinary text and survives.
        assert_eq!(
            bounded("dm-0 ext4".to_owned()),
            ("dm-0 ext4".to_owned(), false)
        );
    }

    #[test]
    fn a_conclusion_cannot_outgrow_its_bound() {
        // The property the type exists for. Compose the largest conclusion the
        // shape allows -- every reason, the most subjects, each at its maximum
        // length -- and every rendered sentence must still fit.
        let long = "m".repeat(400);
        for reason in EVERY_REASON {
            let conclusion = LiveDataEncryptionConclusion::new(
                LiveDataEncryption::Unknown,
                reason,
                vec![
                    Subject::new(SubjectRole::Mechanism, &long),
                    Subject::new(SubjectRole::Mapping, &long),
                    Subject::new(SubjectRole::Device, &long),
                    Subject::new(SubjectRole::Location, &long),
                    Subject::new(SubjectRole::Detail, &long),
                ],
            );
            assert!(conclusion.subjects.len() <= MAX_SUBJECTS);
            let rendered = render_conclusion(&conclusion);
            assert!(
                rendered.chars().count() <= MAX_RENDERED_CONCLUSION_CHARS,
                "{reason:?} rendered {} chars",
                rendered.chars().count()
            );
            assert!(!rendered.is_empty(), "{reason:?} rendered nothing");
        }
    }

    #[test]
    fn a_shortened_subject_says_so_by_flag() {
        let short = Subject::new(SubjectRole::Mapping, "vg0-root");
        assert!(!short.shortened);
        assert_eq!(short.value, "vg0-root");
        let long = Subject::new(SubjectRole::Mapping, &"mapping-".repeat(20));
        assert!(long.shortened);
        assert_eq!(long.value.chars().count(), MAX_SUBJECT_CHARS);
    }

    const EVERY_REASON: [ReasonCode; 23] = [
        ReasonCode::CryptCoversEveryPath,
        ReasonCode::NoCryptOnAnyPath,
        ReasonCode::SomePathsUncovered,
        ReasonCode::StorePathUnresolved,
        ReasonCode::StoreNotInspectable,
        ReasonCode::MountTableUnreadable,
        ReasonCode::MountTableUnparsed,
        ReasonCode::MountTableEmpty,
        ReasonCode::NoMountForStore,
        ReasonCode::MountAndStatDisagree,
        ReasonCode::MemoryBackedFilesystem,
        ReasonCode::OverlayFilesystem,
        ReasonCode::UserspaceFilesystem,
        ReasonCode::NetworkFilesystem,
        ReasonCode::NoBlockDeviceForFilesystem,
        ReasonCode::LinkResolvesToNothing,
        ReasonCode::DeviceUnreadable,
        ReasonCode::ReadOverBudget,
        ReasonCode::StackOverBudget,
        ReasonCode::NoPhysicalLeaf,
        ReasonCode::MappingWithoutBackingDevice,
        ReasonCode::LoopbackImage,
        ReasonCode::PlatformNotSupported,
    ];
}
