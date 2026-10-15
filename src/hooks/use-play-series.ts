import { useNavigate } from '@tanstack/react-router';

import { usePlayerStore } from '@/stores/player';

/**
 * 「点卡片/选定剧 → 直接播放」的统一路径：点名目标并回首页沉浸流播放。
 *
 * 播放只有首页一个宿主（带类型 tab，/player 已并回）：requestGoto 让信息流
 * 把游标对到目标剧（流里没有就钉在最前），随后锁连播——从别的页**选定**
 * 一部剧进来，滚轮/↑↓ 是切集（原 /player 语义），Esc 退出连播回到换剧。
 *
 * 不预解析分集——首页的 useSeriesEpisodes 自带回落解析与整页加载
 * 反馈（选集表/加载失败态都在那边），调用方只管表达意图。
 */
export function usePlaySeries() {
  const navigate = useNavigate();
  const requestGoto = usePlayerStore((s) => s.requestGoto);
  const setBinge = usePlayerStore((s) => s.setBinge);
  return (seriesId: string, vidIndex = 1) => {
    requestGoto(seriesId, vidIndex);
    setBinge(seriesId);
    void navigate({ to: '/' });
  };
}
