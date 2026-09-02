//! Live-data disk-encryption readings.
//!
//! Every case here fixes one plausible wrong answer and proves the reader does
//! not give it. The shape that matters is the asymmetry: a reading that fails
//! must land on `Unknown`, never on `Enabled` and never on `Disabled`.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use beacond::{
    LiveDataEncryption, LiveDataEncryptionReport, MAX_RENDERED_CONCLUSION_CHARS, ReasonCode,
    SubjectRole, SystemView, disk_encryption_detection_candidate_for_test,
    live_data_encryption_for_test, render_conclusion,
};
use uuid::Uuid;

const OBSERVED_AT_MS: u64 = 1_756_000_000_000;

struct Fixture {
    root: PathBuf,
    store: PathBuf,
    sys: PathBuf,
    mountinfo: PathBuf,
    /// The store's real device number, and the majors this fixture invents on.
    host: String,
    disk_major: u32,
    mapped_major: u32,
    foreign_major: u32,
    /// Every device number this fixture handed out. `node` refuses anything
    /// else, so a literal cannot creep back in.
    invented: RefCell<HashSet<String>>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        Self::under(std::env::temp_dir(), label)
    }

    /// A fixture whose store sits on a memory-backed filesystem, so the kernel
    /// reports an ANONYMOUS device number for it (major 0). That is the only
    /// way to reach the block-device-of-source path: the reader skips the
    /// mount-versus-`stat` corroboration exactly when `stat` cannot corroborate.
    /// `None` when this host has no such filesystem to build on, which the
    /// caller reports rather than passing quietly.
    fn on_anonymous_device(label: &str) -> Option<Self> {
        let base = PathBuf::from("/dev/shm");
        if !base.is_dir() {
            return None;
        }
        let fixture = Self::under(base, label);
        fixture.host_device().starts_with("0:").then_some(fixture)
    }

    fn under(base: PathBuf, label: &str) -> Self {
        let root = base.join(format!("beacon-diskenc-{label}-{}", Uuid::new_v4()));
        let sys = root.join("sys");
        fs::create_dir_all(&sys).unwrap();
        let store = root.join("beacon.sqlite3");
        fs::write(&store, b"").unwrap();
        let raw = fs::metadata(&store).unwrap().dev();
        let host_major = libc::major(raw);
        // Majors for invented nodes, each moved off the store's real one.
        //
        // A fixture that invents the number the store is already on is not
        // describing a second device. The walk starts at the store's number, so
        // that "child" is its own ancestor and a fan-out silently becomes a
        // cycle. This is not hypothetical: node-a's /tmp is on 259:2 and the CI
        // runner's is on 8:1, so `many-leaves` (which invented 8:0..8:11) read
        // `SomePathsUncovered` here and `NoPhysicalLeaf` there -- a rendered
        // answer decided by the host, which is the class this module forbids.
        let away = |preferred: u32| {
            if preferred == host_major {
                host_major + 1_000
            } else {
                preferred
            }
        };
        Self {
            mountinfo: root.join("mountinfo"),
            host: format!("{host_major}:{}", libc::minor(raw)),
            disk_major: away(8),
            mapped_major: away(253),
            foreign_major: away(9),
            invented: RefCell::new(HashSet::new()),
            store,
            sys,
            root,
        }
    }

    /// The device number the kernel actually reports for this fixture's store.
    ///
    /// The reader corroborates the mount table against `stat`, so a fixture
    /// that invented a device number could never reach the block-stack walk.
    /// Using the real one keeps that corroboration live rather than stubbed.
    fn host_device(&self) -> String {
        self.host.clone()
    }

    /// A plain disk or partition this fixture invents.
    fn disk(&self, minor: usize) -> String {
        self.invent(self.disk_major, minor)
    }

    /// A device-mapper node this fixture invents.
    fn mapped(&self, minor: usize) -> String {
        self.invent(self.mapped_major, minor)
    }

    /// A device the store is provably NOT on, for the cases that need a mount
    /// entry to disagree with `stat`. A literal could agree by accident on a
    /// host that happens to be on it.
    fn foreign(&self, minor: usize) -> String {
        self.invent(self.foreign_major, minor)
    }

    fn invent(&self, major: u32, minor: usize) -> String {
        let device = format!("{major}:{minor}");
        assert_ne!(
            device, self.host,
            "an invented device must differ from the store's own"
        );
        self.invented.borrow_mut().insert(device.clone());
        device
    }

    fn mount_lines(&self, lines: &[String]) {
        fs::write(&self.mountinfo, format!("{}\n", lines.join("\n"))).unwrap();
    }

    fn mount_line(&self, device: &str, fstype: &str, source: &str) -> String {
        self.known(device);
        format!(
            "36 25 {device} / {} rw,relatime shared:1 - {fstype} {source} rw",
            self.root.display()
        )
    }

    /// The ordinary case: one mount covering the store. A real, non-anonymous
    /// `st_dev` keeps the corroboration live. On an anonymous fixture host,
    /// `stat` cannot corroborate by definition, so choose one fixture-owned
    /// non-anonymous mount device rather than letting `/tmp` vs `/dev/shm`
    /// decide whether the candidate reaches its stack walk.
    fn mounted_on_host_device(&self) -> String {
        let device = if self.host.starts_with("0:") {
            self.foreign(0)
        } else {
            self.host_device()
        };
        self.mount_lines(&[self.mount_line(&device, "ext4", "/dev/nvme0n1p2")]);
        device
    }

    fn node(&self, device: &str) -> PathBuf {
        self.known(device);
        let path = self.sys.join(device);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn dm(&self, device: &str, uuid: &str, name: &str) {
        let dm = self.node(device).join("dm");
        fs::create_dir_all(&dm).unwrap();
        fs::write(dm.join("uuid"), format!("{uuid}\n")).unwrap();
        fs::write(dm.join("name"), format!("{name}\n")).unwrap();
    }

    /// `slaves/<name>` is a symlink to the parent device in real sysfs; a real
    /// directory holding the same `dev` file reads identically.
    fn slaves(&self, device: &str, children: &[(&str, &str)]) {
        for (name, child) in children {
            self.known(child);
            let entry = self.node(device).join("slaves").join(name);
            fs::create_dir_all(&entry).unwrap();
            fs::write(entry.join("dev"), format!("{child}\n")).unwrap();
        }
    }

    /// Every device a fixture touches is either the store's own -- where the
    /// walk starts -- or one this fixture invented, which is kept off the
    /// store's number. A bare literal would describe a different stack on a
    /// host whose store happens to be on it.
    fn known(&self, device: &str) {
        assert!(
            device == self.host || self.invented.borrow().contains(device),
            "device {device} is neither the store's own nor invented by this fixture"
        );
    }

    fn view(&self) -> SystemView {
        SystemView::new(self.mountinfo.clone(), self.sys.clone())
    }

    fn read(&self) -> LiveDataEncryptionReport {
        disk_encryption_detection_candidate_for_test(&self.store, &self.view(), OBSERVED_AT_MS)
    }

    fn read_v1(&self) -> LiveDataEncryptionReport {
        live_data_encryption_for_test(&self.store, &self.view(), OBSERVED_AT_MS)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

/// The rendered sentence, for cases that care what an operator reads.
fn reason(report: &LiveDataEncryptionReport) -> String {
    render_conclusion(report.conclusion())
}

/// The closed reason code, which is what most cases should assert: a code
/// cannot drift the way a substring match against a sentence can.
fn code(report: &LiveDataEncryptionReport) -> ReasonCode {
    report.conclusion().reason()
}

fn mechanism(report: &LiveDataEncryptionReport) -> String {
    report
        .conclusion()
        .subjects()
        .iter()
        .filter(|subject| subject.role() == SubjectRole::Mechanism)
        .map(|subject| subject.value().to_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

fn evidence_text(report: &LiveDataEncryptionReport) -> String {
    report
        .evidence()
        .iter()
        .map(|entry| format!("{} => {}", entry.source(), entry.observation()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_unknown_only_v1(fixture: &Fixture) {
    let report = fixture.read_v1();
    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(
        report.conclusion().reason(),
        ReasonCode::DetectionNotEvaluated
    );
    assert!(
        !report.evidence().is_empty(),
        "unknown-only v1 still carries what the probe read"
    );
}

/// The harness's own property, and the reason it has one.
///
/// `every_conclusion_survives_its_components_and_fits_the_adapter` passed on
/// node-a and failed on CI at `f65f819`: /tmp is 259:2 here and 8:1 on the
/// runner, and `many-leaves` invented children 8:0..8:11. On the runner one
/// invented "child" WAS the store's own device, so the fan-out was a cycle and
/// the reader answered `NoPhysicalLeaf` -- a rendered answer decided by the
/// host, which is exactly the class this module forbids. Every invented number
/// is now drawn from a major the store is not on.
#[test]
fn invented_device_numbers_never_share_the_stores_major() {
    let fixture = Fixture::new("invented-numbers");
    let host = fixture.host_device();
    let host_major = host.split_once(':').unwrap().0;
    for minor in 0..66 {
        for invented in [
            fixture.disk(minor),
            fixture.mapped(minor),
            fixture.foreign(minor),
        ] {
            assert_ne!(invented, host, "invented {invented} is the store's own");
            assert_ne!(
                invented.split_once(':').unwrap().0,
                host_major,
                "invented {invented} shares the store's major; some host is on it"
            );
        }
    }
}

/// The guard that keeps the property from decaying: a bare literal is refused,
/// so the next fixture cannot reintroduce the collision by writing "8:1".
#[test]
#[should_panic(expected = "neither the store's own nor invented")]
fn a_fixture_refuses_a_device_number_it_did_not_invent() {
    let fixture = Fixture::new("literal-device");
    let host_major: u32 = fixture
        .host_device()
        .split_once(':')
        .unwrap()
        .0
        .parse()
        .unwrap();
    // Neither the store's own nor from an allocator, on any host.
    fixture.node(&format!("{}:0", host_major + 2_000));
}

#[test]
fn luks2_volume_reports_enabled_with_the_dm_uuid_it_read() {
    let fixture = Fixture::new("luks2");
    let device = fixture.mounted_on_host_device();
    fixture.dm(
        &device,
        "CRYPT-LUKS2-a1b2c3d4e5f60718293a4b5c6d7e8f90-live",
        "live",
    );
    // A crypt mapping sits on a partition. Without one the stack terminates
    // nowhere, and an unread stack is not an encrypted one.
    fixture.slaves(&device, &[("sda2", &fixture.disk(2))]);
    fixture.node(&fixture.disk(2));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Enabled);
    assert_eq!(mechanism(&report), "dm-crypt (LUKS2)");
    assert!(evidence_text(&report).contains("CRYPT-LUKS2"));
    assert_unknown_only_v1(&fixture);
}

#[test]
fn luks_under_lvm_reports_enabled_through_the_slaves_chain() {
    let fixture = Fixture::new("lvm-on-luks");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(&device, &[("dm-0", &fixture.mapped(0))]);
    fixture.dm(
        &fixture.mapped(0),
        "CRYPT-LUKS2-0011223344556677889900aabbccddee-root",
        "root",
    );
    fixture.slaves(&fixture.mapped(0), &[("sda2", &fixture.disk(2))]);
    fixture.node(&fixture.disk(2));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Enabled);
    assert_eq!(mechanism(&report), "dm-crypt (LUKS2)");
}

#[test]
fn encrypted_branch_does_not_mask_a_plain_sibling() {
    let fixture = Fixture::new("mixed-stack");
    let device = fixture.mounted_on_host_device();
    // One volume group over two physical volumes: one LUKS2, one plain. Extents
    // on the plain member are not protected by the crypt layer on its sibling.
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(
        &device,
        &[("dm-0", &fixture.mapped(0)), ("sdb1", &fixture.disk(17))],
    );
    fixture.dm(
        &fixture.mapped(0),
        "CRYPT-LUKS2-0011223344556677889900aabbccddee-pv0",
        "pv0",
    );
    fixture.slaves(&fixture.mapped(0), &[("sda2", &fixture.disk(2))]);
    fixture.node(&fixture.disk(2));
    fixture.node(&fixture.disk(17));

    let report = fixture.read();

    assert_ne!(report.conclusion().state(), LiveDataEncryption::Enabled);
    assert_eq!(report.conclusion().state(), LiveDataEncryption::Disabled);
    // The candidate conclusion must name the branch that is not covered.
    assert!(report.conclusion().subjects().iter().any(|subject| {
        subject.role() == SubjectRole::Device && subject.value() == fixture.disk(17)
    }));
    // And still say what it did find, so the operator is not told "nothing".
    assert!(evidence_text(&report).contains("CRYPT-LUKS2"));
    // The reason must not deny the encryption this very report observed. An
    // operator who can see CRYPT-LUKS2 in the evidence and reads "found no
    // encryption" learns to disbelieve the warning.
    let reason = reason(&report);
    assert!(reason.contains("pv0 is encrypted"), "{reason}");
    assert!(reason.contains("8:17 is not"), "{reason}");
    assert!(!reason.contains("found no"), "{reason}");
    assert_unknown_only_v1(&fixture);
}

#[test]
fn review_r3_long_mixed_mapping_names_preserve_the_disabled_conclusion() {
    let fixture = Fixture::new("long-names");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(
        &device,
        &[
            ("dm-0", &fixture.mapped(0)),
            ("dm-1", &fixture.mapped(1)),
            ("sdb1", &fixture.disk(17)),
        ],
    );
    // Valid, non-hex, and long enough that concatenating the names would spend
    // the whole 200-character bound before either half of the sentence.
    let first = "encrypted-volume-".repeat(20);
    let second = "another-encrypted-volume-".repeat(12);
    fixture.dm(
        &fixture.mapped(0),
        "CRYPT-LUKS2-0011223344556677889900aabbccddee-a",
        &first,
    );
    fixture.slaves(&fixture.mapped(0), &[("sda2", &fixture.disk(2))]);
    fixture.dm(
        &fixture.mapped(1),
        "CRYPT-LUKS2-1122334455667788990011bbccddeeff-b",
        &second,
    );
    fixture.slaves(&fixture.mapped(1), &[("sda3", &fixture.disk(3))]);
    fixture.node(&fixture.disk(2));
    fixture.node(&fixture.disk(3));
    fixture.node(&fixture.disk(17));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Disabled);
    // The conclusion is not evidence and is never truncated away: both halves
    // of the sentence survive names long enough to consume the bound.
    let reason = reason(&report);
    assert!(reason.contains("is encrypted"), "{reason}");
    assert!(reason.contains("8:17 is not"), "{reason}");
    assert!(
        reason.chars().count() <= 200,
        "{} chars",
        reason.chars().count()
    );
    // The names are shortened rather than dropped, and say that they were.
    assert!(reason.contains('\u{2026}'), "{reason}");
}

#[test]
fn every_conclusion_survives_its_components_and_fits_the_adapter() {
    // The composition edges gpt1 named, each asserted on both halves of the
    // ruling: the sentence stays complete, and the adapter's 200-character
    // bound is met without the sentence being what got cut.
    fn conclusion(fixture: &Fixture) -> String {
        let report = fixture.read();
        // Bounded by construction now: every subject is capped when it is made,
        // so this asserts the property holds rather than that a cut happened.
        for subject in report.conclusion().subjects() {
            assert!(subject.value().chars().count() <= 24);
        }
        let text = render_conclusion(report.conclusion());
        assert!(
            text.chars().count() <= MAX_RENDERED_CONCLUSION_CHARS,
            "{} chars: {text}",
            text.chars().count()
        );
        for entry in report.evidence() {
            assert!(entry.source().chars().count() <= 200);
            assert!(entry.observation().chars().count() <= 200);
        }
        text
    }

    // A 300-character mapping with no backing device.
    let unbacked = Fixture::new("unbacked-long-name");
    let device = unbacked.mounted_on_host_device();
    unbacked.dm(
        &device,
        "CRYPT-LUKS2-a1b2c3d4e5f60718293a4b5c6d7e8f90-x",
        &"unbacked-mapping-".repeat(20),
    );
    fs::create_dir_all(unbacked.node(&device).join("slaves")).unwrap();
    let reason = conclusion(&unbacked);
    assert!(
        reason.contains("has no readable backing device"),
        "{reason}"
    );
    assert!(reason.contains('\u{2026}'), "{reason}");

    // Many uncovered leaves under one plain mapping.
    let many = Fixture::new("many-leaves");
    let device = many.mounted_on_host_device();
    many.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0");
    let children: Vec<(String, String)> = (0..12)
        .map(|index| (format!("sd{index}"), many.disk(index)))
        .collect();
    let borrowed: Vec<(&str, &str)> = children
        .iter()
        .map(|(name, dev)| (name.as_str(), dev.as_str()))
        .collect();
    many.slaves(&device, &borrowed);
    for (_, dev) in &children {
        many.node(dev);
    }
    let reason = conclusion(&many);
    assert!(reason.contains("no crypt layer"), "{reason}");
    // Twelve uncovered leaves, three named: the count says the rest exist.
    assert!(reason.contains("and 9 more"), "{reason}");

    // Many distinct crypt mechanisms on one covered stack.
    let mechanisms = Fixture::new("many-mechanisms");
    let device = mechanisms.mounted_on_host_device();
    mechanisms.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0");
    let kinds = ["LUKS1", "LUKS2", "BITLK", "PLAIN", "TEMP", "VERITY"];
    let names: Vec<(String, String)> = kinds
        .iter()
        .enumerate()
        .map(|(index, _)| (format!("dm-{index}"), mechanisms.mapped(index)))
        .collect();
    let borrowed: Vec<(&str, &str)> = names
        .iter()
        .map(|(name, dev)| (name.as_str(), dev.as_str()))
        .collect();
    mechanisms.slaves(&device, &borrowed);
    for (index, kind) in kinds.iter().enumerate() {
        let dev = mechanisms.mapped(index);
        mechanisms.dm(
            &dev,
            &format!("CRYPT-{kind}-0011223344556677889900aabbccdd{index:02}-m"),
            &format!("mapping-{index}"),
        );
        mechanisms.slaves(&dev, &[("disk", &mechanisms.disk(index))]);
        mechanisms.node(&mechanisms.disk(index));
    }
    let mechanism = conclusion(&mechanisms);
    assert!(mechanism.contains("dm-crypt"), "{mechanism}");
    // Six mechanisms, three named and three counted.
    assert!(mechanism.contains("and 3 more"), "{mechanism}");
}

#[test]
fn shortened_evidence_says_that_it_was_shortened() {
    let fixture = Fixture::new("marked-evidence");
    let device = fixture.mounted_on_host_device();
    // A mapping name past the evidence bound. Silent truncation makes a
    // shortened observation read as the whole value.
    fixture.dm(
        &device,
        "CRYPT-LUKS2-a1b2c3d4e5f60718293a4b5c6d7e8f90-x",
        &"evidence-mapping-".repeat(20),
    );
    fixture.slaves(&device, &[("sda2", &fixture.disk(2))]);
    fixture.node(&fixture.disk(2));

    let report = fixture.read();

    let text = evidence_text(&report);
    assert!(text.contains('\u{2026}'), "{text}");
    for entry in report.evidence() {
        assert!(entry.observation().chars().count() <= 200);
    }

    // The FLAG, not the ellipsis. Forcing every `truncated` to false left this
    // test green while the panel said nothing was shortened, because an
    // ellipsis from identifier redaction in one field satisfied an assertion
    // about a cut in the other.
    let cut: Vec<&str> = report
        .evidence()
        .iter()
        .filter(|entry| entry.truncated())
        .map(|entry| entry.observation())
        .collect();
    assert_eq!(cut.len(), 1, "exactly one reading was cut: {text}");
    assert_eq!(cut[0].chars().count(), 200);
    // And a reading that was NOT cut does not claim it was.
    assert!(
        report
            .evidence()
            .iter()
            .any(|entry| !entry.truncated() && entry.observation().chars().count() < 200),
        "{text}"
    );
}

/// The count is about what the reading found, not about what the caller
/// remembered to pass. Four encrypted mappings and one uncovered device is five
/// subjects; three fit; two are omitted. `crypt_seen.iter().take(1)` trimmed the
/// mappings before the conclusion saw them, so this said 0 -- a capped list
/// claiming it was whole -- and the mapping the sentence names could be crowded
/// out by the devices beside it.
#[test]
fn a_mixed_stack_counts_every_subject_that_did_not_fit() {
    let fixture = Fixture::new("mixed-omissions");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0");
    let mut children: Vec<(String, String)> = (0..4)
        .map(|index| (format!("dm-{index}"), fixture.mapped(index)))
        .collect();
    children.push(("sdb1".to_owned(), fixture.disk(17)));
    let borrowed: Vec<(&str, &str)> = children
        .iter()
        .map(|(name, dev)| (name.as_str(), dev.as_str()))
        .collect();
    fixture.slaves(&device, &borrowed);
    for index in 0..4 {
        let mapping = fixture.mapped(index);
        fixture.dm(
            &mapping,
            &format!("CRYPT-LUKS2-a1b2c3d4e5f60718293a4b5c6d7e8f9{index}-m"),
            &format!("crypt-{index}"),
        );
        fixture.slaves(&mapping, &[("disk", &fixture.disk(index))]);
        fixture.node(&fixture.disk(index));
    }
    fixture.node(&fixture.disk(17));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Disabled);
    assert_eq!(code(&report), ReasonCode::SomePathsUncovered);
    assert_eq!(report.conclusion().omitted_subjects(), 2);
    // And the role the sentence names survived the cap: a conclusion that
    // spent every slot on mappings would render "an unnamed subject" where the
    // uncovered device belongs.
    for role in [SubjectRole::Mapping, SubjectRole::Device] {
        assert!(
            report
                .conclusion()
                .subjects()
                .iter()
                .any(|subject| subject.role() == role),
            "{role:?} was crowded out: {}",
            reason(&report)
        );
    }
    let rendered = reason(&report);
    assert!(rendered.contains("and 2 more"), "{rendered}");
    assert!(!rendered.contains("an unnamed subject"), "{rendered}");
}

/// A filesystem that exposes no block device names itself, and the name is
/// bounded like every other subject. Reaching this path needs a store whose own
/// device number is anonymous, which is the named condition below.
#[test]
fn an_anonymous_filesystem_names_itself_within_the_bound() {
    let Some(fixture) = Fixture::on_anonymous_device("anonymous-fs") else {
        // Not a silent pass: this host has no memory-backed filesystem to build
        // the case on, and the reader's anonymous-device path went untested.
        eprintln!(
            "SKIPPED an_anonymous_filesystem_names_itself_within_the_bound: \
             /dev/shm is absent or not anonymous on this host"
        );
        return;
    };
    let long_fstype = "beaconfs-".repeat(60);
    fixture.mount_lines(&[fixture.mount_line(
        &fixture.host_device(),
        &long_fstype,
        "not-an-absolute-path",
    )]);

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::NoBlockDeviceForFilesystem);
    let named = report
        .conclusion()
        .subjects()
        .iter()
        .find(|subject| subject.role() == SubjectRole::Location)
        .expect("the filesystem names itself");
    assert!(named.shortened());
    assert!(named.value().chars().count() <= 24);
    let rendered = reason(&report);
    assert!(
        rendered.chars().count() <= MAX_RENDERED_CONCLUSION_CHARS,
        "{rendered}"
    );
}

#[test]
fn review_crypt_mapping_without_a_physical_leaf_is_unknown() {
    let fixture = Fixture::new("crypt-no-leaf");
    let device = fixture.mounted_on_host_device();
    // A crypt mapping over nothing. Real LUKS always sits on a partition or a
    // disk; a mapping whose backing device Beacon never saw is an unread stack,
    // not an encrypted one.
    fixture.dm(
        &device,
        "CRYPT-LUKS2-a1b2c3d4e5f60718293a4b5c6d7e8f90-live",
        "live",
    );
    fs::create_dir_all(fixture.node(&device).join("slaves")).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::MappingWithoutBackingDevice);
}

#[test]
fn review_one_covered_leaf_cannot_mask_a_cyclic_sibling() {
    let fixture = Fixture::new("covered-plus-cycle");
    let device = fixture.mounted_on_host_device();
    fixture.dm(
        &device,
        "CRYPT-LUKS2-aabbccddeeff00112233445566778899-live",
        "live",
    );
    fixture.slaves(
        &device,
        &[("sda2", &fixture.disk(2)), ("dm-9", &fixture.mapped(9))],
    );
    fixture.node(&fixture.disk(2));
    // One branch terminates at a real disk under the crypt layer. The other
    // never terminates, so the volume's topology was never established -- and a
    // sibling that finished is not proof about the branch that did not.
    fixture.node(&fixture.mapped(9));
    fixture.slaves(&fixture.mapped(9), &[("loop", &fixture.mapped(9))]);

    let report = fixture.read();

    assert_ne!(report.conclusion().state(), LiveDataEncryption::Enabled);
    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
}

#[test]
fn review_crypt_above_a_nested_fan_out_covers_both_plain_leaves() {
    let fixture = Fixture::new("nested-fan-out");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-1122334455667788990011223344556677", "outer");
    fixture.slaves(&device, &[("dm-0", &fixture.mapped(0))]);
    fixture.dm(
        &fixture.mapped(0),
        "CRYPT-LUKS2-0011223344556677889900aabbccddee-crypt",
        "crypt",
    );
    fixture.slaves(&fixture.mapped(0), &[("dm-1", &fixture.mapped(1))]);
    fixture.dm(
        &fixture.mapped(1),
        "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0",
        "inner",
    );
    fixture.slaves(
        &fixture.mapped(1),
        &[("sda2", &fixture.disk(2)), ("sdb1", &fixture.disk(17))],
    );
    fixture.node(&fixture.disk(2));
    fixture.node(&fixture.disk(17));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Enabled);
    assert_eq!(mechanism(&report), "dm-crypt (LUKS2)");
}

