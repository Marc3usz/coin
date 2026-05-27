use crate::crypto::{Address, Hash};

pub(super) const KEY_HEAD: &[u8] = b"meta:head";
pub(super) const KEY_HEIGHT: &[u8] = b"meta:height";

pub(super) fn account_key(address: &Address) -> Vec<u8> {
    prefixed_key(b"acct:", address)
}

pub(super) fn code_key(hash: &Hash) -> Vec<u8> {
    prefixed_key(b"code:", hash)
}

pub(super) fn block_hash_key(hash: &Hash) -> Vec<u8> {
    prefixed_key(b"block:", hash)
}

pub(super) fn receipt_key(hash: &Hash) -> Vec<u8> {
    prefixed_key(b"receipt:", hash)
}

pub(super) fn diff_key(hash: &Hash) -> Vec<u8> {
    prefixed_key(b"diff:", hash)
}

pub(super) fn mempool_key(hash: &Hash) -> Vec<u8> {
    prefixed_key(b"mempool:", hash)
}

pub(super) fn vm_state_key(address: &Address, field_idx: u8) -> Vec<u8> {
    let mut k = Vec::from(&b"vm:"[..]);
    k.extend_from_slice(address);
    k.push(field_idx);
    k
}

pub(super) fn height_key(height: u64) -> Vec<u8> {
    [b"height:".as_slice(), &height.to_be_bytes()].concat()
}

pub(super) fn decode_32(bytes: &[u8]) -> anyhow::Result<Hash> {
    anyhow::ensure!(bytes.len() == 32, "expected 32 bytes");
    let mut out = [0; 32];
    out.copy_from_slice(bytes);
    Ok(out)
}

pub(super) fn decode_u64(bytes: &[u8]) -> anyhow::Result<u64> {
    anyhow::ensure!(bytes.len() == 8, "expected u64");
    Ok(u64::from_be_bytes(bytes.try_into().unwrap()))
}

fn prefixed_key(prefix: &[u8], value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(prefix.len() + value.len());
    out.extend_from_slice(prefix);
    out.extend_from_slice(value);
    out
}
