//! `wasi-nn` backend for CVITEK / Milk-V Duo TPU runtimes.

use wasmtime::Error as WasmtimeError;
use wasmtime_wasi_nn::backend::{BackendError, BackendInner};
use wasmtime_wasi_nn::wit::{ExecutionTarget, GraphEncoding};
use wasmtime_wasi_nn::{Backend, Graph};

const SUPPORTED_TARGET_MESSAGE: &str =
    "wasi-nn-cvitek accepts only graph.load(..., autodetect, tpu)";
const SUPPORTED_PLATFORM_MESSAGE: &str =
    "wasi-nn-cvitek backend is available only on Linux riscv64 targets";
pub fn backend() -> Backend {
    Backend::from(CvitekBackend)
}

#[derive(Default)]
pub struct CvitekBackend;

impl BackendInner for CvitekBackend {
    fn encoding(&self) -> GraphEncoding {
        GraphEncoding::Autodetect
    }

    fn load(&mut self, builders: &[&[u8]], target: ExecutionTarget) -> Result<Graph, BackendError> {
        if builders.len() != 1 {
            return Err(BackendError::InvalidNumberOfBuilders(1, builders.len()));
        }
        if target != ExecutionTarget::Tpu {
            return Err(runtime_error(format!(
                "{SUPPORTED_TARGET_MESSAGE}; got target {target:?}"
            )));
        }
        imp::load_graph(builders[0])
    }

    fn as_dir_loadable(&mut self) -> Option<&mut dyn wasmtime_wasi_nn::backend::BackendFromDir> {
        None
    }
}

fn runtime_error(message: impl Into<String>) -> BackendError {
    BackendError::BackendAccess(WasmtimeError::msg(message.into()))
}

#[cfg(not(all(target_os = "linux", target_arch = "riscv64")))]
mod imp {
    use super::*;

    pub(super) fn load_graph(_builder: &[u8]) -> Result<Graph, BackendError> {
        Err(runtime_error(SUPPORTED_PLATFORM_MESSAGE))
    }
}

#[cfg(all(target_os = "linux", target_arch = "riscv64"))]
mod imp {
    use super::*;
    use std::ffi::{CStr, c_char, c_void};
    use std::ptr;
    use std::sync::{Arc, Mutex};
    use wasmtime_wasi_nn::backend::{BackendExecutionContext, BackendGraph, Id, NamedTensor};
    use wasmtime_wasi_nn::wit::TensorType;
    use wasmtime_wasi_nn::{ExecutionContext, Tensor};

    const AUTODETECT_MODEL_MESSAGE: &str = "wasi-nn-cvitek autodetect accepts only .cvimodel bytes";

    const CVI_FMT_FP32: i32 = 0;
    const CVI_FMT_INT32: i32 = 1;
    const CVI_FMT_UINT32: i32 = 2;
    const CVI_FMT_BF16: i32 = 3;
    const CVI_FMT_INT16: i32 = 4;
    const CVI_FMT_UINT16: i32 = 5;
    const CVI_FMT_INT8: i32 = 6;
    const CVI_FMT_UINT8: i32 = 7;
    const CVI_MEM_SYSTEM: i32 = 1;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TensorInfo {
        name: String,
        dimensions: Vec<u32>,
        ty: TensorType,
        byte_len: usize,
    }

    fn find_slot(id: &Id, slots: &[TensorInfo]) -> Result<usize, BackendError> {
        match id {
            Id::Index(index) => {
                let index = usize::try_from(*index).map_err(|_| {
                    runtime_error(format!("tensor index {index} does not fit usize"))
                })?;
                if index < slots.len() {
                    Ok(index)
                } else {
                    Err(runtime_error(format!(
                        "tensor index {index} is out of range for {} slots",
                        slots.len()
                    )))
                }
            }
            Id::Name(name) => slots
                .iter()
                .position(|slot| slot.name == *name)
                .ok_or_else(|| runtime_error(format!("tensor '{name}' was not found"))),
        }
    }

