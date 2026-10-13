//! 剧集档案持久化。

use crate::app_state::AppState;
use hongguo_common::AppResult;
use hongguo_common::model::Series;

/// 登记剧集并落库。
pub fn upsert_and_persist(state: &AppState, series: Series) -> AppResult<()> {
    state.store.upsert_series(&series)
}
