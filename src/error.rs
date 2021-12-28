use std::{error::Error, fmt};

/// Errors encountered when creating a [`Switchyard`](crate::Switchyard).
#[derive(Debug)]
pub enum SwitchyardCreationError {
    InvalidAffinity { affinity: usize, total_threads: usize },
}

impl fmt::Display for SwitchyardCreationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
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
