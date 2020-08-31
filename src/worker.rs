use crate::{
    task::{Job, ThreadLocalTask},
    threads::ThreadAllocationOutput,
    util::ThreadLocalPointer,
    Queue, Shared, ThreadLocalQueue,
};
use futures_task::Poll;
use priority_queue::PriorityQueue;
use std::{
    rc::Rc,
    sync::{atomic::Ordering, Arc},
};

pub(crate) fn body<TD, TDFunc>(
    shared: Arc<Shared<TD>>,
    thread_info: ThreadAllocationOutput,
    thread_local_sender: std::sync::mpsc::Sender<ThreadLocalPointer<TD>>,
    thread_local_creator: Arc<TDFunc>,
) -> impl FnOnce()
where
    TD: 'static,
    TDFunc: Fn() -> TD + Send + Sync + 'static,
{
    move || {
        let thread_locals: Rc<TD> = Rc::new(thread_local_creator());
        let thread_local_ptr = &*thread_locals as *const _ as *mut _;
        let thread_queue = Arc::new(ThreadLocalQueue::new(PriorityQueue::new()));

        // Send thead local address
        thread_local_sender
            .send(ThreadLocalPointer(thread_local_ptr))
            .unwrap_or_else(|_| panic!("Could not send data"));
        // Drop sender so receiver will stop waiting
        drop(thread_local_sender);

        loop {
            let queue: &Queue<TD> = &shared.queues[thread_info.pool as usize];
            let mut global_guard = queue.inner.lock();
            let local_guard = thread_queue.lock();
            let mut local_guard = if global_guard.is_empty() && local_guard.is_empty() {
                drop(local_guard);
                queue.cond_var.wait(&mut global_guard);
                if shared.death_signal.load(Ordering::Acquire) {
                    break;
                }
                thread_queue.lock()
            } else {
                local_guard
            };
            drop(global_guard);

            // First try the local queue
            if let Some((task, _)) = local_guard.pop() {
                drop(local_guard);

                let task: Arc<ThreadLocalTask<TD>> = task;

                if let Poll::Ready(()) = unsafe { task.poll() } {
                    shared.count.fetch_sub(1, Ordering::AcqRel);
                }
                continue;
            } else {
                drop(local_guard);
            }

            // Then the global one
            let mut global_guard = queue.inner.lock();
            if let Some((job, queue_priority)) = global_guard.pop() {
                drop(global_guard);

                let job: Job<TD> = job;

                match job {
                    Job::Future(task) => {
                        debug_assert_eq!(task.priority, queue_priority);
                        if let Poll::Ready(()) = task.poll() {
                            shared.count.fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                    Job::Local(func) => {
                        // SAFETY: This reference will only be read in this thread,
                        // and this thread's stack stores all data for the thread.
                        let fut = func(Rc::clone(&thread_locals));
                        let task = ThreadLocalTask::new(
                            Arc::clone(&shared),
                            Arc::clone(&thread_queue),
                            fut,
                            thread_info.pool,
                            queue_priority,
                        );
                        if let Poll::Ready(()) = unsafe { task.poll() } {
                            shared.count.fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                };
                continue;
            } else {
                drop(global_guard);
            }
        }
    }
}
