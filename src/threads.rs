//! Helper functions for creating iterators of [`ThreadAllocationOutput`] for spawning on the [`Switchyard`](crate::Switchyard).

use crate::Pool;

/// Information about threads on the system
// TODO: Detect and expose big.LITTLE
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadAllocationInput {
    /// Amount of physical cores in the system.
    pub physical: usize,
    /// Amount of logical cores in the system. Must be greater than `physical`.
    pub logical: usize,
}

/// Spawn information for a worker thread.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadAllocationOutput {
    /// Name.
    pub name: Option<String>,
    /// Identifier.
    pub ident: usize,
    /// Job pool index that the thread services.
    pub pool: Pool,
    /// Core index to pin thread to. If a platform doesn't support affinity, will have
    /// no effect. Currently only windows and linux support affinities.
    pub affinity: Option<usize>,
}

/// System thread information to pass into other functions in this module.
pub fn thread_info() -> ThreadAllocationInput {
    ThreadAllocationInput {
        logical: num_cpus::get(),
        physical: num_cpus::get_physical(),
    }
}

/// A single thread, single pool thread configuration.
///
/// - Creates a single pool of jobs with index 0.
/// - Creates a single thread.
pub fn single_pool_single_thread(
    thread_name: Option<String>,
    affinity: Option<usize>,
) -> impl Iterator<Item = ThreadAllocationOutput> {
    std::iter::once(ThreadAllocationOutput {
        name: thread_name,
        ident: 0,
        pool: 0,
        affinity,
    })
}

/// A single thread per core, single pool thread configuration.
///
/// - Creates a single pool of jobs with index 0.
/// - One thread per logical core.
/// - Each thread assigned to a different core.
///
/// `input` is the result of calling [`thread_info`].
pub fn single_pool_one_to_one<'a>(
    input: ThreadAllocationInput,
    thread_name: Option<&'a str>,
) -> impl Iterator<Item = ThreadAllocationOutput> + 'a {
    (0..input.logical).map(move |idx| ThreadAllocationOutput {
        name: thread_name.map(ToOwned::to_owned),
        ident: idx,
        pool: 0,
        affinity: Some(idx),
    })
}

/// A two thread per core, single pool thread configuration.
///
/// - Creates a single pool of jobs with index 0.
/// - Two threads per logical core.
/// - Each set of two threads assigned to a different core.
///
/// `input` is the result of calling [`thread_info`].
pub fn single_pool_two_to_one<'a>(
    input: ThreadAllocationInput,
    thread_name: Option<&'a str>,
) -> impl Iterator<Item = ThreadAllocationOutput> + 'a {
    (0..(input.logical * 2)).map(move |idx| ThreadAllocationOutput {
        name: thread_name.map(ToOwned::to_owned),
        ident: idx,
        pool: 0,
        affinity: Some(idx / 2),
    })
}

/// A one thread per core, two pool thread configuration.
///
/// - Creates two pools with indices 0 and 1.
/// - One thread per logical core.
/// - Each thread assigned to a different core.
/// - Threads get assigned alternating pools:
///   - Thread 0 -> Pool 0
///   - Thread 1 -> Pool 1
///   - Thread 2 -> Pool 0
///   - Thread 3 -> Pool 1
///
/// `input` is the result of calling [`thread_info`].
pub fn double_pool_one_to_one<'a>(
    input: ThreadAllocationInput,
    thread_name: Option<&'a str>,
) -> impl Iterator<Item = ThreadAllocationOutput> + 'a {
    (0..input.logical).map(move |idx| ThreadAllocationOutput {
        name: thread_name.map(ToOwned::to_owned),
        ident: idx,
        pool: (idx % 2) as u8,
        affinity: Some(idx),
    })
}

/// A two thread per core, two pool thread configuration.
///
/// - Creates two pools with indices 0 and 1.
/// - Two threads per logical core.
/// - Each set of two threads assigned to a different core.
/// - Threads get assigned alternating pools:
///   - Thread 0 (Core 0) -> Pool 0
///   - Thread 1 (Core 0) -> Pool 1
///   - Thread 2 (Core 1) -> Pool 0
///   - Thread 3 (Core 1) -> Pool 1
///
/// `input` is the result of calling [`thread_info`].
pub fn double_pool_two_to_one<'a>(
    input: ThreadAllocationInput,
    thread_name: Option<&'a str>,
) -> impl Iterator<Item = ThreadAllocationOutput> + 'a {
    (0..(input.logical * 2)).map(move |idx| ThreadAllocationOutput {
        name: thread_name.map(ToOwned::to_owned),
        ident: idx,
        pool: (idx % 2) as u8,
        affinity: Some(idx / 2),
    })
}