#[test]
fn review_unreadable_leaf_next_to_a_luks_branch_is_unknown() {
    let fixture = Fixture::new("unreadable-sibling");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(
        &device,
        &[("dm-0", &fixture.mapped(0)), ("sdb1", &fixture.disk(17))],
    );
    fixture.dm(
        &fixture.mapped(0),
        "CRYPT-LUKS2-0011223344556677889900aabbccddee-pv0",
        "pv0",
    );
    fixture.slaves(&fixture.mapped(0), &[("sda2", &fixture.disk(2))]);
    fixture.node(&fixture.disk(2));
    // 8:17 is never created: one complete LUKS path, one branch Beacon cannot read.

    let report = fixture.read();

    assert_ne!(report.conclusion().state(), LiveDataEncryption::Enabled);
    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
}

#[test]
fn a_crypt_layer_above_a_plain_leaf_covers_that_path() {
    let fixture = Fixture::new("luks-over-lvm");
    let device = fixture.mounted_on_host_device();
    // LUKS on top of a logical volume: the physical members are plain, but every
    // path to them passes through the crypt layer.
    fixture.dm(
        &device,
        "CRYPT-LUKS2-aabbccddeeff00112233445566778899-live",
        "live",
    );
    fixture.slaves(&device, &[("dm-1", &fixture.mapped(1))]);
    fixture.dm(
        &fixture.mapped(1),
        "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0",
        "vg0-root",
    );
    fixture.slaves(
        &fixture.mapped(1),
        &[("sda2", &fixture.disk(2)), ("sdb1", &fixture.disk(17))],
    );
    fixture.node(&fixture.disk(2));
    fixture.node(&fixture.disk(17));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Enabled);
    assert_eq!(mechanism(&report), "dm-crypt (LUKS2)");
}

