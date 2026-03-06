//! CLI 命令模块
//!
//! 包含所有 nanobot CLI 命令的实现。

mod agent;
mod auth;
mod channels;
mod cron;
mod gateway;
mod onboard;
pub mod registry;
mod search;
mod status;

pub use agent::cmd_agent;
pub use auth::cmd_auth_copilot;
pub use channels::cmd_channels_status;
pub use cron::{
    cmd_cron_add, cmd_cron_disable, cmd_cron_enable, cmd_cron_list, cmd_cron_remove, cmd_cron_show,
};
pub use gateway::cmd_gateway;
pub use onboard::cmd_onboard;
pub use search::{cmd_search_rebuild, cmd_search_status, cmd_search_update};
pub use status::{cmd_auth_status, cmd_status};
