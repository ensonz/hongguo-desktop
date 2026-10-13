//! 预览假数据（`screenshot` feature）：合成封面与「字段齐平的 API 模型快照」。
//!
//! 全部离线、确定性（无网络、无时钟、无随机）：封面是按 seed 生成的渐变
//! 位图，模型快照的手填值对齐真实接口的形态（热度文案、档位、收藏数等），
//! 保证预览图和线上数据渲染走的是同一段布局代码。首字头像占位、进度条
//! 00:00 这类「引擎/网络缺席」的形态是预览的已知边界，见 `mod.rs` 头注释。

use std::sync::Arc;

use gpui_kit::RenderImage;
use hongguo_api::danmaku::CommentItem;
use hongguo_api::detail::RelatedItem;
use hongguo_api::rank::RankItem;
use hongguo_common::model::{DownloadTask, TaskStatus, VideoDefinition};

/// 剧目池：(series_id, 剧名, 热度文案)。在播剧取第一条，其余铺满三个
/// tab 源（沉浸流滚轮/切 tab 的数据面）。
pub(crate) const SERIES: [(&str, &str, &str); 6] = [
    ("7100001", "命运契约", "3182万热度"),
    ("7100002", "重生之千金归来", "2751万热度"),
    ("7100003", "闪婚老公是豪门", "2290万热度"),
    ("7100004", "我在长安当神探", "1876万热度"),
    ("7100005", "末日庇护所", "1544万热度"),
    ("7100006", "助理大人请自重", "1208万热度"),
];

/// 合成封面：seed 决定渐变双色，横向条纹位置随 seed 变化——肉眼可分辨
/// 「每行封面各不相同」即可，不追求内容语义。走 [`crate::image_upload::
/// render_image`]（crate 内唯一合法的 `RenderImage` 出口，BGRA 转换在那里）。
pub(crate) fn cover(seed: usize) -> Arc<RenderImage> {
    let (w, h) = (480u32, 270u32);
    // 双色从 seed 推导：低位置出基色，高位色相错开 97（素数，避免相邻
    // seed 撞相近色相）。
    let (r0, g0, b0) = pastel(seed);
    let (r1, g1, b1) = pastel(seed + 97);
    let mut img = image::RgbaImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let t = (x + y) as f32 / (w + h) as f32;
        // 中段叠一条浅色横带（封面构图占位），其余线性渐变
        let band: f32 = if (y as f32 / h as f32 - 0.62).abs() < 0.05 {
            0.25
        } else {
            0.
        };
        let mix = |a: u8, b: u8| (a as f32 * (1. - t) + b as f32 * t + band * 64.) as u8;
        *px = image::Rgba([mix(r0, r1), mix(g0, g1), mix(b0, b1), 255]);
    }
    Arc::new(crate::image_upload::render_image(img))
}

/// seed → 柔和 RGB 三元组（低饱和高明度，浅深主题都不刺眼；47° 素数
/// 步进让相邻 seed 色相拉开，肉眼可分辨每张封面不同）。
fn pastel(seed: usize) -> (u8, u8, u8) {
    let hue = ((seed * 47) % 360) as f32;
    let (r, g, b) = hsv(hue, 0.35, 0.85);
    (r, g, b)
}

/// HSV → RGB（无外部依赖的最小实现，只服务假封面）。
fn hsv(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1. - ((h / 60.).rem_euclid(2.)) - 1.).abs();
    let (r, g, b) = match h as u32 / 60 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    let m = v - c;
    (
        ((r + m) * 255.) as u8,
        ((g + m) * 255.) as u8,
        ((b + m) * 255.) as u8,
    )
}

/// 角标/季标签池（与 [`SERIES`] 同下标配对：有新剧/高分/今日更新三类
/// 角标，季标签集数各不相同，验收叠层排版）。
pub(crate) fn stream_tags(index: usize) -> (&'static str, &'static str) {
    const POOLS: [(&str, &str); SERIES.len()] = [
        ("", "全80集"),
        ("新剧上线", "全65集"),
        ("", "全52集"),
        ("高分", "全40集"),
        ("", "全36集"),
        ("今日更新", "全48集"),
    ];
    POOLS[index % POOLS.len()]
}

/// 清晰度菜单快照（1080/720/540 三档，与真实档案下发同形态）。
pub(crate) fn definitions() -> Vec<VideoDefinition> {
    [(1080, 1920, 1080), (720, 1280, 720), (540, 960, 540)]
        .into_iter()
        .map(|(value, width, height)| VideoDefinition {
            value,
            width,
            height,
        })
        .collect()
}

