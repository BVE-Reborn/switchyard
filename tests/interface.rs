use switchyard::{threads::single_thread, Switchyard};

#[test]
#[should_panic]
fn spawn_finished() {
    let mut yard = Switchyard::new(single_thread(None, None), || ()).unwrap();
    yard.finish();

    yard.spawn(0, async move {});
}

#[test]
#[should_panic]
fn insane_affinity() {
    let _yard = Switchyard::new(single_thread(None, Some(usize::MAX)), || ()).unwrap();
}
