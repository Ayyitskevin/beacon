//! Running one command under the sandbox, with bounds — ADR 0029, Slice C.
//!
//! Slice A proved that Landlock contains. Containment is not metering: a
//! contained process can still spin forever, allocate until the host swaps, or
//! write until the disk fills. This module adds the bounds the Blueprint
//! requires — "time, output, memory, and cost are bounded" — and the interrupt
//! it requires alongside them.
//!
//! # What runs where
//!
//! The parent builds the ruleset and the limits. The child, between `fork` and
//! `exec`, sets its resource limits, puts itself in a fresh process group, and
//! arms Landlock. All of that is async-signal-safe. Nothing here decides
//! *whether* a command may run; that is the capability's job.

use std::io::Read;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::landlock::{self, Ruleset};

/// The ceilings a Run may not exceed.
///
/// Every field is a hard bound rather than a hint. A Run that reaches one is
/// terminated, and the Receipt says which ceiling ended it — "it stopped" and
/// "it stopped because it ran out of time" are different facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionBounds {
    /// Wall-clock ceiling. Enforced by the parent, because a process blocked on
    /// I/O consumes no CPU and would outlive a CPU limit forever.
    pub wall_clock_ms: u64,
    /// CPU-seconds ceiling, enforced by the kernel via `RLIMIT_CPU`.
    pub cpu_seconds: u64,
    /// Address-space ceiling in bytes, via `RLIMIT_AS`.
    pub address_space_bytes: u64,
    /// Largest file the Run may create, via `RLIMIT_FSIZE`.
    pub file_size_bytes: u64,
    /// Captured output ceiling per stream. Reading stops here; the Receipt
    /// records that it was truncated rather than pretending it was complete.
    pub output_bytes: u64,
}

impl Default for ExecutionBounds {
    fn default() -> Self {
        Self {
            wall_clock_ms: 300_000,
            cpu_seconds: 240,
            address_space_bytes: 2 * 1024 * 1024 * 1024,
            file_size_bytes: 256 * 1024 * 1024,
            output_bytes: 1024 * 1024,
        }
    }
}

/// How a bounded Run ended.
///
/// A successful process exit is not a successful Mission, so this type reports
/// only what happened to the process.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ExecutionOutcome {
    /// The process ran to completion and the kernel reported a status.
    Exited { code: i32 },
    /// The process was terminated by a signal it did not choose.
    Signalled { signal: i32 },
    /// A ceiling ended it. Which ceiling is part of the record.
    Bounded { ceiling: Ceiling },
    /// The operator interrupted it.
    Interrupted,
    /// It could not be started. Nothing ran, so no exit status exists.
    NotStarted { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ceiling {
    WallClock,
    Cpu,
    AddressSpace,
    FileSize,
}

/// What one bounded Run produced.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionRecord {
    pub argv: Vec<String>,
    pub working_directory: String,
    pub outcome: ExecutionOutcome,
    pub stdout: String,
    pub stderr: String,
    /// Whether either stream hit the output ceiling. A truncated log that does
    /// not say it was truncated is a lie a Receipt must not tell.
    pub output_truncated: bool,
    pub elapsed_ms: u64,
    pub sandbox_version: String,
}

/// A handle the operator can use to stop a Run in flight.
#[derive(Debug, Clone)]
pub struct Interrupt(mpsc::Sender<()>);

impl Interrupt {
    /// Request termination. Idempotent; a Run that already ended ignores it.
    pub fn stop(&self) {
        let _ = self.0.send(());
    }
}

