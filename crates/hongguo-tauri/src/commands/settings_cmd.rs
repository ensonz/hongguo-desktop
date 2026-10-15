//! 设置相关 command。

use tauri::State;

use hongguo_common::AppResult;
use hongguo_common::model::{ProxyTestResult, Settings};
use hongguo_service::app_state::AppState;

use super::boss_key_cmd;

/// 读设置。
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings()
}

/// 存设置（立即生效，无需重启）。老板键在保存后重注册（换键/启停即时生效）。
#[tauri::command]
pub fn save_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> AppResult<Settings> {
    let saved = hongguo_service::settings_service::save(state.inner(), settings)?;
    boss_key_cmd::apply(&app, &saved.boss_key)?;
    Ok(saved)
}

/// 测试代理连通性。
#[tauri::command]
pub async fn test_proxy(
    state: State<'_, AppState>,
    draft: Option<hongguo_common::model::ProxyConfig>,
) -> AppResult<ProxyTestResult> {
    let cfg = draft.unwrap_or_else(|| state.settings().proxy);
    hongguo_service::settings_service::proxy::test_proxy(&cfg).await
}
