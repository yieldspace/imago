use criterion::{Criterion, black_box, criterion_group, criterion_main};
use imagod_server::bench_internals::{bench_forward_snapshot_datagrams, bench_retry_send_attempts};

fn bench_logs_forwarder(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("tokio runtime should be created for bench");

    c.bench_function("logs_forwarder/retry_send_no_failure", |b| {
        b.iter(|| {
            runtime
                .block_on(bench_retry_send_attempts(black_box(1024), black_box(0)))
                .expect("retry send without injected failure should succeed");
        });
    });

    c.bench_function("logs_forwarder/retry_send_one_failure", |b| {
        b.iter(|| {
            runtime
                .block_on(bench_retry_send_attempts(black_box(1024), black_box(1)))
                .expect("retry send with one injected failure should succeed");
        });
    });

    c.bench_function("logs_forwarder/forward_large_snapshot", |b| {
        b.iter(|| {
            let _ = runtime.block_on(bench_forward_snapshot_datagrams(black_box(128 * 1024)));
        });
    });
}

criterion_group!(logs_forwarder, bench_logs_forwarder);
criterion_main!(logs_forwarder);
