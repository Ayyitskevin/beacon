//! The execution sandbox — ADR 0029.
//!
//! Beacon does not report a sandbox it has not demonstrated. Detection builds a
//! real ruleset, applies it in a child, and reports only what that child
//! observed. A sandbox that is assumed is not a sandbox.

mod bounded;
mod landlock;
mod probe;
mod worktree_launcher;

use serde::{Deserialize, Serialize};

pub use bounded::{
    Ceiling, ExecutionBounds, ExecutionOutcome, ExecutionRecord, Interrupt, interrupt_channel,
};

/// Public test-facing entry for a bounded Run whose only write target is
/// `working_directory`. It exercises the same descriptor-bound launcher used
/// by Declared Check execution.
pub fn run_bounded_for_test(
    argv: &[String],
    working_directory: &std::path::Path,
    bounds: ExecutionBounds,
    interrupt: Option<std::sync::mpsc::Receiver<()>>,
) -> ExecutionRecord {
    let capability = detect();
    WorktreeLauncher::bind(working_directory, &capability).run(argv, ".", bounds, interrupt)
}
pub(crate) use worktree_launcher::WorktreeLauncher;

/// What this host can actually enforce, as observed rather than as configured.
///
/// Never a boolean: a kernel whose Landlock predates network restriction is a
/// different capability from one that has it, and both are different from a
/// host where enforcement was attempted and did not hold.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SandboxCapability {
    /// Filesystem writes are bounded and TCP is denied. Both observed, and the
    /// observation travels with the claim so a Receipt records what was proven
    /// rather than only what was concluded.
    Enforcing {
        landlock_abi: u8,
        observed: ProbeObservation,
    },
    /// Filesystem writes are bounded; the network is **not**. This never
    /// presents as `Enforcing`, and Execution does not accept it — the
    /// Blueprint denies network by default.
    FilesystemOnly {
        landlock_abi: u8,
        observed: ProbeObservation,
    },
    Unavailable {
        reason: SandboxUnavailableReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum SandboxUnavailableReason {
    /// The kernel does not implement Landlock.
    ///
    /// Braces are deliberate. `deny_unknown_fields` is inert on a *unit*
    /// variant of an internally tagged enum — serde reads the tag and matches
    /// the rest leniently — so an empty struct body is what makes the guard
    /// real. Found by claude-69 while building Slice B.
    LandlockAbsent {},
    /// The ruleset could not be constructed.
    RulesetRefused { errno: i32 },
    /// `PR_SET_NO_NEW_PRIVS` was refused, so Landlock cannot arm.
    NoNewPrivsRefused { errno: i32 },
    /// `landlock_restrict_self` was refused.
    RestrictRefused { errno: i32 },
    /// **The syscalls reported success and enforcement did not hold.** The case
    /// a boolean would have hidden.
    ProbeRefused { observed: ProbeObservation },
    /// The probe child never reported. Not the same as reporting failure.
    ProbeInconclusive { detail: String },
    /// Not a Linux host. Braces deliberate; see `LandlockAbsent`.
    UnsupportedPlatform {},
}

impl SandboxCapability {
    /// Whether a Run may execute under this capability.
    ///
    /// `FilesystemOnly` is deliberately not sufficient (ADR 0029).
    pub fn admits_execution(&self) -> bool {
        matches!(self, Self::Enforcing { .. })
    }

    /// The value recorded in a Receipt's `sandboxVersion`.
    pub fn receipt_version(&self) -> String {
        match self {
            Self::Enforcing { landlock_abi, .. } => {
                format!("landlock-abi-{landlock_abi}/enforcing")
            }
            Self::FilesystemOnly { landlock_abi, .. } => {
                format!("landlock-abi-{landlock_abi}/filesystem-only")
            }
            Self::Unavailable { reason } => format!("unavailable/{}", reason.slug()),
        }
    }
}

impl SandboxCapability {
    /// What the probe observed, when it reported at all. `None` means the probe
    /// never ran or never answered — which is not the same as answering no.
    pub fn observation(&self) -> Option<ProbeObservation> {
        match self {
            Self::Enforcing { observed, .. } | Self::FilesystemOnly { observed, .. } => {
                Some(*observed)
            }
            Self::Unavailable {
                reason: SandboxUnavailableReason::ProbeRefused { observed },
            } => Some(*observed),
            Self::Unavailable { .. } => None,
        }
    }
}

impl SandboxUnavailableReason {
    fn slug(&self) -> &'static str {
        match self {
            Self::LandlockAbsent { .. } => "landlock-absent",
            Self::RulesetRefused { .. } => "ruleset-refused",
            Self::NoNewPrivsRefused { .. } => "no-new-privs-refused",
            Self::RestrictRefused { .. } => "restrict-refused",
            Self::ProbeRefused { .. } => "probe-refused",
            Self::ProbeInconclusive { .. } => "probe-inconclusive",
            Self::UnsupportedPlatform { .. } => "unsupported-platform",
        }
    }
}

