use crate::core::config;
use tauri::Emitter;

#[tauri::command]
pub async fn get_config(
    config_state: tauri::State<'_, config::ConfigState>,
) -> Result<config::AppConfig, String> {
    Ok(config_state.0.lock().await.clone())
}

#[tauri::command]
pub async fn save_config_cmd(
    new_config: config::AppConfig,
    config_state: tauri::State<'_, config::ConfigState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let mut current = config_state.0.lock().await;
    *current = new_config.clone();
    config::save_config(&new_config);
    let active = new_config.active_config();
    println!(
        "[配置] 已保存应用配置，当前激活: {} (main_model={})",
        new_config.active_profile_id, active.main_model
    );
    let _ = app.emit("config-updated", ());
    Ok(())
}

#[tauri::command]
pub async fn get_image_compress_config(
    config_state: tauri::State<'_, config::ConfigState>,
) -> Result<serde_json::Value, String> {
    let app_config = config_state.0.lock().await;
    let active = app_config.active_config();
    Ok(serde_json::json!({
        "maxWidth": active.image_max_width.unwrap_or(1920),
        "maxHeight": active.image_max_height.unwrap_or(1080),
        "quality": active.image_quality.unwrap_or(0.8),
    }))
}
