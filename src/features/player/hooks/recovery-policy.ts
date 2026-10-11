/** 播放失败恢复策略：错误分类 + 失败账本 → 恢复动作（纯函数）。 */

import type { MediaErrorKind } from './media-error';

/** 恢复动作。 */
export type RecoveryAction = 'retry' | 'fallback' | 'errorPage';

/** 当前集上各类错误的已发生次数。 */
export type FailureCounts = Record<MediaErrorKind, number>;

/** 每类错误的重试预算：第二次同类失败才升级动作。 */
const RETRY_BUDGET_PER_KIND = 1;

/** 空账本（换集后的起点）。 */
export function emptyFailureCounts(): FailureCounts {
  return { decode: 0, network: 0, other: 0 };
}

/**
 * 裁决一次在线播放失败后的恢复动作。
 *
 * decode（3/4）不是单一语义：既有确定性失败（私有编码、无 HEVC 扩展），
 * 也有瞬时管线故障（CDN 坏块、解码器 hiccup）——重取一条防缓存钉的新流
 * 能否存活就是两者的判别器，所以 decode 首次与其他错误一样先重试，复发
 * 才升级整集转码兜底（分钟级代价只付在确定性失败上）。network/other 维持
 * 既有语义：重试一次，复发落错误页。
 */
export function recoveryAction(kind: MediaErrorKind, counts: FailureCounts): RecoveryAction {
  if (counts[kind] < RETRY_BUDGET_PER_KIND) return 'retry';
  return kind === 'decode' ? 'fallback' : 'errorPage';
}
