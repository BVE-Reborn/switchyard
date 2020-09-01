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
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

mod task;
pub mod threads;
mod util;
mod worker;

const MAX_POOLS: u8 = 8;

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
    inner: Mutex<PriorityQueue<ThreadLocalJob, u32>>,
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
    thread_local_data: Vec<*mut TD>,
}
impl<TD: 'static> Switchyard<TD> {
    pub fn new<TDFunc>(
        max_pools: Pool,
        allocation: impl IntoIterator<Item = ThreadAllocationOutput>,
        data_creation: TDFunc,
    ) -> Self
    where
        TDFunc: Fn() -> TD + Send + Sync + 'static,
    {
        assert!(max_pools < MAX_POOLS);

        let (sender, thread_local_receiver) = std::sync::mpsc::channel();

        let data_creation_arc = Arc::new(data_creation);
        let allocation_vec: Vec<_> = allocation.into_iter().collect();

        let shared = Arc::new(Shared {
            queues: (0..max_pools)
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
                        sender.clone(),
                        data_creation_arc.clone(),
                    ))
                    .unwrap_or_else(|_| panic!("Could not spawn thread")),
            );
        }
        // drop the sender we own, so we can retrieve pointers until all senders are dropped
        drop(sender);

        let mut thread_local_data = Vec::with_capacity(threads.len());
        while let Ok(ThreadLocalPointer(ptr)) = thread_local_receiver.recv() {
            thread_local_data.push(ptr);
        }

        Self {
            threads,
            shared,
            thread_local_data,
        }
    }

    /// Things that must be done every time a task is spawned
    fn spawn_header(&self, pool: Pool) {
        assert!((pool as usize) < self.shared.queues.len());

        // SAFETY: we must grab and increment this counter so `access_per_thread_data` knows
        // we're in flight.
        self.shared.job_count.fetch_add(1, Ordering::AcqRel);

        // Say we're no longer idle so that `yard.spawn(); yard.wait_for_idle()`
        // won't "return early". If the thread hasn't woken up fully yet by the
        // time wait_for_idle is called, it will immediately return even though logically there's
        // still an outstanding, active, job.
        self.shared.idle_wait.reset();
    }

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
                sender.send(fut.await).unwrap_or_else(|_| panic!("Could not send data"));
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

    pub fn spawn_local<Func, Fut, T>(&self, pool: Pool, priority: Priority, async_fn: Func) -> JoinHandle<T>
    where
        Func: FnOnce(Rc<TD>) -> Fut + Send + 'static,
        Fut: Future<Output = T>,
        T: Send + 'static,
    {
        self.spawn_header(pool);

        let (sender, receiver) = oneshot_channel();
        let job = Job::Local(Box::new(move |td| {
            Box::pin(async move {
                sender
                    .send(async_fn(td).await)
                    .unwrap_or_else(|_| panic!("Could not send data"));
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
        let count = self.shared.job_count.load(Ordering::Acquire);

        // SAFETY: No more jobs can be added because we have an exclusive reference to the yard.
        if count != 0 {
            return None;
        }

        // SAFETY:
        //  - We know there are no jobs running because `count` is zero and we have an exclusive reference to the yard.
        //  - Threads do not hold any reference to their thread local data unless they are running jobs.
        //  - All threads have yielded waiting for jobs, and no jobs can be added, so this cannot change.
        //  - We are allowed to deref this from another thread as `TD` is `Send`.
        // TODO:
        //  - How can we be sure that threads haven't panicked after decrementing the count.
        let data: Vec<&mut TD> = self.thread_local_data.iter().map(|&ptr| unsafe { &mut *ptr }).collect();

        Some(data)
    }

    /// Kill all threads as soon as they come idle. All jobs submitted after this point
    /// will not run.
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