#[test]
fn a_stack_that_reaches_no_physical_device_reports_unknown() {
    let fixture = Fixture::new("no-leaf");
    let device = fixture.mounted_on_host_device();
    // Every branch loops. Nothing terminates, so "every leaf is covered" is
    // vacuously true -- and a vacuous truth must not become `enabled`.
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(&device, &[("dm-1", &fixture.mapped(1))]);
    fixture.node(&fixture.mapped(1));
    fixture.slaves(&fixture.mapped(1), &[("top", &device)]);

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::NoPhysicalLeaf);
}

#[test]
fn lvm_over_plain_disks_reports_disabled_not_enabled() {
    let fixture = Fixture::new("lvm-plain");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(&device, &[("sda2", &fixture.disk(2))]);
    fixture.node(&fixture.disk(2));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Disabled);
    assert!(evidence_text(&report).contains("not a crypt target"));
}

#[test]
fn unreadable_dm_uuid_reports_unknown_not_enabled() {
    let fixture = Fixture::new("uuid-unreadable");
    let device = fixture.mounted_on_host_device();
    // Present but not readable as a value. The distinction the reader must keep
    // is "there is a uuid here I could not read" versus "there is no uuid here".
    // The name IS readable, so only the uuid read can fail this reading -- with
    // both missing the test passed for the wrong reason and a mutant that
    // swallowed the uuid failure alone survived.
    let dm = fixture.node(&device).join("dm");
    fs::create_dir_all(dm.join("uuid")).unwrap();
    fs::write(dm.join("name"), "live\n").unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::DeviceUnreadable);
}

