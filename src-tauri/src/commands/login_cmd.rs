//! 登录相关 command：发码 / 短信登录 / MFA 轮询 / 状态 / 退出。
//!
//! 只做参数校验与返回模型组装；登录编排（后台轮询 / 会话凭据 / 账号落库）
//! 在 [`crate::service::login_service`]。

use serde::Serialize;
use tauri::State;

use crate::app_state::AppState;
use hongguo_api::login::{
    self, LoginOutcome, MfaContext, MfaFlow, PassportUser, UpsmsState,
};
use crate::service::login_service::{account, csrf, polling};
use hongguo_common::model::AccountState;
use hongguo_common::{AppError, AppResult};

/// 发送短信验证码。发码会话的 csrf 凭据暂存 AppState（sms_login 消费）。
#[tauri::command]
pub async fn login_send_code(
    state: State<'_, AppState>,
    mobile: String,
) -> AppResult<login::SendCodeOutcome> {
    validate_mobile(&mobile)?;
    let env = state.api_env();
    match login::send_sms_code(&env, &mobile).await {
        Ok(outcome) => {
            log::info!("[Login] 发码成功: {}", outcome.message);
            if !outcome.csrf_cookie.is_empty() {
                *state.login_csrf.write() = Some(outcome.csrf_cookie.clone());
            }
            Ok(outcome)
        }
        Err(e) => {
            log::warn!("[Login] 发码失败: {e}");
            Err(e)
        }
    }
}

/// 短信验证码登录。成功即落库账号；MFA 需二次验证时返回上下文。
///
/// 会话绑定走发码下发的 csrf Cookie（1.1.3 实测），不再有 ticket 回传。
#[tauri::command]
pub async fn login_sms_login(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mobile: String,
    code: String,
    // MFA 重试上下文（上一次返回的 retryTag / smsCodeKey）
    mfa_retry_tag: Option<String>,
    mfa_sms_code_key: Option<String>,
) -> AppResult<LoginResult> {
    validate_mobile(&mobile)?;
    let code = code.trim().to_string();
    if code.is_empty() {
        return Err(AppError::Auth("验证码不能为空".into()));
    }
    let env = csrf::with_csrf(&state, state.api_env());
    let mfa = match (&mfa_retry_tag, &mfa_sms_code_key) {
        (Some(t), Some(k)) => Some((t.as_str(), k.as_str())),
        _ => None,
    };
    let outcome = login::sms_login(&env, &mobile, &code, mfa).await;
    if let Err(e) = &outcome {
        log::warn!("[Login] 登录失败: {e}");
    }
    match outcome? {
        LoginOutcome::Success {
            cookies,
            user,
            token,
        } => {
            log::info!("[Login] 登录成功: {} ({})", user.name, user.user_id);
            let account = account::account_from_login(mobile.clone(), cookies, &user, token);
            account::complete_login(&state, account)?;
            Ok(LoginResult::Success { user })
        }
        LoginOutcome::Mfa(ctx) => {
            // 暂存流程并转后台轮询：用户回复短信后自动重登并发事件——
            // 前端界面关掉也不丢状态（2026-10-04 体验事故修复）
            *state.login_mfa.write() = Some(MfaFlow {
                ctx: (*ctx).clone(),
                mobile,
                code,
            });
            polling::spawn_mfa_polling(app, state.inner().clone());
            Ok(mfa_result(&ctx))
        }
    }
}

/// MFA 上行短信验证轮询（单次调用；前端 3 秒间隔重发）。
///
/// 轮询到 `registered` 后**自动重登**换取会话并落库（用户只需回复短信）。
#[tauri::command]
pub async fn login_mfa_verify(state: State<'_, AppState>) -> AppResult<LoginResult> {
    let flow = state
        .login_mfa
        .read()
        .clone()
        .ok_or_else(|| AppError::Auth("没有进行中的 MFA 验证，请重新登录".into()))?;
    let env = csrf::with_csrf(&state, state.api_env());
    match login::upsms_verify(&flow.ctx, &env).await? {
        UpsmsState::Waiting => Ok(LoginResult::MfaWaiting),
        UpsmsState::Registered { .. } => {
            match login::mfa_relogin(&env, &flow.mobile, &flow.code, &flow.ctx).await? {
                LoginOutcome::Success {
                    cookies,
                    user,
                    token,
                } => {
                    let account =
                        account::account_from_login(flow.mobile.clone(), cookies, &user, token);
                    account::complete_login(&state, account)?;
                    Ok(LoginResult::Success { user })
                }
                LoginOutcome::Mfa(ctx) => {
                    // 新一轮 MFA（换了一种验证方式）：更新流程继续轮询
                    *state.login_mfa.write() = Some(MfaFlow {
                        ctx: (*ctx).clone(),
                        mobile: flow.mobile.clone(),
                        code: flow.code.clone(),
                    });
                    Ok(mfa_result(&ctx))
                }
            }
        }
    }
}

