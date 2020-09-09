use crate::MAX_POOLS;
use std::{error::Error, fmt};

/// Errors encountered when creating a [`Switchyard`](crate::Switchyard).
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
    InvalidAffinity {
        affinity: usize,
        total_threads: usize,
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
            Self::InvalidAffinity {
                affinity,
                total_threads,
            } => write!(
                f,
                "Requested affinity {} which is greater than total logical core count {}",
                affinity, total_threads
            ),
        }
    }
}

impl Error for SwitchyardCreationError {}
