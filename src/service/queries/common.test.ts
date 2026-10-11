import { describe, expect, it, vi } from 'vitest';
import { dedupBy } from './common';

// feed.ts 的模块链拉到 locales 与 tauri/invoke（模块级读 localStorage/
// navigator/document 探测并初始化语言）——node 测试环境没有这些 global，
// 在动态 import 前打桩，feedItems 用动态 import 拉进来
vi.stubGlobal('localStorage', { getItem: () => null, setItem: () => {} });
vi.stubGlobal('navigator', { language: 'zh-CN' });
vi.stubGlobal('document', { documentElement: { lang: '' } });

/** 构造一页推荐流（相邻页重叠 seriesId，模拟推荐位轮换）。 */
function page(ids: string[]): import('../schema').FeedPage {
  return {
    items: ids.map((seriesId) => ({
      seriesId,
      title: `剧 ${seriesId}`,
      cover: '',
      horizCover: '',
      vid: `v${seriesId}`,
      episodeCnt: 1,
      playCnt: 0,
      commentCount: 0,
      score: 0,
      tags: [],
      seasonTag: '',
      heatText: '',
      badge: '',
      contentType: 0,
    })),
    hasMore: true,
    nextOffset: 0,
    sessionId: 's',
  };
}

describe('feedItems', () => {
  it('跨页去重：推荐位轮换的重复条目只留首个', async () => {
    const { feedItems } = await import('./feed');
    const pages = [page(['1', '2', '3']), page(['2', '3', '4']), page(['4', '5'])];
    expect(feedItems(pages).map((i) => i.seriesId)).toEqual(['1', '2', '3', '4', '5']);
  });

  it('空页列表返回空数组', async () => {
    const { feedItems } = await import('./feed');
    expect(feedItems([])).toEqual([]);
  });
});

describe('dedupBy', () => {
  it('按 key 去重并保序（首个出现位置为准）', () => {
    const items = [
      { id: 'a', v: 1 },
      { id: 'b', v: 2 },
      { id: 'a', v: 3 },
    ];
    expect(dedupBy(items, (i) => i.id)).toEqual([
      { id: 'a', v: 1 },
      { id: 'b', v: 2 },
    ]);
  });

  it('空输入返回空数组', () => {
    expect(dedupBy([], (i) => i)).toEqual([]);
  });
});
