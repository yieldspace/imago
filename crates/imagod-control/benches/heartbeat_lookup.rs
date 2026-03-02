use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use imagod_control::bench_internals::{bench_lookup_with_index, bench_lookup_with_linear_scan};

fn bench_heartbeat_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("heartbeat_lookup");
    for services in [50usize, 200, 500] {
        group.bench_with_input(
            BenchmarkId::new("indexed", services),
            &services,
            |b, size| {
                b.iter(|| {
                    let _ = bench_lookup_with_index(black_box(*size), black_box(20_000));
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("linear_scan", services),
            &services,
            |b, size| {
                b.iter(|| {
                    let _ = bench_lookup_with_linear_scan(black_box(*size), black_box(20_000));
                });
            },
        );
    }
    group.finish();
}

criterion_group!(heartbeat_lookup, bench_heartbeat_lookup);
criterion_main!(heartbeat_lookup);
