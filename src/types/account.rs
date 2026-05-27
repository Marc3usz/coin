use crate::crypto::Hash;
use serde::{Deserialize, Serialize};

use super::Amount;

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Account {
    pub balance: Amount,
    pub account_index: u64,
    pub code_hash: Option<Hash>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StateDiff {
    pub key: Vec<u8>,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}
