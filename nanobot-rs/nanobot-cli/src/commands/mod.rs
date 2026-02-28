//! CLI 命令模块
//!
//! 包含所有 nanobot CLI 命令的实现。

mod agent;
mod auth;
mod channels;
mod gateway;
mod onboard;
mod status;

pub use agent::cmd_agent;
pub use auth::cmd_auth_copilot;
pub use channels::cmd_channels_status;
pub use gateway::cmd_gateway;
pub use onboard::cmd_onboard;
pub use status::{cmd_auth_status, cmd_status};
