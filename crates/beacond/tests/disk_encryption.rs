//! Live-data disk-encryption readings.
//!
//! Every case here fixes one plausible wrong answer and proves the reader does
//! not give it. The shape that matters is the asymmetry: a reading that fails
//! must land on `Unknown`, never on `Enabled` and never on `Disabled`.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use beacond::{
    LiveDataEncryption, LiveDataEncryptionReport, SystemView, live_data_encryption_for_test,
};
use uuid::Uuid;

const OBSERVED_AT_MS: u64 = 1_756_000_000_000;

struct Fixture {
    root: PathBuf,
    store: PathBuf,
    sys: PathBuf,
    mountinfo: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!("beacon-diskenc-{label}-{}", Uuid::new_v4()));
        let sys = root.join("sys");
        fs::create_dir_all(&sys).unwrap();
        let store = root.join("beacon.sqlite3");
        fs::write(&store, b"").unwrap();
        Self {
            mountinfo: root.join("mountinfo"),
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
        let raw = fs::metadata(&self.store).unwrap().dev();
        format!("{}:{}", libc::major(raw), libc::minor(raw))
    }

    fn mount_lines(&self, lines: &[String]) {
        fs::write(&self.mountinfo, format!("{}\n", lines.join("\n"))).unwrap();
    }

    fn mount_line(&self, device: &str, fstype: &str, source: &str) -> String {
        format!(
            "36 25 {device} / {} rw,relatime shared:1 - {fstype} {source} rw",
            self.root.display()
        )
    }

    /// The ordinary case: one mount covering the store, on the host's device.
    fn mounted_on_host_device(&self) -> String {
        let device = self.host_device();
        self.mount_lines(&[self.mount_line(&device, "ext4", "/dev/nvme0n1p2")]);
        device
    }

    fn node(&self, device: &str) -> PathBuf {
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
            let entry = self.node(device).join("slaves").join(name);
            fs::create_dir_all(&entry).unwrap();
            fs::write(entry.join("dev"), format!("{child}\n")).unwrap();
        }
    }

    fn view(&self) -> SystemView {
        SystemView::new(self.mountinfo.clone(), self.sys.clone())
    }

    fn read(&self) -> LiveDataEncryptionReport {
        live_data_encryption_for_test(&self.store, &self.view(), OBSERVED_AT_MS)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

fn reason(report: &LiveDataEncryptionReport) -> String {
    match &report.mechanism {
        beacond::Recorded::Absent { reason } => reason.clone(),
        beacond::Recorded::Present { value } => panic!("expected no mechanism, found {value}"),
    }
}

fn mechanism(report: &LiveDataEncryptionReport) -> String {
    match &report.mechanism {
        beacond::Recorded::Present { value } => value.clone(),
        beacond::Recorded::Absent { reason } => panic!("expected a mechanism, found {reason}"),
    }
}

fn evidence_text(report: &LiveDataEncryptionReport) -> String {
    report
        .evidence
        .iter()
        .map(|entry| format!("{} => {}", entry.source, entry.observation))
        .collect::<Vec<_>>()
        .join("\n")
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
    fixture.slaves(&device, &[("sda2", "8:2")]);
    fixture.node("8:2");

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Enabled);
    assert_eq!(mechanism(&report), "dm-crypt (LUKS2)");
    assert!(evidence_text(&report).contains("CRYPT-LUKS2"));
}

#[test]
fn luks_under_lvm_reports_enabled_through_the_slaves_chain() {
    let fixture = Fixture::new("lvm-on-luks");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(&device, &[("dm-0", "253:0")]);
    fixture.dm(
        "253:0",
        "CRYPT-LUKS2-0011223344556677889900aabbccddee-root",
        "root",
    );
    fixture.slaves("253:0", &[("sda2", "8:2")]);
    fixture.node("8:2");

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Enabled);
    assert_eq!(mechanism(&report), "dm-crypt (LUKS2)");
}

