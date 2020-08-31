use futures_intrusive::channel::shared::oneshot_channel as oneshot_async;
use std::{cell::Cell, sync::mpsc::channel as oneshot_sync};
use switchyard::{threads::single_pool_single_thread, Runtime};

#[test]
fn single_task() {
    let yard = Runtime::new(1, single_pool_single_thread(None, None), || ());
    let handle = yard.spawn(0, 0, async { 12 });
    let result = futures_executor::block_on(handle);

    assert_eq!(yard.jobs(), 0);
    assert_eq!(result, Some(12));
}

#[test]
fn single_local_task() {
    let yard = Runtime::new(1, single_pool_single_thread(None, None), || Cell::new(30));
    let handle = yard.spawn_local(0, 0, |value| async move { value.take() + 12 });
    let result = futures_executor::block_on(handle);

    assert_eq!(yard.jobs(), 0);
    assert_eq!(result, Some(42));
}

#[test]
fn single_waiting_task() {
    let yard = Runtime::new(1, single_pool_single_thread(None, None), || ());

    // The design of this test is to only send the value to wait for after we confirm that the
    // future is running, which gives it a very high chance of the await returning Pending
    // allowing us to test our wakers.
    let (sender1, receiver1) = oneshot_sync();
    let (sender2, receiver2) = oneshot_async();

    let handle = yard.spawn(0, 0, async move {
        // We're running the future
        sender1.send(()).unwrap();
        // Wait for data
        receiver2.receive().await.unwrap()
    });

    // Wait until the future is running
    receiver1.recv().unwrap();
    // Send data
    sender2.send(42).unwrap();

    let result = futures_executor::block_on(handle);

    assert_eq!(yard.jobs(), 0);
    assert_eq!(result, Some(42));
}

#[test]
fn single_local_waiting_task() {
    let yard = Runtime::new(1, single_pool_single_thread(None, None), || ());

    // The design of this test is to only send the value to wait for after we confirm that the
    // future is running, which gives it a very high chance of the await returning Pending
    // allowing us to test our wakers.
    let (sender1, receiver1) = oneshot_sync();
    let (sender2, receiver2) = oneshot_async();

    let handle = yard.spawn_local(0, 0, move |_| async move {
        // We're running the future
        sender1.send(()).unwrap();
        // Wait for data
        receiver2.receive().await.unwrap()
    });

    // Wait until the future is running
    receiver1.recv().unwrap();

    assert_eq!(yard.jobs(), 1);

    // Send data
    sender2.send(42).unwrap();

    let result = futures_executor::block_on(handle);

    assert_eq!(yard.jobs(), 0);
    assert_eq!(result, Some(42));
}

#[test]
fn multi_tasks() {
    let yard = Runtime::new(1, single_pool_single_thread(None, None), || ());

    let task_one = yard.spawn(0, 0, async move { 2 * 2 });
    let task_two = yard.spawn(0, 0, async move { 4 * 4 });

    assert_eq!(futures_executor::block_on(task_one), Some(2 * 2));
    assert_eq!(futures_executor::block_on(task_two), Some(4 * 4));

    assert_eq!(yard.jobs(), 0);
}
