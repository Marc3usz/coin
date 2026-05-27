use super::{sha3_256, Hash};

pub type Address = [u8; 32];

pub fn address_from_public_key(public_key: &[u8]) -> Address {
    sha3_256(public_key)
}

pub fn address_from_hash(hash: Hash) -> Address {
    hash
}
