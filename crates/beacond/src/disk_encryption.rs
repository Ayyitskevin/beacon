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

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{OperatorAccountError, OperatorAccountService, Recorded};

const MAX_MOUNTINFO_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MOUNTINFO_ENTRIES: usize = 16_384;
const MAX_MOUNTINFO_OPTIONAL_FIELDS: usize = 64;
const MAX_SYSFS_FILE_BYTES: u64 = 4 * 1024;
const MAX_STACK_DEPTH: usize = 8;
const MAX_STACK_NODES: usize = 64;
const MAX_EVIDENCE_ENTRIES: usize = 32;
const MAX_OBSERVATION_CHARS: usize = 200;
const MAX_CRYPT_TYPE_CHARS: usize = 16;
/// A hexadecimal run at least this long is treated as an identifier, not text.
const MAX_IDENTIFIER_RUN_CHARS: usize = 12;

const NOT_VERIFIED_OPAL: &str = "drive-firmware encryption (OPAL or another self-encrypting drive), which Beacon cannot observe";
const NOT_VERIFIED_FSCRYPT: &str = "filesystem-level encryption such as fscrypt or ext4 encryption";
const NOT_VERIFIED_BELOW_HOST: &str =
    "encryption applied beneath this host, such as by a hypervisor or a storage array";
const NOT_VERIFIED_KEY: &str = "the strength, custody, or current lock state of any key";

const NO_CRYPT_LAYER: &str =
    "Beacon found no device-mapper crypt layer in this volume's block stack";

/// What Beacon proved about operating-system disk encryption for the volume
/// holding this Installation's live data.
///
/// Three states, closed. `Disabled` is a proven negative — a block stack that
/// was read end to end and contained no crypt layer — not merely an absence of
/// proof. Absence of proof is `Unknown`, and `mechanism` carries its reason.
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
}

/// The result of one live reading. Never stored, so every field describes the
/// host at `observed_at_ms` and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveDataEncryptionReport {
    pub state: LiveDataEncryption,
    /// The volume this report is about. One subject in this slice; the field
    /// exists so a second managed location is additive rather than a reshape.
    pub mount_point: Recorded<String>,
    /// Present only for `Enabled`. For the other two states this is the reason
    /// Beacon reports what it reports.
    pub mechanism: Recorded<String>,
    pub evidence: Vec<EncryptionEvidence>,
    pub not_verified: Vec<String>,
    pub observed_at_ms: u64,
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
            Recorded::Absent {
                reason: "the platform was not inspected".to_owned(),
            },
            "not detectable on this platform",
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
    let no_mount = || Recorded::Absent {
        reason: "the volume holding live data was not identified".to_owned(),
    };

    let target = match resolve_target(store_path) {
        Ok(target) => target,
        Err(reason) => return unknown_owned(evidence, no_mount(), reason, observed_at_ms),
    };
    let Some(stat_device) = device_of(&target) else {
        return unknown(
            evidence,
            no_mount(),
            "the live store could not be inspected",
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
        Err(reason) => return unknown_owned(evidence, no_mount(), reason, observed_at_ms),
    };
    let Some(mount) = select_mount(&mounts, &target) else {
        return unknown(
            evidence,
            no_mount(),
            "no mount table entry covers the live store",
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
    let mount_point = Recorded::Present {
        value: mount.mount_point.clone(),
    };

    // A longest-prefix selection that picked the wrong entry would answer
    // confidently about a volume the store does not live on. An anonymous
    // st_dev (major 0) cannot corroborate, so it is not asked to.
    if stat_device.major != 0 && stat_device != mount.device {
        return unknown(
            evidence,
            mount_point,
            "the mount table and the live store disagree about which volume holds it",
            observed_at_ms,
        );
    }

    if let Some(reason) = unsupported_filesystem(&mount.fstype) {
        return unknown(evidence, mount_point, reason, observed_at_ms);
    }

    let root = if mount.device.major != 0 {
        mount.device
    } else {
        // btrfs and ZFS report an anonymous device number, so the block device
        // has to come from the mount source instead.
        match block_device_of_source(&mount.source) {
            Some(device) => {
                record(
                    &mut evidence,
                    format!("stat({})", mount.source),
                    format!("anonymous mount device; block device is {device}"),
                );
                device
            }
            None => {
                let reason = format!(
                    "this filesystem ({}) exposes no block device Beacon can inspect",
                    mount.fstype
                );
                return unknown_owned(evidence, mount_point, reason, observed_at_ms);
            }
        }
    };

    match walk_block_stack(system, root, &mut evidence) {
        Walk::Encrypted { mechanism } => LiveDataEncryptionReport {
            state: LiveDataEncryption::Enabled,
            mount_point,
            mechanism: Recorded::Present { value: mechanism },
            evidence,
            not_verified: vec![NOT_VERIFIED_KEY.to_owned()],
            observed_at_ms,
        },
        Walk::NoCryptProven => LiveDataEncryptionReport {
            state: LiveDataEncryption::Disabled,
            mount_point,
            mechanism: Recorded::Absent {
                reason: NO_CRYPT_LAYER.to_owned(),
            },
            evidence,
            not_verified: vec![
                NOT_VERIFIED_OPAL.to_owned(),
                NOT_VERIFIED_FSCRYPT.to_owned(),
                NOT_VERIFIED_BELOW_HOST.to_owned(),
            ],
            observed_at_ms,
        },
        Walk::Undetermined { reason } => {
            unknown_owned(evidence, mount_point, reason, observed_at_ms)
        }
    }
}

fn unknown(
    evidence: Vec<EncryptionEvidence>,
    mount_point: Recorded<String>,
    reason: &str,
    observed_at_ms: u64,
) -> LiveDataEncryptionReport {
    unknown_owned(evidence, mount_point, reason.to_owned(), observed_at_ms)
}

fn unknown_owned(
    evidence: Vec<EncryptionEvidence>,
    mount_point: Recorded<String>,
    reason: String,
    observed_at_ms: u64,
) -> LiveDataEncryptionReport {
    LiveDataEncryptionReport {
        state: LiveDataEncryption::Unknown,
        mount_point,
        mechanism: Recorded::Absent { reason },
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
    evidence.push(EncryptionEvidence {
        source: bounded(source),
        observation: bounded(observation),
    });
}

fn bounded(value: String) -> String {
    let cleaned: String = redact_identifiers(&value)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(MAX_OBSERVATION_CHARS)
        .collect();
    cleaned.trim().to_owned()
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
fn resolve_target(store_path: &Path) -> Result<PathBuf, String> {
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
                        .ok_or_else(|| unresolved("the live store path has no parent directory"))?;
                    fs::canonicalize(parent)
                        .map_err(|error| unresolved(&format!("{:?}", error.kind())))
                }
                Ok(_) => Err("the live store path is a link that resolves to nothing".to_owned()),
                Err(error) => Err(unresolved(&format!("{:?}", error.kind()))),
            }
        }
        Err(error) => Err(unresolved(&format!("{:?}", error.kind()))),
    }
}

