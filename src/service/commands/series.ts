import { call } from '../tauri/invoke';

// ---------------------------------------------------------------- 剧集

export const series = {
  episodes: (seriesId: string) => call('get_series_episodes', { seriesId }),
  resolve: (input: string) => call('resolve_series', { input }),
  /** 相关作品·系列（同系列各季 + 同 IP；失败由上层静默降级） */
  related: (seriesId: string) => call('related_series', { seriesId }),
  /** 详情页头部元信息（追剧/播放/季徽/标签/备案号；失败前端静默降级）。 */
  meta: (seriesId: string) => call('series_meta', { seriesId }),
};