/// 评论快照：覆盖首字头像占位（avatar 空）、点赞红心（user_digg）、
/// 剧评评分（score 非空）、多回复四类形态。
pub(crate) fn comments() -> Vec<CommentItem> {
    let base = 1_791_000_000_000_i64; // 展示用固定时刻（相对时间文案的输入）
    vec![
        CommentItem {
            comment_id: "c1".into(),
            user_id: "1001".into(),
            user_name: "追剧到三点的猫".into(),
            text: "这集反转太狠了，编剧是懂悬念的".into(),
            create_time: base,
            digg_count: 1024,
            reply_count: 36,
            user_digg: true,
            ..Default::default()
        },
        CommentItem {
            comment_id: "c2".into(),
            user_id: "1002".into(),
            user_name: "老张".into(),
            text: "男主这个发型真的很出戏，但是剧情可以".into(),
            create_time: base - 3_600_000,
            digg_count: 233,
            reply_count: 5,
            ..Default::default()
        },
        CommentItem {
            comment_id: "c3".into(),
            user_id: "1003".into(),
            user_name: "剧评人小王".into(),
            text: "节奏紧凑不注水，权当近年短剧标杆了。".into(),
            create_time: base - 86_400_000,
            digg_count: 87,
            score: "8.5".into(),
            score_suffix_text: "观看1小时后点评".into(),
            ..Default::default()
        },
        CommentItem {
            comment_id: "c4".into(),
            user_id: "1004".into(),
            user_name: "路过的甲乙丙".into(),
            text: "蹲一个第二季".into(),
            create_time: base - 172_800_000,
            digg_count: 12,
            reply_count: 2,
            ..Default::default()
        },
    ]
}

/// 下载任务快照：Running/Pending/Completed/Failed 四态各一（任务行的
/// 状态徽标、进度、操作按钮全被覆盖）。
pub(crate) fn download_tasks() -> Vec<DownloadTask> {
    let now = 1_791_000_000_i64;
    [
        (
            "t1",
            "命运契约",
            3,
            "第3集 惊天反转",
            TaskStatus::Running,
            486_000_000,
            1_073_741_824,
        ),
        (
            "t2",
            "命运契约",
            4,
            "第4集 真相浮出",
            TaskStatus::Pending,
            0,
            1_073_741_824,
        ),
        (
            "t3",
            "重生之千金归来",
            1,
            "第1集",
            TaskStatus::Completed,
            966_367_642,
            966_367_642,
        ),
        (
            "t4",
            "我在长安当神探",
            2,
            "第2集",
            TaskStatus::Failed,
            104_857_600,
            1_073_741_824,
        ),
    ]
    .into_iter()
    .map(
        |(id, title, vid_index, ep_title, status, downloaded, total)| DownloadTask {
            id: id.into(),
            series_id: "7100001".into(),
            series_title: title.into(),
            vid_index,
            vid: format!("vid-{vid_index}"),
            ep_title: ep_title.into(),
            // 完成态给成品路径（选集浮层的「已下载」标记按它判）
            file_path: if matches!(status, TaskStatus::Completed) {
                "/downloads/命运契约/第1集.mp4".into()
            } else {
                String::new()
            },
            temp_path: String::new(),
            status,
            downloaded,
            total,
            error: if matches!(status, TaskStatus::Failed) {
                "流地址过期（示例错误）".into()
            } else {
                String::new()
            },
            created_at: now,
            updated_at: now,
        },
    )
    .collect()
}

/// 系列剧季卡片快照：三季 + 一部未上线（「即将上线」预约分支）。
pub(crate) fn related_seasons() -> Vec<RelatedItem> {
    vec![
        RelatedItem {
            series_id: "7100001".into(),
            title: "命运契约 第一季".into(),
            tag: "第1季".into(),
            score: 8.2,
            play_cnt: 5_200_000,
            episode_cnt: 80,
            video_desc: "契约游戏的开始".into(),
            ..Default::default()
        },
        RelatedItem {
            series_id: "7100102".into(),
            title: "命运契约 第二季".into(),
            tag: "第2季".into(),
            score: 8.6,
            play_cnt: 3_100_000,
            episode_cnt: 65,
            video_desc: "终局之前的暗流".into(),
            ..Default::default()
        },
        RelatedItem {
            series_id: "7100103".into(),
            title: "命运契约 最终季".into(),
            tag: "第3季".into(),
            episode_cnt: 0,
            video_desc: "即将上线".into(),
            ..Default::default()
        },
    ]
}

/// 排行榜快照：前三甲 + 常规行 + 未上线行（预约按钮分支）。
pub(crate) fn rank_items() -> Vec<RankItem> {
    let rows: [(&str, &str, &str, f64, i64, u32, bool); 6] = [
        (
            "7100001",
            "命运契约",
            "都市·全80集",
            9.1,
            137_070_000,
            80,
            false,
        ),
        (
            "7100002",
            "重生之千金归来",
            "逆袭·全65集",
            8.7,
            98_450_000,
            65,
            false,
        ),
        (
            "7100003",
            "闪婚老公是豪门",
            "甜宠·全52集",
            8.3,
            76_210_000,
            52,
            false,
        ),
        (
            "7100004",
            "我在长安当神探",
            "古装·全40集",
            8.0,
            51_006_000,
            40,
            false,
        ),
        (
            "7100005",
            "末日庇护所",
            "科幻·全36集",
            7.8,
            33_890_000,
            36,
            false,
        ),
        (
            "7100006",
            "助理大人请自重",
            "职场·全48集",
            7.5,
            21_450_000,
            48,
            true,
        ),
    ];
    rows.into_iter()
        .enumerate()
        .map(
            |(i, (series_id, title, sub_title, score, play_cnt, episode_cnt, upcoming))| RankItem {
                series_id: series_id.into(),
                title: title.into(),
                rank: (i + 1) as u32,
                sub_title: sub_title.into(),
                score,
                play_cnt,
                episode_cnt,
                rec_text: format!("{}万最高热度", play_cnt / 10_000),
                secondary_infos: vec![format!("{}万收藏", play_cnt / 100_000)],
                upcoming,
                ..Default::default()
            },
        )
        .collect()
}
