use crate::{
    task::{Job, ThreadLocalTask},
    threads::ThreadAllocationOutput,
    util::ThreadLocalPointer,
    Queue, Shared, ThreadLocalQueue,
};
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
) -> impl FnOnce() -> () + Send + 'static
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
            let mut guard = queue.inner.lock();
            if guard.is_empty() {
                queue.cond_var.wait(&mut guard);
                if shared.death_signal.load(Ordering::Acquire) {
                    break;
                }
            }

            if let Some((job, queue_priority)) = guard.pop() {
                drop(guard);

                let job: Job<TD> = job;

                match job {
                    Job::Future(task) => {
                        debug_assert_eq!(task.priority, queue_priority);
                        task.poll();
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
                        unsafe { task.poll() };
                    }
                };
            }
        }
    }
}
