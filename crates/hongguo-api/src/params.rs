//! 官方 App 接口的请求体（静态类型承载）。
//!
//! ⚠️ 这些字段是**照抄现版**（`hongguo.js` / hgplayer）的实测值，改任何一个
//! 都会让服务端返回 `Code: 110001`（签名对但参数错）或 200+0。验证时开
//! `RUST_LOG=debug`：响应字节数 > 0 只说明签名过了，还要看 body 里 `code`
//! 是不是 0。
//!
//! 字段名一律 `#[serde(rename = "…")]` 显式声明服务端字面量（逐值对齐
//! 抓包，不用 rename_all——服务端字段并非统一命名风格）。

use std::collections::BTreeMap;

use serde::Serialize;

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
    /// 短剧（整部剧；追更的 do_action 对象，2026-10-15 抓 player v1.2.1 实包）。
    pub const OBJECT_TYPE_SERIES: i64 = 10;

    /// `action_type`（articleapi do_action 动作码）：点赞 3/4 之外的动作。
    /// 追更 34 / 取消追更 35（2026-10-15 抓 player 实包往返三次锁定）。
    pub const ACTION_TYPE_FOLLOW: i64 = 34;
    pub const ACTION_TYPE_UNFOLLOW: i64 = 35;

    /// 追更列表频道（get_user_action `action_channel`，抓包锁定）。
    pub const ACTION_CHANNEL_FOLLOW: i64 = 9;

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

// ---------------------------------------------------------------- 播放域路径

/// 分集详情接口路径。
pub const DETAIL_PATH: &str = "/novel/player/multi_video_detail/preload/v1";

/// 清晰度列表（取流地址）批量接口路径。
///
/// ⚠️ 必须打 **lq 域**（`crate::client::LQ_API_ORIGIN`）。2026-10-14
/// 抓包实锤：批量形态对**全新注册的真机设备**也回 200+0（hgplayer 自己
/// 4 连空），真实播放走的是单集端点 [`SINGLE_MODEL_PATH`]（hgplayer：
/// 批量空 → 回落单集 → CDN 直链）。
pub const MODEL_PATH: &str = "/novel/player/multi_video_model/player/v1";

/// 单集取流端点（hgplayer 现役路径），2026-10-14 实测现有设备直接可用
/// （code=0）。
pub const SINGLE_MODEL_PATH: &str = "/novel/player/video_model/v1/";

/// 详情页头部元信息端点（2026-10-07 抓 hgplayer 详情页锁定）。
pub const VIDEO_DETAIL_PATH: &str = "/novel/player/video_detail/v1/";

// ---------------------------------------------------------------- 详情 biz_param

/// 分集详情 biz_param（字段名 = 服务端字面量）。
#[derive(Debug, Clone, Serialize)]
pub struct DetailBizParam {
    #[serde(rename = "detail_page_version")]
    pub detail_page_version: i64,
    #[serde(rename = "disable_digg_stat")]
    pub disable_digg_stat: bool,
    #[serde(rename = "image_shrink_datas_str")]
    pub image_shrink_datas_str: String,
    #[serde(rename = "need_all_video_definition")]
    pub need_all_video_definition: bool,
    #[serde(rename = "need_mp4_align")]
    pub need_mp4_align: bool,
    /// ⚠️ 抓包形态是**字符串**（"900"），不是数字。
    #[serde(rename = "screen_width_px")]
    pub screen_width_px: String,
    #[serde(rename = "source")]
    pub source: i64,
    #[serde(rename = "use_os_player")]
    pub use_os_player: bool,
    #[serde(rename = "use_server_dns")]
    pub use_server_dns: bool,
}

/// 分集详情请求体。
#[derive(Debug, Clone, Serialize)]
pub struct DetailPayload {
    #[serde(rename = "biz_param")]
    pub biz_param: DetailBizParam,
    #[serde(rename = "dr_scene")]
    pub dr_scene: &'static str,
    #[serde(rename = "series_id")]
    pub series_id: String,
}