#[test]
fn encrypted_branch_does_not_mask_a_plain_sibling() {
    let fixture = Fixture::new("mixed-stack");
    let device = fixture.mounted_on_host_device();
    // One volume group over two physical volumes: one LUKS2, one plain. Extents
    // on the plain member are not protected by the crypt layer on its sibling.
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(&device, &[("dm-0", "253:0"), ("sdb1", "8:17")]);
    fixture.dm(
        "253:0",
        "CRYPT-LUKS2-0011223344556677889900aabbccddee-pv0",
        "pv0",
    );
    fixture.slaves("253:0", &[("sda2", "8:2")]);
    fixture.node("8:2");
    fixture.node("8:17");

    let report = fixture.read();

    assert_ne!(report.state, LiveDataEncryption::Enabled);
    assert_eq!(report.state, LiveDataEncryption::Disabled);
    // The report must name the branch that is not covered.
    assert!(evidence_text(&report).contains("8:17"));
    // And still say what it did find, so the operator is not told "nothing".
    assert!(evidence_text(&report).contains("CRYPT-LUKS2"));
    // The reason must not deny the encryption this very report observed. An
    // operator who can see CRYPT-LUKS2 in the evidence and reads "found no
    // encryption" learns to disbelieve the warning.
    let reason = reason(&report);
    assert!(reason.contains("pv0 is encrypted"), "{reason}");
    assert!(reason.contains("8:17 is not"), "{reason}");
    assert!(!reason.contains("found no"), "{reason}");
}

#[test]
fn review_r3_long_mixed_mapping_names_preserve_the_disabled_conclusion() {
    let fixture = Fixture::new("long-names");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(
        &device,
        &[("dm-0", "253:0"), ("dm-1", "253:1"), ("sdb1", "8:17")],
    );
    // Valid, non-hex, and long enough that concatenating the names would spend
    // the whole 200-character bound before either half of the sentence.
    let first = "encrypted-volume-".repeat(20);
    let second = "another-encrypted-volume-".repeat(12);
    fixture.dm(
        "253:0",
        "CRYPT-LUKS2-0011223344556677889900aabbccddee-a",
        &first,
    );
    fixture.slaves("253:0", &[("sda2", "8:2")]);
    fixture.dm(
        "253:1",
        "CRYPT-LUKS2-1122334455667788990011bbccddeeff-b",
        &second,
    );
    fixture.slaves("253:1", &[("sda3", "8:3")]);
    fixture.node("8:2");
    fixture.node("8:3");
    fixture.node("8:17");

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Disabled);
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
        let text = match &report.mechanism {
            beacond::Recorded::Absent { reason } => reason.clone(),
            beacond::Recorded::Present { value } => value.clone(),
        };
        assert!(
            text.chars().count() <= 200,
            "{} chars: {text}",
            text.chars().count()
        );
        for entry in &report.evidence {
            assert!(entry.source.chars().count() <= 200);
            assert!(entry.observation.chars().count() <= 200);
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
        .map(|index| (format!("sd{index}"), format!("8:{index}")))
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
    assert!(reason.contains("more"), "{reason}");

    // Many distinct crypt mechanisms on one covered stack.
    let mechanisms = Fixture::new("many-mechanisms");
    let device = mechanisms.mounted_on_host_device();
    mechanisms.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0");
    let kinds = ["LUKS1", "LUKS2", "BITLK", "PLAIN", "TEMP", "VERITY"];
    let names: Vec<(String, String)> = kinds
        .iter()
        .enumerate()
        .map(|(index, _)| (format!("dm-{index}"), format!("253:{index}")))
        .collect();
    let borrowed: Vec<(&str, &str)> = names
        .iter()
        .map(|(name, dev)| (name.as_str(), dev.as_str()))
        .collect();
    mechanisms.slaves(&device, &borrowed);
    for (index, kind) in kinds.iter().enumerate() {
        let dev = format!("253:{index}");
        mechanisms.dm(
            &dev,
            &format!("CRYPT-{kind}-0011223344556677889900aabbccdd{index:02}-m"),
            &format!("mapping-{index}"),
        );
        mechanisms.slaves(&dev, &[("disk", &format!("8:{index}"))]);
        mechanisms.node(&format!("8:{index}"));
    }
    let mechanism = conclusion(&mechanisms);
    assert!(mechanism.starts_with("dm-crypt"), "{mechanism}");
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
    fixture.slaves(&device, &[("sda2", "8:2")]);
    fixture.node("8:2");

    let report = fixture.read();

    let text = evidence_text(&report);
    assert!(text.contains('\u{2026}'), "{text}");
    for entry in &report.evidence {
        assert!(entry.observation.chars().count() <= 200);
    }
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("no readable backing device"));
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
    fixture.slaves(&device, &[("sda2", "8:2"), ("dm-9", "253:9")]);
    fixture.node("8:2");
    // One branch terminates at a real disk under the crypt layer. The other
    // never terminates, so the volume's topology was never established -- and a
    // sibling that finished is not proof about the branch that did not.
    fixture.node("253:9");
    fixture.slaves("253:9", &[("loop", "253:9")]);

    let report = fixture.read();

    assert_ne!(report.state, LiveDataEncryption::Enabled);
    assert_eq!(report.state, LiveDataEncryption::Unknown);
}

