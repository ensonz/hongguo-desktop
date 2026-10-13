//! 登录态与壳内 SMS 登录（发码/提交/MFA 后台轮询）、登录面板渲染与
//! 评论/回复头像的拉取与渲染。

use std::sync::Arc;
use std::time::Duration;
#[cfg(feature = "screenshot")]
use std::time::Instant;

use gpui_kit::component::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::Input;
use gpui_kit::component::v_flex;
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{SharedString, div};

#[cfg(feature = "screenshot")]
use super::controls::CONTROLS_HOLD_SECS;
use crate::bridge;

impl super::HomeView {
    /// 登录态（settings 账号卡片的 user_id 非空）。
    pub(super) fn logged_in(&self) -> bool {
        !self.my_user_id().is_empty()
    }

    /// 当前登录用户 id（未登录空串）：评论/回复「删除」按钮的 isMine 判据
    ///（Tauri myUserId 同口径）。
    pub(super) fn my_user_id(&self) -> String {
        self.state
            .settings()
            .account
            .map(|a| a.user_id)
            .unwrap_or_default()
    }

    /// 打开/关闭登录面板（Shell 侧栏入口）。
    pub(crate) fn toggle_login_panel(&mut self, cx: &mut Context<Self>) {
        self.login_panel_open = !self.login_panel_open;
        self.login_message = None;
        cx.notify();
    }

    /// 退出登录（settings.account 清空落库；Tauri login_logout 同口径）。
    pub(crate) fn login_logout(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = hongguo_service::login_service::account::persist_account(&self.state, None)
        {
            log::warn!("[Login] 退出失败: {e}");
            return;
        }
        self.mfa_retry = None;
        self.login_message = Some("已退出登录".to_string());
        log::info!("[Login] 已退出登录");
        cx.notify();
    }