#[test]
fn unreadable_dm_name_reports_unknown() {
    let fixture = Fixture::new("dm-name-unreadable");
    let device = fixture.mounted_on_host_device();
    let dm = fixture.node(&device).join("dm");
    fs::create_dir_all(&dm).unwrap();
    fs::write(
        dm.join("uuid"),
        "CRYPT-LUKS2-a1b2c3d4e5f60718293a4b5c6d7e8f90-live\n",
    )
    .unwrap();
    // The uuid read succeeds and the name read does not. A read that fails
    // after another succeeded is still a read that failed.
    fs::create_dir_all(dm.join("name")).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::DeviceUnreadable);
}

#[test]
fn unreadable_slave_device_number_reports_unknown() {
    let fixture = Fixture::new("slave-unreadable");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fs::create_dir_all(fixture.node(&device).join("slaves").join("sda2")).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::DeviceUnreadable);
}

#[test]
fn failed_metadata_on_loop_backing_file_reports_unknown() {
    let fixture = Fixture::new("loop-metadata");
    let device = fixture.mounted_on_host_device();
    // `loop` is a file, so asking about `loop/backing_file` errors with
    // NotADirectory. A boolean exists() reads that as "no loop device".
    fs::write(fixture.node(&device).join("loop"), b"not a directory\n").unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::DeviceUnreadable);
}