/// Run `argv` in `working_directory`, contained by `ruleset` and bounded.
///
/// The environment is cleared: a Run receives only what it is given, never the
/// supervisor's variables, which is where credentials live.
pub(crate) fn run_bounded(
    argv: &[String],
    working_directory: &Path,
    ruleset: &Ruleset,
    bounds: ExecutionBounds,
    sandbox_version: &str,
    interrupt: Option<mpsc::Receiver<()>>,
) -> ExecutionRecord {
    let started = Instant::now();
    let record = |outcome, stdout: String, stderr: String, truncated| ExecutionRecord {
        argv: argv.to_vec(),
        working_directory: working_directory.to_string_lossy().into_owned(),
        outcome,
        stdout,
        stderr,
        output_truncated: truncated,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        sandbox_version: sandbox_version.to_owned(),
    };

    let Some((program, arguments)) = argv.split_first() else {
        return record(
            ExecutionOutcome::NotStarted {
                reason: "the command has no program".to_owned(),
            },
            String::new(),
            String::new(),
            false,
        );
    };

    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(working_directory)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let ruleset_fd = ruleset.raw_fd();
    let limits = bounds;
    // SAFETY: `pre_exec` runs between `fork` and `exec`. Everything in the
    // closure is async-signal-safe: `setrlimit`, `setpgid`, `prctl`, and one
    // Landlock syscall. Nothing allocates and nothing takes a lock.
    unsafe {
        command.pre_exec(move || {
            apply_rlimit(libc::RLIMIT_CPU, limits.cpu_seconds)?;
            apply_rlimit(libc::RLIMIT_AS, limits.address_space_bytes)?;
            apply_rlimit(libc::RLIMIT_FSIZE, limits.file_size_bytes)?;
            // A fresh process group, so an interrupt reaches descendants rather
            // than only the process Beacon happens to know about.
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // Landlock last: it is irreversible, and everything above must
            // still be permitted to run.
            landlock::restrict_current_thread(ruleset_fd)
                .map_err(std::io::Error::from_raw_os_error)?;
            Ok(())
        })
    };

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return record(
                ExecutionOutcome::NotStarted {
                    reason: format!("the command could not be started: {error}"),
                },
                String::new(),
                String::new(),
                false,
            );
        }
    };

    let group = child.id();
    // Read on separate threads. `read_to_end` returns at EOF, and a child's
    // pipes reach EOF only when it exits — so reading here inline would block
    // for exactly as long as the Run lasts, and the deadline below would never
    // be consulted. A bound that cannot be reached is not a bound.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let cap = bounds.output_bytes;
    let stdout_reader = std::thread::spawn(move || read_capped(stdout_pipe, cap));
    let stderr_reader = std::thread::spawn(move || read_capped(stderr_pipe, cap));
    let collect = move || {
        let (stdout, stdout_truncated) = stdout_reader
            .join()
            .unwrap_or_else(|_| (String::new(), false));
        let (stderr, stderr_truncated) = stderr_reader
            .join()
            .unwrap_or_else(|_| (String::new(), false));
        (stdout, stderr, stdout_truncated || stderr_truncated)
    };

    // Wall clock and interrupt are the parent's job. A child blocked on I/O
    // burns no CPU, so RLIMIT_CPU alone would let it wait forever.
    let deadline = Duration::from_millis(bounds.wall_clock_ms);
    let mut ended_by = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let outcome = match (status.code(), status.signal()) {
                    // A Run stopped by the parent reports why the parent
                    // stopped it, not merely that a signal arrived.
                    (_, _) if ended_by.is_some() => ended_by
                        .take()
                        .unwrap_or(ExecutionOutcome::Signalled { signal: 0 }),
                    (Some(code), _) => classify_exit(code),
                    (None, Some(signal)) => ExecutionOutcome::Signalled { signal },
                    (None, None) => ExecutionOutcome::Signalled { signal: 0 },
                };
                let (stdout, stderr, truncated) = collect();
                return record(outcome, stdout, stderr, truncated);
            }
            Ok(None) => {}
            Err(error) => {
                let (stdout, stderr, truncated) = collect();
                return record(
                    ExecutionOutcome::NotStarted {
                        reason: format!("the Run could not be awaited: {error}"),
                    },
                    stdout,
                    stderr,
                    truncated,
                );
            }
        }
        if ended_by.is_none() {
            if started.elapsed() >= deadline {
                ended_by = Some(ExecutionOutcome::Bounded {
                    ceiling: Ceiling::WallClock,
                });
                terminate_group(group);
            } else if interrupt
                .as_ref()
                .is_some_and(|channel| channel.try_recv().is_ok())
            {
                ended_by = Some(ExecutionOutcome::Interrupted);
                terminate_group(group);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A kernel ceiling surfaces as a signal or a conventional shell code. Naming
/// which ceiling ended a Run keeps "it failed" and "it exceeded its budget"
/// apart in the Receipt.
fn classify_exit(code: i32) -> ExecutionOutcome {
    match code {
        // 128 + SIGXCPU / SIGXFSZ, as a shell reports them.
        152 => ExecutionOutcome::Bounded {
            ceiling: Ceiling::Cpu,
        },
        153 => ExecutionOutcome::Bounded {
            ceiling: Ceiling::FileSize,
        },
        code => ExecutionOutcome::Exited { code },
    }
}

/// Send SIGTERM to the whole group, then SIGKILL, so a Run that ignores the
/// first still ends. Interrupt is a guarantee, not a request.
fn terminate_group(group: u32) {
    let Ok(pgid) = i32::try_from(group) else {
        return;
    };
    // SAFETY: `pgid` is the group the child created for itself in `pre_exec`.
    unsafe {
        libc::kill(-pgid, libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(250));
    // SAFETY: same group; SIGKILL cannot be caught, so this is terminal.
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

/// Read a stream up to `cap` bytes, reporting whether it was cut short.
fn read_capped(pipe: Option<impl Read>, cap: u64) -> (String, bool) {
    let Some(stream) = pipe else {
        return (String::new(), false);
    };
    let cap = usize::try_from(cap).unwrap_or(usize::MAX);
    let mut buffer = Vec::new();
    // One byte over the cap, so hitting it exactly is distinguishable from
    // exceeding it.
    let mut limited = stream.take(cap as u64 + 1);
    let _ = limited.read_to_end(&mut buffer);
    let truncated = buffer.len() > cap;
    buffer.truncate(cap);
    (String::from_utf8_lossy(&buffer).into_owned(), truncated)
}

/// Set one resource limit. Child-side; async-signal-safe.
fn apply_rlimit(resource: libc::__rlimit_resource_t, value: u64) -> std::io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value,
        rlim_max: value,
    };
    // SAFETY: `limit` is a live, fully initialized value and `resource` is a
    // kernel constant.
    if unsafe { libc::setrlimit(resource, &limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Create an interrupt channel for a Run the operator may stop.
pub fn interrupt_channel() -> (Interrupt, mpsc::Receiver<()>) {
    let (sender, receiver) = mpsc::channel();
    (Interrupt(sender), receiver)
}
