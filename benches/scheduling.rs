use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use switchyard::threads::{one_to_one, thread_info};

pub fn wide(c: &mut Criterion) {
    let mut group = c.benchmark_group("wide");

    let count = 100_000;

    let future_creation = || {
        let mut array = Vec::with_capacity(count);
        for _ in 0..count {
            array.push(async move { 2 * 2 });
        }
        array
    };

    group.throughput(Throughput::Elements(count as u64));

    group.bench_function("async-std", |b| {
        b.iter_batched(
            future_creation,
            |input| {
                let handle_vec: Vec<_> = input.into_iter().map(|fut| async_std::task::spawn(fut)).collect();
                futures_executor::block_on(async move {
                    for handle in handle_vec {
                        handle.await;
                    }
                })
            },
            BatchSize::PerIteration,
        )
    });

    let yard = switchyard::Switchyard::new(
        one_to_one(thread_info(), Some("thread-name")),
        || (),
    )
    .unwrap();

    group.bench_function("switchyard", |b| {
        b.iter_batched(
            future_creation,
            |input| {
                let handle_vec: Vec<_> = input.into_iter().map(|fut| yard.spawn(0, fut)).collect();
                futures_executor::block_on(async move {
                    for handle in handle_vec {
                        handle.await;
                    }
                })
            },
            BatchSize::PerIteration,
        )
    });

    group.finish();
}

pub fn chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("chain");

    let count = 1_000;

    let (sender, receiver) = flume::unbounded();

    group.throughput(Throughput::Elements(count as u64));

    group.bench_function("async-std", |b| {
        b.iter_batched(
            || {
                let receiver = receiver.clone();
                let mut head = async_std::task::spawn(async move {
                    receiver.recv_async().await.unwrap();
                });
                for _ in 0..count {
                    let old_head = head;
                    head = async_std::task::spawn(async move {
                        old_head.await;
                    });
                }
                head
            },
            |input| {
                sender.send(()).unwrap();
                futures_executor::block_on(input)
            },
            BatchSize::PerIteration,
        )
    });

    let yard = switchyard::Switchyard::new(
        one_to_one(thread_info(), Some("switchyard")),
        || (),
    )
    .unwrap();

    group.bench_function("switchyard", |b| {
        b.iter_batched(
            || {
                let receiver = receiver.clone();
                let mut head = yard.spawn(0, async move {
                    receiver.recv_async().await.unwrap();
                });
                for _ in 0..count {
                    let old_head = head;
                    head = yard.spawn(0, async move {
                        old_head.await;
                    });
                }
                head
            },
            |input| {
                sender.send(()).unwrap();
                futures_executor::block_on(input)
            },
            BatchSize::PerIteration,
        )
    });

    group.finish();
}

criterion_group!(benches, wide, chain);
criterion_main!(benches);