#[test]
fn failed_metadata_on_dm_uuid_reports_unknown() {
    let fixture = Fixture::new("dm-metadata");
    let device = fixture.mounted_on_host_device();
    fs::write(fixture.node(&device).join("dm"), b"not a directory\n").unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::DeviceUnreadable);
}

#[test]
fn failed_metadata_on_slaves_reports_unknown() {
    let fixture = Fixture::new("slaves-metadata");
    let device = fixture.mounted_on_host_device();
    // Not a missing slaves directory -- an unreadable one. The difference
    // between "this device has no parents" and "Beacon could not ask" is the
    // difference between a proven negative and no answer at all.
    fs::write(fixture.node(&device).join("slaves"), b"not a directory\n").unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::DeviceUnreadable);
}

#[test]
fn an_over_budget_mount_table_reports_unknown() {
    let fixture = Fixture::new("mountinfo-budget");
    let device = fixture.host_device();
    let mut lines = vec![fixture.mount_line(&device, "ext4", "/dev/nvme0n1p2")];
    // Past the byte budget. A truncated read parses cleanly and answers about
    // whatever mounts happened to fit, which is a confident wrong answer.
    let filler = "x".repeat(4096);
    while lines.iter().map(String::len).sum::<usize>() <= 4 * 1024 * 1024 {
        lines.push(format!("36 25 9:9 / /pad-{} rw - ext4 /dev/pad rw", filler));
    }
    fixture.mount_lines(&lines);
    fixture.node(&device);

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::ReadOverBudget);
}

