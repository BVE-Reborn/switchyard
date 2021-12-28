use futures_executor::block_on;
use switchyard::{
    threads::{one_to_one, thread_info},
    Switchyard,
};

#[test]
fn ten_thousand() {
    let yard = Switchyard::new(one_to_one(thread_info(), None), || ()).unwrap();

    let handles: Vec<_> = (0..10_000).map(|idx| yard.spawn(0, async move { idx * 2 })).collect();

    block_on(async {
        for (idx, handle) in handles.into_iter().enumerate() {
            assert_eq!(handle.await, idx * 2);
        }
        yard.wait_for_idle().await;
    });

    assert_eq!(yard.jobs(), 0);
}
