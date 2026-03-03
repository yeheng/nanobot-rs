//! External shell hook system — UNIX philosophy for agent extensibility.
//!
//! Instead of internal Rust trait objects, hooks are **external shell scripts**
//! executed via subprocess. Data flows through stdin/stdout as JSON.
//!
//! ## Hook Directory
//!
//! Scripts live in `~/.nanobot/hooks/`:
//! - `pre_request.sh`  — intercept/modify user input before processing
//! - `post_response.sh` — audit/alert after the agent responds
//!
//! ## Data Flow
//!
//! ```text
//! Rust → stdin (JSON) → Shell Script → stdout (JSON) → Rust
//!                         stderr → tracing::debug!
//! ```
//!
//! ## Defensive Execution
//!
//! - **2-second timeout** — scripts that hang are killed
//! - **1 MB stdout cap** — prevents memory exhaustion
//! - **Non-blocking** — uses `tokio::process::Command`

mod external;

pub use external::{ExternalHookInput, ExternalHookOutput, ExternalHookRunner};
