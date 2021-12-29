//! Helper functions for creating iterators of [`ThreadAllocationOutput`] for spawning on the [`Switchyard`](crate::Switchyard).

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
    /// Size of spawned thread's stack. Will use system default if `None`.
    pub stack_size: Option<usize>,
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

/// A single thread configuration.
///
/// - Creates a single thread.
pub fn single_thread(
    thread_name: Option<String>,
    affinity: Option<usize>,
) -> impl Iterator<Item = ThreadAllocationOutput> {
    std::iter::once(ThreadAllocationOutput {
        name: thread_name,
        ident: 0,
        stack_size: None,
        affinity,
    })
}

/// A single thread per core configuration.
///
/// - One thread per logical core.
/// - Each thread assigned to a different core.
///
/// `input` is the result of calling [`thread_info`].
pub fn one_to_one<'a>(
    input: ThreadAllocationInput,
    thread_name: Option<&'a str>,
) -> impl Iterator<Item = ThreadAllocationOutput> + 'a {
    (0..input.logical).map(move |idx| ThreadAllocationOutput {
        name: thread_name.map(ToOwned::to_owned),
        ident: idx,
        stack_size: None,
        affinity: Some(idx),
    })
}

/// A two thread per core, single pool thread configuration.
///
/// - Two threads per logical core.
/// - Each set of two threads assigned to a different core.
///
/// `input` is the result of calling [`thread_info`].
pub fn two_to_one<'a>(
    input: ThreadAllocationInput,
    thread_name: Option<&'a str>,
) -> impl Iterator<Item = ThreadAllocationOutput> + 'a {
    (0..(input.logical * 2)).map(move |idx| ThreadAllocationOutput {
        name: thread_name.map(ToOwned::to_owned),
        ident: idx,
        stack_size: None,
        affinity: Some(idx / 2),
    })
}
