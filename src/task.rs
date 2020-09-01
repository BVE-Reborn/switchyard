use crate::{util::SenderSyncer, Pool, Priority, Queue, Shared, ThreadLocalQueue};
use futures_task::ArcWake;
use parking_lot::Mutex;
use slotmap::{DefaultKey, KeyData};
use std::{
    future::Future,
    hash::{Hash, Hasher},
    pin::Pin,
    rc::Rc,
    sync::Arc,
    task::{Context, Poll},
};

#[allow(clippy::type_complexity)]
pub(crate) enum Job<TD> {
    Resume(DefaultKey),
    Future(Arc<Task<TD>>),
    Local(Box<dyn FnOnce(Rc<TD>) -> Pin<Box<dyn Future<Output = ()>>> + Send>),
}

impl<TD> Job<TD> {
    fn to_address(&self) -> usize {
        // SAFETY: These addresses are `Pin`, and we won't be removing them from their boxes, so this
        // should be valid to use for ParitalEq and Hash.
        match self {
            &Self::Resume(key) => KeyData::from(key).as_ffi() as usize,
            Self::Future(fut) => &*fut.future.lock() as *const _ as *const () as usize,
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

#[allow(clippy::type_complexity)]
pub(crate) enum ThreadLocalJob {
    Resume(DefaultKey),
}

impl ThreadLocalJob {
    fn to_address(&self) -> usize {
        // SAFETY: These addresses are `Pin`, and we won't be removing them from their boxes, so this
        // should be valid to use for ParitalEq and Hash.
        match self {
            &Self::Resume(key) => KeyData::from(key).as_ffi() as usize,
        }
    }
}

impl PartialEq for ThreadLocalJob {
    fn eq(&self, other: &Self) -> bool {
        self.to_address() == other.to_address()
    }
}

impl Eq for ThreadLocalJob {}

impl Hash for ThreadLocalJob {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.to_address());
    }
}

pub(crate) struct Task<TD> {
    pub shared: Arc<Shared<TD>>,
    pub future: Mutex<Pin<Box<dyn Future<Output = ()> + Send>>>,
    pub pool: Pool,
    pub priority: Priority,
}
impl<TD> Task<TD> {
    pub fn new<Fut>(shared: Arc<Shared<TD>>, future: Fut, pool: Pool, priority: Priority) -> Arc<Self>
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        Arc::new(Self {
            shared,
            future: Mutex::new(Box::pin(future)),
            pool,
            priority,
        })
    }

    pub fn poll(self: Arc<Self>) -> Poll<()> {
        let (raw_waker, key) = Waker::from_task(&self);
        let waker = futures_task::waker(raw_waker);
        let mut ctx = Context::from_waker(&waker);

        let mut guard = self.future.lock();

        let poll_value = guard.as_mut().poll(&mut ctx);

        if let Poll::Ready(_) = poll_value {
            let mut slot_guard = self.shared.queues[self.pool as usize].waiting.lock();
            slot_guard.remove(key);
            drop(slot_guard)
        }

        poll_value
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

impl<TD> ArcWake for Waker<TD> {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let queue: &Queue<TD> = &arc_self.shared.queues[arc_self.pool as usize];

        let mut queue_guard = queue.inner.lock();
        queue_guard.push(Job::Resume(arc_self.future_key), arc_self.priority);
        queue.cond_var.notify_one();
        drop(queue_guard);
    }
}

// TODO: Are any of these threads just passthrough to the waker
pub(crate) struct ThreadLocalTask<TD> {
    pub shared: Arc<Shared<TD>>,
    pub return_queue: Arc<ThreadLocalQueue<TD>>,
    pub future: SenderSyncer<Mutex<Pin<Box<dyn Future<Output = ()>>>>>,
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
        Arc::new(Self {
            shared,
            return_queue,
            future: unsafe { SenderSyncer::new(Mutex::new(Box::pin(future))) },
            pool,
            priority,
        })
    }

    /// # Safety
    ///
    /// - This function can only be called on the thread that `new` was called on.
    pub unsafe fn poll(self: Arc<Self>) -> Poll<()> {
        let (raw_waker, key) = ThreadLocalWaker::from_task(&self);
        let waker = futures_task::waker(raw_waker);
        let mut ctx = Context::from_waker(&waker);

        let mut guard = self.future.inner_ref().lock();

        let poll_value = guard.as_mut().poll(&mut ctx);

        if let Poll::Ready(_) = poll_value {
            let mut slot_guard = self.return_queue.waiting.lock();
            slot_guard.remove(key);
            drop(slot_guard)
        }

        poll_value
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

impl<TD> ArcWake for ThreadLocalWaker<TD> {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let global_queue: &Queue<TD> = &arc_self.shared.queues[arc_self.pool as usize];

        // Always grab global -> local

        // Lock global mutex for condvar
        let global_guard = global_queue.inner.lock();
        // Lock local mutex for modification
        let mut local_guard = arc_self.return_queue.inner.lock();
        local_guard.push(ThreadLocalJob::Resume(arc_self.future_key), arc_self.priority);
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
