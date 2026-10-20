//! 面向客户的错误分型（规格 §2）。
//!
//! 调用方只需要两件事：`class::class_of(code, cause)` 拿类，`Class::title/explain/action`
//! 拿三句话。**别在别处再造分类表**。

pub mod class;
pub mod recent;

pub use class::{class_of, Cause, Class, ALL};
pub use recent::{Recent, Seen, WINDOW_MS};
