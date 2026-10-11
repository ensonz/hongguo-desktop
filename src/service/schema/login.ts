/**
 * 与 Rust serde 模型对应的 zod schema（登录域）。
 */
import { z } from 'zod';

export const passportUserSchema = z.object({
  userId: z.string(),
  name: z.string(),
  mobile: z.string(),
  avatarUrl: z.string().catch(''),
});

/** 发码结果：mobileTicket 登录时必须回传 */
export const sendCodeOutcomeSchema = z.object({
  message: z.string(),
  mobileTicket: z.string(),
  /** 重发等待秒数（服务端 retry_time） */
  retryTime: z.number().default(60),
});

/**
 * MFA 后台轮询事件负载（`login-mfa-state`，Rust `login_cmd.rs::LOGIN_MFA_STATE`）：
 * success = 自动登录完成（带昵称）；failed = 验证失败/超时；waiting = 新一轮
 * 上行短信验证提示。三态字段互斥，discriminated union 钉死。
 */
export const loginMfaStateSchema = z.discriminatedUnion('state', [
  z.object({ state: z.literal('success'), name: z.string() }),
  z.object({ state: z.literal('failed'), message: z.string() }),
  z.object({ state: z.literal('waiting'), message: z.string() }),
]);

export type LoginMfaState = z.infer<typeof loginMfaStateSchema>;

/** 登录命令返回：成功 / 需要 MFA 上行短信验证 / MFA 等待中 */
export const loginResultSchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('success'), user: passportUserSchema }),
  z.object({
    kind: z.literal('mfa'),
    retryTag: z.string(),
    smsCodeKey: z.string(),
    /** 上行短信通道号（如 9515211003；提示文案已拼进 tips） */
    channelMobile: z.string().default(''),
    /** 要回复的短信内容（如 "YZ"） */
    smsContent: z.string().default(''),
    tips: z.string(),
  }),
  z.object({ kind: z.literal('mfaWaiting') }),
]);

export type LoginResult = z.infer<typeof loginResultSchema>;
