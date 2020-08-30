use std::cell::Cell;
use switchyard::{threads::single_pool_single_thread, Runtime};

#[test]
fn single_task() {
    let yard = Runtime::new(1, single_pool_single_thread(None, None), || ());
    let handle = yard.spawn(0, 0, async { 12 });
    let result = futures_executor::block_on(handle);
    assert_eq!(result, Some(12));
}

#[test]
fn single_local_task() {
    let yard = Runtime::new(1, single_pool_single_thread(None, None), || Cell::new(30));
    let handle = yard.spawn_local(0, 0, |value| async move { value.take() + 12 });
    let result = futures_executor::block_on(handle);
    assert_eq!(result, Some(42));
}