    fn validate_tensor(expected: &TensorInfo, tensor: &Tensor) -> Result<(), BackendError> {
        if tensor.ty != expected.ty {
            return Err(runtime_error(format!(
                "tensor '{}' expects type {:?}, got {:?}",
                expected.name, expected.ty, tensor.ty
            )));
        }
        if tensor.dimensions != expected.dimensions {
            return Err(runtime_error(format!(
                "tensor '{}' expects dimensions {:?}, got {:?}",
                expected.name, expected.dimensions, tensor.dimensions
            )));
        }
        if tensor.data.len() != expected.byte_len {
            return Err(runtime_error(format!(
                "tensor '{}' expects {} bytes, got {}",
                expected.name,
                expected.byte_len,
                tensor.data.len()
            )));
        }
        Ok(())
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CviShape {
        dim: [i32; 6],
        dim_size: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CviTensor {
        name: *mut c_char,
        shape: CviShape,
        fmt: i32,
        count: usize,
        mem_size: usize,
        sys_mem: *mut u8,
        paddr: u64,
        mem_type: i32,
        qscale: f32,
        zero_point: i32,
        pixel_format: i32,
        aligned: bool,
        mean: [f32; 3],
        scale: [f32; 3],
        owner: *mut c_void,
        reserved: [c_char; 32],
    }

    type CviModelHandle = *mut c_void;
    type CviRc = i32;

    unsafe extern "C" {
        fn CVI_NN_RegisterModelFromBuffer(
            buf: *const i8,
            size: u32,
            model: *mut CviModelHandle,
        ) -> CviRc;
        fn CVI_NN_CloneModel(model: CviModelHandle, cloned: *mut CviModelHandle) -> CviRc;
        fn CVI_NN_GetInputOutputTensors(
            model: CviModelHandle,
            inputs: *mut *mut CviTensor,
            input_num: *mut i32,
            outputs: *mut *mut CviTensor,
            output_num: *mut i32,
        ) -> CviRc;
        fn CVI_NN_Forward(
            model: CviModelHandle,
            inputs: *mut CviTensor,
            input_num: i32,
            outputs: *mut CviTensor,
            output_num: i32,
        ) -> CviRc;
        fn CVI_NN_CleanupModel(model: CviModelHandle) -> CviRc;
        fn CVI_NN_SetTensorPtr(tensor: *mut CviTensor, mem: *mut c_void) -> CviRc;
    }

    struct ModelHandle(CviModelHandle);
    unsafe impl Send for ModelHandle {}
    unsafe impl Sync for ModelHandle {}

    impl ModelHandle {
        fn register_from_buffer(builder: &[u8]) -> Result<Self, BackendError> {
            if builder.is_empty() {
                return Err(runtime_error("cvimodel builder must not be empty"));
            }
            let size = u32::try_from(builder.len())
                .map_err(|_| runtime_error("cvimodel builder is too large"))?;
            let mut model = ptr::null_mut();
            let rc = unsafe {
                CVI_NN_RegisterModelFromBuffer(builder.as_ptr().cast::<i8>(), size, &mut model)
            };
            check_rc(
                rc,
                format!("failed to register model from buffer; {AUTODETECT_MODEL_MESSAGE}"),
            )?;
            if model.is_null() {
                return Err(runtime_error("runtime registered a null cvimodel handle"));
            }
            Ok(Self(model))
        }

        fn clone_model(&self) -> Result<Self, BackendError> {
            let mut cloned = ptr::null_mut();
            let rc = unsafe { CVI_NN_CloneModel(self.0, &mut cloned) };
            check_rc(rc, "failed to clone cvimodel")?;
            if cloned.is_null() {
                return Err(runtime_error("runtime cloned a null cvimodel handle"));
            }
            Ok(Self(cloned))
        }

        fn io_tensors(&self) -> Result<(Vec<CviTensor>, Vec<CviTensor>), BackendError> {
            let mut inputs = ptr::null_mut();
            let mut outputs = ptr::null_mut();
            let mut input_num = 0;
            let mut output_num = 0;
            let rc = unsafe {
                CVI_NN_GetInputOutputTensors(
                    self.0,
                    &mut inputs,
                    &mut input_num,
                    &mut outputs,
                    &mut output_num,
                )
            };
            check_rc(rc, "failed to fetch cvimodel input/output tensors")?;
            let input_num = usize::try_from(input_num)
                .map_err(|_| runtime_error("negative input tensor count"))?;
            let output_num = usize::try_from(output_num)
                .map_err(|_| runtime_error("negative output tensor count"))?;
            if inputs.is_null() && input_num != 0 {
                return Err(runtime_error("runtime returned null input tensor array"));
            }
            if outputs.is_null() && output_num != 0 {
                return Err(runtime_error("runtime returned null output tensor array"));
            }
            let inputs = if input_num == 0 {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(inputs, input_num) }.to_vec()
            };
            let outputs = if output_num == 0 {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(outputs, output_num) }.to_vec()
            };
            Ok((inputs, outputs))
        }
    }

