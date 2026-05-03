//! Slash-command dispatcher for gasket clients (CLI today, Web tomorrow).
//!
//! This crate intentionally does not depend on `gasket-engine`. Built-in
//! handlers that need engine capabilities receive them through the
//! [`CommandHost`] trait, whose implementation lives in the consuming crate.

pub mod host;
pub mod types;

pub use host::CommandHost;
pub use types::{BuiltinHandler, Command, CommandKind, CommandResult, RouteOutcome};
