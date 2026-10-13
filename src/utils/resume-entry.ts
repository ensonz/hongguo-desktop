/**
 * 续播起点决策（单一实现）：看过 > 3 秒续播本地断点那一集，否则云端
 * 观看历史 > 3 秒取那一集，都没有回落第 1 集。与后端
 * `play_service::resume::resolve_entry` 同口径（技术栈不同不共享实现，
 * 阈值改动两边同步）；2026-10-09 用户口径：不看片尾。
 */

export const RESUME_THRESHOLD_SECS = 3;
export const RESUME_THRESHOLD_MS = RESUME_THRESHOLD_SECS * 1000;

interface ResumeLocalProgress {
  vidIndex: number;
  currentTime: number;
}

interface ResumeHistoryItem {
  vidIndex: number;
  positionMs: number;
}

/**
 * 可续播的起点；`null` = 没有可续播记录（调用方回落第 1 集）。
 *
 * 本地断点命中时 `positionMs` 恒为 0：本地续播位置由后端 `resumeAt`
 * 直接带回，前端只负责选集；`positionMs` 仅供云端历史驱动续播提示。
 */
export function resolveResumeEntry(
  localProgress: ResumeLocalProgress | null | undefined,
  historyItem: ResumeHistoryItem | null | undefined,
): { index: number; positionMs: number } | null {
  if (localProgress && localProgress.currentTime > RESUME_THRESHOLD_SECS) {
    return { index: localProgress.vidIndex, positionMs: 0 };
  }
  if (historyItem && historyItem.positionMs > RESUME_THRESHOLD_MS) {
    return { index: Math.max(1, historyItem.vidIndex), positionMs: historyItem.positionMs };
  }
  return null;
}
