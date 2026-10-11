import { lazy, Suspense } from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { PendingFallback } from '@/components/common/route-fallbacks';

/**
 * 剧集详情页。seriesId 走 search 参数（可收藏/可分享的深链）：
 * 播放器左下 @剧名 与相关推荐卡片都跳到这里。
 *
 * prefill（title/cover/tags/desc）：来源页（榜单行等）自带的档案快照。
 * 未上线剧解析分集必然失败（平台无分集可给），带 prefill 时详情页用
 * 它渲染「即将上线」降级视图，而不是整页报错。
 *
 * prefill 从 search 提取要读本路由的 useSearch，而页面本体是 lazy chunk
 * ——所以包装组件常驻路由文件（拿得到 Route），页面本体 React.lazy 切
 * 出去，chunk 加载期间给 pending 骨架。routeTree.gen 是 as any 生成物，
 * 会污染 useSearch 返回类型，形状以 validateSearch 为准 double cast 换回。
 */
const SeriesDetailPage = lazy(() =>
  import('@/pages/series/components/series-detail-page').then((m) => ({
    default: m.SeriesDetailPage,
  })),
);

function DetailRoute() {
  const { seriesId, title, cover, tags, desc } = Route.useSearch() as unknown as {
    seriesId: string;
    title: string;
    cover: string;
    tags: string;
    desc: string;
  };
  return (
    <Suspense fallback={<PendingFallback />}>
      <SeriesDetailPage
        seriesId={seriesId}
        prefill={title !== '' || cover !== '' ? { title, cover, tags, desc } : undefined}
      />
    </Suspense>
  );
}

export const Route = createFileRoute('/detail')({
  validateSearch: (search: Record<string, unknown>) => ({
    // seriesId 一律字符串（main.tsx 的 parseSearch 已按字符串保真，
    // 19 位纯数字 id 不会丢精度）；缺失给空串，由详情页指路
    seriesId: typeof search.seriesId === 'string' ? search.seriesId : '',
    title: typeof search.title === 'string' ? search.title : '',
    cover: typeof search.cover === 'string' ? search.cover : '',
    tags: typeof search.tags === 'string' ? search.tags : '',
    desc: typeof search.desc === 'string' ? search.desc : '',
  }),
  component: DetailRoute,
});