    impl Drop for ModelHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                let _ = unsafe { CVI_NN_CleanupModel(self.0) };
            }
        }
    }

    struct SharedModel {
        model: ModelHandle,
        inputs: Vec<TensorInfo>,
        outputs: Vec<TensorInfo>,
        clone_lock: Mutex<()>,
    }
    unsafe impl Send for SharedModel {}
    unsafe impl Sync for SharedModel {}

    struct CvitekGraph(Arc<SharedModel>);
    unsafe impl Send for CvitekGraph {}
    unsafe impl Sync for CvitekGraph {}

    impl BackendGraph for CvitekGraph {
        fn init_execution_context(&self) -> Result<ExecutionContext, BackendError> {
            let _guard = self.0.clone_lock.lock().unwrap();
            let model = self.0.model.clone_model()?;
            let (raw_inputs, raw_outputs) = model.io_tensors()?;
            if raw_inputs.len() != self.0.inputs.len() || raw_outputs.len() != self.0.outputs.len()
            {
                return Err(runtime_error(
                    "cloned cvimodel tensor metadata does not match template",
                ));
            }
            let context: Box<dyn BackendExecutionContext> = Box::new(CvitekExecutionContext {
                model,
                inputs_meta: self.0.inputs.clone(),
                outputs_meta: self.0.outputs.clone(),
                raw_inputs,
                raw_outputs,
                inputs: vec![None; self.0.inputs.len()],
                outputs: vec![None; self.0.outputs.len()],
            });
            Ok(context.into())
        }
    }

    struct CvitekExecutionContext {
        model: ModelHandle,
        inputs_meta: Vec<TensorInfo>,
        outputs_meta: Vec<TensorInfo>,
        raw_inputs: Vec<CviTensor>,
        raw_outputs: Vec<CviTensor>,
        inputs: Vec<Option<Tensor>>,
        outputs: Vec<Option<Tensor>>,
    }
    unsafe impl Send for CvitekExecutionContext {}
    unsafe impl Sync for CvitekExecutionContext {}

    impl BackendExecutionContext for CvitekExecutionContext {
        fn set_input(&mut self, id: Id, tensor: &Tensor) -> Result<(), BackendError> {
            let index = find_slot(&id, &self.inputs_meta)?;
            validate_tensor(&self.inputs_meta[index], tensor)?;
            self.inputs[index] = Some(tensor.clone());
            Ok(())
        }

        fn get_output(&mut self, id: Id) -> Result<Tensor, BackendError> {
            let index = find_slot(&id, &self.outputs_meta)?;
            self.outputs[index].clone().ok_or_else(|| {
                runtime_error(format!(
                    "missing output tensor '{}'; call compute first",
                    self.outputs_meta[index].name
                ))
            })
        }

        fn compute(
            &mut self,
            inputs: Option<Vec<NamedTensor>>,
        ) -> Result<Option<Vec<NamedTensor>>, BackendError> {
            let return_named_outputs = inputs.is_some();
            if let Some(inputs) = inputs {
                for slot in &mut self.inputs {
                    *slot = None;
                }
                for input in inputs {
                    self.set_input(Id::Name(input.name), &input.tensor)?;
                }
            }

            for (index, tensor) in self.inputs.iter().enumerate() {
                if tensor.is_none() {
                    return Err(runtime_error(format!(
                        "missing input tensor '{}'",
                        self.inputs_meta[index].name
                    )));
                }
            }

            let mut raw_inputs = self.raw_inputs.clone();
            let mut raw_outputs = self.raw_outputs.clone();
            let mut input_buffers = Vec::with_capacity(self.inputs.len());
            let mut output_buffers = Vec::with_capacity(self.outputs_meta.len());

            for (index, tensor) in self.inputs.iter().enumerate() {
                let mut buffer = tensor
                    .as_ref()
                    .expect("input presence checked above")
                    .data
                    .clone();
                raw_inputs[index].mem_type = CVI_MEM_SYSTEM;
                check_rc(
                    unsafe {
                        CVI_NN_SetTensorPtr(
                            &mut raw_inputs[index],
                            buffer.as_mut_ptr().cast::<c_void>(),
                        )
                    },
                    format!(
                        "failed to bind input tensor '{}'",
                        self.inputs_meta[index].name
                    ),
                )?;
                input_buffers.push(buffer);
            }

            for (index, meta) in self.outputs_meta.iter().enumerate() {
                let mut buffer = vec![0u8; meta.byte_len];
                raw_outputs[index].mem_type = CVI_MEM_SYSTEM;
                check_rc(
                    unsafe {
                        CVI_NN_SetTensorPtr(
                            &mut raw_outputs[index],
                            buffer.as_mut_ptr().cast::<c_void>(),
                        )
                    },
                    format!("failed to bind output tensor '{}'", meta.name),
                )?;
                output_buffers.push(buffer);
            }

            check_rc(
                unsafe {
                    CVI_NN_Forward(
                        self.model.0,
                        raw_inputs.as_mut_ptr(),
                        raw_inputs.len() as i32,
                        raw_outputs.as_mut_ptr(),
                        raw_outputs.len() as i32,
                    )
                },
                "failed to execute cvimodel on TPU",
            )?;

            let mut named_outputs = Vec::with_capacity(self.outputs_meta.len());
            for (index, meta) in self.outputs_meta.iter().enumerate() {
                let tensor = Tensor::new(
                    meta.dimensions.clone(),
                    meta.ty,
                    output_buffers[index].clone(),
                );
                self.outputs[index] = Some(tensor.clone());
                named_outputs.push(NamedTensor {
                    name: meta.name.clone(),
                    tensor,
                });
            }

            if return_named_outputs {
                Ok(Some(named_outputs))
            } else {
                Ok(None)
            }
        }
    }

    pub(super) fn load_graph(builder: &[u8]) -> Result<Graph, BackendError> {
        let model = ModelHandle::register_from_buffer(builder)?;
        let (inputs, outputs) = model.io_tensors()?;
        let inputs = tensor_infos(&inputs)?;
        let outputs = tensor_infos(&outputs)?;
        let graph: Box<dyn BackendGraph> = Box::new(CvitekGraph(Arc::new(SharedModel {
            model,
            inputs,
            outputs,
            clone_lock: Mutex::new(()),
        })));
        Ok(graph.into())
    }

    fn tensor_infos(tensors: &[CviTensor]) -> Result<Vec<TensorInfo>, BackendError> {
        tensors
            .iter()
            .enumerate()
            .map(|(index, tensor)| tensor_info(index, tensor))
            .collect()
    }

    fn tensor_info(index: usize, tensor: &CviTensor) -> Result<TensorInfo, BackendError> {
        let name = if tensor.name.is_null() {
            format!("tensor-{index}")
        } else {
            unsafe { CStr::from_ptr(tensor.name) }
                .to_string_lossy()
                .into_owned()
        };
        let dimensions = shape_to_dimensions(&tensor.shape)?;
        let ty = fmt_to_tensor_type(tensor.fmt)?;
        Ok(TensorInfo {
            name,
            dimensions,
            ty,
            byte_len: tensor.mem_size,
        })
    }

    fn shape_to_dimensions(shape: &CviShape) -> Result<Vec<u32>, BackendError> {
        let dim_size = shape.dim_size;
        if dim_size > shape.dim.len() {
            return Err(runtime_error(format!(
                "tensor rank {dim_size} exceeds CVI_DIM_MAX"
            )));
        }
        let mut dimensions = Vec::with_capacity(dim_size);
        for dim in shape.dim.iter().take(dim_size) {
            let dim = u32::try_from(*dim)
                .map_err(|_| runtime_error(format!("tensor dimension {dim} is negative")))?;
            dimensions.push(dim);
        }
        Ok(dimensions)
    }

    fn fmt_to_tensor_type(fmt: i32) -> Result<TensorType, BackendError> {
        match fmt {
            CVI_FMT_FP32 => Ok(TensorType::Fp32),
            CVI_FMT_INT32 => Ok(TensorType::I32),
            CVI_FMT_BF16 => Ok(TensorType::Bf16),
            CVI_FMT_INT8 | CVI_FMT_UINT8 => Ok(TensorType::U8),
            CVI_FMT_UINT32 | CVI_FMT_INT16 | CVI_FMT_UINT16 => Err(runtime_error(format!(
                "unsupported CVI tensor format {fmt}; wasi-nn cannot represent this integer tensor type"
            ))),
            _ => Err(runtime_error(format!(
                "unsupported CVI tensor format {fmt}"
            ))),
        }
    }

    fn check_rc(rc: CviRc, context: impl Into<String>) -> Result<(), BackendError> {
        if rc == 0 {
            Ok(())
        } else {
            Err(runtime_error(format!("{} (rc={rc})", context.into())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime_wasi_nn::backend::BackendInner;

    #[test]
    fn backend_reports_autodetect_encoding() {
        let backend = CvitekBackend;
        assert_eq!(backend.encoding(), GraphEncoding::Autodetect);
    }

    #[test]
    fn load_rejects_non_tpu_targets() {
        let mut backend = CvitekBackend;
        let err = match backend.load(&[&[0x43, 0x56, 0x49]], ExecutionTarget::Cpu) {
            Ok(_) => panic!("non-tpu targets must fail"),
            Err(err) => err,
        };
        match err {
            BackendError::BackendAccess(inner) => {
                assert!(inner.to_string().contains(SUPPORTED_TARGET_MESSAGE));
            }
            other => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn load_rejects_wrong_builder_count() {
        let mut backend = CvitekBackend;
        let err = match backend.load(&[&[0x01], &[0x02]], ExecutionTarget::Tpu) {
            Ok(_) => panic!("multiple builders must fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("expects 1 buffers"));
    }
}
