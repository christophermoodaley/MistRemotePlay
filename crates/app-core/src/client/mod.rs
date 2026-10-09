//! Client side: pairing, session runner, renderer window and sinks.

pub mod pairing;
pub mod session;
pub mod shared;
pub mod sinks;
pub mod window;

pub use pairing::pair;
pub use session::SessionOptions;
pub use shared::{ClientCmd, ClientShared};
