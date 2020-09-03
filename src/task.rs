use crate::{util::SenderSyncer, Pool, Priority, Queue, Shared, ThreadLocalQueue};
use futures_task::ArcWake;
use parking_lot::Mutex;
use slotmap::DefaultKey;
use std::{
    future::Future,
    hash::{Hash, Hasher},
    pin::Pin,
    sync::{atomic::Ordering, Arc},
    task::{Context, Poll},
};

#[allow(clippy::type_complexity)]
pub(crate) enum Job<TD> {
    Future(Arc<Task<TD>>),
    Local(Box<dyn FnOnce(Arc<TD>) -> Pin<Box<dyn Future<Output = ()>>> + Send>),
}

impl<TD> Job<TD> {
    fn to_address(&self) -> usize {
        // SAFETY: These addresses are `Pin`, and we won't be removing them from their boxes, so this
        // should be valid to use for ParitalEq and Hash.
        match self {
            Self::Future(fut) => fut.future_address,
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

pub(crate) enum ThreadLocalJob<TD> {
    Future(Arc<ThreadLocalTask<TD>>),
}

impl<TD> ThreadLocalJob<TD> {
    fn to_address(&self) -> usize {
        // SAFETY: These addresses are `Pin`, and we won't be removing them from their boxes, so this
        // should be valid to use for PartialEq and Hash.
        match self {
            Self::Future(fut) => fut.future_address,
        }
    }
}

impl<TD> PartialEq for ThreadLocalJob<TD> {
    fn eq(&self, other: &Self) -> bool {
        self.to_address() == other.to_address()
    }
}

impl<TD> Eq for ThreadLocalJob<TD> {}

impl<TD> Hash for ThreadLocalJob<TD> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.to_address());
    }
}

pub(crate) struct StoredFuture {
    pub inner: Pin<Box<dyn Future<Output = ()> + Send>>,
    // Future has returned Ready(T) and future polls should be ignored
    pub completed: bool,
}
impl StoredFuture {
    pub fn new(future: Pin<Box<dyn Future<Output = ()> + Send>>) -> Self {
        Self {
            inner: future,
            completed: false,
        }
    }
}

pub(crate) struct Task<TD> {
    pub shared: Arc<Shared<TD>>,
    pub future: Mutex<StoredFuture>,
    pub future_address: usize,
    pub pool: Pool,
    pub priority: Priority,
}
impl<TD> Task<TD> {
    pub fn new<Fut>(shared: Arc<Shared<TD>>, future: Fut, pool: Pool, priority: Priority) -> Arc<Self>
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        let fut_box = Box::pin(future);
        Arc::new(Self {
            shared,
            future_address: &*fut_box as *const _ as usize,
            future: Mutex::new(StoredFuture::new(fut_box)),
            pool,
            priority,
        })
    }

    pub fn poll(self: Arc<Self>) -> Option<Poll<()>> {
        let (raw_waker, key) = Waker::from_task(&self);
        let waker = futures_task::waker(raw_waker);
        let mut ctx = Context::from_waker(&waker);

        let mut guard = self.future.lock();

        if guard.completed {
            // This future is finished
            return None;
        }

        let poll_value = guard.inner.as_mut().poll(&mut ctx);

        if let Poll::Ready(_) = poll_value {
            // Removing from the waiting queue is often enough, but if the task
            // is re-awoken _during_ the call to poll which returns Ready(T), we
            // still need to prevent it from running again

            let mut slot_guard = self.shared.queues[self.pool as usize].waiting.lock();
            slot_guard.remove(key);
            drop(slot_guard);

            guard.completed = true;
        }

        Some(poll_value)
    }
}

pub(crate) struct Waker<TD> {
    pub shared: Arc<Shared<TD>>,
    pub future_key: DefaultKey,
    pub pool: Pool,
    pub priority: Priority,
}

impl<TD> Waker<TD> {
    fn from_task(task: &Arc<Task<TD>>) -> (Arc<Self>, DefaultKey) {
        let queue: &Queue<TD> = &task.shared.queues[task.pool as usize];

        let mut waiting_lock = queue.waiting.lock();
        let future_key = waiting_lock.insert(Arc::clone(&task));
        drop(waiting_lock);

        (
            Arc::new(Self {
                shared: Arc::clone(&task.shared),
                future_key,
                pool: task.pool,
                priority: task.priority,
            }),
            future_key,
        )
    }
}

