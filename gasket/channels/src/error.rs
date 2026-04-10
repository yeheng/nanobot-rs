use thiserror::Error;

/// Channel-specific configuration validation errors
#[derive(Debug, Error)]
pub enum ChannelConfigError {
    #[error("Channel '{0}' has invalid configuration: {1}")]
    InvalidChannelConfig(String, String),
}
