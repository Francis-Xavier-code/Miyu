//! agent 层的测试，按被测主题分文件。
//!
//! 原本是一个三千多行的 `mod tests`。分组按测试实际在测什么，不按代码位置。

mod shared;
mod reasoning;
mod artifacts;
mod prompt;
mod context;
mod vision;
mod stream;
mod input;
mod queue_journal;
