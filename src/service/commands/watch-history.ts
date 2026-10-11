import { call } from '../tauri/invoke';

// ---------------------------------------------------------------- 云端观看历史

/** 云端观看历史（官方 App「历史」同源；登录后可用，匿名回空表）。 */
export const watchHistory = {
  list: (offset = 0) => call('watch_history_list', { offset }),
  /**
   * 观看进度云上报（read_history/update + read_progress/upload 双接口）。
   * 后端匿名时静默跳过、失败只记日志——fire-and-forget 即可。
   */
  reportProgress: (seriesId: string, vid: string, vidIndex: number, positionMs: number) =>
    call('cloud_report_progress', { seriesId, vid, vidIndex, positionMs }),
  /** 批量删除云端历史（read_history/update 的 is_delete+use_soft_delete 形态）。 */
  delete: (items: { seriesId: string; vid: string; vidIndex: number }[]) =>
    call('watch_history_delete', { items }),
};
