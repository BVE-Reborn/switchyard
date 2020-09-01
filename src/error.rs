use crate::MAX_POOLS;
use std::{error::Error, fmt};

#[derive(Debug)]
pub enum SwitchyardCreationError {
    InvalidPoolIndex {
        thread_idx: usize,
        pool_idx: u8,
        total_pools: u8,
    },
    TooManyPools {
        pools_requested: u8,
    },
}

impl fmt::Display for SwitchyardCreationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidPoolIndex {
                thread_idx,
                pool_idx,
                total_pools,
            } => write!(
                f,
                "Thread index {} requested job pool {} but there are only {} pools",
                thread_idx, pool_idx, total_pools
            ),
            Self::TooManyPools { pools_requested } => write!(
                f,
                "Requested {} pools which is greater than the max {}",
                pools_requested, MAX_POOLS
            ),
        }
    }
}

impl Error for SwitchyardCreationError {}
