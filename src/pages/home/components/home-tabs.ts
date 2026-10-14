/** 首页顶栏 tab 列表的纯逻辑：服务端下发列表 → 渲染列表（含回落与登录
 *  闸门）。与组件分离——组件模块的 import 链带 DOM 依赖，纯函数要在
 *  node 测试环境可测。 */

import { t, type MessageKey } from '@/locales';
import type { FeedTab } from '@/service/schema';

/** 推荐 tab 的 tab_type：种子（「你的内容」）只进这个 tab，也是持久化选中
 *  失效后的重置目标。 */
export const RECOMMEND_TAB_TYPE = '16';

/** 关注 tab：登录后才有数据面（游客态服务端不下发 bookstore_id），未登录隐藏。 */
const FOLLOW_TAB_TYPE = '45';

/** 服务端列表到手前的回落三件套（抓包锁定：推荐16/漫剧36/真人剧39），标题走 i18n。 */
const FALLBACK_TABS: { id: string; labelKey: MessageKey }[] = [
  { id: '16', labelKey: 'home.tabFeed' },
  { id: '36', labelKey: 'home.tabComic' },
  { id: '39', labelKey: 'home.tabHuman' },
];

/** 渲染用的一条顶栏 tab（id 即 tab_type，标题为服务端原文或回落名）。 */
export interface HomeTab {
  id: string;
  label: string;
}

/** 已知 tab_type 的 i18n 回落名（服务端 title 为空时兜底，未知 id 直接显示 id）。 */
function fallbackLabel(id: string): string {
  const hit = FALLBACK_TABS.find((f) => f.id === id);
  return hit ? t(hit.labelKey) : id;
}

/**
 * 服务端顶栏 → 渲染列表（纯函数）。
 *
 * 服务端列表保序直用；未登录隐藏关注（45 需登录才有数据面）；服务端
 * 拉取前/失败/为空时回落本地三件套（i18n 标题）。
 */
export function buildTabList(serverTabs: FeedTab[] | undefined, loggedIn: boolean): HomeTab[] {
  if (!serverTabs?.length) {
    return FALLBACK_TABS.map(({ id, labelKey }) => ({ id, label: t(labelKey) }));
  }
  return serverTabs
    .filter((tab) => loggedIn || tab.tabType !== FOLLOW_TAB_TYPE)
    .map((tab) => ({ id: tab.tabType, label: tab.title || fallbackLabel(tab.tabType) }));
}
