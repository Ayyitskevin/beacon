use std::process::Command;

fn beacond() -> Command {
    Command::new(env!("CARGO_BIN_EXE_beacond"))
}

#[test]
fn help_is_successful_and_printed_without_template_artifacts() {
    let output = beacond().arg("--help").output().unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("Usage:\n"));
    assert!(!stdout.contains('$'));
}

#[test]
fn command_errors_are_clear_and_do_not_contain_template_artifacts() {
    let output = beacond().arg("unknown").output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("beacond: unknown command: unknown\n"));
    assert!(stderr.contains("Usage:\n"));
    assert!(!stderr.contains('$'));
}
