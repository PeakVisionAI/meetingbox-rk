use log::info;
use tauri::{AppHandle, Emitter, Manager};

use super::manager::DatabaseManager;
use super::repositories::setting::SettingsRepository;
use crate::state::AppState;

/// Initialize database on app startup
/// Handles first launch detection and conditional initialization
pub async fn initialize_database_on_startup(app: &AppHandle) -> Result<(), String> {
    // Check if this is the first launch (no database exists yet)
    let is_first_launch = DatabaseManager::is_first_launch(app)
        .await
        .map_err(|e| format!("Failed to check first launch status: {}", e))?;

    if is_first_launch {
        info!("First launch detected - will notify window when ready");

        // Delay event emission to ensure window is ready and React listeners are registered
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            app_handle
                .emit("first-launch-detected", ())
                .expect("Failed to emit first-launch-detected event");
            info!("Emitted first-launch-detected after delay");
        });
    } else {
        // Normal flow - initialize database immediately
        let db_manager = DatabaseManager::new_from_app_handle(app)
            .await
            .map_err(|e| format!("Failed to initialize database manager: {}", e))?;

        let defaults_applied = SettingsRepository::ensure_rk_summary_defaults(db_manager.pool())
            .await
            .map_err(|e| format!("Failed to initialize RK summary defaults: {}", e))?;
        if defaults_applied {
            info!("Initialized RK1828 summary defaults");
        }
        let transcript_defaults_applied =
            SettingsRepository::ensure_rk_transcript_defaults(db_manager.pool())
                .await
                .map_err(|e| format!("Failed to initialize RK transcript defaults: {}", e))?;
        if transcript_defaults_applied {
            info!("Initialized RK3588 transcript defaults");
        }

        app.manage(AppState { db_manager });
        info!("Database initialized successfully");
    }

    Ok(())
}