    /// 发送短信验证码（发码下发的 csrf Cookie 暂存 state，sms_login 消费；
    /// Tauri login_send_code 同款编排）。
    fn login_send_code(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.login_mobile else {
            return;
        };
        let mobile = input.read(cx).value().trim().to_string();
        if mobile.len() != 11 || !mobile.starts_with('1') || self.login_busy {
            self.login_message = Some("手机号格式不正确".into());
            cx.notify();
            return;
        }
        self.login_busy = true;
        self.login_message = None;
        let state = Arc::clone(&self.state);
        let rx = bridge::spawn(async move {
            let env = state.api_env();
            hongguo_api::login::send_sms_code(&env, &mobile).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| {
                view.login_busy = false;
                match result {
                    Ok(outcome) => {
                        log::info!("[Login] 发码成功: {}", outcome.message);
                        if !outcome.csrf_cookie.is_empty() {
                            *view.state.login_csrf.write() = Some(outcome.csrf_cookie);
                        }
                        view.login_message = Some(format!("{}（注意查收短信）", outcome.message));
                    }
                    Err(e) => {
                        log::warn!("[Login] 发码失败: {e}");
                        view.login_message = Some(format!("发码失败：{e}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 短信验证码登录：成功落库账号（persist_account 无 tauri 依赖）+
    /// 后台拉一次 user_info 刷新昵称/头像；MFA（风控短信上行）转「重试
    /// 登录」路径——点重试用暂存的 retry 上下文原样重发。
    fn login_submit(&mut self, cx: &mut Context<Self>) {
        let Some(mobile_input) = &self.login_mobile else {
            return;
        };
        let Some(code_input) = &self.login_code else {
            return;
        };
        let mobile = mobile_input.read(cx).value().trim().to_string();
        let code = code_input.read(cx).value().trim().to_string();
        if mobile.len() != 11 || !mobile.starts_with('1') {
            self.login_message = Some("手机号格式不正确".into());
            cx.notify();
            return;
        }
        if code.is_empty() || self.login_busy {
            return;
        }
        self.login_busy = true;
        self.login_message = None;
        let state = Arc::clone(&self.state);
        let mfa = self.mfa_retry.clone();
        let mobile_for_api = mobile.clone();
        let rx = bridge::spawn(async move {
            let env = hongguo_service::login_service::csrf::with_csrf(&state, state.api_env());
            let mfa = mfa.as_ref().map(|(t, k)| (t.as_str(), k.as_str()));
            hongguo_api::login::sms_login(&env, &mobile_for_api, &code, mfa).await
        });
        cx.spawn(async move |this, cx| {
            let result = rx.await.expect("tokio 桥不会丢弃发送端");
            let _ = this.update(cx, |view, cx| match result {
                Err(e) => {
                    view.login_busy = false;
                    log::warn!("[Login] 登录失败: {e}");
                    view.login_message = Some(format!("登录失败：{e}"));
                    cx.notify();
                }
                Ok(hongguo_api::login::LoginOutcome::Success {
                    cookies,
                    user,
                    token,
                }) => {
                    view.login_busy = false;
                    log::info!("[Login] 登录成功: {} ({})", user.name, user.user_id);
                    let account = hongguo_service::login_service::account::account_from_login(
                        mobile, cookies, &user, token,
                    );
                    if let Err(e) = hongguo_service::login_service::account::persist_account(
                        &view.state,
                        Some(account),
                    ) {
                        log::warn!("[Login] 账号落库失败: {e}");
                        view.login_message = Some(format!("账号落库失败：{e}"));
                        cx.notify();
                        return;
                    }
                    view.mfa_retry = None;
                    // 后台拉 user_info 刷新昵称/头像（sms_login 只给默认
                    // 档案；refresh_profile_after_login 耦合 tauri runtime，
                    // 这里自行编排——失败只记日志）
                    let state = Arc::clone(&view.state);
                    cx.spawn(async move |this, cx| {
                        let refreshed =
                            hongguo_service::login_service::account::fetch_and_refresh_user(&state)
                                .await;
                        let _ = this.update(cx, |view, cx| {
                            match refreshed {
                                Ok(user) => {
                                    view.login_message = Some(format!("登录成功：{}", user.name));
                                }
                                Err(e) => {
                                    log::warn!("[Login] 用户资料刷新失败: {e}");
                                    view.login_message = Some("登录成功".into());
                                }
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                    cx.notify();
                }
                Ok(hongguo_api::login::LoginOutcome::Mfa(ctx)) => {
                    view.login_busy = false;
                    view.mfa_retry = Some((ctx.retry_tag.clone(), ctx.sms_code_key.clone()));
                    log::info!("[Login] 需要短信上行验证，转后台自动轮询");
                    view.login_message =
                        Some(hongguo_service::login_service::polling::mfa_tips(&ctx));
                    view.spawn_mfa_polling(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// MFA 后台自动轮询（Tauri polling::spawn_mfa_polling 的 gpui 编排）：
    /// 每 3 秒一次 `upsms_verify`，用户回复短信后自动 `mfa_relogin` 落库，
    /// 全程只更新面板 message（壳无事件总线，message 就是状态出口）。
    /// 退出条件：5 分钟超时 / 流程被清（取消/退出登录）/ 指纹变更（用户
    /// 重新发起登录，旧循环自行退出）。
    fn spawn_mfa_polling(&mut self, cx: &mut Context<Self>) {
        const MFA_TIMEOUT: Duration = Duration::from_secs(300);
        const MFA_POLL_INTERVAL: Duration = Duration::from_secs(3);
        let state = Arc::clone(&self.state);
        cx.spawn(async move |this, cx| {
            let deadline = std::time::Instant::now() + MFA_TIMEOUT;
            let mut round: u32 = 0;
            loop {
                if std::time::Instant::now() > deadline {
                    let _ = this.update(cx, |view, cx| {
                        view.state.login_mfa.write().take();
                        view.login_message = Some("5 分钟未确认，登录已超时退出".into());
                        view.mfa_retry = None;
                        cx.notify();
                    });
                    log::warn!("[Login] MFA 5 分钟未确认，超时退出");
                    return;
                }
                // 流程被清（取消/退出登录）→ 循环退出
                let Some(flow) = state.login_mfa.read().clone() else {
                    return;
                };
                let fingerprint = (flow.ctx.retry_tag.clone(), flow.ctx.log_id.clone());
                let env = hongguo_service::login_service::csrf::with_csrf(&state, state.api_env());
                round += 1;
                let outcome = hongguo_api::login::upsms_verify(&flow.ctx, &env).await;
                match outcome {
                    Ok(hongguo_api::login::UpsmsState::Registered { mfa_token, .. }) => {
                        log::info!("[Login] MFA 第 {round} 轮 registered，自动重登");
                        // registered 响应下发新 mfa_token，重登必须带它
                        let mut ctx = flow.ctx.clone();
                        ctx.mfa_token = mfa_token;
                        let relogin = hongguo_api::login::mfa_relogin(
                            &env,
                            &flow.mobile,
                            &flow.code,
                            &ctx,
                        )
                        .await;
                        match relogin {
                            Ok(hongguo_api::login::LoginOutcome::Success {
                                cookies,
                                user,
                                token,
                            }) => {
                                let account =
                                    hongguo_service::login_service::account::account_from_login(
                                        flow.mobile.clone(),
                                        cookies,
                                        &user,
                                        token,
                                    );
                                let _ = this.update(cx, |view, cx| {
                                    if let Err(e) =
                                        hongguo_service::login_service::account::persist_account(
                                            &view.state,
                                            Some(account),
                                        ) {
                                        view.login_message =
                                            Some(format!("账号落库失败：{e}"));
                                    } else {
                                        view.login_message =
                                            Some(format!("登录成功：{}", user.name));
                                    }
                                    view.state.login_mfa.write().take();
                                    view.mfa_retry = None;
                                    cx.notify();
                                });
                                // 后台拉 user_info 刷新昵称/头像（同 login_submit）
                                let state = Arc::clone(&state);
                                let _ = this.update(cx, |_, cx| {
                                    cx.spawn(async move |this, cx| {
                                        let refreshed = hongguo_service::login_service::account::fetch_and_refresh_user(&state).await;
                                        let _ = this.update(cx, |view, cx| {
                                            if let Ok(user) = refreshed {
                                                view.login_message =
                                                    Some(format!("登录成功：{}", user.name));
                                                cx.notify();
                                            }
                                        });
                                    })
                                    .detach();
                                });
                                log::info!(
                                    "[Login] MFA 自动登录成功: {} ({})",
                                    user.name,
                                    user.user_id
                                );
                                return;
                            }
                            Ok(hongguo_api::login::LoginOutcome::Mfa(next)) => {
                                // 新一轮验证（换了方式）：替换流程继续等
                                let _ = this.update(cx, |view, cx| {
                                    view.state.login_mfa.write().replace(
                                        hongguo_api::login::MfaFlow {
                                            ctx: (*next).clone(),
                                            mobile: flow.mobile.clone(),
                                            code: flow.code.clone(),
                                        },
                                    );
                                    view.mfa_retry = Some((
                                        next.retry_tag.clone(),
                                        next.sms_code_key.clone(),
                                    ));
                                    view.login_message =
                                        Some(hongguo_service::login_service::polling::mfa_tips(
                                            &next,
                                        ));
                                    cx.notify();
                                });
                            }
                            Err(e) => {
                                let _ = this.update(cx, |view, cx| {
                                    view.state.login_mfa.write().take();
                                    view.mfa_retry = None;
                                    view.login_message = Some(format!("登录失败：{e}"));
                                    cx.notify();
                                });
                                return;
                            }
                        }
                    }
                    Ok(hongguo_api::login::UpsmsState::Waiting) => {
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
                cx.background_executor()
                    .timer(MFA_POLL_INTERVAL)
                    .await;
            }
        })
        .detach();
    }

    /// 拉评论/回复头像（avatar URL → RGBA）。同一阶梯（嗅探透传 jpeg，
    /// 失败退回首字圆形占位）；key 就是 URL，列表里同人多条只拉一次。
    /// `on_miss` 场景不重试：头像属装饰信息，失败静默（与封面同口径）。
    pub(super) fn load_avatar(&mut self, url: String, cx: &mut Context<Self>) {
        if url.is_empty()
            || self.avatars.contains_key(&url)
            || !self.avatars_pending.insert(url.clone())
        {
            return;
        }
        let rx = bridge::spawn({
            let remote = url.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    hongguo_protocol::cover::acquire_renderable(&remote).map(|(bytes, _)| bytes)
                })
                .await
                .expect("阻塞池 join")
            }
        });
        cx.spawn(async move |this, cx| {
            let bytes: Vec<u8> = match rx.await.expect("tokio 桥不会丢弃发送端") {
                Ok(b) => b,
                Err(e) => {
                    log::warn!("[Home] 头像加载失败: {e}");
                    return;
                }
            };
            let _ = this.update(cx, |view, cx| match image::load_from_memory(&bytes) {
                Ok(decoded) => {
                    view.avatars.insert(
                        url,
                        Arc::new(crate::image_upload::render_image(decoded.to_rgba8())),
                    );
                    cx.notify();
                }
                Err(e) => log::warn!("[Home] 头像解码失败: {e}"),
            });
        })
        .detach();
    }

    /// 登录面板（居中卡片）：已登录显示账号+退出；未登录显示手机号/验证码
    /// 表单 + 登录按钮。点遮罩收起。
    pub(super) fn render_login_panel(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let account = self.state.settings().account;
        v_flex()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::rgba(0x00000099))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.login_panel_open = false;
                    cx.notify();
                }),
            )
            .child({
                let mobile_input = self.login_mobile.clone();
                let code_input = self.login_code.clone();
                v_flex()
                    .w(gpui_kit::px(300.0))
                    .p_5()
                    .gap_3()
                    .rounded(gpui_kit::px(12.0))
                    .bg(gpui_kit::rgb(0x1f1f1f))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(gpui_kit::rgb(0xffffff))
                            .child(match &account {
                                Some(a) => SharedString::from(format!("账号：{}", a.user_name)),
                                None => "登录红果".into(),
                            }),
                    )
                    .when_some(account.clone(), |s, a| {
                        s.child(
                            div()
                                .text_xs()
                                .text_color(gpui_kit::rgba(0xffffff99))
                                .child(SharedString::from(format!("UID {}", a.user_id))),
                        )
                        .child(
                            Button::new("login-logout")
                                .label("退出登录")
                                .danger()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.login_logout(cx);
                                    this.login_panel_open = false;
                                    cx.notify();
                                })),
                        )
                    })
                    .when(account.is_none(), |s| {
                        s.children(
                            mobile_input
                                .as_ref()
                                .map(|input| Input::new(input).h(gpui_kit::px(32.0)).text_sm()),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .children(code_input.as_ref().map(|input| {
                                    Input::new(input).flex_1().h(gpui_kit::px(32.0)).text_sm()
                                }))
                                .child(
                                    Button::new("login-send-code")
                                        .label(if self.login_busy {
                                            "发送中…"
                                        } else {
                                            "发送验证码"
                                        })
                                        .ghost()
                                        .small()
                                        .disabled(self.login_busy)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.login_send_code(cx);
                                        })),
                                ),
                        )
                        .child(
                            Button::new("login-submit")
                                .label(if self.mfa_retry.is_some() {
                                    "重试登录"
                                } else {
                                    "登录"
                                })
                                .primary()
                                .small()
                                .disabled(self.login_busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.login_submit(cx);
                                })),
                        )
                    })
                    .children(self.login_message.clone().map(|msg| {
                        div()
                            .text_xs()
                            .text_color(gpui_kit::rgba(0x93c5fdff))
                            .child(SharedString::from(msg))
                    }))
                    .into_any_element()
            })
            .into_any_element()
    }

    /// 头像元素：有图用图（圆形裁切），无图/未加载退回昵称首字圆形占位。
    /// 外层固定尺寸框是必须的——img 直接 size_full 在无约束 flex 里会撑爆。
    pub(super) fn render_avatar(&self, url: &str, name: &str, size: f32) -> gpui_kit::AnyElement {
        let frame = div()
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
            .size(gpui_kit::px(size));
        match self.avatars.get(url) {
            Some(img) => frame
                .child(
                    gpui_kit::img(gpui_kit::ImageSource::Render(Arc::clone(img)))
                        .size_full()
                        .object_fit(gpui_kit::ObjectFit::Cover),
                )
                .into_any_element(),
            None => frame
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::rgba(0x404040ff))
                .text_xs()
                .text_color(gpui_kit::rgb(0xffffff))
                .child({
                    let ch = name.chars().next().unwrap_or('游');
                    SharedString::from(ch.to_string())
                })
                .into_any_element(),
        }
    }

    /// 取证辅助（screenshot feature 的 HONGGUO_SHOT_PANEL=login）：打开
    /// 登录面板（无人值守抓登录 UI）。
    #[cfg(feature = "screenshot")]
    pub(crate) fn debug_open_login(&mut self, cx: &mut Context<Self>) {
        self.login_panel_open = true;
        self.controls_until = Some(Instant::now() + Duration::from_secs(CONTROLS_HOLD_SECS));
        cx.notify();
    }
}
