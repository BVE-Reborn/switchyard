use switchyard::{threads::single_pool_single_thread, Switchyard};

#[test]
#[should_panic]
fn panicking_future() {
    let yard = Switchyard::new(1, single_pool_single_thread(None, None), || ()).unwrap();

    let handle = yard.spawn(0, 0, async move {
        panic!("whoops!");
    });

    futures_executor::block_on(handle);
}

#[test]
#[should_panic]
fn panicking_function() {
    let yard = Switchyard::new(1, single_pool_single_thread(None, None), || ()).unwrap();

    let handle = yard.spawn_local(0, 0, |_| {
        panic!("whoops!");
        #[allow(unreachable_code)]
        async move {}
    });

    futures_executor::block_on(handle);
}

#[test]
#[should_panic]
fn panicking_local_future() {
    let yard = Switchyard::new(1, single_pool_single_thread(None, None), || ()).unwrap();

    let handle = yard.spawn_local(0, 0, |_| async move { panic!("whoops!") });

    futures_executor::block_on(handle);
}

#[test]
fn continue_from_panicking_future() {
    let yard = Switchyard::new(1, single_pool_single_thread(None, None), || ()).unwrap();

    // ignore handle, it'll panic
    yard.spawn(0, 0, async move {
        panic!("whoops!");
    });
    let handle = yard.spawn(0, 0, async move { 1 });

    assert_eq!(futures_executor::block_on(handle), 1);
}
