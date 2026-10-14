//! 官方 App 接口的请求体常量。
//!
//! ⚠️ 这些字段是**照抄现版**（`hongguo.js`）的实测值，改任何一个都会让服务端
//! 返回 `Code: 110001`（签名对但参数错）。验证时开 `RUST_LOG=debug`：
//! 响应字节数 > 0 只说明签名过了，还要看 body 里 `code` 是不是 0。

use serde_json::{Value, json};

/// commentapi 互动体系的枚举值。
///
/// ⚠️ 这些值是抓包锁定的服务端契约：错一个数字，服务端就回
/// `103008「无社区功能」` 或 `100103 PARAM_INVALID`——而且**三套体系
/// （评论 / 剧评 / 弹幕）互相只差一两个数字**。历史上它们散落为字面量，
/// 被「顺手统一」的风险很高，所以集中声明，使用点只引用。
///
/// 字段速查：
/// - `object_type`（do_action 互动对象）：视频 6 / 剧评 2 / 评论 8
/// - `comment_type`（内容类型）：弹幕 20 / 评论 4 / 剧评 2；**发送剧评是 0**
/// - `commit_source`（发送来源）：弹幕 1500 / 评论 3 / 评论回复 9 /
///   剧评发送 12 / 剧评回复 13
/// - `data_type`（负载类型）：弹幕 20 / 评论 4 / 剧评 2；删除恒 9
/// - `group_type`（会话维度）：剧评 1 / 评论·弹幕 30
/// - `server_channel`（列表通道）：弹幕 1000 / 评论 18 / 剧评 34
/// - `service_id`（删除的内容类型）：剧评 2 / 评论·回复 4
/// - 列表侧 `comment_source`（与发送侧 `commit_source` 是**两个不同枚举**）：
///   弹幕 601 / 评论 4 / 剧评 1 / 剧评回复 501 / 评论回复 504
pub mod interaction {
    /// `object_type`：互动对象类型。
    pub const OBJECT_TYPE_VIDEO: i64 = 6;
    pub const OBJECT_TYPE_REVIEW: i64 = 2;
    pub const OBJECT_TYPE_COMMENT: i64 = 8;

    /// `comment_type`：内容类型（剧评**发送**时是 0，列表 / 点赞是 2）。
    pub const COMMENT_TYPE_DANMAKU: i64 = 20;
    pub const COMMENT_TYPE_COMMENT: i64 = 4;
    pub const COMMENT_TYPE_REVIEW: i64 = 2;
    pub const COMMENT_TYPE_REVIEW_SEND: i64 = 0;

    /// `commit_source`：发送来源。
    pub const COMMIT_SOURCE_DANMAKU: i64 = 1500;
    pub const COMMIT_SOURCE_COMMENT: i64 = 3;
    pub const COMMIT_SOURCE_COMMENT_REPLY: i64 = 9;
    pub const COMMIT_SOURCE_REVIEW: i64 = 12;
    pub const COMMIT_SOURCE_REVIEW_REPLY: i64 = 13;

    /// `data_type`：负载类型（删除语义恒 9，与发送侧无关）。
    pub const DATA_TYPE_DANMAKU: i64 = 20;
    pub const DATA_TYPE_COMMENT: i64 = 4;
    pub const DATA_TYPE_REVIEW: i64 = 2;
    pub const DATA_TYPE_DELETE: i64 = 9;

    /// `group_type`：会话维度。
    pub const GROUP_TYPE_REVIEW: i64 = 1;
    pub const GROUP_TYPE_COMMENT: i64 = 30;

    /// `server_channel`：列表查询的服务端通道。
    pub const SERVER_CHANNEL_DANMAKU: i64 = 1000;
    pub const SERVER_CHANNEL_COMMENT: i64 = 18;
    pub const SERVER_CHANNEL_REVIEW: i64 = 34;

    /// `service_id`：删除接口的内容类型。
    pub const SERVICE_ID_REVIEW: i64 = 2;
    pub const SERVICE_ID_COMMENT: i64 = 4;

    /// 列表查询侧的 `comment_source`（与发送侧 `commit_source` 是两个枚举）。
    pub const LIST_SOURCE_DANMAKU: i64 = 601;
    pub const LIST_SOURCE_COMMENT: i64 = 4;
    pub const LIST_SOURCE_REVIEW: i64 = 1;
    pub const LIST_SOURCE_REVIEW_REPLIES: i64 = 501;
    pub const LIST_SOURCE_COMMENT_REPLIES: i64 = 504;
}

/// 分集详情接口路径。
pub const DETAIL_PATH: &str = "/novel/player/multi_video_detail/preload/v1";

