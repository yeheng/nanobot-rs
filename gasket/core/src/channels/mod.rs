//! Channel integrations — re-exports from gasket-channels crate
//!
//! This module re-exports the channel abstractions and implementations
//! from the `gasket-channels` crate, maintaining backward compatibility
//! for all `crate::channels::*` imports within gasket-core.

pub mod base {
    pub use gasket_channels::base::*;
}
pub mod middleware {
    pub use gasket_channels::middleware::*;
}
pub mod outbound {
    pub use gasket_channels::outbound::*;
}

#[cfg(feature = "telegram")]
pub mod telegram {
    pub use gasket_channels::telegram::*;
}
#[cfg(feature = "discord")]
pub mod discord {
    pub use gasket_channels::discord::*;
}
#[cfg(feature = "slack")]
pub mod slack {
    pub use gasket_channels::slack::*;
}
#[cfg(feature = "email")]
pub mod email {
    pub use gasket_channels::email::*;
}
#[cfg(feature = "dingtalk")]
pub mod dingtalk {
    pub use gasket_channels::dingtalk::*;
}
#[cfg(feature = "feishu")]
pub mod feishu {
    pub use gasket_channels::feishu::*;
}
#[cfg(feature = "wecom")]
pub mod wecom {
    pub use gasket_channels::wecom::*;
}
#[cfg(feature = "webhook")]
pub mod websocket {
    pub use gasket_channels::websocket::*;
}

// Convenience re-exports
pub use gasket_channels::{
    log_inbound, Channel, ChannelError, InboundSender, OutboundSender, OutboundSenderRegistry,
    SimpleAuthChecker, SimpleRateLimiter,
};
