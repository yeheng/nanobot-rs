//! Webhook HTTP server for receiving callbacks from messaging platforms.
//!
//! Platform-specific webhook routes are now exposed directly by each adapter
//! via the [`ImProvider::routes`](crate::provider::ImProvider::routes) method.
