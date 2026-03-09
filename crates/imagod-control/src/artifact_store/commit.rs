use std::path::Path;

use imagod_common::ImagodError;
use imagod_spec::ByteRange;
use sha2::{Digest, Sha256};
use tokio::{fs::OpenOptions, io::AsyncReadExt};

pub(super) async fn digest_file(path: &Path) -> Result<String, ImagodError> {
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .await
        .map_err(|e| super::map_internal(super::STAGE_COMMIT, e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 64];
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| super::map_internal(super::STAGE_COMMIT, e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub(super) fn is_complete(ranges: &[ByteRange], total: u64) -> bool {
    if ranges.len() != 1 {
        return false;
    }
    let first = &ranges[0];
    first.offset == 0 && first.length == total
}

pub(super) fn next_missing_range(ranges: &[ByteRange], total: u64) -> Option<ByteRange> {
    if total == 0 {
        return None;
    }
    if ranges.is_empty() {
        return Some(ByteRange {
            offset: 0,
            length: total,
        });
    }

    let mut cursor = 0u64;
    for range in ranges {
        let start = range.offset;
        let end = range.offset.saturating_add(range.length);
        if cursor < start {
            return Some(range_from_start_end(cursor, start));
        }
        cursor = end;
    }
    if cursor < total {
        return Some(range_from_start_end(cursor, total));
    }
    None
}

pub(super) fn all_missing_ranges(ranges: &[ByteRange], total: u64) -> Vec<ByteRange> {
    if total == 0 {
        return Vec::new();
    }
    if ranges.is_empty() {
        return vec![ByteRange {
            offset: 0,
            length: total,
        }];
    }

    let mut missing = Vec::new();
    let mut cursor = 0u64;
    for range in ranges {
        let start = range.offset;
        let end = range.offset.saturating_add(range.length);
        if cursor < start {
            missing.push(range_from_start_end(cursor, start));
        }
        cursor = cursor.max(end);
        if cursor >= total {
            break;
        }
    }
    if cursor < total {
        missing.push(range_from_start_end(cursor, total));
    }
    missing
}

pub(super) fn merge_range(ranges: &mut Vec<ByteRange>, incoming: ByteRange) {
    ranges.push(incoming);
    ranges.sort_by_key(|r| r.offset);

    let mut merged: Vec<ByteRange> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        match merged.last_mut() {
            Some(last) if range.offset <= last.offset.saturating_add(last.length) => {
                let current_end = range.offset.saturating_add(range.length);
                let merged_end = last.offset.saturating_add(last.length).max(current_end);
                last.length = merged_end.saturating_sub(last.offset);
            }
            _ => merged.push(range),
        }
    }

    *ranges = merged;
}

pub(super) fn range_from_start_end(start: u64, end: u64) -> ByteRange {
    ByteRange {
        offset: start,
        length: end.saturating_sub(start),
    }
}
