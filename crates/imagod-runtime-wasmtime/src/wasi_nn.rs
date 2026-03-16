use wasmtime::component::Linker;
use wasmtime_wasi_nn::{
    InMemoryRegistry, Registry, backend,
    wit::{WasiNnCtx, WasiNnView},
};

use imagod_common::ImagodError;

use crate::{WasiState, map_runtime_error};

pub(crate) fn new_context() -> WasiNnCtx {
    WasiNnCtx::new(
        configured_backends(),
        Registry::from(InMemoryRegistry::new()),
    )
}

pub(crate) fn enabled_feature_names() -> &'static [&'static str] {
    &[
        #[cfg(feature = "wasi-nn-cvitek")]
        "wasi-nn-cvitek",
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
    configured_backends()
        .into_iter()
        .map(|backend| backend.encoding().to_string())
        .collect()
}

fn configured_backends() -> Vec<wasmtime_wasi_nn::Backend> {
    #[cfg(not(feature = "wasi-nn-cvitek"))]
    {
        backend::list()
    }

    #[cfg(feature = "wasi-nn-cvitek")]
    {
        let mut backends = backend::list();
        backends.push(imagod_runtime_wasi_nn_cvitek::backend());
        backends
    }
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

#[cfg(test)]
mod tests {
    #[test]
    fn enabled_feature_names_include_cvitek_when_enabled() {
        #[cfg(feature = "wasi-nn-cvitek")]
        assert!(super::enabled_feature_names().contains(&"wasi-nn-cvitek"));
    }

    #[test]
    fn available_backend_names_include_autodetect_when_cvitek_is_enabled() {
        #[cfg(feature = "wasi-nn-cvitek")]
        assert!(
            super::available_backend_names()
                .iter()
                .any(|name| name == "autodetect")
        );
    }
}
