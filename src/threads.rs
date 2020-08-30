use crate::Pool;

/// Information about threads on the system
// TODO: Detect and expose big.LITTLE
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadAllocationInput {
    pub physical: usize,
    pub logical: usize,
}

/// Spawn information for a worker thread.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadAllocationOutput {
    /// Name
    pub name: Option<String>,
    /// Identifier.
    pub ident: usize,
    /// Job pool that the thread services.
    pub pool: Pool,
    /// Core index to pin thread to.
    pub affinity: Option<usize>,
}

pub fn thread_info() -> ThreadAllocationInput {
    ThreadAllocationInput {
        logical: num_cpus::get(),
        physical: num_cpus::get_physical(),
    }
}

/// Creates a single pool jobs with index 0.
/// One thread per logical core.
/// Each thread assigned to a different core.
pub fn single_pool_one_to_one(
    input: ThreadAllocationInput,
    thread_name: Option<String>,
) -> impl Iterator<Item = ThreadAllocationOutput> {
    (0..input.logical).map(move |idx| ThreadAllocationOutput {
        name: thread_name.clone(),
        ident: idx,
        pool: 0,
        affinity: Some(idx),
    })
}

/// Creates a single pool jobs with index 0.
/// One thread per logical core.
/// Two threads assigned to each core
pub fn single_pool_two_to_one(
    input: ThreadAllocationInput,
    thread_name: Option<String>,
) -> impl Iterator<Item = ThreadAllocationOutput> {
    (0..(input.logical * 2)).map(move |idx| ThreadAllocationOutput {
        name: thread_name.clone(),
        ident: idx,
        pool: 0,
        affinity: Some(idx / 2),
    })
}

/// Creates two pools with indices 0 and 1.
/// One thread per logical core.
/// Threads get assigned alternating pools:
///
/// Thread 0 -> Pool 0
/// Thread 1 -> Pool 1
/// Thread 2 -> Pool 0
/// Thread 3 -> Pool 1
pub fn double_pool_one_to_one(
    input: ThreadAllocationInput,
    thread_name: Option<String>,
) -> impl Iterator<Item = ThreadAllocationOutput> {
    (0..input.logical).map(move |idx| ThreadAllocationOutput {
        name: thread_name.clone(),
        ident: idx,
        pool: (idx % 2) as u8,
        affinity: Some(idx),
    })
}