fn unresolved(detail: &str) -> String {
    format!("the live store path could not be resolved: {detail}")
}

fn device_of(path: &Path) -> Option<DeviceNumber> {
    Some(DeviceNumber::from_raw(fs::metadata(path).ok()?.dev()))
}

fn block_device_of_source(source: &str) -> Option<DeviceNumber> {
    let path = Path::new(source);
    if !path.is_absolute() {
        return None;
    }
    let metadata = fs::metadata(path).ok()?;
    if !metadata.file_type().is_block_device() {
        return None;
    }
    Some(DeviceNumber::from_raw(metadata.rdev()))
}

/// Filesystems whose encryption is not decided by a block device under this
/// host. Each is `Unknown` with its own reason rather than a guess.
fn unsupported_filesystem(fstype: &str) -> Option<&'static str> {
    match fstype {
        "tmpfs" | "ramfs" => Some("the live store is on a memory-backed filesystem"),
        "overlay" | "overlayfs" => Some(
            "the live store is on an overlay filesystem; its layers are decided outside this mount",
        ),
        _ if fstype.starts_with("fuse") => Some(
            "the live store is on a userspace filesystem; its encryption is decided outside the kernel block layer",
        ),
        "nfs" | "nfs4" | "cifs" | "smb3" | "9p" | "virtiofs" | "ceph" | "afs" => Some(
            "the live store is on a network filesystem; its encryption is decided off this host",
        ),
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

fn read_mountinfo(path: &Path) -> Result<Vec<MountEntry>, String> {
    const UNREADABLE: &str = "the mount table could not be read";
    let file = File::open(path).map_err(|_| UNREADABLE.to_owned())?;
    let mut bytes = Vec::new();
    // One byte past the budget, so a mount table too large to read is an
    // overflow rather than a clean parse of whichever mounts happened to fit.
    file.take(MAX_MOUNTINFO_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| UNREADABLE.to_owned())?;
    if bytes.len() as u64 > MAX_MOUNTINFO_BYTES {
        return Err(over_budget(path));
    }
    let text = String::from_utf8(bytes).map_err(|_| UNREADABLE.to_owned())?;
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if entries.len() >= MAX_MOUNTINFO_ENTRIES {
            return Err("the mount table exceeded Beacon's read budget".to_owned());
        }
        // A line Beacon cannot parse is a line it does not understand, and a
        // misunderstood mount table is exactly how a confident wrong volume
        // gets chosen. The format is stable kernel ABI, so this is strict.
        entries.push(
            parse_mount_line(line)
                .ok_or_else(|| "the mount table could not be parsed".to_owned())?,
        );
    }
    if entries.is_empty() {
        return Err("the mount table was empty".to_owned());
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
enum Walk {
    /// Every leaf was reached and every path to it passed through a crypt layer.
    Encrypted {
        mechanism: String,
    },
    /// Every leaf was reached and at least one path to one had no crypt layer.
    NoCryptProven,
    Undetermined {
        reason: String,
    },
}

/// Walks every branch of the block stack to its physical leaves.
///
/// A crypt layer is a fact about one path, never a verdict about the volume. A
/// logical volume over one LUKS member and one plain member has half its
/// extents in the clear, so the walk carries "crypt seen on this path" down
/// each branch and only calls the volume encrypted when every leaf is covered.
/// A crypt layer ABOVE a plain leaf covers it; one on a sibling branch does not.
///
/// The visited set is keyed on the device AND that flag, because the same
/// physical disk can be reached by a covered and an uncovered path, and pruning
/// the second visit is how a plain branch would disappear.
fn walk_block_stack(
    system: &SystemView,
    root: DeviceNumber,
    evidence: &mut Vec<EncryptionEvidence>,
) -> Walk {
    let mut queue = VecDeque::from([(root, 0usize, false)]);
    let mut seen: HashSet<(DeviceNumber, bool)> = HashSet::new();
    let mut nodes = 0usize;
    let mut crypt_kinds: BTreeSet<String> = BTreeSet::new();
    let mut plain_leaves: Vec<DeviceNumber> = Vec::new();
    let mut leaves = 0usize;

    while let Some((device, depth, crypt_above)) = queue.pop_front() {
        if !seen.insert((device, crypt_above)) {
            continue;
        }
        nodes += 1;
        if nodes > MAX_STACK_NODES || depth > MAX_STACK_DEPTH {
            return Walk::Undetermined {
                reason: "the volume's device stack exceeded Beacon's read budget".to_owned(),
            };
        }
        let node = system.sys_dev_block.join(device.to_string());
        match directory_presence(&node) {
            Ok(Presence::Present) => {}
            Ok(Presence::Missing) => return unreadable(&node),
            Err(reason) => return Walk::Undetermined { reason },
        }

        let backing = node.join("loop/backing_file");
        match presence(&backing) {
            Ok(Presence::Present) => {
                let path = match read_sysfs(&backing) {
                    Ok(path) => path,
                    Err(reason) => return Walk::Undetermined { reason },
                };
                return Walk::Undetermined {
                    reason: format!(
                        "the volume is a loopback image backed by {path}; Beacon does not follow the backing file"
                    ),
                };
            }
            Ok(Presence::Missing) => {}
            Err(reason) => return Walk::Undetermined { reason },
        }

        let uuid_path = node.join("dm/uuid");
        let uuid = match presence(&uuid_path) {
            Ok(Presence::Present) => match read_sysfs(&uuid_path) {
                Ok(uuid) => Some(uuid),
                Err(reason) => return Walk::Undetermined { reason },
            },
            Ok(Presence::Missing) => None,
            Err(reason) => return Walk::Undetermined { reason },
        };
        let mut crypt_here = false;
        if let Some(uuid) = uuid {
            // A mapping with a uuid always has a name beside it. Substituting a
            // placeholder for a failed read would let a read failure reach a
            // confident state, which is the one thing this reader must never do.
            let name_path = node.join("dm/name");
            let name = match read_sysfs(&name_path) {
                Ok(name) => name,
                Err(reason) => return Walk::Undetermined { reason },
            };
            match crypt_type(&uuid) {
                Some(kind) => {
                    // The type prefix only. The remainder of a crypt UUID is a
                    // stable identifier for this host's volume and has no place
                    // in a report an operator may export.
                    record(
                        evidence,
                        uuid_path.display().to_string(),
                        format!("device-mapper uuid prefix {}, mapping {name}", label(&kind)),
                    );
                    crypt_kinds.insert(mechanism_label(&kind));
                    crypt_here = true;
                }
                None => record(
                    evidence,
                    uuid_path.display().to_string(),
                    format!(
                        "device-mapper uuid prefix {}, mapping {name}; not a crypt target",
                        subsystem_prefix(&uuid)
                    ),
                ),
            }
        }
        let crypt_on_path = crypt_above || crypt_here;

        // A missing slaves directory means this device has no parents. A
        // DANGLING one answers NotFound from read_dir too, so presence is asked
        // first: symlink_metadata sees the link, and a link to nothing is an
        // error rather than a device with no parents.
        let slaves = node.join("slaves");
        let slaves_present = match presence(&slaves) {
            Ok(found) => found == Presence::Present,
            Err(reason) => return Walk::Undetermined { reason },
        };
        let children = match fs::read_dir(&slaves) {
            Ok(children) => children,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !slaves_present => {
                leaves += 1;
                if !crypt_on_path {
                    plain_leaves.push(device);
                }
                continue;
            }
            Err(error) => {
                return Walk::Undetermined {
                    reason: unreadable_reason(&slaves, error.kind()),
                };
            }
        };
        let mut parents = 0usize;
        for child in children {
            let child = match child {
                Ok(child) => child,
                Err(error) => {
                    return Walk::Undetermined {
                        reason: unreadable_reason(&slaves, error.kind()),
                    };
                }
            };
            let dev_file = child.path().join("dev");
            let parent = match read_sysfs(&dev_file) {
                Ok(value) => match DeviceNumber::parse(&value) {
                    Some(parent) => parent,
                    None => return unreadable(&dev_file),
                },
                Err(reason) => return Walk::Undetermined { reason },
            };
            parents += 1;
            queue.push_back((parent, depth + 1, crypt_on_path));
        }
        // An empty slaves directory is a whole disk: a leaf like a partition.
        if parents == 0 {
            leaves += 1;
            if !crypt_on_path {
                plain_leaves.push(device);
            }
        }
    }

    // Nothing terminated the stack, so "every leaf is covered" would be true of
    // no leaves at all. A vacuous truth is not proof of encryption.
    if leaves == 0 {
        return Walk::Undetermined {
            reason: "no physical device terminated the volume's stack".to_owned(),
        };
    }
    if plain_leaves.is_empty() {
        return Walk::Encrypted {
            mechanism: crypt_kinds.into_iter().collect::<Vec<_>>().join(", "),
        };
    }
    // Name the branches that are not covered; a negative nobody can locate is
    // not much better than no answer.
    let named = plain_leaves
        .iter()
        .map(DeviceNumber::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    record(
        evidence,
        system.sys_dev_block.display().to_string(),
        format!("no crypt layer covers block device(s) {named}"),
    );
    Walk::NoCryptProven
}

fn unreadable(path: &Path) -> Walk {
    Walk::Undetermined {
        reason: format!(
            "a device in the volume's stack could not be read: {}",
            path.display()
        ),
    }
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

fn presence(path: &Path) -> Result<Presence, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(Presence::Present),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Presence::Missing),
        Err(error) => Err(unreadable_reason(path, error.kind())),
    }
}

/// The same question for a directory. `fs::metadata` follows, deliberately:
/// every entry under /sys/dev/block is a symlink to the device's real
/// directory, so a non-following check would call every live device a
/// non-directory. A link that resolves to nothing is an error, not an absence.
fn directory_presence(path: &Path) -> Result<Presence, String> {
    match presence(path)? {
        Presence::Missing => Ok(Presence::Missing),
        Presence::Present => match fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => Ok(Presence::Present),
            Ok(_) => Err(unreadable_reason(path, std::io::ErrorKind::NotADirectory)),
            Err(error) => Err(unreadable_reason(path, error.kind())),
        },
    }
}