#[test]
fn an_over_budget_sysfs_read_reports_unknown() {
    let fixture = Fixture::new("sysfs-budget");
    let device = fixture.mounted_on_host_device();
    let dm = fixture.node(&device).join("dm");
    fs::create_dir_all(&dm).unwrap();
    fs::write(dm.join("uuid"), "C".repeat(8 * 1024)).unwrap();
    fs::write(dm.join("name"), "live\n").unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::ReadOverBudget);
}

#[test]
fn unreadable_sysfs_directory_reports_unknown_not_disabled() {
    let fixture = Fixture::new("sysfs-absent");
    fixture.mounted_on_host_device();
    // No node created: the block-device tree cannot be read at all.

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::DeviceUnreadable);
}

#[test]
fn plain_partition_with_no_dm_layer_reports_disabled_and_names_what_it_cannot_see() {
    let fixture = Fixture::new("plain");
    let device = fixture.mounted_on_host_device();
    fixture.node(&device);

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Disabled);
    // Nothing encrypted was seen here, so the reason says only that.
    assert!(reason(&report).starts_with("at least one path to disk has no crypt layer"));
    assert!(!report.not_verified().is_empty());
    let gaps = report.not_verified().join(" ");
    assert!(gaps.contains("self-encrypting drive"));
    assert!(gaps.contains("fscrypt"));
    assert!(gaps.contains("beneath this host"));
}

#[test]
fn review_dangling_store_symlink_is_broken_not_absent() {
    let fixture = Fixture::new("dangling-store");
    let device = fixture.mounted_on_host_device();
    fixture.node(&device);
    // canonicalize returns NotFound for a link whose target is gone, exactly as
    // it does for a store that was never created. The entry IS there, so the
    // parent is a different subject and answering about it is a wrong answer.
    fs::remove_file(&fixture.store).unwrap();
    std::os::unix::fs::symlink(fixture.root.join("gone.sqlite3"), &fixture.store).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::LinkResolvesToNothing);
}

#[test]
fn review_r3_dangling_loop_symlink_is_broken_not_absent() {
    let fixture = Fixture::new("dangling-loop");
    let device = fixture.mounted_on_host_device();
    let node = fixture.node(&device);
    // `loop` is a link to nothing, so asking about `loop/backing_file` answers
    // NotFound -- the same answer a device with no loop backing gives.
    std::os::unix::fs::symlink(node.join("gone"), node.join("loop")).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::LinkResolvesToNothing);
}

#[test]
fn a_dangling_dm_link_is_broken_not_a_plain_disk() {
    let fixture = Fixture::new("dangling-dm");
    let device = fixture.mounted_on_host_device();
    let node = fixture.node(&device);
    // The same class one directory over: a dangling `dm` link would make a
    // mapping look like a disk with no mapping at all.
    std::os::unix::fs::symlink(node.join("gone"), node.join("dm")).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::LinkResolvesToNothing);
}

#[test]
fn a_dangling_slave_device_link_is_broken_not_a_leaf() {
    let fixture = Fixture::new("dangling-slave-dev");
    let device = fixture.mounted_on_host_device();
    let entry = fixture.node(&device).join("slaves").join("sda2");
    fs::create_dir_all(&entry).unwrap();
    // And one hop further out: the `dev` file behind a slave entry.
    std::os::unix::fs::symlink(entry.join("gone"), entry.join("dev")).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::LinkResolvesToNothing);
}

#[test]
fn a_dangling_dm_uuid_link_is_broken_not_absent() {
    let fixture = Fixture::new("dangling-dm-uuid");
    let device = fixture.mounted_on_host_device();
    let dm = fixture.node(&device).join("dm");
    fs::create_dir_all(&dm).unwrap();
    // The exact composition edge: the `dm` directory is there, the uuid inside
    // it is a link to nothing. One helper answers for every followed link, so
    // this must read the same as the others rather than a generic read error.
    std::os::unix::fs::symlink(dm.join("gone"), dm.join("uuid")).unwrap();
    fs::write(dm.join("name"), "live\n").unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::LinkResolvesToNothing);
}

