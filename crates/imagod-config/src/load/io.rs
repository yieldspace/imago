use std::path::Path;

use imagod_common::ImagodError;
use imagod_spec::ErrorCode;

pub(crate) fn read_to_string(path: &Path) -> Result<String, ImagodError> {
    std::fs::read_to_string(path).map_err(|e| {
        ImagodError::new(
            ErrorCode::BadRequest,
            "config.load",
            format!("config read failed: {e}"),
        )
        .with_detail("path", path.to_string_lossy())
    })
}
