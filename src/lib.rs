pub mod commands;
pub mod daemon;
pub mod model;
pub mod p2p;
pub mod snapshot;
pub mod store;

pub const FORMAT_VERSION: u32 = 1;
pub const ALPN: &[u8] = b"workmesh/1";
pub const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;
pub const MAX_FRAME_SIZE: usize = 96 * 1024 * 1024;
