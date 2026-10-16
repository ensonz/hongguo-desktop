import { call } from '../tauri/invoke';

// ---------------------------------------------------------------- 观看历史

/** 云端观看历史（官方 App「历史」同源；登录后可用，匿名回空表）。 */
export const watchHistory = {
  list: (offset = 0) => call('watch_history_list', { offset }),
  /**
   * 记录观看历史（issue #10）：本地必记（未登录也有历史、可管理），
   * 登录态顺路上报云端。后端失败只记日志——fire-and-forget 即可。
   */
  record: (item: {
    seriesId: string;
    vid: string;
    vidIndex: number;
    positionMs: number;
    durationMs: number;
    title: string;
    cover: string;
    episodeCnt: number;
  }) => call('record_watch_history', item),
  /** 批量删除云端历史（read_history/update 的 is_delete+use_soft_delete 形态）。 */
  delete: (items: { seriesId: string; vid: string; vidIndex: number }[]) =>
    call('watch_history_delete', { items }),
  /** 本地观看历史（未登录模式的历史页数据源；字段与云端条目同形）。 */
  localList: () => call('local_history_list'),
  /** 批量删除本地观看历史（按剧）。 */
  localDelete: (seriesIds: string[]) => call('local_history_delete', { seriesIds }),
};
