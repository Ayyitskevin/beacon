use serde::Serialize;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SessionPhase {
    OperatorSetupRequired,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopSessionStatus {
    phase: SessionPhase,
    workspace_access: bool,
    operator_commands: bool,
}

// Tauri command registration and typed Result/value transport:
// https://v2.tauri.app/develop/calling-rust/#commands
#[tauri::command]
fn session_status() -> DesktopSessionStatus {
    DesktopSessionStatus {
        phase: SessionPhase::OperatorSetupRequired,
        workspace_access: false,
        operator_commands: false,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![session_status])
        .run(tauri::generate_context!())
        .expect("failed to run the Mosaic Phase 0 desktop shell");
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::session_status;

    #[test]
    fn signed_out_status_exposes_no_workspace_data_or_operator_command() {
        let status = serde_json::to_value(session_status()).unwrap();

        assert_eq!(status["phase"], "operator_setup_required");
        assert_eq!(status["workspaceAccess"], false);
        assert_eq!(status["operatorCommands"], false);
        let object = status.as_object().unwrap();
        assert_eq!(
            object.keys().cloned().collect::<Vec<_>>(),
            ["operatorCommands", "phase", "workspaceAccess"]
        );
        assert!(!contains_workspace_material(&status));
    }

    fn contains_workspace_material(value: &Value) -> bool {
        value.to_string().to_lowercase().contains("workspace_id")
            || value.to_string().to_lowercase().contains("workspace_name")
    }
}