/// 构造分集详情请求体。
pub fn detail_payload(series_id: &str) -> DetailPayload {
    DetailPayload {
        biz_param: DetailBizParam {
            detail_page_version: 0,
            disable_digg_stat: false,
            image_shrink_datas_str: "W3siaW1hZ2VfdHlwZSI6MywiaW1hZ2Vfd2lkdGgiOjkwMCwic2hyaW5rX3R5cGUiOjN9LHsiaW1hZ2VfdHlwZSI6NCwiaW1hZ2Vfd2lkdGgiOjcyLCJzaHJpbmtfdHlwZSI6NH1d\n".to_string(),
            need_all_video_definition: false,
            need_mp4_align: false,
            screen_width_px: "900".to_string(),
            source: 7,
            use_os_player: false,
            use_server_dns: false,
        },
        dr_scene: "preload",
        series_id: series_id.to_string(),
    }
}

// ---------------------------------------------------------------- 取流 biz_param

/// 取流 biz_param 的公共字段（批量/单集只差 `video_platform` 与
/// `from_video_id` 是否在场，见各自 payload）。
#[derive(Debug, Clone, Serialize)]
pub struct ModelBizParam {
    #[serde(rename = "detail_page_version")]
    pub detail_page_version: i64,
    #[serde(rename = "device_level")]
    pub device_level: i64,
    #[serde(rename = "disable_digg_stat")]
    pub disable_digg_stat: bool,
    #[serde(rename = "disable_video_relate_book")]
    pub disable_video_relate_book: bool,
    /// 仅单集形态在场（2026-10-14 hgplayer 抓包）。
    #[serde(rename = "from_video_id")]
    pub from_video_id: String,
    #[serde(rename = "need_all_video_definition")]
    pub need_all_video_definition: bool,
    #[serde(rename = "need_mp4_align")]
    pub need_mp4_align: bool,
    #[serde(rename = "use_os_player")]
    pub use_os_player: bool,
    #[serde(rename = "use_server_dns")]
    pub use_server_dns: bool,
    /// 批量 1024 / 单集 3（抓包实锤）。
    #[serde(rename = "video_platform")]
    pub video_platform: i64,
}

/// 批量取流请求体（`dr_scene` 与端点同名 "player"）。
///
/// `mixed_video_id_map` 的 key 是「清晰度分组 id」，现版用 `"1004"`；
/// 值是「当前 + 信息流后续几部」的 vid 列表（hgplayer 一次 ~5 个）。
#[derive(Debug, Clone, Serialize)]
pub struct ModelPayload {
    #[serde(rename = "biz_param")]
    pub biz_param: ModelBizParam,
    #[serde(rename = "dr_scene")]
    pub dr_scene: &'static str,
    #[serde(rename = "mixed_video_id_map")]
    pub mixed_video_id_map: BTreeMap<String, Vec<String>>,
}

/// 构造批量取流请求体。
pub fn model_payload(vids: &[String]) -> ModelPayload {
    ModelPayload {
        biz_param: ModelBizParam {
            detail_page_version: 0,
            device_level: 3,
            disable_digg_stat: false,
            disable_video_relate_book: false,
            from_video_id: String::new(),
            need_all_video_definition: true,
            need_mp4_align: false,
            use_os_player: false,
            use_server_dns: false,
            video_platform: 1024,
        },
        dr_scene: "player",
        mixed_video_id_map: BTreeMap::from([("1004".to_string(), vids.to_vec())]),
    }
}

/// 单集取流请求体（2026-10-14 hgplayer 抓包逐值对齐）：
/// `video_platform:3`（批量是 1024）、`from_video_id:""` 在场、
/// `content_type:1004` = 清晰度分组 id、`video_id` 单集。
#[derive(Debug, Clone, Serialize)]
pub struct SingleModelPayload {
    #[serde(rename = "biz_param")]
    pub biz_param: ModelBizParam,
    #[serde(rename = "content_type")]
    pub content_type: i64,
    #[serde(rename = "video_id")]
    pub video_id: String,
}

/// 构造单集取流请求体。
pub fn single_model_payload(vid: &str) -> SingleModelPayload {
    SingleModelPayload {
        biz_param: ModelBizParam {
            detail_page_version: 0,
            device_level: 3,
            disable_digg_stat: false,
            disable_video_relate_book: false,
            from_video_id: String::new(),
            need_all_video_definition: true,
            need_mp4_align: false,
            use_os_player: false,
            use_server_dns: false,
            video_platform: 3,
        },
        content_type: 1004,
        video_id: vid.to_string(),
    }
}

