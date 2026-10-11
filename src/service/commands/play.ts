import { call } from '../tauri/invoke';

// ---------------------------------------------------------------- 播放

export const play = {
  series: (
    seriesId: string,
    vidIndex: number,
    filePath = '',
    preferOnline = false,
    definition?: number,
  ) =>
    call('play_series', {
      request: { seriesId, vidIndex, filePath, preferOnline, definition: definition ?? null },
    }),
  /** 预取一部剧第 1 集的在线流（沉浸流「一切就下一部」）。幂等且静默。 */
  prefetch: (seriesId: string) => call('play_prefetch', { seriesId }),
  // duration 必须回传：后端靠它判断「接近片尾就不续播」，不记就等于没这道防线
  savePosition: (seriesId: string, vidIndex: number, currentTime: number, duration: number) =>
    call('save_playback_position', { seriesId, vidIndex, currentTime, duration }),
  /** 一部剧最近看到的那一集（本地 playback 表真值；没看过返回 null） */
  progress: (seriesId: string) => call('series_progress', { seriesId }),
};