#[test]
fn a_dangling_dm_name_link_is_broken_not_absent() {
    let fixture = Fixture::new("dangling-dm-name");
    let device = fixture.mounted_on_host_device();
    let dm = fixture.node(&device).join("dm");
    fs::create_dir_all(&dm).unwrap();
    fs::write(
        dm.join("uuid"),
        "CRYPT-LUKS2-a1b2c3d4e5f60718293a4b5c6d7e8f90-x\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(dm.join("gone"), dm.join("name")).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::LinkResolvesToNothing);
}

#[test]
fn a_dangling_slave_entry_link_is_broken_not_absent() {
    let fixture = Fixture::new("dangling-slave-entry");
    let device = fixture.mounted_on_host_device();
    let slaves = fixture.node(&device).join("slaves");
    fs::create_dir_all(&slaves).unwrap();
    // The `../device` hop itself: in real sysfs each slaves entry is a symlink
    // to the parent device's directory, and this one points at nothing.
    std::os::unix::fs::symlink(slaves.join("gone"), slaves.join("sda2")).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::LinkResolvesToNothing);
}

#[test]
fn review_dangling_slaves_symlink_is_broken_not_a_leaf() {
    let fixture = Fixture::new("dangling-slaves");
    let device = fixture.mounted_on_host_device();
    let node = fixture.node(&device);
    // read_dir also answers NotFound for a dangling link, which would make this
    // device look like a disk with no parents.
    std::os::unix::fs::symlink(node.join("gone"), node.join("slaves")).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    // One helper answers this for every link the walk follows, so the wording
    // is the same here as for a dangling store, dm, loop, or dev entry.
    assert_eq!(code(&report), ReasonCode::LinkResolvesToNothing);
}

#[test]
fn an_existing_unresolvable_store_does_not_fall_back_to_its_parent() {
    let fixture = Fixture::new("unresolvable-store");
    let device = fixture.mounted_on_host_device();
    fixture.node(&device);
    // The store is present and cannot be resolved. Its parent still resolves,
    // and the block stack under it reads as a proven negative -- so falling
    // back would answer confidently about a subject that is not the store.
    fs::remove_file(&fixture.store).unwrap();
    std::os::unix::fs::symlink("beacon.sqlite3", &fixture.store).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::StorePathUnresolved);
}

#[test]
fn every_rendered_field_stays_within_the_adapter_bound() {
    // The desktop adapter refuses mountPoint or mechanism text over 200
    // characters. A reason built from a long sysfs path would otherwise make an
    // otherwise-valid report unreadable at the boundary.
    let fixture = Fixture::new("bounded-text-".repeat(6).trim_end_matches('-'));
    fixture.mounted_on_host_device();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    let mount_point = report
        .mount_point()
        .map(|subject| subject.value().chars().count())
        .unwrap_or(0);
    assert!(mount_point <= 24, "mountPoint is {mount_point} characters");
    let rendered = reason(&report).chars().count();
    assert!(rendered <= MAX_RENDERED_CONCLUSION_CHARS);
    for entry in report.evidence() {
        assert!(entry.source().chars().count() <= 200);
        assert!(entry.observation().chars().count() <= 200);
    }
}

#[test]
fn missing_mountinfo_reports_unknown() {
    let fixture = Fixture::new("no-mountinfo");
    fixture.node(&fixture.foreign(1));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::MountTableUnreadable);
}

#[test]
fn unparseable_mount_table_reports_unknown() {
    let fixture = Fixture::new("bad-mountinfo");
    fixture.mount_lines(&["36 25 not-a-device / /somewhere rw - ext4 /dev/sda1 rw".to_owned()]);

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::MountTableUnparsed);
}

#[test]
fn mount_table_and_stat_disagreement_reports_unknown() {
    let fixture = Fixture::new("disagree");
    fixture.mount_lines(&[fixture.mount_line(&fixture.foreign(9), "ext4", "/dev/sdz1")]);
    fixture.node(&fixture.foreign(9));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::MountAndStatDisagree);
}

#[test]
fn shadowed_mount_point_selects_the_last_entry() {
    let fixture = Fixture::new("shadowed");
    let device = fixture.host_device();
    // Same mount point twice. The later mount is the live one; selecting the
    // earlier entry answers about a volume nothing is stored on.
    fixture.mount_lines(&[
        fixture.mount_line(&fixture.foreign(9), "ext4", "/dev/sdz1"),
        fixture.mount_line(&device, "ext4", "/dev/nvme0n1p2"),
    ]);
    fixture.node(&device);

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Disabled);
}

#[test]
fn tmpfs_backed_store_reports_unknown_with_its_reason() {
    let fixture = Fixture::new("tmpfs");
    let device = fixture.host_device();
    fixture.mount_lines(&[fixture.mount_line(&device, "tmpfs", "tmpfs")]);
    fixture.node(&device);

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::MemoryBackedFilesystem);
}

#[test]
fn overlay_mounted_store_reports_unknown() {
    let fixture = Fixture::new("overlay");
    let device = fixture.host_device();
    // An overlay's layers are decided outside this mount, so the block device
    // under it answers a different question than the one being asked.
    fixture.mount_lines(&[fixture.mount_line(&device, "overlay", "overlay")]);
    fixture.node(&device);

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::OverlayFilesystem);
}

#[test]
fn network_filesystem_reports_unknown_with_its_reason() {
    let fixture = Fixture::new("nfs");
    let device = fixture.host_device();
    fixture.mount_lines(&[fixture.mount_line(&device, "nfs4", "server:/export")]);
    fixture.node(&device);

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::NetworkFilesystem);
}

#[test]
fn loopback_backed_volume_reports_unknown() {
    let fixture = Fixture::new("loop");
    let device = fixture.mounted_on_host_device();
    let loop_dir = fixture.node(&device).join("loop");
    fs::create_dir_all(&loop_dir).unwrap();
    fs::write(loop_dir.join("backing_file"), "/srv/images/beacon.img\n").unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    let reason = reason(&report);
    assert!(reason.contains("loopback image"));
    // The backing path is recorded so a later slice can follow it.
    assert!(reason.contains("/srv/images/beacon.img"));
    assert!(reason.chars().count() <= 200);
}

