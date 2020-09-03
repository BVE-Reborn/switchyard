use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use switchyard::threads;

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
        1,
        threads::single_pool_one_to_one(threads::thread_info(), Some("switchyard")),
        || (),
    )
    .unwrap();

    group.bench_function("switchyard", |b| {
        b.iter_batched(
            future_creation,
            |input| {
                let handle_vec: Vec<_> = input.into_iter().map(|fut| yard.spawn(0, 0, fut)).collect();
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

criterion_group!(benches, wide);
criterion_main!(benches);
