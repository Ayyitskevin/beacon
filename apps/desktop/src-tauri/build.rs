const COMMANDS: &[&str] = &[
    "authentication_capabilities",
    "session_status",
    "session_policy",
    "set_session_timeout",
    "record_operator_activity",
    "platform_session_monitor_status",
    "begin_password_setup",
    "complete_operator_setup",
    "authenticate_password",
    "recover_operator_account",
    "lock_session",
    "starter_organization",
    "accept_starter_organization",
    "discard_starter_organization",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to build the Beacon Phase 0 Tauri shell");
}
