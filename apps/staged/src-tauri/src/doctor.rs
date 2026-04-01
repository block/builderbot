//! Tauri command wrappers for the doctor health-check system.

use doctor::DoctorReport;

/// Run all health checks and return the report.
#[tauri::command]
pub async fn run_doctor() -> DoctorReport {
    doctor::run_checks().await
}

/// Run a fix command from a doctor check.
///
/// The frontend sends the raw command string from `DoctorCheck.fixCommand`.
#[tauri::command]
pub async fn run_doctor_fix(command: String) -> Result<(), String> {
    doctor::execute_command(command).await
}
