//! Running one command under the sandbox, with bounds — ADR 0029, Slice C.
//!
//! Slice A proved that Landlock contains. Containment is not metering: a
//! contained process can still spin forever, allocate until the host swaps, or
//! write until the disk fills. This module adds the bounds the Blueprint
//! requires — "time, output, memory, and cost are bounded" — and the interrupt
//! it requires alongside them.
//!
//! # One resource this module does not bound
//!
//! A reader whose pipe is held by a process that never closes it blocks
//! forever, and `std` offers no cancellation for a blocking read. The Run is
//! bounded and the Receipt is honest, but that thread, its pipe descriptor,
//! and up to `output_bytes` of buffer are retained for the life of the
//! supervisor — once per such Run. A daemon that outlives `beacond` therefore
//! leaks a thread per Run that spawned it. Stated here rather than discovered
//! later: a module whose purpose is bounding resources should name the one it
//! does not bound.
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

/// How long a Run that ended on its own may take to finish reporting output.
const OUTPUT_GRACE_MS: u64 = 2_000;

/// How long a Run the parent *stopped* may take to finish reporting.
///
/// This was zero, on the reasoning that whatever still holds the pipe is the
/// reason the Run was stopped. That is true of an escaped descendant and wrong
/// about the ordinary case: after `terminate_group` the child is already dead,
/// EOF has already happened, and the reader's value is microseconds away. Zero
/// made keeping that output depend on the scheduler — measurably so. Zeroing
/// this module's incidental sleeps loses the output of every stopped Run, five
/// times out of five, while the Receipt still reads as honest.
///
/// A bounded non-zero grace serves both cases: it cannot reintroduce anything
/// like an unbounded wait, and it makes the ordinary stop a guarantee rather
/// than a coincidence. The operator's most valuable output is the last thing a
/// check printed before it hung.
const STOP_OUTPUT_GRACE_MS: u64 = 250;

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
    /// The operator interrupted it. Braces deliberate: `deny_unknown_fields`
    /// is inert on a unit variant of an internally tagged enum.
    Interrupted {},
    /// It could not be started. Nothing ran, so no exit status exists.
    NotStarted { reason: String },
    /// The process started, but Beacon could not establish its terminal
    /// status. Effects and continued activity may therefore remain.
    ObservationFailed { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ceiling {
    WallClock,
    Cpu,
    FileSize,
}

// `RLIMIT_AS` is still set, but deliberately has no `Ceiling`. Exceeding it
// surfaces as an allocation failure *inside* the program, never as a status the
// parent can read, so a variant for it would be a vocabulary entry nothing can
// produce — which reads as coverage later. What we set is recorded in the
// bounds; what we observed is recorded in the outcome, and they are not the
// same claim.

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
    /// Whether a reader was still blocked when the Run's budget ran out — a
    /// detached descendant is holding the pipe open. Distinct from
    /// `output_truncated`: one means "there was more than we would keep", the
    /// other means "we stopped waiting to find out".
    pub output_incomplete: bool,
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
    let record = |outcome, stdout: String, stderr: String, truncated, incomplete| ExecutionRecord {
        argv: argv.to_vec(),
        working_directory: working_directory.to_string_lossy().into_owned(),
        outcome,
        stdout,
        stderr,
        output_truncated: truncated,
        output_incomplete: incomplete,
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
            // Soft below hard for the two that signal: the kernel sends
            // SIGXCPU/SIGXFSZ at the soft limit and SIGKILL at the hard one,
            // and with them equal the two coincide and the ceiling becomes
            // unobservable.
            apply_rlimit(libc::RLIMIT_CPU, limits.cpu_seconds, limits.cpu_seconds + 5)?;
            apply_rlimit(
                libc::RLIMIT_FSIZE,
                limits.file_size_bytes,
                limits.file_size_bytes.saturating_add(1 << 20),
            )?;
            // No soft/hard split: RLIMIT_AS is never signalled, so there is
            // nothing to observe between them.
            apply_rlimit(
                libc::RLIMIT_AS,
                limits.address_space_bytes,
                limits.address_space_bytes,
            )?;
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
    // Readers report over a channel rather than being joined. `read_to_end`
    // returns at EOF, and EOF on an inherited pipe arrives when the LAST holder
    // closes it -- not when our direct child exits. A Declared Check that leaves
    // a detached descendant holding stdout would otherwise block the collector
    // forever, and `terminate_group` cannot reach a process that left the group.
    // Joining here would put the same unbounded wait we removed from the loop
    // straight back into the collection step.
    let (stdout_tx, stdout_rx) = mpsc::channel();
    let (stderr_tx, stderr_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = stdout_tx.send(read_capped(stdout_pipe, cap));
    });
    std::thread::spawn(move || {
        let _ = stderr_tx.send(read_capped(stderr_pipe, cap));
    });
    let collect = move |grace: Duration| {
        // One deadline for both streams, computed once. Moving this inside
        // `take` would give each stream its own grace and bound the total at
        // twice the budget rather than at the budget.
        let deadline = Instant::now() + grace;
        let take = |channel: &mpsc::Receiver<(String, bool)>| {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match channel.recv_timeout(remaining) {
                Ok((text, truncated)) => (text, truncated, false),
                // The reader is still blocked on a pipe someone else holds. We
                // report what we have and say plainly that we stopped waiting;
                // that thread ends on its own when the holder finally closes.
                Err(_) => (String::new(), false, true),
            }
        };
        let (stdout, stdout_truncated, stdout_incomplete) = take(&stdout_rx);
        let (stderr, stderr_truncated, stderr_incomplete) = take(&stderr_rx);
        (
            stdout,
            stderr,
            stdout_truncated || stderr_truncated,
            stdout_incomplete || stderr_incomplete,
        )
    };

    // Wall clock and interrupt are the parent's job. A child blocked on I/O
    // burns no CPU, so RLIMIT_CPU alone would let it wait forever.
    let deadline = Duration::from_millis(bounds.wall_clock_ms);
    let mut ended_by = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let ended_cleanly = ended_by.is_none();
                let outcome = match (status.code(), status.signal()) {
                    // A Run stopped by the parent reports why the parent
                    // stopped it, not merely that a signal arrived.
                    //
                    // This arm also breaks a tie, deliberately. `try_wait` is
                    // checked first each iteration, so a child that exits
                    // during the poll sleep is seen with `ended_by` still
                    // `None` and reports its real status -- the whole interval
                    // is safe. The only ambiguous span is the few instructions
                    // between that syscall returning "still running" and the
                    // deadline comparison below, measured at roughly fifty
                    // nanoseconds.
                    //
                    // It is not a race awaiting a fix, because there is no
                    // atomic version to buy: a child's exit is an asynchronous
                    // kernel event, so any implementation must observe "not
                    // exited" and then decide "deadline passed" as two steps.
                    // `pidfd` with a timeout has the identical ambiguity at the
                    // instant both are true. What remains is a simultaneity,
                    // and the tie breaks toward `Bounded` on purpose: it
                    // under-claims success rather than reporting a clean exit
                    // for a Run whose ceiling had already passed. The cost is a
                    // pessimistic Receipt and one wasted signal at an
                    // already-dead group.
                    (_, _) if ended_by.is_some() => ended_by
                        .take()
                        .unwrap_or(ExecutionOutcome::Signalled { signal: 0 }),
                    (Some(code), _) => ExecutionOutcome::Exited { code },
                    (None, Some(signal)) => classify_signal(signal),
                    (None, None) => ExecutionOutcome::Signalled { signal: 0 },
                };
                // Both cases are bounded; a stopped Run simply gets less,
                // because an escaped descendant will never close and waiting on
                // it is what the ceiling exists to prevent.
                let grace = Duration::from_millis(if ended_cleanly {
                    OUTPUT_GRACE_MS
                } else {
                    STOP_OUTPUT_GRACE_MS
                });
                let (stdout, stderr, truncated, incomplete) = collect(grace);
                return record(outcome, stdout, stderr, truncated, incomplete);
            }
            Ok(None) => {}
            Err(error) => {
                let (stdout, stderr, truncated, incomplete) =
                    collect(Duration::from_millis(OUTPUT_GRACE_MS));
                return record(
                    ExecutionOutcome::ObservationFailed {
                        reason: format!("the Run could not be awaited: {error}"),
                    },
                    stdout,
                    stderr,
                    truncated,
                    incomplete,
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
                ended_by = Some(ExecutionOutcome::Interrupted {});
                terminate_group(group);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Classify a terminating signal.
///
/// The kernel delivers SIGXCPU and SIGXFSZ when a *soft* limit is crossed, so
/// the soft limits below sit strictly under their hard ones — otherwise the
/// soft signal and the hard SIGKILL arrive at the same instant and a real CPU
/// ceiling is indistinguishable from any other kill.
///
/// Classification reads the signal and never an exit code. A shell reports
/// these as 128+n, but a Run's exit code is a value the Run itself chooses,
/// and a Declared Check runs the repository's own code: keying a ceiling on it
/// would let a Run that merely wants to look throttled say so and be believed.
/// That is the guard-on-caller-supplied-value trap this crate documents
/// elsewhere and had fallen into here.
fn classify_signal(signal: i32) -> ExecutionOutcome {
    match signal {
        libc::SIGXCPU => ExecutionOutcome::Bounded {
            ceiling: Ceiling::Cpu,
        },
        libc::SIGXFSZ => ExecutionOutcome::Bounded {
            ceiling: Ceiling::FileSize,
        },
        signal => ExecutionOutcome::Signalled { signal },
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
fn apply_rlimit(resource: libc::__rlimit_resource_t, soft: u64, hard: u64) -> std::io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: soft,
        rlim_max: hard,
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
