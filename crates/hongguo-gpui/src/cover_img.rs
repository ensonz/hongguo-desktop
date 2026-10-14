//! 封面字节获取：HEIC 签名 URL → cover 管线解码前字节。全屏占位
//! （home）与详情页封面共用的取数口——管线归属 `hongguo_protocol::cover`，
//! 这里只是「投进阻塞池」的统一形态。

use tokio::sync::oneshot;

use crate::bridge;

/// 取一张封面字节（同步管线投阻塞池；oneshot 回传给 GPUI 执行器 await）。
/// 错误侧沿用管线的 String（调用方只记日志）。
pub(crate) fn fetch(remote: String) -> oneshot::Receiver<Result<Vec<u8>, String>> {
    bridge::spawn(async move {
        tokio::task::spawn_blocking(move || {
            hongguo_protocol::cover::acquire_renderable(&remote).map(|(bytes, _)| bytes)
        })
        .await
        .expect("阻塞池 join")
    })
}
