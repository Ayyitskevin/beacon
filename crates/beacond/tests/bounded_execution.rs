//! Bounded execution — ADR 0029, Slice C.
//!
//! Containment is not metering. Slice A proved Landlock contains; these tests
//! prove a contained process is also bounded, and that each ceiling is named
//! rather than collapsed into "it failed".
//!
//! Every test here runs a real process under a real ruleset.

use std::time::{Duration, Instant};

use beacond::{Ceiling, ExecutionBounds, ExecutionOutcome, detect_sandbox, run_bounded_for_test};

fn tiny_bounds() -> ExecutionBounds {
    ExecutionBounds {
        wall_clock_ms: 3_000,
        cpu_seconds: 2,
        address_space_bytes: 512 * 1024 * 1024,
        file_size_bytes: 1024 * 1024,
        output_bytes: 4096,
    }
}

/// Skip when the host cannot enforce, rather than pretend the test proved
/// something. A kernel without Landlock is legitimate; a vacuous pass is not.
fn enforcing_or_skip() -> bool {
    if detect_sandbox().admits_execution() {
        return true;
    }
    eprintln!("skipped: this host reports no enforcing sandbox");
    false
}

#[test]
fn a_command_runs_and_reports_its_exit_code() {
    if !enforcing_or_skip() {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    let record = run_bounded_for_test(
        &["/bin/echo".to_owned(), "hello".to_owned()],
        &scratch,
        tiny_bounds(),
        None,
    );
    assert_eq!(record.outcome, ExecutionOutcome::Exited { code: 0 });
    assert_eq!(record.stdout.trim(), "hello");
    assert!(!record.output_truncated);
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn a_nonzero_exit_is_reported_not_hidden() {
    if !enforcing_or_skip() {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    let record = run_bounded_for_test(
        &["/bin/sh".to_owned(), "-c".to_owned(), "exit 3".to_owned()],
        &scratch,
        tiny_bounds(),
        None,
    );
    assert_eq!(record.outcome, ExecutionOutcome::Exited { code: 3 });
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn the_environment_is_cleared_so_a_run_inherits_no_credential() {
    if !enforcing_or_skip() {
        return;
    }
    // SAFETY: single-threaded test setup; the variable is removed below.
    unsafe { std::env::set_var("BEACON_SECRET_PROBE", "must-not-leak") };
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    let record = run_bounded_for_test(&["/usr/bin/env".to_owned()], &scratch, tiny_bounds(), None);
    unsafe { std::env::remove_var("BEACON_SECRET_PROBE") };
    assert!(
        !record.stdout.contains("must-not-leak"),
        "the supervisor's environment reached the Run: {}",
        record.stdout
    );
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn a_write_outside_the_working_directory_is_refused_while_inside_succeeds() {
    if !enforcing_or_skip() {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    let outside = std::env::temp_dir().join(format!("beacon-outside-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&outside).unwrap();

    let record = run_bounded_for_test(
        &[
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            format!(
                "echo inside > ./allowed.txt && echo outside > {}/denied.txt",
                outside.display()
            ),
        ],
        &scratch,
        tiny_bounds(),
        None,
    );
    // The first write is permitted, the second is not, so the shell fails.
    assert_ne!(record.outcome, ExecutionOutcome::Exited { code: 0 });
    assert!(
        scratch.join("allowed.txt").exists(),
        "the Run could not write its own target"
    );
    assert!(
        !outside.join("denied.txt").exists(),
        "containment failed: the Run wrote outside its working directory"
    );
    std::fs::remove_dir_all(&scratch).ok();
    std::fs::remove_dir_all(&outside).ok();
}

#[test]
fn a_run_that_never_ends_is_stopped_by_the_wall_clock_and_says_so() {
    if !enforcing_or_skip() {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    let mut bounds = tiny_bounds();
    bounds.wall_clock_ms = 1_000;
    let started = Instant::now();
    // `sleep` burns no CPU, so only a wall-clock bound can end it. This is why
    // RLIMIT_CPU alone is not sufficient.
    let record = run_bounded_for_test(
        &["/bin/sleep".to_owned(), "60".to_owned()],
        &scratch,
        bounds,
        None,
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the bound did not hold"
    );
    assert_eq!(
        record.outcome,
        ExecutionOutcome::Bounded {
            ceiling: Ceiling::WallClock
        },
        "a Run stopped by its clock must name the ceiling, not merely fail"
    );
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn output_beyond_the_ceiling_is_truncated_and_the_record_admits_it() {
    if !enforcing_or_skip() {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    let mut bounds = tiny_bounds();
    bounds.output_bytes = 256;
    let record = run_bounded_for_test(
        &[
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "for i in $(seq 1 500); do echo aaaaaaaaaaaaaaaaaaaaaaaa; done".to_owned(),
        ],
        &scratch,
        bounds,
        None,
    );
    assert!(record.stdout.len() <= 256);
    assert!(
        record.output_truncated,
        "a truncated log that does not say so is a lie a Receipt must not tell"
    );
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn an_interrupt_stops_a_run_and_is_distinguished_from_a_ceiling() {
    if !enforcing_or_skip() {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    let (interrupt, receiver) = beacond::interrupt_channel();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        interrupt.stop();
    });
    let started = Instant::now();
    let record = run_bounded_for_test(
        &["/bin/sleep".to_owned(), "60".to_owned()],
        &scratch,
        tiny_bounds(),
        Some(receiver),
    );
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(
        record.outcome,
        ExecutionOutcome::Interrupted {},
        "an operator interrupt is not a budget exhaustion"
    );
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn a_missing_program_never_started_rather_than_exited() {
    if !enforcing_or_skip() {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    let record = run_bounded_for_test(
        &["/nonexistent/program".to_owned()],
        &scratch,
        tiny_bounds(),
        None,
    );
    // Nothing ran, so there is no exit status to report.
    assert!(matches!(
        record.outcome,
        ExecutionOutcome::NotStarted { .. }
    ));
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn an_interrupted_outcome_rejects_a_smuggled_field() {
    // Same hazard as SandboxUnavailableReason: Interrupted carries no data, so
    // without an empty struct body its guard would be inert.
    assert_eq!(
        serde_json::to_string(&ExecutionOutcome::Interrupted {}).unwrap(),
        r#"{"kind":"interrupted"}"#
    );
    assert!(
        serde_json::from_str::<ExecutionOutcome>(r#"{"kind":"interrupted","ceiling":"cpu"}"#)
            .is_err(),
        "an interrupted Run must not accept a ceiling it never hit"
    );
}

#[test]
fn a_detached_descendant_holding_the_pipe_cannot_outlast_the_ceiling() {
    if !enforcing_or_skip() {
        return;
    }
    // The regression that motivated the bounded collector. The direct child
    // exits at once, but leaves a `setsid` grandchild holding stdout. EOF
    // arrives only when the LAST holder closes, and `terminate_group` cannot
    // reach a process that left the group -- so joining the reader put the same
    // unbounded wait back that had just been removed from the poll loop.
    //
    // Every existing test here uses a direct child with no descendants, which
    // is exactly why none of them could see it.
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    let mut bounds = tiny_bounds();
    bounds.wall_clock_ms = 1_000;
    let started = Instant::now();
    let record = run_bounded_for_test(
        &[
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "setsid sleep 8 & echo started; exit 0".to_owned(),
        ],
        &scratch,
        bounds,
        None,
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "run_bounded took {}ms under a 1000ms ceiling: a descendant held it open",
        elapsed.as_millis()
    );
    // The child really did exit cleanly, so the outcome is its exit -- the
    // ceiling bounded the *collector*, not the process.
    assert_eq!(record.outcome, ExecutionOutcome::Exited { code: 0 });
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn a_real_cpu_ceiling_reaches_bounded_rather_than_signalled() {
    if !enforcing_or_skip() {
        return;
    }
    // With `rlim_cur == rlim_max` the kernel's SIGXCPU and its SIGKILL arrive
    // at the same instant, so a real CPU ceiling was indistinguishable from any
    // other kill and reported `Signalled { signal: 9 }`. The soft limit now
    // sits strictly below the hard one so the ceiling is observable.
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    let mut bounds = tiny_bounds();
    bounds.cpu_seconds = 1;
    bounds.wall_clock_ms = 30_000;
    let record = run_bounded_for_test(
        &[
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "while :; do :; done".to_owned(),
        ],
        &scratch,
        bounds,
        None,
    );
    assert_eq!(
        record.outcome,
        ExecutionOutcome::Bounded {
            ceiling: Ceiling::Cpu
        },
        "a Run that burned its CPU budget must name the ceiling"
    );
    std::fs::remove_dir_all(&scratch).ok();
}

#[test]
fn a_ceiling_cannot_be_forged_by_the_run_itself() {
    if !enforcing_or_skip() {
        return;
    }
    // Classification reads the delivered signal, never the exit code. A
    // Declared Check runs the repository's own code, so a ceiling keyed on an
    // exit code would let a Run that merely wants to look throttled say so and
    // be believed -- a guard on a value the guarded path supplies.
    let scratch = std::env::temp_dir().join(format!("beacon-exec-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).unwrap();
    for code in [152, 153, 137] {
        let record = run_bounded_for_test(
            &[
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                format!("exit {code}"),
            ],
            &scratch,
            tiny_bounds(),
            None,
        );
        assert_eq!(
            record.outcome,
            ExecutionOutcome::Exited { code },
            "exit {code} must stay an exit, not become a ceiling the Run chose"
        );
    }
    std::fs::remove_dir_all(&scratch).ok();
}