#[test]
fn review_crypt_above_a_nested_fan_out_covers_both_plain_leaves() {
    let fixture = Fixture::new("nested-fan-out");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-1122334455667788990011223344556677", "outer");
    fixture.slaves(&device, &[("dm-0", "253:0")]);
    fixture.dm(
        "253:0",
        "CRYPT-LUKS2-0011223344556677889900aabbccddee-crypt",
        "crypt",
    );
    fixture.slaves("253:0", &[("dm-1", "253:1")]);
    fixture.dm("253:1", "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "inner");
    fixture.slaves("253:1", &[("sda2", "8:2"), ("sdb1", "8:17")]);
    fixture.node("8:2");
    fixture.node("8:17");

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Enabled);
    assert_eq!(mechanism(&report), "dm-crypt (LUKS2)");
}

#[test]
fn review_unreadable_leaf_next_to_a_luks_branch_is_unknown() {
    let fixture = Fixture::new("unreadable-sibling");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(&device, &[("dm-0", "253:0"), ("sdb1", "8:17")]);
    fixture.dm(
        "253:0",
        "CRYPT-LUKS2-0011223344556677889900aabbccddee-pv0",
        "pv0",
    );
    fixture.slaves("253:0", &[("sda2", "8:2")]);
    fixture.node("8:2");
    // 8:17 is never created: one complete LUKS path, one branch Beacon cannot read.

    let report = fixture.read();

    assert_ne!(report.state, LiveDataEncryption::Enabled);
    assert_eq!(report.state, LiveDataEncryption::Unknown);
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
    fixture.slaves(&device, &[("dm-1", "253:1")]);
    fixture.dm("253:1", "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves("253:1", &[("sda2", "8:2"), ("sdb1", "8:17")]);
    fixture.node("8:2");
    fixture.node("8:17");

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Enabled);
    assert_eq!(mechanism(&report), "dm-crypt (LUKS2)");
}

#[test]
fn a_stack_that_reaches_no_physical_device_reports_unknown() {
    let fixture = Fixture::new("no-leaf");
    let device = fixture.mounted_on_host_device();
    // Every branch loops. Nothing terminates, so "every leaf is covered" is
    // vacuously true -- and a vacuous truth must not become `enabled`.
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(&device, &[("dm-1", "253:1")]);
    fixture.node("253:1");
    fixture.slaves("253:1", &[("top", &device)]);

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("no physical device"));
}

#[test]
fn lvm_over_plain_disks_reports_disabled_not_enabled() {
    let fixture = Fixture::new("lvm-plain");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fixture.slaves(&device, &[("sda2", "8:2")]);
    fixture.node("8:2");

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Disabled);
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("could not be read"));
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("could not be read"));
}

#[test]
fn unreadable_slave_device_number_reports_unknown() {
    let fixture = Fixture::new("slave-unreadable");
    let device = fixture.mounted_on_host_device();
    fixture.dm(&device, "LVM-9f8e7d6c5b4a39281706f5e4d3c2b1a0", "vg0-root");
    fs::create_dir_all(fixture.node(&device).join("slaves").join("sda2")).unwrap();

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("could not be read"));
}

#[test]
fn failed_metadata_on_loop_backing_file_reports_unknown() {
    let fixture = Fixture::new("loop-metadata");
    let device = fixture.mounted_on_host_device();
    // `loop` is a file, so asking about `loop/backing_file` errors with
    // NotADirectory. A boolean exists() reads that as "no loop device".
    fs::write(fixture.node(&device).join("loop"), b"not a directory\n").unwrap();

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("could not be read"));
}

