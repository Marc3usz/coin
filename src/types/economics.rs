use super::{Amount, BASE_REWARD, HALVINGS, HALVING_INTERVAL, MIN_GAS_PRICE, TAIL_REWARD};

pub fn block_reward(height: u64) -> Amount {
    let halvings = (height / HALVING_INTERVAL).min(HALVINGS);
    let reward = BASE_REWARD >> halvings;
    reward.max(TAIL_REWARD)
}

pub fn next_gas_price(
    parent_gas_price: Amount,
    parent_gas_used: u64,
    block_gas_limit: u64,
) -> Amount {
    if parent_gas_used == 0 {
        return ((parent_gas_price * 75) / 100).max(MIN_GAS_PRICE);
    }
    if parent_gas_used >= block_gas_limit {
        return ((parent_gas_price * 110) / 100).max(parent_gas_price + 1);
    }
    let used = parent_gas_used as u128;
    let limit = block_gas_limit.max(1) as u128;
    let delta = ((parent_gas_price * (limit - used) * 10) / limit / 100).max(1);
    parent_gas_price.saturating_sub(delta).max(MIN_GAS_PRICE)
}