/// 清晰度列表（取流地址）接口路径。
///
/// ⚠️ 必须打 **lq 域**（[`crate::client::LQ_API_ORIGIN]）。2026-10-14
/// 抓包实锤：批量形态 `multi_video_model/player/v1` 对**全新注册的真机
/// 设备**也回 200+0（hgplayer 自己 4 连空），真实播放走的是单集端点
/// [`SINGLE_MODEL_PATH`]（hgplayer：批量空 → 回落单集 → CDN 直链）。
pub const MODEL_PATH: &str = "/novel/player/multi_video_model/player/v1";

/// 单集取流端点（hgplayer 现役路径）：POST JSON
/// `{biz_param{…,video_platform:3,from_video_id:""}, content_type:1004,
/// video_id}`，2026-10-14 实测我们现有设备直接可用（code=0）。
pub const SINGLE_MODEL_PATH: &str = "/novel/player/video_model/v1/";

/// 分集详情的 biz_param。
pub fn detail_biz_param() -> Value {
    json!({
        "detail_page_version": 0,
        "disable_digg_stat": false,
        "image_shrink_datas_str": "W3siaW1hZ2VfdHlwZSI6MywiaW1hZ2Vfd2lkdGgiOjkwMCwic2hyaW5rX3R5cGUiOjN9LHsiaW1hZ2VfdHlwZSI6NCwiaW1hZ2Vfd2lkdGgiOjcyLCJzaHJpbmtfdHlwZSI6NH1d\n",
        "need_all_video_definition": false,
        "need_mp4_align": false,
        "screen_width_px": "900",
        "source": 7,
        "use_os_player": false,
        "use_server_dns": false,
    })
}

/// 清晰度列表的 biz_param（2026-10-08 抓包逐值对齐，含
/// disable_video_relate_book）。
pub fn model_biz_param() -> Value {
    json!({
        "detail_page_version": 0,
        "device_level": 3,
        "disable_digg_stat": false,
        "disable_video_relate_book": false,
        "need_all_video_definition": true,
        "need_mp4_align": false,
        "use_os_player": false,
        "use_server_dns": false,
        "video_platform": 1024,
    })
}

/// 构造分集详情请求体。
pub fn detail_payload(series_id: &str) -> Value {
    json!({
        "biz_param": detail_biz_param(),
        "dr_scene": "preload",
        "series_id": series_id,
    })
}

/// 构造取流地址请求体（2026-10-08 抓包：dr_scene 与端点同名 "player"）。
///
/// 平台用 `mixed_video_id_map` 传 vid，key 是「清晰度分组 id」，现版用 `"1004"`。
/// hgplayer 的批量形态一次带「当前 + 信息流后续几部」共 ~5 个 vid 做预取。
pub fn model_payload(vids: &[String]) -> Value {
    json!({
        "biz_param": model_biz_param(),
        "dr_scene": "player",
        "mixed_video_id_map": { "1004": vids },
    })
}

/// 构造单集取流请求体（2026-10-14 hgplayer 抓包逐值对齐）：
/// `video_platform:3`（批量是 1024）、`from_video_id:""` 在场、
/// `content_type:1004` = 清晰度分组 id、`video_id` 单集。
pub fn single_model_payload(vid: &str) -> Value {
    json!({
        "biz_param": {
            "detail_page_version": 0,
            "device_level": 3,
            "disable_digg_stat": false,
            "disable_video_relate_book": false,
            "from_video_id": "",
            "need_all_video_definition": true,
            "need_mp4_align": false,
            "use_os_player": false,
            "use_server_dns": false,
            "video_platform": 3,
        },
        "content_type": 1004,
        "video_id": vid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_have_no_trailing_slash() {
        // 尾部斜杠会被服务端当成不同接口
        assert!(!DETAIL_PATH.ends_with('/'));
        assert!(!MODEL_PATH.ends_with('/'));
        assert!(DETAIL_PATH.contains("/preload/v1"));
        assert!(MODEL_PATH.contains("/player/v1"));
    }

    #[test]
    fn detail_payload_uses_preload_scene() {
        let p = detail_payload("123");
        assert_eq!(p["dr_scene"], "preload");
        assert_eq!(p["series_id"], "123");
        assert!(p["biz_param"]["image_shrink_datas_str"].is_string());
    }

    #[test]
    fn model_payload_wraps_vids_in_mixed_map() {
        let p = model_payload(&["v123".to_string(), "v456".to_string()]);
        assert_eq!(p["mixed_video_id_map"]["1004"][0], "v123");
        assert_eq!(p["mixed_video_id_map"]["1004"][1], "v456");
        // 2026-10-08 抓包：dr_scene 与端点同名 "player"
        assert_eq!(p["dr_scene"], "player");
        assert_eq!(p["biz_param"]["disable_video_relate_book"], false);
    }

    #[test]
    fn model_biz_param_requests_all_definitions() {
        assert_eq!(model_biz_param()["need_all_video_definition"], true);
    }
}
