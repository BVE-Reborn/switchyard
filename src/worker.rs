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

        let queue: &Queue<TD> = &shared.queues[thread_info.pool as usize];

        loop {
            let mut global_guard = queue.inner.lock();
            let local_guard = thread_queue.lock();

            let mut local_guard = if global_guard.is_empty() && local_guard.is_empty() {
                // release the local guard while we wait
                drop(local_guard);

                // signal threads waiting for idle
                let active_threads = shared.active_threads.fetch_sub(1, Ordering::AcqRel) - 1;
                if active_threads == 0 {
                    // all threads are starved, wake up any idle waiters
                    shared.idle_wait.set();
                }

                // check if death was requested before
                if shared.death_signal.load(Ordering::Acquire) {
                    break;
                }

                // wait for condvar signal
                queue.cond_var.wait(&mut global_guard);

                // check if death was requested
                if shared.death_signal.load(Ordering::Acquire) {
                    break;
                }

                // this thread is now active
                shared.active_threads.fetch_add(1, Ordering::AcqRel);
                // threads waiting for idle should wait
                shared.idle_wait.reset();

                // relock the local queue
                thread_queue.lock()
            } else {
                local_guard
            };
            // release the global guard for now
            drop(global_guard);

            // First try the local queue
            if let Some((task, _)) = local_guard.pop() {
                drop(local_guard);

                let task: Arc<ThreadLocalTask<TD>> = task;

                if let Poll::Ready(()) = unsafe { Arc::clone(&task).poll() } {
                    assert_eq!(
                        Arc::strong_count(&task),
                        1,
                        "A future has retained its waker after returning Ready. This is mean."
                    );
                    shared.job_count.fetch_sub(1, Ordering::AcqRel);
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
                        if let Poll::Ready(()) = Arc::clone(&task).poll() {
                            assert_eq!(
                                Arc::strong_count(&task),
                                1,
                                "A future has retained its waker after returning Ready. This is mean."
                            );
                            shared.job_count.fetch_sub(1, Ordering::AcqRel);
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
                            shared.job_count.fetch_sub(1, Ordering::AcqRel);
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
