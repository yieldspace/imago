use criterion::{Criterion, black_box, criterion_group, criterion_main};
use imagod_control::bench_internals::bench_log_buffer_push_and_tail;

fn bench_log_buffer(c: &mut Criterion) {
    c.bench_function("log_buffer/push_and_tail_lines", |b| {
        b.iter(|| {
            let _ = bench_log_buffer_push_and_tail(
                black_box(1_000),
                black_box(256),
                black_box(64 * 1024),
                black_box(200),
            );
        });
    });
}

criterion_group!(log_buffer, bench_log_buffer);
criterion_main!(log_buffer);
