pub const MAGIC: u32 = 0x434f_494e;
pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_BLOCK_BODY_BYTES: usize = 16 * 1024 * 1024;
pub const TARGET_BLOCK_SECONDS: u64 = 32;
pub const RETARGET_BLOCKS: u64 = 128;
pub const LIVE_REORG_DEPTH: u64 = 16;
pub const GENESIS_GAS_PRICE: u128 = 1000;
pub const MIN_GAS_PRICE: u128 = 1;
pub const BASE_REWARD: u128 = 4_294_967_296_000;
pub const TAIL_REWARD: u128 = 1000;
pub const HALVING_INTERVAL: u64 = 1_000_000;
pub const HALVINGS: u64 = 32;

pub type Amount = u128;
