/** MediaError 分类：决定播放失败走「重试自愈」还是「直接兜底转码」。 */

export type MediaErrorKind = 'decode' | 'network' | 'other';

/**
 * 按 MediaError.code 分类（MDN 语义）：
 *
 * - 3 MEDIA_ERR_DECODE / 4 MEDIA_ERR_SRC_NOT_SUPPORTED → 解码类。无 HEVC
 *   扩展、私有参数集这类失败是**确定性**的——重试同一 URL 必然复现，只有
 *   转码兜底能救；
 * - 2 MEDIA_ERR_NETWORK → 网络类，在线渐进流断供换新流可自愈；
 * - 1 aborted、0/未知 → other，维持原有「重试一次 → 报错」路径。
 */
export function classifyMediaError(code: number | null | undefined): MediaErrorKind {
  if (code === 3 || code === 4) return 'decode';
  if (code === 2) return 'network';
  return 'other';
}