#[test]
fn failed_metadata_on_dm_uuid_reports_unknown() {
    let fixture = Fixture::new("dm-metadata");
    let device = fixture.mounted_on_host_device();
    fs::write(fixture.node(&device).join("dm"), b"not a directory\n").unwrap();

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("could not be read"));
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("could not be read"));
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("budget"));
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("budget"));
}

#[test]
fn unreadable_sysfs_directory_reports_unknown_not_disabled() {
    let fixture = Fixture::new("sysfs-absent");
    fixture.mounted_on_host_device();
    // No node created: the block-device tree cannot be read at all.

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("could not be read"));
}

#[test]
fn plain_partition_with_no_dm_layer_reports_disabled_and_names_what_it_cannot_see() {
    let fixture = Fixture::new("plain");
    let device = fixture.mounted_on_host_device();
    fixture.node(&device);

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Disabled);
    // Nothing encrypted was seen here, so the reason says only that.
    assert!(reason(&report).starts_with("at least one path to disk has no crypt layer"));
    assert!(!report.not_verified.is_empty());
    let gaps = report.not_verified.join(" ");
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("resolves to nothing"));
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("resolves to nothing"));
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("resolves to nothing"));
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("resolves to nothing"));
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(
        reason(&report).contains("resolves to nothing"),
        "{}",
        reason(&report)
    );
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(
        reason(&report).contains("resolves to nothing"),
        "{}",
        reason(&report)
    );
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(
        reason(&report).contains("resolves to nothing"),
        "{}",
        reason(&report)
    );
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    // One helper answers this for every link the walk follows, so the wording
    // is the same here as for a dangling store, dm, loop, or dev entry.
    assert!(reason(&report).contains("resolves to nothing"));
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("could not be resolved"));
}

#[test]
fn every_rendered_field_stays_within_the_adapter_bound() {
    // The desktop adapter refuses mountPoint or mechanism text over 200
    // characters. A reason built from a long sysfs path would otherwise make an
    // otherwise-valid report unreadable at the boundary.
    let fixture = Fixture::new("bounded-text-".repeat(6).trim_end_matches('-'));
    fixture.mounted_on_host_device();

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    let mount_point = match &report.mount_point {
        beacond::Recorded::Present { value } => value.chars().count(),
        beacond::Recorded::Absent { reason } => reason.chars().count(),
    };
    assert!(mount_point <= 200, "mountPoint is {mount_point} characters");
    let mechanism = reason(&report).chars().count();
    assert!(mechanism <= 200, "mechanism is {mechanism} characters");
    for entry in &report.evidence {
        assert!(entry.source.chars().count() <= 200);
        assert!(entry.observation.chars().count() <= 200);
    }
}

#[test]
fn missing_mountinfo_reports_unknown() {
    let fixture = Fixture::new("no-mountinfo");
    fixture.node("1:1");

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert_eq!(reason(&report), "the mount table could not be read");
}

#[test]
fn unparseable_mount_table_reports_unknown() {
    let fixture = Fixture::new("bad-mountinfo");
    fixture.mount_lines(&["36 25 not-a-device / /somewhere rw - ext4 /dev/sda1 rw".to_owned()]);

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert_eq!(reason(&report), "the mount table could not be parsed");
}

#[test]
fn mount_table_and_stat_disagreement_reports_unknown() {
    let fixture = Fixture::new("disagree");
    fixture.mount_lines(&[fixture.mount_line("9:9", "ext4", "/dev/sdz1")]);
    fixture.node("9:9");

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("disagree about which volume holds it"));
}

#[test]
fn shadowed_mount_point_selects_the_last_entry() {
    let fixture = Fixture::new("shadowed");
    let device = fixture.host_device();
    // Same mount point twice. The later mount is the live one; selecting the
    // earlier entry answers about a volume nothing is stored on.
    fixture.mount_lines(&[
        fixture.mount_line("9:9", "ext4", "/dev/sdz1"),
        fixture.mount_line(&device, "ext4", "/dev/nvme0n1p2"),
    ]);
    fixture.node(&device);

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Disabled);
}

#[test]
fn tmpfs_backed_store_reports_unknown_with_its_reason() {
    let fixture = Fixture::new("tmpfs");
    let device = fixture.host_device();
    fixture.mount_lines(&[fixture.mount_line(&device, "tmpfs", "tmpfs")]);
    fixture.node(&device);

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert_eq!(
        reason(&report),
        "the live store is on a memory-backed filesystem"
    );
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("overlay filesystem"));
}

