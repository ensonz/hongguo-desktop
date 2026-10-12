//! MFA 后台轮询：回复短信后自动重登并发状态事件。

use tauri::Emitter;

use super::account::{account_from_login, complete_login};
use super::csrf::with_csrf;
use super::events::{LOGIN_MFA_STATE, LoginMfaStateEvent};
use crate::app_state::AppState;
use hongguo_api::login::{self, LoginOutcome, MfaContext, MfaFlow, UpsmsState};

/// 轮询上限：超过即报超时退出。
const MFA_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// 轮询间隔。
const MFA_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

/// MFA 上行短信验证的展示文案（补上「回复 X 到 Y」）。
///
/// 通道号已在 tips 里出现时不再补，避免重复。`tips` 是服务端下发的展示
/// 内容，原样透传，不做 i18n。
pub fn mfa_tips(ctx: &MfaContext) -> String {
    if ctx.channel_mobile.is_empty() || ctx.tips.contains(&ctx.channel_mobile) {
        ctx.tips.clone()
    } else {
        format!(
            "{}（回复 {} 到 {}）",
            ctx.tips, ctx.sms_content, ctx.channel_mobile
        )
    }
}

/// MFA 后台轮询：`MFA_POLL_INTERVAL` 一次直到 registered / 取消 / 超时。
///
/// 成功即自动重登、落库并发 `login-mfa-state` 事件（success/failed）；
/// 每轮先校验流程指纹（用户重新发起登录会替换流程，旧循环自行退出）。
pub fn spawn_mfa_polling(app: tauri::AppHandle, state: AppState) {
    tauri::async_runtime::spawn(async move {
        let deadline = std::time::Instant::now() + MFA_TIMEOUT;
        let mut round: u32 = 0;
        loop {
            if std::time::Instant::now() > deadline {
                *state.login_mfa.write() = None;
                log::warn!("[Login] MFA 5 分钟未确认，超时退出");
                emit(
                    &app,
                    LoginMfaStateEvent::Failed {
                        message_kind: "settings.loginMfaTimeout".into(),
                        message: String::new(),
                    },
                );
                return;
            }
            let Some(flow) = state.login_mfa.read().clone() else {
                return; // 用户取消 / 退出登录
            };
            let fingerprint = (flow.ctx.retry_tag.clone(), flow.ctx.log_id.clone());
            let env = with_csrf(&state, state.api_env());
            round += 1;
            match login::upsms_verify(&flow.ctx, &env).await {
                Ok(UpsmsState::Registered { mfa_token, .. }) => {
                    log::info!("[Login] MFA 第 {round} 轮 registered，自动重登");
                    // registered 响应下发新 mfa_token，重登必须带它
                    let mut ctx = flow.ctx.clone();
                    ctx.mfa_token = mfa_token;
                    match login::mfa_relogin(&env, &flow.mobile, &flow.code, &ctx).await {
                        Ok(LoginOutcome::Success {
                            cookies,
                            user,
                            token,
                        }) => {
                            let account =
                                account_from_login(flow.mobile.clone(), cookies, &user, token);
                            if let Err(e) = complete_login(&state, account) {
                                emit(
                                    &app,
                                    LoginMfaStateEvent::Failed {
                                        message_kind: e.i18n_key().into(),
                                        message: e.to_string(),
                                    },
                                );
                                return;
                            }
                            log::info!(
                                "[Login] MFA 自动登录成功: {} ({})",
                                user.name,
                                user.user_id
                            );
                            emit(&app, LoginMfaStateEvent::Success { name: user.name });
                            return;
                        }
                        Ok(LoginOutcome::Mfa(next)) => {
                            // 新一轮验证（换了方式）：替换流程继续等
                            *state.login_mfa.write() = Some(MfaFlow {
                                ctx: (*next).clone(),
                                mobile: flow.mobile.clone(),
                                code: flow.code.clone(),
                            });
                            emit(
                                &app,
                                LoginMfaStateEvent::Waiting {
                                    server_tip: mfa_tips(&next),
                                },
                            );
                        }
                        Err(e) => {
                            *state.login_mfa.write() = None;
                            emit(
                                &app,
                                LoginMfaStateEvent::Failed {
                                    message_kind: e.i18n_key().into(),
                                    message: e.to_string(),
                                },
                            );
                            return;
                        }
                    }
                }
                Ok(UpsmsState::Waiting) => {
                    // 30 秒打一条，用户在等的时候日志可查轮询是否活着
                    if round % 10 == 1 {
                        log::info!("[Login] MFA 第 {round} 轮：等待用户回复短信");
                    }
                }
                Err(e) => {
                    // 单次网络抖动不致命：流程还在就继续等，直到超时
                    log::warn!("[Login] MFA 轮询出错（继续等待）: {e}");
                }
            }
            // 指纹变了说明用户发起了新登录，旧循环退出
            let current = state.login_mfa.read().clone();
            if let Some(cur) = current
                && (cur.ctx.retry_tag, cur.ctx.log_id) != fingerprint
            {
                return;
            }
            tokio::time::sleep(MFA_POLL_INTERVAL).await;
        }
    });
}

/// 发一个 MFA 状态事件（前端不在场发不出去，无害）。
fn emit(app: &tauri::AppHandle, event: LoginMfaStateEvent) {
    let _ = app.emit(LOGIN_MFA_STATE, event);
}
