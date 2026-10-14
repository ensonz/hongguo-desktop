//! 表情插入的行为断言（复核清单第 4 项的行为核心）：`InputState::insert`
//! 把 `[名字]` 代码插到光标处、连选追加、已有文本尾巴不丢。
//!
//! 需要 gpui 的 test-support（`screenshot` feature 已连带开启——壳默认
//! 构建不编入），运行方式：
//! `cargo test -p hongguo-gpui --features screenshot --test emoji_insert`

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{AppContext as _, Entity, Render, TestAppContext, VisualTestContext};

/// 最小宿主视图：add_window 的根必须实现 Render，状态经它挂进窗口。
struct InputHost {
    state: Entity<InputState>,
}

impl Render for InputHost {
    fn render(
        &mut self,
        _window: &mut gpui_kit::Window,
        _cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        Input::new(&self.state)
    }
}

#[gpui_kit::test]
async fn emoji_insert_puts_code_at_cursor(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.add_window(|window, cx| {
        let state = cx.new(|cx| InputState::new(window, cx));
        InputHost { state }
    });
    let mut vx = VisualTestContext::from_window(window.into(), cx);
    vx.update(|window, cx| {
        let host = window
            .root::<InputHost>()
            .flatten()
            .expect("root view is InputHost");
        host.update(cx, |host, cx| {
            host.state
                .update(cx, |s, cx| s.insert("[微笑]", window, cx));
        });
        // 连选第二个：落在前一个之后（光标语义），不是覆盖
        host.update(cx, |host, cx| {
            host.state.update(cx, |s, cx| s.insert("[赞]", window, cx));
        });
        let text = host.read(cx).state.read(cx).value().to_string();
        assert_eq!(text, "[微笑][赞]");
    });
}

#[gpui_kit::test]
async fn emoji_insert_into_existing_text_keeps_tail(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let window = cx.add_window(|window, cx| {
        let state = cx.new(|cx| InputState::new(window, cx));
        InputHost { state }
    });
    let mut vx = VisualTestContext::from_window(window.into(), cx);
    vx.update(|window, cx| {
        let host = window
            .root::<InputHost>()
            .flatten()
            .expect("root view is InputHost");
        host.update(cx, |host, cx| {
            host.state
                .update(cx, |s, cx| s.insert("哈哈哈", window, cx));
        });
        // 光标默认在末尾：插入拼在已有文本后，尾巴不丢
        host.update(cx, |host, cx| {
            host.state.update(cx, |s, cx| s.insert("[笑]", window, cx));
        });
        let text = host.read(cx).state.read(cx).value().to_string();
        assert_eq!(text, "哈哈哈[笑]");
    });
}
