use std::path::Path;

use imagod_common::ImagodError;
use tokio::{
    fs::OpenOptions,
    io::{AsyncSeekExt, AsyncWriteExt},
};

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct FileChunkSink;

impl FileChunkSink {
    pub(super) async fn create_preallocated_file(
        &self,
        path: &Path,
        artifact_size: u64,
    ) -> Result<(), ImagodError> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(true)
            .open(path)
            .await
            .map_err(|e| super::map_internal(super::STAGE_PREPARE, e.to_string()))?;
        file.set_len(artifact_size)
            .await
            .map_err(|e| super::map_internal(super::STAGE_PREPARE, e.to_string()))?;
        file.flush()
            .await
            .map_err(|e| super::map_internal(super::STAGE_PREPARE, e.to_string()))
    }

    pub(super) async fn write_chunk_to_file(
        &self,
        path: &Path,
        offset: u64,
        chunk: &[u8],
    ) -> Result<(), ImagodError> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .await
            .map_err(|e| super::map_internal(super::STAGE_PUSH, e.to_string()))?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| super::map_internal(super::STAGE_PUSH, e.to_string()))?;
        file.write_all(chunk)
            .await
            .map_err(|e| super::map_internal(super::STAGE_PUSH, e.to_string()))?;
        file.flush()
            .await
            .map_err(|e| super::map_internal(super::STAGE_PUSH, e.to_string()))
    }
}
