use sha3::{Digest, Sha3_256};

pub type Hash = [u8; 32];

pub fn sha3_256(data: &[u8]) -> Hash {
    let digest = Sha3_256::digest(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

pub fn triple_sha3_256(data: &[u8]) -> Hash {
    let first = sha3_256(data);
    let second = sha3_256(&first);
    sha3_256(&second)
}

pub fn merkle_root(mut leaves: Vec<Hash>) -> Hash {
    if leaves.is_empty() {
        return sha3_256(&[]);
    }
    while leaves.len() > 1 {
        let mut next = Vec::with_capacity(leaves.len().div_ceil(2));
        for pair in leaves.chunks(2) {
            let right = if pair.len() == 2 { pair[1] } else { pair[0] };
            let mut data = Vec::with_capacity(64);
            data.extend_from_slice(&pair[0]);
            data.extend_from_slice(&right);
            next.push(sha3_256(&data));
        }
        leaves = next;
    }
    leaves[0]
}

pub fn hex_hash(hash: &Hash) -> String {
    hex::encode(hash)
}

pub fn decode_hash(s: &str) -> anyhow::Result<Hash> {
    let bytes = hex::decode(s.trim_start_matches("0x"))?;
    anyhow::ensure!(bytes.len() == 32, "hash must be 32 bytes");
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}
