//! Audit logging for sandbox operations
//!
//! Provides comprehensive audit logging for all sandbox operations,
//! including command execution, permission changes, and security events.

mod event;
mod log;

pub use event::{AuditEvent, AuditEventType};
pub use log::AuditLog;