#[test]
fn network_filesystem_reports_unknown_with_its_reason() {
    let fixture = Fixture::new("nfs");
    let device = fixture.host_device();
    fixture.mount_lines(&[fixture.mount_line(&device, "nfs4", "server:/export")]);
    fixture.node(&device);

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("network filesystem"));
}

#[test]
fn loopback_backed_volume_reports_unknown() {
    let fixture = Fixture::new("loop");
    let device = fixture.mounted_on_host_device();
    let loop_dir = fixture.node(&device).join("loop");
    fs::create_dir_all(&loop_dir).unwrap();
    fs::write(loop_dir.join("backing_file"), "/srv/images/beacon.img\n").unwrap();

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
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

    assert_eq!(report.state, LiveDataEncryption::Unknown);
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
    fixture.slaves(&device, &[("dm-1", "253:1")]);
    fixture.node("253:1");
    fixture.slaves("253:1", &[("top", &device)]);

    let report = fixture.read();

    // The point is that this returns at all rather than looping. It is no longer
    // a proven negative: nothing in a cycle terminates, so the walk reaches no
    // physical device and has proved nothing either way. Asserting the reason,
    // not just the state, keeps the visited-set mutant dead -- without it the
    // walk exhausts the node budget and says so instead.
    assert_eq!(report.state, LiveDataEncryption::Unknown);
    assert!(reason(&report).contains("no physical device"));
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
        .map(|index| (format!("sd{index}"), format!("8:{index}")))
        .collect();
    let borrowed: Vec<(&str, &str)> = children
        .iter()
        .map(|(name, dev)| (name.as_str(), dev.as_str()))
        .collect();
    fixture.slaves(&device, &borrowed);

    // Poison the entry the walk must never reach, in the order read_dir will
    // actually return -- captured here rather than assumed. Its `dev` becomes a
    // directory, so reading it fails with a reason no budget stop produces. An
    // implementation that enumerated past the ceiling and reported the budget
    // afterwards would still touch this and cannot pass.
    let slaves = fixture.node(&device).join("slaves");
    let order: Vec<_> = fs::read_dir(&slaves)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    let poison = order.last().expect("the fan-out has entries").join("dev");
    fs::remove_file(&poison).unwrap();
    fs::create_dir(&poison).unwrap();

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
    let reason = reason(&report);
    assert!(reason.contains("read budget"), "{reason}");
    assert!(!reason.contains("could not be read"), "{reason}");
}

#[test]
fn device_stack_deeper_than_the_budget_reports_unknown() {
    let fixture = Fixture::new("deep");
    let device = fixture.mounted_on_host_device();
    fixture.slaves(&device, &[("dm-0", "253:0")]);
    for step in 0..12 {
        let child = format!("253:{}", step + 1);
        fixture.node(&format!("253:{step}"));
        fixture.slaves(&format!("253:{step}"), &[("next", &child)]);
    }
    fixture.node("253:12");

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Unknown);
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
    fixture.slaves(&device, &[("sda2", "8:2")]);
    fixture.node("8:2");

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Enabled);
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
    assert_eq!(before.state, LiveDataEncryption::Disabled);

    fixture.dm(
        &device,
        "CRYPT-LUKS2-aabbccddeeff00112233445566778899-live",
        "live",
    );
    fixture.slaves(&device, &[("sda2", "8:2")]);
    fixture.node("8:2");
    let after = fixture.read();

    assert_eq!(after.state, LiveDataEncryption::Enabled);
}

#[test]
fn stat_evidence_reports_the_hosts_own_device_number() {
    let fixture = Fixture::new("stat");
    let device = fixture.mounted_on_host_device();
    fixture.node(&device);

    let report = fixture.read();

    let stat_line = report
        .evidence
        .iter()
        .find(|entry| entry.source == "stat(live store)")
        .expect("the reading records the device it stat'd");
    assert_eq!(stat_line.observation, format!("st_dev = {device}"));
    assert!(matches!(
        &report.mount_point,
        beacond::Recorded::Present { value } if Path::new(value) == fixture.root
    ));
}
