use std::{cell::Cell, time::Duration};
use switchyard::{threads::single_thread, Switchyard};

#[test]
fn single_task() {
    let yard = Switchyard::new(single_thread(None, None), || ()).unwrap();
    let handle = yard.spawn(0, async { 12 });
    let result = futures_executor::block_on(handle);

    assert_eq!(result, 12);
}

#[test]
fn single_local_task() {
    let yard = Switchyard::new(single_thread(None, None), || Cell::new(30)).unwrap();
    let handle = yard.spawn_local(0, |value| async move { value.take() + 12 });
    let result = futures_executor::block_on(handle);

    assert_eq!(result, 42);
}

#[test]
fn single_waiting_task() {
    let yard = Switchyard::new(single_thread(None, None), || ()).unwrap();

    // The design of this test is to only send the value to wait for after we confirm that the
    // future is running, which gives it a very high chance of the await returning Pending
    // allowing us to test our wakers.
    let (sender1, receiver1) = flume::unbounded();
    let (sender2, receiver2) = flume::unbounded();

    let handle = yard.spawn(0, async move {
        // We're running the future
        sender1.send(()).unwrap();
        // Wait for data
        receiver2.recv_async().await.unwrap()
    });

    // Wait until the future is running
    receiver1.recv().unwrap();
    // Send data
    sender2.send(42).unwrap();

    let result = futures_executor::block_on(handle);

    assert_eq!(result, 42);
}

#[test]
fn single_local_waiting_task() {
    let yard = Switchyard::new(single_thread(None, None), || ()).unwrap();

    // The design of this test is to only send the value to wait for after we confirm that the
    // future is running, which gives it a very high chance of the await returning Pending
    // allowing us to test our wakers.
    let (sender1, receiver1) = flume::unbounded();
    let (sender2, receiver2) = flume::unbounded();

    let handle = yard.spawn_local(0, move |_| async move {
        // We're running the future
        sender1.send(()).unwrap();
        // Wait for data
        receiver2.recv_async().await.unwrap()
    });

    // Wait until the future is running
    receiver1.recv().unwrap();

    assert_eq!(yard.jobs(), 1);

    // Send data
    sender2.send(42).unwrap();

    let result = futures_executor::block_on(handle);

    assert_eq!(result, 42);
}

#[test]
fn multi_tasks() {
    let yard = Switchyard::new(single_thread(None, None), || ()).unwrap();

    let task_one = yard.spawn(0, async move { 2 * 2 });
    let task_two = yard.spawn(0, async move { 4 * 4 });

    assert_eq!(futures_executor::block_on(task_one), 2 * 2);
    assert_eq!(futures_executor::block_on(task_two), 4 * 4);
}

#[test]
fn wait_for_idle() {
    let yard = Switchyard::new(single_thread(None, None), || ()).unwrap();

    let (sender, receiver) = flume::unbounded();
    let task = yard.spawn(0, async move {
        // Regular sleep to hard block the thread
        std::thread::sleep(Duration::from_millis(100));
        sender.send(2 * 2).unwrap()
    });

    futures_executor::block_on(yard.wait_for_idle());

    assert_eq!(receiver.recv().unwrap(), 4);
    assert_eq!(futures_executor::block_on(task), ());
    assert_eq!(yard.jobs(), 0);
}

#[test]
fn wait_for_idle_stalled() {
    let yard = Switchyard::new(single_thread(None, None), || ()).unwrap();

    let (sender, receiver) = flume::unbounded();
    let task = yard.spawn(0, async move {
        receiver.recv_async().await.unwrap();
        2 * 2
    });

    assert_eq!(yard.jobs(), 1);

    // nothing has been sent, wait for idle should clear immediately
    futures_executor::block_on(yard.wait_for_idle());

    assert_eq!(yard.jobs(), 1);

    sender.send(()).unwrap();

    assert_eq!(futures_executor::block_on(task), 4);
}

#[test]
fn access_per_thread_data() {
    let mut yard = Switchyard::new(single_thread(None, None), || Cell::new(12)).unwrap();

    futures_executor::block_on(yard.wait_for_idle());

    assert_eq!(yard.access_per_thread_data(), Some(vec![&mut Cell::new(12)]));

    let (job_sender, test_receiver) = flume::unbounded();
    let (test_sender, job_receiver) = flume::unbounded();
    let task = yard.spawn_local(0, move |value| async move {
        job_sender.send(()).unwrap();
        job_receiver.recv_async().await.unwrap();

        value.set(4);
        4
    });

    assert_eq!(yard.jobs(), 1);

    // wait until the job actually starts
    test_receiver.recv().unwrap();

    // nothing has been sent, there are still jobs in flight using the data, we must wait.
    assert_eq!(yard.access_per_thread_data(), None);
    assert_eq!(yard.jobs(), 1);

    test_sender.send(()).unwrap();

    assert_eq!(futures_executor::block_on(task), 4);

    // the task has cleared, but the thread may not be shut down yet
    futures_executor::block_on(yard.wait_for_idle());

    assert_eq!(yard.jobs(), 0);
    assert_eq!(yard.access_per_thread_data(), Some(vec![&mut Cell::new(4)]));
}
