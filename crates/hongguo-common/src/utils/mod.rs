//! 无业务语义的纯工具层。
//!
//! 准入标准：不依赖 AppState/领域类型、纯函数、可单测。各域重复实现
//! 自这里收敛——同一份逻辑只在本层存一份，域内旧副本一律删除。

pub mod hex;
pub mod json;
pub mod time;
pub mod url;
