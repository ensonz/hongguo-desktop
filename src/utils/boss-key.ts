/** 老板键组合的纯函数（无模块级副作用，可被同置测试直接引用）。

 *  老板键是全局快捷键（tauri accelerator，如 "CommandOrControl+Alt+H"），
 *  播放器快捷键是应用内 `e.key` 值（如 "h" / " " / "ArrowLeft"）：全局
 *  组合在应用内按下时 keydown 的 key 就是末段裸键，两者会同时触发，
 *  因此按「末段裸键 vs 播放器键」做冲突比对。 */

/** 老板键组合 → 末段裸键名（小写）。空串/残串返回空。 */
export function bossKeyCoreKey(bossKey: string): string {
  const parts = bossKey
    .split('+')
    .map((p) => p.trim().toLowerCase())
    .filter(Boolean);
  return parts[parts.length - 1] ?? '';
}

export interface BossKeyConflict {
  /** 冲突的动作标识（调用方的领域键，如 ShortcutAction） */
  action: string;
  /** 冲突的播放器键（可读形态） */
  label: string;
}

/** 老板键 vs 播放器快捷键冲突检测。`actions` 由调用方从自己的快捷键
 *  映射展开（可读键名 + e.key 值），本函数保持无依赖。 */
export function bossKeyConflicts(
  bossKey: string,
  actions: { action: string; label: string; key: string }[],
): BossKeyConflict[] {
  const core = bossKeyCoreKey(bossKey);
  if (!core) return [];
  return actions
    .filter((a) => a.key.trim().toLowerCase() === core)
    .map(({ action, label }) => ({ action, label }));
}
