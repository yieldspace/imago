use std::collections::BTreeMap;

use super::{ServiceLogEvent, ServiceLogStream, log_buffer::CompositeLogBuffer};

/// Pushes synthetic log events and returns the resulting tailed bytes/event counts.
pub fn bench_log_buffer_push_and_tail(
    iterations: usize,
    chunk_size: usize,
    max_bytes: usize,
    tail_lines: u32,
) -> (usize, usize) {
    let mut buffer = CompositeLogBuffer::new(max_bytes.max(1));
    let chunk_size = chunk_size.max(1);
    for idx in 0..iterations.max(1) {
        let mut bytes = vec![b'a'; chunk_size];
        if let Some(last) = bytes.last_mut() {
            *last = b'\n';
        }
        if !bytes.is_empty() {
            bytes[0] = b'0' + (idx % 10) as u8;
        }
        buffer.push_event(ServiceLogEvent {
            stream: if idx % 2 == 0 {
                ServiceLogStream::Stdout
            } else {
                ServiceLogStream::Stderr
            },
            bytes,
            timestamp_unix_ms: idx as u64,
        });
    }

    let (tail_bytes, tail_events) = buffer.tail_snapshot(tail_lines);
    (tail_bytes.len(), tail_events.len())
}

fn benchmark_fixture(
    service_count: usize,
) -> (
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    Vec<String>,
) {
    let mut service_to_runner = BTreeMap::new();
    let mut runner_to_service = BTreeMap::new();
    let mut runner_ids = Vec::new();
    let size = service_count.max(1);
    for idx in 0..size {
        let service_name = format!("svc-{idx}");
        let runner_id = format!("runner-{idx}");
        service_to_runner.insert(service_name.clone(), runner_id.clone());
        runner_to_service.insert(runner_id.clone(), service_name);
        runner_ids.push(runner_id);
    }
    (service_to_runner, runner_to_service, runner_ids)
}

/// Bench helper for indexed heartbeat lookup emulation.
pub fn bench_lookup_with_index(service_count: usize, iterations: usize) -> usize {
    let (service_to_runner, runner_to_service, runner_ids) = benchmark_fixture(service_count);
    let mut found = 0usize;
    for idx in 0..iterations.max(1) {
        let runner_id = &runner_ids[idx % runner_ids.len()];
        if let Some(service_name) = runner_to_service.get(runner_id)
            && service_to_runner
                .get(service_name)
                .is_some_and(|stored_runner| stored_runner == runner_id)
        {
            found = found.saturating_add(1);
            continue;
        }
        if service_to_runner
            .values()
            .any(|stored_runner| stored_runner == runner_id)
        {
            found = found.saturating_add(1);
        }
    }
    found
}

/// Bench helper for linear heartbeat lookup emulation.
pub fn bench_lookup_with_linear_scan(service_count: usize, iterations: usize) -> usize {
    let (service_to_runner, _, runner_ids) = benchmark_fixture(service_count);
    let mut found = 0usize;
    for idx in 0..iterations.max(1) {
        let runner_id = &runner_ids[idx % runner_ids.len()];
        if service_to_runner
            .values()
            .any(|stored_runner| stored_runner == runner_id)
        {
            found = found.saturating_add(1);
        }
    }
    found
}
