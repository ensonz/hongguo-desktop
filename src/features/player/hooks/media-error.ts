/** MediaError 分类：播放失败恢复策略的输入（裁决见 recovery-policy）。 */

export type MediaErrorKind = 'decode' | 'network' | 'other';

/**
 * 按 MediaError.code 分类（MDN 语义）：
 *
 * - 3 MEDIA_ERR_DECODE / 4 MEDIA_ERR_SRC_NOT_SUPPORTED → 解码类。注意这
 *   不是单一语义：确定性失败（私有编码、无 HEVC 扩展）与瞬时管线故障
 *   （CDN 坏块、解码器 hiccup）都落在这里，区分两者的重试判别在
 *   recovery-policy；
 * - 2 MEDIA_ERR_NETWORK → 网络类，在线渐进流断供换新流可自愈；
 * - 1 aborted、0/未知 → other，维持原有「重试一次 → 报错」路径。
 */
export function classifyMediaError(code: number | null | undefined): MediaErrorKind {
  if (code === 3 || code === 4) return 'decode';
  if (code === 2) return 'network';
  return 'other';
}
