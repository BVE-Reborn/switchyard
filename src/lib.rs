//! Real time compute focused async executor.

#![deny(future_incompatible)]
#![deny(nonstandard_style)]
#![deny(rust_2018_idioms)]
// Rustdoc Warnings
#![deny(intra_doc_link_resolution_failure)]

use crate::{
    task::{Job, Task, ThreadLocalJob, ThreadLocalTask},
    threads::ThreadAllocationOutput,
    util::ThreadLocalPointer,
};
use arrayvec::ArrayVec;
use futures_intrusive::{
    channel::shared::{oneshot_channel, ChannelReceiveFuture, OneshotReceiver},
    sync::ManualResetEvent,
};
use futures_task::{Context, Poll};
use parking_lot::{Condvar, Mutex, RawMutex};
use priority_queue::PriorityQueue;
use slotmap::{DefaultKey, DenseSlotMap};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

mod error;
mod task;
pub mod threads;
mod util;
mod worker;

pub use error::*;

pub const MAX_POOLS: u8 = 8;

pub type Priority = u32;
pub type Pool = u8;
pub type PoolCount = u8;

pub struct JoinHandle<T: 'static> {
    _receiver: OneshotReceiver<T>,
    receiver_future: ChannelReceiveFuture<RawMutex, T>,
}
impl<T: 'static> Future for JoinHandle<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        let fut = unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().receiver_future) };
        fut.poll(ctx)
    }
}

struct ThreadLocalQueue<TD> {
    waiting: Mutex<DenseSlotMap<DefaultKey, Arc<ThreadLocalTask<TD>>>>,
    inner: Mutex<PriorityQueue<ThreadLocalJob<TD>, u32>>,
}
struct Queue<TD> {
    waiting: Mutex<DenseSlotMap<DefaultKey, Arc<Task<TD>>>>,
    inner: Mutex<PriorityQueue<Job<TD>, u32>>,
    cond_var: Condvar,
}
type Queues<TD> = ArrayVec<[Queue<TD>; MAX_POOLS as usize]>;

struct Shared<TD> {
    active_threads: AtomicUsize,
    idle_wait: ManualResetEvent,
    job_count: AtomicUsize,
    death_signal: AtomicBool,
    queues: Queues<TD>,
}

