//! Real time compute focused async executor.

#![deny(future_incompatible)]
#![deny(nonstandard_style)]
#![deny(rust_2018_idioms)]
// Rustdoc Warnings
#![deny(intra_doc_link_resolution_failure)]

use arrayvec::ArrayVec;
use futures_intrusive::{
    channel::shared::{oneshot_channel, ChannelReceiveFuture},
    sync::ManualResetEvent,
};
use parking_lot::{Condvar, Mutex, RawMutex};
use priority_queue::PriorityQueue;
use std::sync::atomic::AtomicBool;
use std::task::{Context, RawWaker, Waker};
use std::{
    future::Future,
    hash::{Hash, Hasher},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

const MAX_POOLS: u8 = 8;

pub type Priority = u32;
pub type Pool = u8;
pub type PoolCount = u8;

pub type JoinHandle<T> = ChannelReceiveFuture<RawMutex, T>;

pub struct ThreadAllocation<'a> {
    pub name: &'a mut dyn FnMut(&ThreadAllocationOutput) -> Option<String>,
    pub allocator: &'a mut dyn FnMut(ThreadAllocationInput) -> Vec<ThreadAllocationOutput>,
}

/// Information about threads on the system
// TODO: Detect and expose big.LITTLE
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ThreadAllocationInput {
    pub physical: usize,
    pub logical: usize,
}

/// Spawn information for a worker thread.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ThreadAllocationOutput {
    /// Identifier.
    pub ident: usize,
    /// Job pool that the thread services.
    pub pool: Pool,
    /// Core index to pin thread to.
    pub affinity: Option<usize>,
}

enum Job<TD> {
    Future(Pin<Box<dyn Future<Output = ()> + Send + Sync>>),
    Local(Box<dyn for<'v> FnOnce(&'v TD) -> Pin<Box<dyn Future<Output = ()> + 'v>> + Send>),
}

impl<TD> Job<TD> {
    fn to_address(&self) -> usize {
        // SAFETY: These addresses are `Pin`, and we won't be removing them from their boxes, so this
        // should be valid to use for ParitalEq and Hash.
        match self {
            Self::Future(fut) => &**fut as *const _ as *const () as usize,
            Self::Local(func) => &**func as *const _ as *const () as usize,
        }
    }
}

impl<TD> PartialEq for Job<TD> {
    fn eq(&self, other: &Self) -> bool {
        self.to_address() == other.to_address()
    }
}

impl<TD> Eq for Job<TD> {}

impl<TD> Hash for Job<TD> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.to_address());
    }
}

struct Queue<TD> {
    inner: Mutex<PriorityQueue<Job<TD>, u32>>,
    cond_var: Condvar,
}
type Queues<TD> = ArrayVec<[Queue<TD>; MAX_POOLS as usize]>;

struct Shared<TD> {
    count: AtomicUsize,
    death_signal: AtomicBool,
    queues: Queues<TD>,
}

