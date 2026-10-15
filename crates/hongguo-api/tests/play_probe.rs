//! 取流请求探针（手工跑，#[ignore] 门控）：现行取流链（单集
//! video_model/v1 @ lq 域）直调 fetch_play_url，验证选流出直链。
//! `cargo test -p hongguo-api --test play_probe -- --ignored --nocapture`
//! vid 可用 `HONGGUO_PROBE_VID` 覆盖。

use hongguo_api::play_url::fetch_play_url;
use hongguo_api::stream_pick::DecoderCapability;

#[tokio::test]
#[ignore = "手工探针：打真实接口"]
async fn probe_play_url() {
    let env = hongguo_api::client::ApiEnv::anonymous(Default::default());
    let vid = std::env::var("HONGGUO_PROBE_VID").unwrap_or_else(|_| "7695012864796412952".into());
    match fetch_play_url(&vid, None, DecoderCapability::Full, &env).await {
        Ok(info) => println!(
            "OK 定义={} codec={} encrypted={} url前60={}",
            info.definition,
            info.codec,
            info.encrypted,
            &info.url[..60.min(info.url.len())]
        ),
        Err(e) => println!("ERR {e}"),
    }
}

#[tokio::test]
#[ignore = "手工探针：打真实接口"]
async fn probe_feed_vids() {
    let env = hongguo_api::client::ApiEnv::anonymous(Default::default());
    match hongguo_api::recommend::fetch_recommend_feed("36", "", 0, &[], &env).await {
        Ok(page) => {
            for it in page.items.iter().take(5) {
                println!(
                    "series={} vid_len={} title={}",
                    &it.series_id[..8.min(it.series_id.len())],
                    it.vid.len(),
                    it.title
                );
            }
        }
        Err(e) => println!("ERR {e}"),
    }
}
