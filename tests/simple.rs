use switchyard::{threads::single_pool_single_thread, Runtime};

#[test]
fn single_task() {
    let yard = Runtime::new(1, single_pool_single_thread(None, None), || ());
    let handle = yard.spawn(0, 0, async { 12 });
    let result = futures_executor::block_on(handle);
    assert_eq!(result, Some(12));
}
