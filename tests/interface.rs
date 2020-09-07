use switchyard::{threads::single_pool_single_thread, Switchyard, SwitchyardCreationError, MAX_POOLS};

#[test]
fn too_few_pools() {
    let result = Switchyard::new(0, single_pool_single_thread(None, None), || ());
    assert!(matches!(
        result,
        Err(SwitchyardCreationError::InvalidPoolIndex {
            thread_idx: 0,
            pool_idx: 0,
            total_pools: 0
        })
    ));
}

#[test]
fn too_many_pools() {
    const POOLS: u8 = MAX_POOLS + 1;
    let result = Switchyard::new(POOLS, single_pool_single_thread(None, None), || ());
    assert!(matches!(
        result,
        Err(SwitchyardCreationError::TooManyPools { pools_requested: POOLS })
    ));
}

#[test]
#[should_panic]
fn invalid_pool_spawn() {
    let yard = Switchyard::new(1, single_pool_single_thread(None, None), || ()).unwrap();
    yard.spawn(1, 0, async move {});
}

#[test]
#[should_panic]
fn spawn_finished() {
    let mut yard = Switchyard::new(1, single_pool_single_thread(None, None), || ()).unwrap();
    yard.finish();

    yard.spawn(0, 0, async move {});
}

#[test]
#[should_panic]
fn insane_affinity() {
    let _yard = Switchyard::new(1, single_pool_single_thread(None, Some(usize::MAX)), || ()).unwrap();
}