/// 取消进行中的 MFA 验证（后台轮询循环会随流程清空自行退出）。
#[tauri::command]
pub fn login_mfa_cancel(state: State<'_, AppState>) -> AppResult<()> {
    *state.login_mfa.write() = None;
    Ok(())
}

/// 登录状态（含用户信息；未登录为 None）。
#[tauri::command]
pub fn login_status(state: State<'_, AppState>) -> AppResult<Option<AccountState>> {
    Ok(state.settings().account)
}

/// 校验会话有效性并刷新用户信息（会话过期返回错误）。
#[tauri::command]
pub async fn login_user_info(state: State<'_, AppState>) -> AppResult<PassportUser> {
    account::fetch_and_refresh_user(&state).await
}

/// 退出登录（清空账号设置；退出是纯本地操作，1.1.3 实测无网络请求）。
#[tauri::command]
pub fn login_logout(state: State<'_, AppState>) -> AppResult<()> {
    *state.login_csrf.write() = None;
    *state.login_mfa.write() = None;
    account::persist_account(&state, None)
}

/// MFA 上下文 → 前端返回（展示回复方式，凭据不回传）。
fn mfa_result(ctx: &MfaContext) -> LoginResult {
    LoginResult::Mfa {
        retry_tag: ctx.retry_tag.clone(),
        sms_code_key: ctx.sms_code_key.clone(),
        channel_mobile: ctx.channel_mobile.clone(),
        sms_content: ctx.sms_content.clone(),
        tips: polling::mfa_tips(ctx),
    }
}

/// 手机号形态校验：11 位、1 开头、全数字（不做号段穷举，服务端会再校验）。
fn validate_mobile(mobile: &str) -> AppResult<()> {
    let ok =
        mobile.len() == 11 && mobile.starts_with('1') && mobile.bytes().all(|b| b.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(AppError::Auth("手机号格式不正确".into()))
    }
}

/// 登录命令的统一返回：成功 / 需 MFA / MFA 等待中。
#[derive(Debug, Clone, Serialize)]
// enum 级 rename_all 只作用于变体名（tag 值）；变体字段必须各自
// 再标一次 camelCase——否则 Mfa 序列化成 retry_tag 之类 snake_case，
// 前端 zod 校验直接挂（2026-10-04「数据格式异常」事故）
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum LoginResult {
    #[serde(rename_all = "camelCase")]
    Success {
        user: PassportUser,
    },
    #[serde(rename_all = "camelCase")]
    Mfa {
        retry_tag: String,
        sms_code_key: String,
        /// 上行短信通道号（如 9515211003）
        channel_mobile: String,
        /// 要回复的内容（如 "YZ"）
        sms_content: String,
        tips: String,
    },
    MfaWaiting,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mfa 变体的序列化形状必须与前端 loginResultSchema 逐字段一致
    /// （一次 zod 校验失败事故的回归锁）。channel_mobile 用替换后的
    /// 真实通道号（parse 层已做 95→106 修正，这里锁序列化形状）。
    #[test]
    fn mfa_serializes_to_frontend_shape() {
        let r = mfa_result(&hongguo_api::login::MfaContext {
            retry_tag: "1".into(),
            sms_code_key: "k".into(),
            channel_mobile: "10691859839103".into(),
            sms_content: "YZ".into(),
            tips: "回复 YZ 到 10691859839103".into(),
            ..Default::default()
        });
        let j = serde_json::to_string(&r).unwrap();
        println!("Mfa JSON = {j}");
        assert!(j.contains(r#""kind":"mfa""#), "tag 必须是 mfa: {j}");
        assert!(j.contains(r#""retryTag":"1""#), "字段必须 camelCase: {j}");
        assert!(j.contains(r#""smsCodeKey":"k""#), "{j}");
        assert!(j.contains(r#""channelMobile":"10691859839103""#), "{j}");
        assert!(j.contains(r#""smsContent":"YZ""#), "{j}");
        assert!(j.contains("10691859839103"), "{j}");
    }
}
