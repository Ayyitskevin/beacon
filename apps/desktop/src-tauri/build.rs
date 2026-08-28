const COMMANDS: &[&str] = &[
    "authentication_capabilities",
    "session_status",
    "begin_password_setup",
    "complete_operator_setup",
    "authenticate_password",
    "recover_operator_account",
    "lock_session",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to build the Mosaic Phase 0 Tauri shell");
}
