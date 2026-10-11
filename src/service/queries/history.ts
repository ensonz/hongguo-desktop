import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { watchHistory } from '../commands';
import { keys } from './common';

// ---------------------------------------------------------------- 云端观看历史

/** 云端观看历史（官方 App「历史」同源；登录后可用，匿名回空表）。 */
export function useWatchHistory() {
  return useQuery({
    queryKey: keys.watchHistory,
    queryFn: () => watchHistory.list(),
    staleTime: 30_000,
  });
}

/** 批量删除云端历史；成功后失效历史列表。 */
export function useWatchHistoryDelete() {
  const queryClient = useQueryClient();
  return useMutation({
    // 部分失败时组件层有带明细的定制 toast，全局错误 toast 静默
    meta: { silentOnError: true },
    mutationFn: (items: { seriesId: string; vid: string; vidIndex: number }[]) =>
      watchHistory.delete(items),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: keys.watchHistory }),
  });
}
