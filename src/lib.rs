//! Real time compute focused async executor.

#![deny(future_incompatible)]
#![deny(nonstandard_style)]
#![deny(rust_2018_idioms)]
// Rustdoc Warnings
#![deny(intra_doc_link_resolution_failure)]

use bitflags::bitflags;
use futures_intrusive::{
    channel::shared::{oneshot_channel, ChannelReceiveFuture},
    sync::ManualResetEvent,
};
use parking_lot::{Mutex, RawMutex, RwLock, RwLockWriteGuard};
use std::{
    collections::VecDeque,
    future::Future,
    ops::Deref,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

pub struct ThreadAllocation<'a> {
    name: ThreadName<'a>,
    style: ThreadAllocationStyle<'a>,
}

pub enum ThreadAllocationStyle<'a> {
    /// All logical threads get a thread, they can execute all jobs
    AllComputeIoOneToOne,
    /// All logical threads gets two threads, they can execute all jobs
    AllComputeIoTwoToOne,
    /// All logical threads get a thread, even cores get compute, odd cores get io
    HalfComputeHalfIoOneToOne,
    /// All logical threads get a thread, all cores get compute, odd cores get io
    AllComputeHalfIoOneToOne,
    /// Custom allocation scheme. Each member in the vector represents one thread.
    Custom(&'a mut dyn FnMut(ThreadAllocationInput) -> Vec<ThreadAllocationOutput>),
}

pub enum ThreadName<'a> {
    /// Name will not be specified.
    Default,
    /// Every thread will have the same name.
    Constant(&'a str),
    /// Custom name for every thread.
    Custom(&'a mut dyn FnMut(&ThreadAllocationOutput) -> String),
}

/// Information about threads on the system
// TODO: Detect and expose big.LITTLE
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ThreadAllocationInput {
    physical: usize,
    logical: usize,
}

/// Spawn information for a worker thread.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ThreadAllocationOutput {
    /// Mask of job priorities thread should execute
    priority_mask: PriorityMask,
    /// Core index to pin thread to
    affinity: Option<usize>,
}

/// Job priority and compute/io split.
#[derive(Debug, Copy, Clone, PartialOrd, PartialEq, Eq, Ord)]
pub enum Priority {
    /// Compute job, low priority.
    Low = 0,
    /// IO job, low priority.
    IoLow = 1,
    /// Compute job, normal priority.
    Normal = 2,
    /// IO job, normal priority.
    IoNormal = 3,
    /// Compute job, high priority.
    High = 4,
    /// IO job, high priority.
    IoHigh = 5,
}

bitflags! {
    /// Priority mask allowing different a thread to execute certain jobs
    pub struct PriorityMask: u8 {
        const NONE = 0b000000;
        const LOW = 0b000001;
        const IO_LOW = 0b000010;
        const NORMAL = 0b000100;
        const IO_NORMAL = 0b001000;
        const HIGH = 0b010000;
        const IO_HIGH = 0b100000;
        const IO = (Self::IO_HIGH.bits | Self::IO_NORMAL.bits | Self::IO_LOW.bits);
        const COMPUTE = (Self::HIGH.bits | Self::NORMAL.bits | Self::LOW.bits);
        const ALL = (Self::IO.bits | Self::COMPUTE.bits);
    }
}

struct Queues<TD> {
    inner: [Mutex<VecDeque<Job<TD>>>; 6],
}

enum Job<TD> {
    Future(Pin<Box<dyn Future<Output = ()> + Send + Sync>>),
    Local(Box<dyn for<'v> FnOnce(&'v TD) -> Pin<Box<dyn Future<Output = ()> + 'v>>>),
}

pub type JoinHandle<T> = ChannelReceiveFuture<RawMutex, T>;

pub struct Runtime<TD> {
    count: RwLock<AtomicUsize>,
    queue: Arc<Queues<TD>>,
    idle_wait: Arc<ManualResetEvent>,
    thread_local_data: Vec<*mut TD>,
}
impl<TD> Runtime<TD> {
    pub fn new(allocation: ThreadAllocation<'_>) -> Self {
        unimplemented!()
    }

    pub fn spawn<Fut, T>(&self, priority: Priority, fut: Fut) -> JoinHandle<T>
    where
        Fut: Future<Output = T> + Send + Sync + 'static,
        T: Send + 'static,
    {
        let (sender, receiver) = oneshot_channel();
        let job = Job::Future(Box::pin(async move {
            sender.send(fut.await);
        }));
        self.queue.inner[priority as usize].lock().push_back(job);
        receiver.receive()
    }

    pub fn spawn_local<Func, Fut, T>(&self, priority: Priority, async_fn: Func) -> JoinHandle<T>
    where
        Func: FnOnce(&TD) -> Fut + Send + 'static,
        Fut: Future<Output = T>,
        T: Send + 'static,
    {
        let (sender, receiver) = oneshot_channel();
        let job = Job::Local(Box::new(move |td| {
            Box::pin(async move {
                sender.send(async_fn(td).await);
            })
        }));
        self.queue.inner[priority as usize].lock().push_back(job);
        receiver.receive()
    }

    pub async fn wait_for_idle(&self) {
        self.idle_wait.wait().await;
        self.idle_wait.reset();
    }

    pub fn queued_jobs(&self) -> usize {
        self.count.read().load(Ordering::Relaxed)
    }

    pub fn access_per_thread_data(&self) -> Option<PerThreadDataGuard<'_, TD>>
    where
        TD: Send,
    {
        let guard = self.count.write();
        let count = guard.load(Ordering::Relaxed);

        // SAFETY: We can check this atomic value normally because we have
        // an exclusive lock on it.
        if count != 0 {
            return None;
        }

        // SAFETY:
        //  - We know there are no jobs running because `count` is zero and we have an exclusive lock on it.
        //  - Threads do not hold any reference to their thread local data unless they are running jobs.
        //  - All threads have yielded waiting for jobs, and no jobs can be added, so this cannot change.
        //  - We are allowed to deref this from another thread as `TD` is `Send`.
        // TODO:
        //  - How can we be sure that threads haven't panicked after decrementing the count and releasing the lock.
        let data: Vec<&mut TD> = self.thread_local_data.iter().map(|&ptr| unsafe { &mut *ptr }).collect();

        Some(PerThreadDataGuard { guard, data })
    }
}

pub struct PerThreadDataGuard<'a, TD> {
    guard: RwLockWriteGuard<'a, AtomicUsize>,
    data: Vec<&'a mut TD>,
}
impl<'a, TD> Deref for PerThreadDataGuard<'a, TD> {
    type Target = [&'a mut TD];

    fn deref(&self) -> &Self::Target {
        self.data.as_ref()
    }
}
