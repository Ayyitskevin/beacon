//! The execution sandbox — ADR 0029.
//!
//! Beacon does not report a sandbox it has not demonstrated. Detection builds a
//! real ruleset, applies it in a child, and reports only what that child
//! observed. A sandbox that is assumed is not a sandbox.

mod bounded;
mod landlock;
mod probe;

use serde::{Deserialize, Serialize};

pub(crate) use bounded::run_bounded;
pub use bounded::{
    Ceiling, ExecutionBounds, ExecutionOutcome, ExecutionRecord, Interrupt, interrupt_channel,
};

/// Run a command under a freshly built ruleset whose only write target is
/// `working_directory`. Test-facing until the Execution surface composes a
/// worktree with a Run; the composition is Slice B's seam, not this module's.
pub fn run_bounded_for_test(
    argv: &[String],
    working_directory: &std::path::Path,
    bounds: ExecutionBounds,
    interrupt: Option<std::sync::mpsc::Receiver<()>>,
) -> ExecutionRecord {
    let capability = detect();
    let Some(abi) = landlock::abi_version() else {
        return ExecutionRecord {
            argv: argv.to_vec(),
            working_directory: working_directory.to_string_lossy().into_owned(),
            outcome: ExecutionOutcome::NotStarted {
                reason: "this host has no sandbox".to_owned(),
            },
            stdout: String::new(),
            stderr: String::new(),
            output_truncated: false,
            elapsed_ms: 0,
            sandbox_version: capability.receipt_version(),
        };
    };
    match Ruleset::build(abi, &[working_directory], true) {
        Ok(ruleset) => run_bounded(
            argv,
            working_directory,
            &ruleset,
            bounds,
            &capability.receipt_version(),
            interrupt,
        ),
        Err(error) => ExecutionRecord {
            argv: argv.to_vec(),
            working_directory: working_directory.to_string_lossy().into_owned(),
            outcome: ExecutionOutcome::NotStarted {
                reason: format!("the ruleset could not be built: {error}"),
            },
            stdout: String::new(),
            stderr: String::new(),
            output_truncated: false,
            elapsed_ms: 0,
            sandbox_version: capability.receipt_version(),
        },
    }
}
pub(crate) use landlock::Ruleset;

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
