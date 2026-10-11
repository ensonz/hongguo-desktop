import { invoke } from '@tauri-apps/api/core';
import { appErrorSchema } from '../schema';
import { t, type MessageKey } from '@/locales';
import { COMMANDS, type CallRest, type CommandName, type CommandResponse } from './registry';

/**
 * 类型化 invoke：命令名、入参键名、响应形状三者在编译期一次钉死（契约
 * 表在 `./registry`，与 EVENTS 同为两端镜像）。
 *
 * 传输层错误归一（`toError`）；响应过 zod 校验——校验失败就是「后端改了
 * 契约」，必须立刻暴露（`error.badPayload`，原始 issue 挂 cause）。
 */
export async function call<C extends CommandName>(
  command: C,
  ...rest: CallRest<C>
): Promise<CommandResponse<C>> {
  const raw: unknown = await invoke(command, rest[0]).catch((e: unknown) => {
    throw toError(e);
  });
  try {
    return COMMANDS[command].response.parse(raw) as CommandResponse<C>;
  } catch (e) {
    // ZodError 的 message 是后端响应体的 JSON 投影（path / code / expected），
    // 糊到 toast 上等于把内部数据契约摊给用户。换成一句能读懂的提示，
    // 原始 issue 挂到 cause 上，出问题时照样能查。
    throw new Error(t('error.badPayload'), { cause: e });
  }
}

/** 把 Tauri 抛出的各种形态归一成 Error。 */
function toError(e: unknown): Error {
  // Rust 的自定义错误会被序列化成 { kind, message }：
  // kind 是 i18n key，message 是后端写死的中文原文。展示用译文，
  // 原文挂在 cause 上，英文界面下也还能靠它排查问题。
  if (e && typeof e === 'object' && 'kind' in e && 'message' in e) {
    const parsed = appErrorSchema.safeParse(e);
    if (parsed.success) {
      const error = new Error(localizedMessage(parsed.data.kind, parsed.data.message));
      error.cause = parsed.data.message;
      return error;
    }
  }
  if (e instanceof Error) return e;
  if (typeof e === 'string') return new Error(e);
  // 认不出来的抛出物：文案只说「操作失败」，command 名属于内部信息，
  // 原始载荷留给 cause。
  return new Error(t('error.internal'), { cause: e });
}

/**
 * 错误文案归一：kind 有对应词条用译文，否则回落 message 原文。
 *
 * 命令错误（`toError`）与事件失败载荷（如 `login-mfa-state` 的 failed）
 * 共用同一策略——kind 的键集由 Rust 侧 AppError 变体定义，跨命令聚合
 * 无法静态枚举，才 cast。
 */
export function localizedMessage(kind: string, message: string): string {
  const localized = t(kind as MessageKey);
  return localized === kind ? message : localized;
}