pub struct Switchyard<TD: 'static> {
    shared: Arc<Shared<TD>>,
    threads: Vec<std::thread::JoinHandle<()>>,
    thread_local_data: Vec<*mut Arc<TD>>,
}
impl<TD: 'static> Switchyard<TD> {
    /// Create a new switchyard.
    ///
    /// Will create `pool_count` job pools.
    ///
    /// For each element in the provided `thread_allocations` iterator, the yard will spawn a worker
    /// thread with the given settings. Helper functions in [`threads`] can generate these iterators
    /// for common situations.
    ///
    /// `thread_local_data_creation` will be called on each thread to create the thread local
    /// data accessible by `spawn_local`.
    pub fn new<TDFunc>(
        pool_count: Pool,
        thread_allocations: impl IntoIterator<Item = ThreadAllocationOutput>,
        thread_local_data_creation: TDFunc,
    ) -> Result<Self, SwitchyardCreationError>
    where
        TDFunc: Fn() -> TD + Send + Sync + 'static,
    {
        if pool_count >= MAX_POOLS {
            return Err(SwitchyardCreationError::TooManyPools {
                pools_requested: pool_count,
            });
        }

        let (thread_local_sender, thread_local_receiver) = std::sync::mpsc::channel();

        let thread_local_data_creation_arc = Arc::new(thread_local_data_creation);
        let allocation_vec: Vec<_> = thread_allocations.into_iter().collect();

        for (idx, allocation) in allocation_vec.iter().enumerate() {
            if allocation.pool >= pool_count {
                return Err(SwitchyardCreationError::InvalidPoolIndex {
                    thread_idx: idx,
                    pool_idx: allocation.pool,
                    total_pools: pool_count,
                });
            }
        }

        let shared = Arc::new(Shared {
            queues: (0..pool_count)
                .map(|_| Queue {
                    waiting: Mutex::new(DenseSlotMap::new()),
                    inner: Mutex::new(PriorityQueue::new()),
                    cond_var: Condvar::new(),
                })
                .collect::<Queues<TD>>(),
            active_threads: AtomicUsize::new(allocation_vec.len()),
            idle_wait: ManualResetEvent::new(false),
            job_count: AtomicUsize::new(0),
            death_signal: AtomicBool::new(false),
        });

        let mut threads = Vec::with_capacity(allocation_vec.len());
        for mut thread_info in allocation_vec {
            let builder = std::thread::Builder::new();
            let builder = if let Some(name) = thread_info.name.take() {
                builder.name(name)
            } else {
                builder
            };
            threads.push(
                builder
                    .spawn(worker::body::<TD, TDFunc>(
                        Arc::clone(&shared),
                        thread_info,
                        thread_local_sender.clone(),
                        thread_local_data_creation_arc.clone(),
                    ))
                    .unwrap_or_else(|_| panic!("Could not spawn thread")),
            );
        }
        // drop the sender we own, so we can retrieve pointers until all senders are dropped
        drop(thread_local_sender);

        let mut thread_local_data = Vec::with_capacity(threads.len());
        while let Ok(ThreadLocalPointer(ptr)) = thread_local_receiver.recv() {
            thread_local_data.push(ptr);
        }

        Ok(Self {
            threads,
            shared,
            thread_local_data,
        })
    }

    /// Things that must be done every time a task is spawned
    fn spawn_header(&self, pool: Pool) {
        assert!(
            !self.shared.death_signal.load(Ordering::Acquire),
            "finish() has been called on this Switchyard. No more jobs may be added."
        );
        assert!(
            (pool as usize) < self.shared.queues.len(),
            "pool {} refers to a non-existant pool. Total pools: {}",
            pool,
            self.shared.queues.len()
        );

        // SAFETY: we must grab and increment this counter so `access_per_thread_data` knows
        // we're in flight.
        self.shared.job_count.fetch_add(1, Ordering::AcqRel);

        // Say we're no longer idle so that `yard.spawn(); yard.wait_for_idle()`
        // won't "return early". If the thread hasn't woken up fully yet by the
        // time wait_for_idle is called, it will immediately return even though logically there's
        // still an outstanding, active, job.
        self.shared.idle_wait.reset();
    }

    /// Spawn a future which can migrate between threads during execution to the given job `pool` at
    /// the given `priority`.
    ///
    /// A higher `priority` will cause the task to be run sooner.
    ///
    /// # Panics
    ///
    /// - `pool` refers to a non-existent job pool.
    /// - [`finish`](Switchyard::finish) has been called on the pool.
    pub fn spawn<Fut, T>(&self, pool: Pool, priority: Priority, fut: Fut) -> JoinHandle<T>
    where
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_header(pool);

        let (sender, receiver) = oneshot_channel();
        let job = Job::Future(Task::new(
            Arc::clone(&self.shared),
            async move {
                // We don't care about the result, if this fails, that just means the join handle
                // has been dropped.
                let _ = sender.send(fut.await);
            },
            pool,
            priority,
        ));

        let queue: &Queue<TD> = &self.shared.queues[pool as usize];

        let mut queue_guard = queue.inner.lock();
        queue_guard.push(job, priority);
        queue.cond_var.notify_one();
        drop(queue_guard);

        JoinHandle {
            receiver_future: receiver.receive(),
            _receiver: receiver,
        }
    }

    /// Spawns an async function which is tied to a single thread during execution to the given job `pool` at
    /// the given `priority`.
    ///
    /// A higher `priority` will cause the task to be run sooner.
    ///
    /// The given function will be provided an `Rc` to the thread-local data to create its future with.
    ///
    /// The function must be `Send`, but the future returned by that function may be `!Send`.
    ///
    /// # Panics
    ///
    /// - Panics is `pool` refers to a non-existent job pool.
    pub fn spawn_local<Func, Fut, T>(&self, pool: Pool, priority: Priority, async_fn: Func) -> JoinHandle<T>
    where
        Func: FnOnce(Arc<TD>) -> Fut + Send + 'static,
        Fut: Future<Output = T>,
        T: Send + 'static,
    {
        self.spawn_header(pool);

        let (sender, receiver) = oneshot_channel();
        let job = Job::Local(Box::new(move |td| {
            Box::pin(async move {
                // We don't care about the result, if this fails, that just means the join handle
                // has been dropped.
                let _ = sender.send(async_fn(td).await);
            })
        }));

        let queue: &Queue<TD> = &self.shared.queues[pool as usize];

        let mut queue_guard = queue.inner.lock();
        queue_guard.push(job, priority);
        queue.cond_var.notify_one();
        drop(queue_guard);

        JoinHandle {
            receiver_future: receiver.receive(),
            _receiver: receiver,
        }
    }

    /// Wait until all working threads are starved of work due
    /// to lack of jobs or all jobs waiting.
    ///
    /// # Safety
    ///
    /// - This function provides no safety guarantees.
    /// - Jobs may be added while the future returns.
    /// - Jobs may be woken while the future returns.
    pub async fn wait_for_idle(&self) {
        // We don't reset it, threads will reset it when they become active again
        self.shared.idle_wait.wait().await;
    }

    /// Current amount of jobs in flight.
    ///
    /// # Safety
    ///
    /// - This function provides no safety guarantees.
    /// - Jobs may be added after the value is received and before it is returned.
    pub fn jobs(&self) -> usize {
        self.shared.job_count.load(Ordering::Relaxed)
    }

    /// Access the per-thread data of each thread. Only available if `TD` is `Send`.
    ///
    /// # Safety
    ///
    /// - This function guarantees that there exist no other references to this data if `Some` is returned.
    /// - This function guarantees that `jobs()` is 0 and will stay zero while the returned references are still live.  
    pub fn access_per_thread_data(&mut self) -> Option<Vec<&mut TD>>
    where
        TD: Send,
    {
        let threads_live = self.shared.active_threads.load(Ordering::Acquire);

        // SAFETY: No more jobs can be added and threads woken because we have an exclusive reference to the yard.
        if threads_live != 0 {
            return None;
        }

        // SAFETY:
        //  - We know there are no threads running because `count` is zero and we have an exclusive reference to the yard.
        //  - Threads do not keep references to their `Arc`'s around while idle, nor hand them to tasks.
        //  - `TD` is allowed to be `!Sync` because we never actually touch a `&TD`, only `&mut TD`.
        let arcs: Vec<&mut Arc<TD>> = self.thread_local_data.iter().map(|&ptr| unsafe { &mut *ptr }).collect();

        let data: Option<Vec<&mut TD>> = arcs.into_iter().map(|arc| Arc::get_mut(arc)).collect();

        data
    }

    /// Kill all threads as soon as they come idle. All calls to spawn and spawn_local will
    /// panic after this function is called.
    ///
    /// This is equivalent to calling drop. Calling this function twice will be a no-op
    /// the second time.
    pub fn finish(&mut self) {
        // send death signal then wake everyone up
        self.shared.death_signal.store(true, Ordering::Release);
        for queue in &self.shared.queues {
            let lock = queue.inner.lock();
            queue.cond_var.notify_all();
            drop(lock);
        }

        self.thread_local_data.clear();
        for thread in self.threads.drain(..) {
            thread.join().unwrap();
        }
    }
}

impl<TD: 'static> Drop for Switchyard<TD> {
    fn drop(&mut self) {
        self.finish()
    }
}

unsafe impl<TD> Send for Switchyard<TD> {}
unsafe impl<TD> Sync for Switchyard<TD> {}