fn unreadable_reason(path: &Path, kind: std::io::ErrorKind) -> String {
    format!(
        "a device in the volume's stack could not be read: {} ({kind:?})",
        path.display()
    )
}

fn over_budget(path: &Path) -> String {
    format!("{} exceeded Beacon's read budget", path.display())
}

/// Reads one bounded value, and can tell a complete read from a truncated one.
///
/// `take(budget)` alone cannot: it returns the first `budget` bytes of a larger
/// file and looks exactly like a whole small file. Reading one byte past the
/// budget makes the overflow visible, and an overflowing read is no answer.
fn read_sysfs(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|error| unreadable_reason(path, error.kind()))?;
    let mut bytes = Vec::new();
    file.take(MAX_SYSFS_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| unreadable_reason(path, error.kind()))?;
    if bytes.len() as u64 > MAX_SYSFS_FILE_BYTES {
        return Err(over_budget(path));
    }
    let value = String::from_utf8(bytes)
        .map_err(|_| unreadable_reason(path, std::io::ErrorKind::InvalidData))?;
    Ok(value.trim().to_owned())
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
        let recorded = bounded(
            "device-mapper uuid prefix CRYPT-LUKS2, mapping luks-4f8c1d2e9a7b6c5d4e3f2a1b0c9d8e7f"
                .to_owned(),
        );
        assert!(recorded.contains("CRYPT-LUKS2"));
        assert!(!recorded.contains("4f8c1d2e9a7b6c5d4e3f2a1b0c9d8e7f"));
        assert!(recorded.contains('\u{2026}'));
        // Short hexadecimal text is ordinary text and survives.
        assert_eq!(bounded("dm-0 ext4".to_owned()), "dm-0 ext4");
    }
}
