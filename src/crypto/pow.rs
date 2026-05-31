use super::Hash;
use ethnum::U256;

pub fn hash_leq_target(hash: &Hash, target: &Hash) -> bool {
    hash <= target
}

pub fn nbits_to_target(nbits: u32) -> Hash {
    if nbits <= 32 {
        let mut target = [0xffu8; 32];
        for b in target.iter_mut().take(nbits as usize) {
            *b = 0;
        }
        return target;
    }

    let exponent = (nbits >> 24) as usize;
    let mantissa = nbits & 0x00ff_ffff;
    let mut target = [0u8; 32];
    let mantissa_bytes = mantissa.to_be_bytes();
    let bytes = &mantissa_bytes[1..4];
    if exponent <= 3 {
        let value = mantissa >> (8 * (3 - exponent));
        target[31] = value as u8;
    } else if exponent <= 32 {
        let start = 32 - exponent;
        target[start..start + 3].copy_from_slice(bytes);
    } else {
        target = [0xff; 32];
    }
    target
}

pub fn target_to_nbits(target: &Hash) -> u32 {
    let first = target.iter().position(|b| *b != 0).unwrap_or(31);
    let exponent = (32 - first) as u32;
    let mut mantissa = [0u8; 4];
    let take = exponent.min(3) as usize;
    mantissa[1..1 + take].copy_from_slice(&target[first..first + take]);
    (exponent << 24) | u32::from_be_bytes(mantissa)
}

pub fn scale_target(target: Hash, actual: u64, expected: u64) -> Hash {
    let actual = actual.max(1) as u128;
    let expected = U256::from(expected.max(1) as u128);
    let actual = U256::from(actual);
    let target = U256::from_be_bytes(target);
    let quotient = target / expected;
    let remainder = target % expected;
    let scaled = quotient
        .checked_mul(actual)
        .unwrap_or(U256::MAX)
        .saturating_add((remainder * actual) / expected);
    scaled.to_be_bytes()
}
