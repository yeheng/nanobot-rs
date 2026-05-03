//! Slash-command dispatcher for gasket clients (CLI today, Web tomorrow).
//!
//! This crate intentionally does not depend on `gasket-engine`. Built-in
//! handlers that need engine capabilities receive them through the
//! [`CommandHost`] trait, whose implementation lives in the consuming crate.

pub mod dispatcher;
pub mod error;
pub mod host;
pub mod parser;
pub mod template;
pub mod types;
pub mod yaml_loader;

pub use dispatcher::{Dispatcher, DispatcherBuilder};
pub use error::BuildError;
pub use host::CommandHost;
pub use types::{BuiltinHandler, Command, CommandKind, CommandResult, RouteOutcome};
