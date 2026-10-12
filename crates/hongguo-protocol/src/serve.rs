//! 三条协议路径的供数入口。
//!
//! 只产出 [`hongguo_stream::range::ProtocolResponse`] 元组（状态码、响应头、
//! 响应体），不做任何 HTTP 类型组装——那是壳层注册胶水的职责。错误语义：
//! [`respond_local`] / [`respond_stream`] 的 `Err` 由壳层映射为 400/404 文本
//! 响应（沿用原 register.rs 的状态码约定）；[`respond_cover`] 自带 404 兜底
//! （封面失败让 `<img>` 的 onError 落占位图，不留错误体）。

use hongguo_stream::range::ProtocolResponse;

use crate::cover;
use crate::local;
use crate::url::parse_stream_path;

/// 供给本地文件。`raw_path` 是请求 URI 的 path（`/f/<b64>`）。
///
/// wry 把 `http://hongguo-local.localhost/f/<b64>` 转回
/// `hongguo-local://localhost/f/<b64>`，所以这里拿到的是 `/f/<b64>`，
/// 要去掉 `f/` 段才是编码后的路径。
pub fn respond_local(raw_path: &str, range: Option<&str>) -> Result<ProtocolResponse, String> {
    let path = raw_path
        .trim_start_matches('/')
        .strip_prefix("f/")
        .unwrap_or_else(|| raw_path.trim_start_matches('/'));
    local::serve(path, range)
}

/// 在线流请求的路径错误：URL 不是本协议生成的形态（对不上就明确报错，不猜 vid）。
#[derive(Debug)]
pub enum StreamServeError {
    /// path 解析失败 → 壳层回 400
    BadPath(String),
    /// 流数据面拒绝（无此流/未注册）→ 壳层回 404
    Serve(String),
}

/// 供给在线流。`path` 是请求 URI 的 path（`/s/{档位}/{vid}`）。
///
/// wry 转回后的 URI 是 `hongguo-stream://localhost/s/{档位}/{vid}`：host 恒为
/// localhost，档位与 vid 都在 path 上。档位不只是路由信息，它就是缓存的键，
/// 决定这份请求去读哪一份数据。
pub fn respond_stream(
    path: &str,
    range: Option<&str>,
) -> Result<ProtocolResponse, StreamServeError> {
    let Some((definition, vid)) = parse_stream_path(path) else {
        return Err(StreamServeError::BadPath(format!(
            "无法解析的流地址: {path}"
        )));
    };
    hongguo_stream::serve(vid, definition, range).map_err(StreamServeError::Serve)
}

/// 供给封面/头像转码。`path` 是请求 URI 的 path（`/c/<b64>`）。
///
/// 任何失败（含 serve 内部 `Err`）都折叠为空体 404：前端 `<img>` 的
/// onError 会落到占位图。
pub fn respond_cover(path: &str) -> ProtocolResponse {
    match cover::serve(path) {
        Ok(resp) => resp,
        Err(_) => (404, Vec::new(), Vec::new()),
    }
}
