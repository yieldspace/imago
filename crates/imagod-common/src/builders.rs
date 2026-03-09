use std::collections::BTreeMap;

use imagod_spec::ErrorCode;

use crate::ImagodError;

pub(crate) fn new_error(
    code: ErrorCode,
    stage: impl Into<String>,
    message: impl Into<String>,
) -> ImagodError {
    ImagodError {
        code,
        stage: stage.into(),
        message: message.into(),
        retryable: false,
        details: BTreeMap::new(),
    }
}

pub(crate) fn insert_detail(
    details: &mut BTreeMap<String, String>,
    key: impl Into<String>,
    value: impl Into<String>,
) {
    details.insert(key.into(), value.into());
}