impl<TD> Drop for Waker<TD> {
    fn drop(&mut self) {
        // We're the last waker, clean up our task
        let queue: &Queue<TD> = &self.shared.queues[self.pool as usize];

        let mut waiting_lock = queue.waiting.lock();
        // let this drop, if someone woke this task already, this will be `None`.
        if let Some(fut) = waiting_lock.remove(self.future_key) {
            // SAFETY: drop this future before decrementing the count
            drop(fut);
            // we're the last of our task, remove it from the active
            self.shared.job_count.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl<TD> ArcWake for Waker<TD> {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let queue: &Queue<TD> = &arc_self.shared.queues[arc_self.pool as usize];

        let mut waiting_lock = queue.waiting.lock();
        let future_opt = waiting_lock.remove(arc_self.future_key);
        drop(waiting_lock);

        let future = match future_opt {
            // Someone got to this task first
            None => return,
            // We're the first to awaken it
            Some(fut) => fut,
        };

        let mut queue_guard = queue.inner.lock();
        queue_guard.push(Job::Future(future), arc_self.priority);
        queue.cond_var.notify_one();
        drop(queue_guard);
    }
}

pub(crate) struct StoredThreadLocalFuture {
    pub inner: Pin<Box<dyn Future<Output = ()>>>,
    // Future has returned Ready(T) and future polls should be ignored
    pub completed: bool,
}
impl StoredThreadLocalFuture {
    pub fn new(future: Pin<Box<dyn Future<Output = ()>>>) -> Self {
        Self {
            inner: future,
            completed: false,
        }
    }
}

// TODO: Are any of these threads just passthrough to the waker
pub(crate) struct ThreadLocalTask<TD> {
    pub shared: Arc<Shared<TD>>,
    pub return_queue: Arc<ThreadLocalQueue<TD>>,
    pub future: SenderSyncer<Mutex<StoredThreadLocalFuture>>,
    pub future_address: usize,
    pub pool: Pool,
    pub priority: Priority,
}
impl<TD> ThreadLocalTask<TD> {
    pub fn new<Fut>(
        shared: Arc<Shared<TD>>,
        return_queue: Arc<ThreadLocalQueue<TD>>,
        future: Fut,
        pool: Pool,
        priority: Priority,
    ) -> Arc<Self>
    where
        Fut: Future<Output = ()> + 'static,
    {
        let fut_box = Box::pin(future);
        Arc::new(Self {
            shared,
            return_queue,
            future_address: &*fut_box as *const _ as usize,
            future: unsafe { SenderSyncer::new(Mutex::new(StoredThreadLocalFuture::new(fut_box))) },
            pool,
            priority,
        })
    }

    /// # Safety
    ///
    /// - This function can only be called on the thread that `new` was called on.
    pub unsafe fn poll(self: Arc<Self>) -> Option<Poll<()>> {
        let (raw_waker, key) = ThreadLocalWaker::from_task(&self);
        let waker = futures_task::waker(raw_waker);
        let mut ctx = Context::from_waker(&waker);

        let mut guard = self.future.inner_ref().lock();

        if guard.completed {
            return None;
        }

        let poll_value = guard.inner.as_mut().poll(&mut ctx);

        if let Poll::Ready(_) = poll_value {
            // Removing from the waiting queue is often enough, but if the task
            // is re-awoken _during_ the call to poll which returns Ready(T), we
            // still need to prevent it from running again

            let mut slot_guard = self.return_queue.waiting.lock();
            slot_guard.remove(key);
            drop(slot_guard);

            guard.completed = true;
        }

        Some(poll_value)
    }
}

pub(crate) struct ThreadLocalWaker<TD> {
    pub shared: Arc<Shared<TD>>,
    pub return_queue: Arc<ThreadLocalQueue<TD>>,
    pub future_key: DefaultKey,
    pub pool: Pool,
    pub priority: Priority,
}

impl<TD> ThreadLocalWaker<TD> {
    fn from_task(task: &Arc<ThreadLocalTask<TD>>) -> (Arc<Self>, DefaultKey) {
        let queue: &ThreadLocalQueue<TD> = &task.return_queue;

        let mut waiting_lock = queue.waiting.lock();
        let future_key = waiting_lock.insert(Arc::clone(&task));
        drop(waiting_lock);

        (
            Arc::new(Self {
                shared: Arc::clone(&task.shared),
                return_queue: Arc::clone(&task.return_queue),
                future_key,
                pool: task.pool,
                priority: task.priority,
            }),
            future_key,
        )
    }
}

impl<TD> Drop for ThreadLocalWaker<TD> {
    fn drop(&mut self) {
        // We're the last waker, clean up our task
        let local_queue: &ThreadLocalQueue<TD> = &self.return_queue;

        let mut waiting_lock = local_queue.waiting.lock();
        // let this drop, if someone woke this task already, this will be `None`.
        if let Some(fut) = waiting_lock.remove(self.future_key) {
            // SAFETY: drop this future before decrementing the count as it may hold a reference
            // to thread local data.
            drop(fut);
            // we're the last of our task, remove it from the active
            self.shared.job_count.fetch_sub(1, Ordering::AcqRel);
        }
        drop(waiting_lock);
    }
}

impl<TD> ArcWake for ThreadLocalWaker<TD> {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let global_queue: &Queue<TD> = &arc_self.shared.queues[arc_self.pool as usize];
        let local_queue: &ThreadLocalQueue<TD> = &arc_self.return_queue;

        let mut waiting_lock = local_queue.waiting.lock();
        let future_opt = waiting_lock.remove(arc_self.future_key);
        drop(waiting_lock);

        let future = match future_opt {
            // Someone got to this task first
            None => return,
            // We're the first to awaken it
            Some(fut) => fut,
        };

        // Always grab global -> local

        // Lock global mutex for condvar
        let global_guard = global_queue.inner.lock();
        // Lock local mutex for modification
        let mut local_guard = local_queue.inner.lock();
        local_guard.push(ThreadLocalJob::Future(future), arc_self.priority);
        global_queue.cond_var.notify_one();
        drop(global_guard);
    }
}

impl<TD> ThreadLocalTask<TD> {
    fn to_address(&self) -> usize {
        // SAFETY: These tasks are put in an arc then never removed.
        self as *const _ as *const () as usize
    }
}

impl<TD> PartialEq for ThreadLocalTask<TD> {
    fn eq(&self, other: &Self) -> bool {
        self.to_address() == other.to_address()
    }
}

impl<TD> Eq for ThreadLocalTask<TD> {}

impl<TD> Hash for ThreadLocalTask<TD> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.to_address());
    }
}
