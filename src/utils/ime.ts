/**
 * IME 组合中判定（中文/日文输入法选词）。Enter 提交类快捷键在组合中
 * 必须让行，否则选词确认会被当「发送」吞掉。标准 `isComposing` 之外
 * 再看 `keyCode === 229`：Safari 旧版组合中不置 isComposing 只发 229。
 *
 * React 合成 KeyboardEvent 与原生事件都暴露这两个字段，直接收鸭子类型。
 */
export function isImeComposing(e: { isComposing: boolean; keyCode: number }): boolean {
  return e.isComposing || e.keyCode === 229;
}
