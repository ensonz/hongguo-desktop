//! 剧集档案持久化。

use tauri::State;

use crate::app_state::AppState;
use hongguo_common::AppResult;
use hongguo_common::model::Series;

/// 登记剧集并落库。
pub fn upsert_and_persist(state: &State<'_, AppState>, series: Series) -> AppResult<()> {
    state.store.upsert_series(&series)
}
