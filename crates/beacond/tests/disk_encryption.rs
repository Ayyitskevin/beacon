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

    let report = fixture.read();

    assert_eq!(report.state, LiveDataEncryption::Enabled);
    assert_eq!(mechanism(&report), "dm-crypt (LUKS2)");
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
    fs::create_dir_all(fixture.node(&device).join("dm").join("uuid")).unwrap();

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
    assert!(!report.not_verified.is_empty());
    let gaps = report.not_verified.join(" ");
    assert!(gaps.contains("self-encrypting drive"));
    assert!(gaps.contains("fscrypt"));
    assert!(gaps.contains("beneath this host"));
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

    // The point is that this returns at all, and that a graph read end to end
    // with no crypt layer is still a proven negative.
    assert_eq!(report.state, LiveDataEncryption::Disabled);
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