pub struct Runtime<TD> {
    shared: Arc<Shared<TD>>,
    idle_wait: Arc<ManualResetEvent>,
    thread_local_data: Vec<*mut TD>,
}
impl<TD: 'static> Runtime<TD> {
    pub fn new<TDFunc>(max_pools: Pool, allocation: ThreadAllocation<'_>, data_creation: TDFunc) -> Self
    where
        TDFunc: Fn() -> TD + Send + Sync + 'static,
    {
        assert!(max_pools < MAX_POOLS);

        let input_data = ThreadAllocationInput {
            logical: num_cpus::get(),
            physical: num_cpus::get_physical(),
        };

        let threads_info: Vec<ThreadAllocationOutput> = (allocation.allocator)(input_data);
        let thread_count = threads_info.len();
        let threads_info_iter = threads_info.into_iter().map(|v| {
            let name = (allocation.name)(&v);
            (v, name)
        });

        let (sender, thread_local_receiver) = std::sync::mpsc::channel();

        let data_creation_arc = Arc::new(data_creation);

        let shared = Arc::new(Shared {
            queues: (0..max_pools)
                .map(|_| Queue {
                    inner: Mutex::new(PriorityQueue::new()),
                    cond_var: Condvar::new(),
                })
                .collect::<Queues<TD>>(),
            count: AtomicUsize::new(0),
            death_signal: AtomicBool::new(false),
        });

        for (thread_info, name) in threads_info_iter {
            let builder = std::thread::Builder::new();
            let builder = if let Some(name) = name {
                builder.name(name)
            } else {
                builder
            };
            builder
                .spawn(worker_thread_work::<TD, TDFunc>(
                    Arc::clone(&shared),
                    thread_info,
                    sender.clone(),
                    data_creation_arc.clone(),
                ))
                .unwrap_or_else(|_| panic!("Could not spawn thread"));
        }

        let mut thread_local_data = Vec::with_capacity(thread_count);
        while let Ok(ThreadLocalPointer(ptr)) = thread_local_receiver.recv() {
            thread_local_data.push(ptr);
        }

        Self {
            shared,
            idle_wait: Arc::new(ManualResetEvent::new(false)),
            thread_local_data,
        }
    }

    pub fn spawn<Fut, T>(&self, pool: Pool, priority: Priority, fut: Fut) -> JoinHandle<T>
    where
        Fut: Future<Output = T> + Send + Sync + 'static,
        T: Send + 'static,
    {
        assert!((pool as usize) < self.shared.queues.len());

        // SAFETY: we must grab and increment this counter so `access_per_thread_data` knows
        // we're in flight.
        self.shared.count.fetch_add(1, Ordering::AcqRel);

        let (sender, receiver) = oneshot_channel();
        let job = Job::Future(Box::pin(async move {
            sender.send(fut.await).unwrap_or_else(|_| panic!("Could not send data"));
        }));

        let queue: &Queue<TD> = &self.shared.queues[pool as usize];
        queue.inner.lock().push(job, priority);
        queue.cond_var.notify_one();

        receiver.receive()
    }

    pub fn spawn_local<Func, Fut, T>(&self, pool: Pool, priority: Priority, mut async_fn: Func) -> JoinHandle<T>
    where
        Func: FnMut(&TD) -> Fut + Send + 'static,
        Fut: Future<Output = T>,
        T: Send + 'static,
    {
        assert!((pool as usize) < self.shared.queues.len());

        // SAFETY: we must grab and increment this counter so `access_per_thread_data` knows
        // we're in flight.
        self.shared.count.fetch_add(1, Ordering::AcqRel);

        let (sender, receiver) = oneshot_channel();
        let job = Job::Local(Box::new(move |td| {
            Box::pin(async move {
                sender
                    .send(async_fn(td).await)
                    .unwrap_or_else(|_| panic!("Could not send data"));
            })
        }));

        let queue: &Queue<TD> = &self.shared.queues[pool as usize];
        queue.inner.lock().push(job, priority);
        queue.cond_var.notify_one();

        receiver.receive()
    }

    pub async fn wait_for_idle(&self) {
        self.idle_wait.wait().await;
        self.idle_wait.reset();
    }

    pub fn queued_jobs(&self) -> usize {
        self.shared.count.load(Ordering::Relaxed)
    }

    pub fn access_per_thread_data(&mut self) -> Option<Vec<&mut TD>>
    where
        TD: Send,
    {
        let count = self.shared.count.load(Ordering::Acquire);

        // SAFETY: No more jobs can be added because we have an exclusive reference to the runtime.
        if count != 0 {
            return None;
        }

        // SAFETY:
        //  - We know there are no jobs running because `count` is zero and we have an exclusive reference to the runtime.
        //  - Threads do not hold any reference to their thread local data unless they are running jobs.
        //  - All threads have yielded waiting for jobs, and no jobs can be added, so this cannot change.
        //  - We are allowed to deref this from another thread as `TD` is `Send`.
        // TODO:
        //  - How can we be sure that threads haven't panicked after decrementing the count.
        let data: Vec<&mut TD> = self.thread_local_data.iter().map(|&ptr| unsafe { &mut *ptr }).collect();

        Some(data)
    }
}

unsafe impl<TD> Send for Runtime<TD> {}
unsafe impl<TD> Sync for Runtime<TD> {}

struct ThreadLocalPointer<TD>(*mut TD);

unsafe impl<TD> Send for ThreadLocalPointer<TD> {}
unsafe impl<TD> Sync for ThreadLocalPointer<TD> {}

fn worker_thread_work<TD, TDFunc>(
    shared: Arc<Shared<TD>>,
    thread_info: ThreadAllocationOutput,
    thread_local_sender: std::sync::mpsc::Sender<ThreadLocalPointer<TD>>,
    thread_local_creator: Arc<TDFunc>,
) -> impl FnOnce() -> () + Send + 'static
where
    TD: 'static,
    TDFunc: Fn() -> TD + Send + Sync + 'static,
{
    move || {
        let mut thread_locals: TD = thread_local_creator();

        // Send thead local address
        thread_local_sender
            .send(ThreadLocalPointer(&mut thread_locals as *mut _))
            .unwrap_or_else(|_| panic!("Could not send data"));
        // Drop sender so receiver will stop waiting
        drop(thread_local_sender);

        loop {
            let queue: &Queue<TD> = &shared.queues[thread_info.pool as usize];
            let mut guard = queue.inner.lock();
            if guard.is_empty() {
                queue.cond_var.wait(&mut guard);
                if shared.death_signal.load(Ordering::Acquire) {
                    break;
                }
            }

            if let Some((job, priority)) = guard.pop() {
                drop(guard);

                let job: Job<TD> = job;
                let priority: Priority = priority;

                let raw_waker = waker_fn::waker_fn();
                let mut context = Context::from_waker(&waker);

                match job {
                    Job::Future(mut future) => future.as_mut().poll(&mut context),
                    Job::Local(mut func) => func(&thread_locals).as_mut().poll(&mut context),
                };
            }
        }
    }
}