#[test]
fn a_long_backing_file_shortens_the_path_not_the_sentence() {
    let fixture = Fixture::new("long-backing");
    let device = fixture.mounted_on_host_device();
    let loop_dir = fixture.node(&device).join("loop");
    fs::create_dir_all(&loop_dir).unwrap();
    // backing_file is a sysfs value, so it can be as long as the read budget.
    let path = format!("/srv/{}/beacon.img", "very-long-directory-name/".repeat(40));
    fs::write(loop_dir.join("backing_file"), format!("{path}\n")).unwrap();

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    let reason = reason(&report);
    // The sentence survives; the path is shortened and says so.
    assert!(reason.contains("loopback image"), "{reason}");
    assert!(
        reason.contains("Beacon does not follow the backing file"),
        "{reason}"
    );
    assert!(reason.contains('\u{2026}'), "{reason}");
    assert!(
        reason.chars().count() <= 200,
        "{} chars",
        reason.chars().count()
    );
}

#[test]
fn device_stack_cycle_terminates_within_budget() {
    let fixture = Fixture::new("cycle");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(&device, &[("dm-1", &fixture.mapped(1))]);
    fixture.node(&fixture.mapped(1));
    fixture.slaves(&fixture.mapped(1), &[("top", &device)]);

    let report = fixture.read();

    // The point is that this returns at all rather than looping. It is no longer
    // a proven negative: nothing in a cycle terminates, so the walk reaches no
    // physical device and has proved nothing either way. Asserting the reason,
    // not just the state, keeps the visited-set mutant dead -- without it the
    // walk exhausts the node budget and says so instead.
    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert_eq!(code(&report), ReasonCode::NoPhysicalLeaf);
}

#[test]
fn a_wide_slaves_directory_stops_at_the_budget_before_reading_it_all() {
    let fixture = Fixture::new("wide-fan-out");
    let device = fixture.mounted_on_host_device();
    // More children than the node budget allows. Every dev file is valid, and
    // no child device node exists -- so if the budget is enforced only AFTER
    // the fan-out is enumerated, the walk reads all 66 dev files, explores the
    // first child, and reports "could not be read". Enforced during
    // enumeration, it stops and says so. The two reasons are distinguishable
    // whatever order read_dir returns.
    let children: Vec<(String, String)> = (0..66)
        .map(|index| (format!("sd{index}"), fixture.disk(index)))
        .collect();
    let borrowed: Vec<(&str, &str)> = children
        .iter()
        .map(|(name, dev)| (name.as_str(), dev.as_str()))
        .collect();
    fixture.slaves(&device, &borrowed);

    // Poison EVERY entry past the ceiling, in the order read_dir will actually
    // return -- captured here rather than assumed. Each poisoned `dev` becomes
    // a directory, so reading one fails with a reason no budget stop produces.
    //
    // Poisoning only the last entry was not enough: an implementation that
    // collected every DirEntry into a Vec and then checked the budget passed
    // that guard, because collecting does not open `dev`. Poisoning the whole
    // tail means any enumeration that walks past the ceiling meets one.
    let slaves = fixture.node(&device).join("slaves");
    let order: Vec<_> = fs::read_dir(&slaves)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(order.len() > 64, "the fan-out must exceed the node ceiling");
    for entry in order.iter().skip(63) {
        let poison = entry.join("dev");
        fs::remove_file(&poison).unwrap();
        fs::create_dir(&poison).unwrap();
    }

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    let reason = reason(&report);
    assert!(reason.contains("read budget"), "{reason}");
    assert!(!reason.contains("could not be read"), "{reason}");
}

#[test]
fn device_stack_deeper_than_the_budget_reports_unknown() {
    let fixture = Fixture::new("deep");
    let device = fixture.mounted_on_host_device();
    fixture.slaves(&device, &[("dm-0", &fixture.mapped(0))]);
    for step in 0..12 {
        let child = fixture.mapped(step + 1);
        fixture.node(&fixture.mapped(step));
        fixture.slaves(&fixture.mapped(step), &[("next", &child)]);
    }
    fixture.node(&fixture.mapped(12));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("read budget"));
}

#[test]
fn dm_uuid_prefix_only_appears_in_evidence() {
    let fixture = Fixture::new("prefix-only");
    let device = fixture.mounted_on_host_device();
    // Distributions commonly name the mapping after the volume identifier, so
    // the identifier reaches the record through dm/name as well as dm/uuid.
    fixture.dm(
        &device,
        "CRYPT-LUKS2-4f8c1d2e9a7b6c5d4e3f2a1b0c9d8e7f-luks-4f8c1d2e9a7b6c5d4e3f2a1b0c9d8e7f",
        "luks-4f8c1d2e9a7b6c5d4e3f2a1b0c9d8e7f",
    );
    fixture.slaves(&device, &[("sda2", &fixture.disk(2))]);
    fixture.node(&fixture.disk(2));

    let report = fixture.read();

    assert_eq!(report.conclusion().state(), LiveDataEncryption::Enabled);
    let text = evidence_text(&report);
    assert!(text.contains("CRYPT-LUKS2"));
    assert!(!text.contains("4f8c1d2e9a7b6c5d4e3f2a1b0c9d8e7f"));
}

#[test]
fn two_calls_re_read_live_state() {
    let fixture = Fixture::new("recheck");
    let device = fixture.mounted_on_host_device();
    fixture.node(&device);

    let before = fixture.read();
    assert_eq!(before.conclusion().state(), LiveDataEncryption::Disabled);

    fixture.dm(
        &device,
        "CRYPT-LUKS2-aabbccddeeff00112233445566778899-live",
        "live",
    );
    fixture.slaves(&device, &[("sda2", &fixture.disk(2))]);
    fixture.node(&fixture.disk(2));
    let after = fixture.read();

    assert_eq!(after.conclusion().state(), LiveDataEncryption::Enabled);
}

#[test]
fn stat_evidence_reports_the_hosts_own_device_number() {
    let fixture = Fixture::new("stat");
    let device = fixture.mounted_on_host_device();
    fixture.node(&device);

    let report = fixture.read();

    let stat_line = report
        .evidence()
        .iter()
        .find(|entry| entry.source() == "stat(live store)")
        .expect("the reading records the device it stat'd");
    assert_eq!(stat_line.observation(), format!("st_dev = {device}"));
    // The mount point is a bounded subject now, not a raw path: present, in
    // its own role, and inside the subject bound whatever the fixture is named.
    let mount_point = report.mount_point().expect("a mount point");
    assert_eq!(mount_point.role(), SubjectRole::Location);
    assert!(mount_point.value().chars().count() <= 24);
}
