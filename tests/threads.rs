use futures_executor::block_on;
use futures_intrusive::channel::shared::oneshot_channel as oneshot_async;
use std::sync::mpsc::channel as oneshot_sync;
use switchyard::{
    threads::{single_pool_one_to_one, thread_info},
    Runtime,
};

#[test]
fn ten_thousand() {
    let yard = Runtime::new(1, single_pool_one_to_one(thread_info(), None), || ());

    let handles: Vec<_> = (0..10_000)
        .map(|idx| yard.spawn(0, 0, async move { idx * 2 }))
        .collect();

    block_on(async {
        for (idx, handle) in handles.into_iter().enumerate() {
            assert_eq!(handle.await, Some(idx * 2));
        }
    });
}