/// What the probe child observed from inside the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProbeObservation {
    /// A write beneath a declared target succeeded, so the sandbox is usable.
    pub wrote_inside_target: bool,
    /// A write outside every declared target was refused.
    pub refused_write_outside_target: bool,
    /// A system path stayed readable, so a binary can still be loaded.
    pub read_system_path: bool,
    /// A TCP connection was refused. Meaningless below the network ABI floor,
    /// which is why the capability, not this flag, decides admission.
    pub refused_tcp_connect: bool,
}

impl ProbeObservation {
    fn filesystem_held(&self) -> bool {
        self.wrote_inside_target && self.refused_write_outside_target && self.read_system_path
    }
}

/// Detect what this host enforces, by enforcing it and looking.
///
/// Runs per call rather than caching: a capability is a claim about a machine,
/// and machines change.
pub fn detect() -> SandboxCapability {
    if !cfg!(target_os = "linux") {
        return SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::UnsupportedPlatform {},
        };
    }
    let Some(abi) = landlock::abi_version() else {
        return SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::LandlockAbsent {},
        };
    };
    probe::verify(abi)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    use uuid::Uuid;

    use super::*;

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!("beacon-{label}-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn bounds() -> ExecutionBounds {
        ExecutionBounds {
            wall_clock_ms: 5_000,
            cpu_seconds: 2,
            address_space_bytes: 512 * 1024 * 1024,
            file_size_bytes: 1024 * 1024,
            output_bytes: 4096,
        }
    }

    #[test]
    fn a_concurrent_swap_cannot_retarget_a_selected_working_directory() {
        let capability = detect();
        if !capability.admits_execution() {
            eprintln!("skipped: this host reports no enforcing sandbox");
            return;
        }
        let root = TempDirectory::new("concurrent-cwd-root");
        let outside = TempDirectory::new("concurrent-cwd-outside");
        let selected = root.0.join("selected");
        let moved = root.0.join("descriptor-pinned");
        fs::create_dir(&selected).unwrap();
        let handoff = Arc::new(Barrier::new(2));
        let swapped = Arc::new(Barrier::new(2));
        let worker_handoff = Arc::clone(&handoff);
        let worker_swapped = Arc::clone(&swapped);
        let worker_selected = selected.clone();
        let worker_moved = moved.clone();
        let worker_outside = outside.0.clone();
        let worker = std::thread::spawn(move || {
            worker_handoff.wait();
            fs::rename(&worker_selected, &worker_moved).unwrap();
            symlink(&worker_outside, &worker_selected).unwrap();
            worker_swapped.wait();
        });

        let launcher = WorktreeLauncher::bind(&root.0, &capability);
        let record = launcher.run_with_before_spawn(
            &["/bin/pwd".to_owned()],
            "selected",
            bounds(),
            None,
            || {
                handoff.wait();
                swapped.wait();
            },
        );
        worker.join().unwrap();

        assert_eq!(record.outcome, ExecutionOutcome::Exited { code: 0 });
        assert_eq!(
            record.stdout.trim(),
            moved.to_string_lossy(),
            "the child did not enter the descriptor-pinned directory"
        );
    }

    #[test]
    fn deletion_after_selection_cannot_run_in_a_replacement_directory() {
        let capability = detect();
        if !capability.admits_execution() {
            eprintln!("skipped: this host reports no enforcing sandbox");
            return;
        }
        let root = TempDirectory::new("deleted-cwd-root");
        let selected = root.0.join("selected");
        let marker = selected.join("replacement-ran");
        fs::create_dir(&selected).unwrap();
        let handoff = Arc::new(Barrier::new(2));
        let replaced = Arc::new(Barrier::new(2));
        let worker_handoff = Arc::clone(&handoff);
        let worker_replaced = Arc::clone(&replaced);
        let worker_selected = selected.clone();
        let worker = std::thread::spawn(move || {
            worker_handoff.wait();
            fs::remove_dir(&worker_selected).unwrap();
            fs::create_dir(&worker_selected).unwrap();
            worker_replaced.wait();
        });

        let launcher = WorktreeLauncher::bind(&root.0, &capability);
        let record = launcher.run_with_before_spawn(
            &[
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                "printf ran > replacement-ran".to_owned(),
            ],
            "selected",
            bounds(),
            None,
            || {
                handoff.wait();
                replaced.wait();
            },
        );
        worker.join().unwrap();

        let expected_reason = format!(
            "working-directory descriptor was unlinked before launch (errno {})",
            libc::ESTALE
        );
        assert!(matches!(
            &record.outcome,
            ExecutionOutcome::NotStarted { reason }
                if reason == &expected_reason
        ));
        assert!(!marker.exists(), "the replacement target program ran");
    }
}
