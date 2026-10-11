//! 红果桌面版共用层。
//!
//! 只放「无域归属、跨 crate 共用」的东西：领域模型、统一错误、纯工具、
//! 跨端常量。禁止在此引入业务编排、IO 实现或 UI 依赖——它是所有上层
//! crate 的公共底座（含未来的 GPUI 端），必须保持叶子。
//!
//! 当前不依赖任何其他 hongguo-* crate；若将来出现反向依赖需求，先怀疑
//! 归属是否放错了层。

pub mod consts;
pub mod error;
pub mod model;
pub mod utils;

pub use error::{AppError, AppResult};
