use wasmtime::component::Linker;
use wasmtime_wasi_nn::{
    InMemoryRegistry, Registry, backend,
    wit::{WasiNnCtx, WasiNnView},
};

use imagod_common::ImagodError;

use crate::{WasiState, map_runtime_error};

pub(crate) fn new_context() -> WasiNnCtx {
    WasiNnCtx::new(backend::list(), Registry::from(InMemoryRegistry::new()))
}

pub(crate) fn enabled_feature_names() -> &'static [&'static str] {
    &[
        #[cfg(feature = "wasi-nn-openvino")]
        "wasi-nn-openvino",
        #[cfg(feature = "wasi-nn-onnx")]
        "wasi-nn-onnx",
    ]
}

pub(crate) fn has_enabled_feature() -> bool {
    !enabled_feature_names().is_empty()
}

pub(crate) fn available_backend_names() -> Vec<String> {
    backend::list()
        .into_iter()
        .map(|backend| backend.encoding().to_string())
        .collect()
}

pub(crate) fn add_to_linker(linker: &mut Linker<WasiState>) -> Result<(), ImagodError> {
    if !has_enabled_feature() {
        return Ok(());
    }

    wasmtime_wasi_nn::wit::add_to_linker(linker, |state| {
        WasiNnView::new(&mut state.table, &mut state.wasi_nn)
    })
    .map_err(|err| map_runtime_error(format!("failed to add WASI NN linker: {err}")))?;
    Ok(())
}
