use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use switchyard::{threads::single_pool_single_thread, Switchyard};

#[test]
fn held_thread_local_data() {
    let mut yard = Switchyard::new(1, single_pool_single_thread(None, None), || ()).unwrap();

    let (sender, receiver) = flume::unbounded();
    yard.spawn_local(0, 0, |arc| async move {
        sender.send(arc).unwrap();
    });

    let arc = receiver.recv().unwrap();

    futures_executor::block_on(yard.wait_for_idle());

    assert_eq!(yard.access_per_thread_data(), None);

    drop(arc)
}

struct ImmediateWake;
impl Future for ImmediateWake {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        cx.waker().wake_by_ref();
        Poll::Ready(())
    }
}

#[test]
fn immediate_wake() {
    let yard = Switchyard::new(1, single_pool_single_thread(None, None), || ()).unwrap();

    let handle = yard.spawn(0, 0, ImmediateWake);

    futures_executor::block_on(handle);
}