// ---------------------------------------------------------------- 详情页头部 meta

/// 详情页头部元信息 biz_param（2026-10-07 抓包逐值对齐）。
#[derive(Debug, Clone, Serialize)]
pub struct VideoDetailBizParam {
    #[serde(rename = "caller_scene")]
    pub caller_scene: &'static str,
    #[serde(rename = "detail_page_version")]
    pub detail_page_version: i64,
    #[serde(rename = "disable_digg_stat")]
    pub disable_digg_stat: bool,
    #[serde(rename = "disable_video_relate_book")]
    pub disable_video_relate_book: bool,
    #[serde(rename = "from_video_id")]
    pub from_video_id: String,
    #[serde(rename = "need_all_video_definition")]
    pub need_all_video_definition: bool,
    #[serde(rename = "need_mp4_align")]
    pub need_mp4_align: bool,
    /// ⚠️ 字符串形态（同 [`DetailBizParam::screen_width_px`]）。
    #[serde(rename = "screen_width_px")]
    pub screen_width_px: String,
    #[serde(rename = "source")]
    pub source: i64,
    #[serde(rename = "use_os_player")]
    pub use_os_player: bool,
    #[serde(rename = "use_server_dns")]
    pub use_server_dns: bool,
    #[serde(rename = "video_id_type")]
    pub video_id_type: i64,
}

/// 详情页头部元信息请求体。
#[derive(Debug, Clone, Serialize)]
pub struct VideoDetailPayload {
    #[serde(rename = "biz_param")]
    pub biz_param: VideoDetailBizParam,
    #[serde(rename = "series_id")]
    pub series_id: String,
}

/// 构造详情页头部元信息请求体。
pub fn video_detail_payload(series_id: &str) -> VideoDetailPayload {
    VideoDetailPayload {
        biz_param: VideoDetailBizParam {
            caller_scene: "single_col",
            detail_page_version: 1,
            disable_digg_stat: false,
            disable_video_relate_book: false,
            from_video_id: String::new(),
            need_all_video_definition: false,
            need_mp4_align: false,
            screen_width_px: "1078".to_string(),
            source: 4,
            use_os_player: false,
            use_server_dns: false,
            video_id_type: 1,
        },
        series_id: series_id.to_string(),
    }
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
        assert_eq!(p.dr_scene, "preload");
        assert_eq!(p.series_id, "123");
        assert!(p.biz_param.detail_page_version == 0);
    }

    #[test]
    fn model_payload_wraps_vids_in_mixed_map() {
        let p = model_payload(&["v123".to_string(), "v456".to_string()]);
        let vids = p.mixed_video_id_map.get("1004").expect("分组 1004 在场");
        assert_eq!(vids, &["v123".to_string(), "v456".to_string()]);
        // 2026-10-08 抓包：dr_scene 与端点同名 "player"
        assert_eq!(p.dr_scene, "player");
        assert!(!p.biz_param.disable_video_relate_book);
        assert_eq!(p.biz_param.video_platform, 1024);
    }

    #[test]
    fn single_model_payload_matches_capture() {
        let p = single_model_payload("v1");
        assert_eq!(p.content_type, 1004);
        assert_eq!(p.video_id, "v1");
        // 单集 video_platform=3、from_video_id 空串在场（hgplayer 抓包形态）
        assert_eq!(p.biz_param.video_platform, 3);
        assert_eq!(p.biz_param.from_video_id, "");
    }

    #[test]
    fn payloads_serialize_with_server_field_names() {
        let p = serde_json::to_value(single_model_payload("v1")).unwrap();
        assert!(p.get("biz_param").is_some());
        assert!(p.get("content_type").is_some());
        assert!(p["biz_param"].get("need_all_video_definition").is_some());
        assert!(p["biz_param"].get("disable_video_relate_book").is_some());
        let d = serde_json::to_value(video_detail_payload("s1")).unwrap();
        assert_eq!(d["series_id"], "s1");
        assert_eq!(d["biz_param"]["video_id_type"], 1);
    }
}
