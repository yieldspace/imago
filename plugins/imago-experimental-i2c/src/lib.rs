use std::{
    collections::BTreeMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use imago_plugin_macros::imago_native_plugin;
use imagod_runtime_wasmtime::WasiState;
use imagod_runtime_wasmtime::native_plugins::{
    HasSelf, NativePlugin, NativePluginLinker, NativePluginResult, map_native_plugin_linker_error,
};
use wasmtime::component::Resource;

pub mod imago_experimental_i2c_plugin_bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "host",
        imports: {
            default: async,
        },
    });
}

#[derive(Debug, Default)]
#[imago_native_plugin(
    wit = "wit",
    world = "host",
    descriptor_only = true,
    multi_imports = true,
    allow_non_resource_types = true,
    generate_bindings = false
)]
pub struct ImagoExperimentalI2cPlugin;

impl NativePlugin for ImagoExperimentalI2cPlugin {
    fn package_name(&self) -> &'static str {
        Self::PACKAGE_NAME
    }

    fn supports_import(&self, import_name: &str) -> bool {
        Self::IMPORTS.contains(&import_name)
    }

    fn symbols(&self) -> &'static [&'static str] {
        Self::SYMBOLS
    }

    fn supports_symbol(&self, symbol: &str) -> bool {
        Self::IMPORTS.iter().any(|import_name| {
            symbol
                .strip_prefix(import_name)
                .is_some_and(|tail| tail.starts_with('.'))
        })
    }

    fn add_to_linker(&self, linker: &mut NativePluginLinker) -> NativePluginResult<()> {
        imago_experimental_i2c_plugin_bindings::Host_::add_to_linker::<_, HasSelf<_>>(
            linker,
            |state| state,
        )
        .map_err(|err| map_native_plugin_linker_error(Self::PACKAGE_NAME, err))
    }
}

type I2cResource = imago_experimental_i2c_plugin_bindings::imago::experimental_i2c::i2c::I2c;
type DelayResource = imago_experimental_i2c_plugin_bindings::imago::experimental_i2c::delay::Delay;
type I2cErrorCode = imago_experimental_i2c_plugin_bindings::imago::experimental_i2c::i2c::ErrorCode;
#[cfg(target_os = "linux")]
type NoAcknowledgeSource =
    imago_experimental_i2c_plugin_bindings::imago::experimental_i2c::i2c::NoAcknowledgeSource;
type I2cOperation = imago_experimental_i2c_plugin_bindings::imago::experimental_i2c::i2c::Operation;

const DEFAULT_I2C_BUS_ENV: &str = "IMAGO_EXPERIMENTAL_I2C_DEFAULT_BUS";
const DEFAULT_I2C_BUS: &str = "/dev/i2c-1";
const I2C_BUS_PREFIX: &str = "/dev/i2c-";
const MAX_IO_BYTES: usize = 4096;
const MAX_TRANSACTION_OPERATIONS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
struct I2cHandle {
    bus: String,
}

static NEXT_I2C_REP: AtomicU32 = AtomicU32::new(1);
static I2C_REGISTRY: OnceLock<Mutex<BTreeMap<u32, I2cHandle>>> = OnceLock::new();
static NEXT_DELAY_REP: AtomicU32 = AtomicU32::new(1);
static DELAY_REGISTRY: OnceLock<Mutex<BTreeMap<u32, ()>>> = OnceLock::new();

fn i2c_registry() -> &'static Mutex<BTreeMap<u32, I2cHandle>> {
    I2C_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn delay_registry() -> &'static Mutex<BTreeMap<u32, ()>> {
    DELAY_REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn register_i2c_handle(handle: I2cHandle) -> u32 {
    loop {
        let rep = NEXT_I2C_REP.fetch_add(1, Ordering::Relaxed);
        if rep == 0 {
            continue;
        }

        let mut registry = i2c_registry()
            .lock()
            .expect("i2c registry lock should not be poisoned");
        if registry.insert(rep, handle.clone()).is_none() {
            return rep;
        }
    }
}

fn lookup_i2c_handle(rep: u32) -> Result<I2cHandle, String> {
    i2c_registry()
        .lock()
        .map_err(|_| "i2c registry lock poisoned".to_string())?
        .get(&rep)
        .cloned()
        .ok_or_else(|| format!("i2c handle not found: rep={rep}"))
}

fn remove_i2c_handle(rep: u32) {
    if let Ok(mut registry) = i2c_registry().lock() {
        registry.remove(&rep);
    }
}

fn register_delay_handle() -> u32 {
    loop {
        let rep = NEXT_DELAY_REP.fetch_add(1, Ordering::Relaxed);
        if rep == 0 {
            continue;
        }

        let mut registry = delay_registry()
            .lock()
            .expect("delay registry lock should not be poisoned");
        if registry.insert(rep, ()).is_none() {
            return rep;
        }
    }
}

fn has_delay_handle(rep: u32) -> bool {
    delay_registry()
        .lock()
        .map(|registry| registry.contains_key(&rep))
        .unwrap_or(false)
}

fn remove_delay_handle(rep: u32) {
    if let Ok(mut registry) = delay_registry().lock() {
        registry.remove(&rep);
    }
}

fn validate_bus_path(bus: &str) -> Result<String, String> {
    let trimmed = bus.trim();
    if trimmed.is_empty() {
        return Err("i2c bus path must not be empty".to_string());
    }
    if trimmed.contains('\0') {
        return Err("i2c bus path must not contain NUL".to_string());
    }
    if !trimmed.starts_with(I2C_BUS_PREFIX) {
        return Err(format!("i2c bus path must start with '{I2C_BUS_PREFIX}'",));
    }
    let suffix = &trimmed[I2C_BUS_PREFIX.len()..];
    if suffix.is_empty() || !suffix.chars().all(|ch| ch.is_ascii_digit()) {
        return Err("i2c bus path must end with a numeric bus id".to_string());
    }
    Ok(trimmed.to_string())
}

fn resolve_default_bus_path() -> Result<String, String> {
    match std::env::var(DEFAULT_I2C_BUS_ENV) {
        Ok(value) => validate_bus_path(&value),
        Err(_) => Ok(DEFAULT_I2C_BUS.to_string()),
    }
}

fn validate_address(address: u16) -> Result<u8, I2cErrorCode> {
    if address > 0x7f {
        return Err(I2cErrorCode::Other);
    }
    Ok(address as u8)
}

fn validate_io_len(len: u64) -> Result<usize, I2cErrorCode> {
    if len > MAX_IO_BYTES as u64 {
        return Err(I2cErrorCode::Other);
    }
    usize::try_from(len).map_err(|_| I2cErrorCode::Other)
}

fn validate_write_len(len: usize) -> Result<(), I2cErrorCode> {
    if len > MAX_IO_BYTES {
        return Err(I2cErrorCode::Other);
    }
    Ok(())
}

fn validate_transaction_len(len: usize) -> Result<(), I2cErrorCode> {
    if len > MAX_TRANSACTION_OPERATIONS {
        return Err(I2cErrorCode::Other);
    }
    Ok(())
}

fn collect_transaction_read_lengths(
    operations: &[I2cOperation],
) -> Result<Vec<usize>, I2cErrorCode> {
    validate_transaction_len(operations.len())?;

    let mut read_lengths = Vec::new();
    for operation in operations {
        match operation {
            I2cOperation::Read(len) => {
                read_lengths.push(validate_io_len(*len)?);
            }
            I2cOperation::Write(data) => {
                validate_write_len(data.len())?;
            }
        }
    }
    Ok(read_lengths)
}

fn unsupported_i2c_error() -> String {
    "unsupported: i2c native backend is available only on Linux".to_string()
}

fn ensure_i2c_supported() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(unsupported_i2c_error())
    }
}

fn map_missing_resource_to_i2c_error(_err: String) -> I2cErrorCode {
    I2cErrorCode::Other
}

fn map_blocking_join_to_i2c_error(_err: tokio::task::JoinError) -> I2cErrorCode {
    I2cErrorCode::Other
}

fn map_blocking_join_to_string_error(err: tokio::task::JoinError) -> String {
    format!("blocking task failed: {err}")
}

fn validate_read_request(address: u16, len: u64) -> Result<(), I2cErrorCode> {
    let _ = validate_address(address)?;
    let _ = validate_io_len(len)?;
    Ok(())
}

fn validate_write_request(address: u16, data: &[u8]) -> Result<(), I2cErrorCode> {
    let _ = validate_address(address)?;
    validate_write_len(data.len())
}

fn validate_write_read_request(
    address: u16,
    write: &[u8],
    read_len: u64,
) -> Result<(), I2cErrorCode> {
    let _ = validate_address(address)?;
    validate_write_len(write.len())?;
    let _ = validate_io_len(read_len)?;
    Ok(())
}

fn validate_transaction_request(
    address: u16,
    operations: &[I2cOperation],
) -> Result<(), I2cErrorCode> {
    let _ = validate_address(address)?;
    let _ = collect_transaction_read_lengths(operations)?;
    Ok(())
}

async fn run_i2c_blocking<T, F>(operation: F) -> Result<T, I2cErrorCode>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, I2cErrorCode> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(map_blocking_join_to_i2c_error)?
}

async fn run_blocking_string<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(map_blocking_join_to_string_error)?
}

#[cfg(any(test, target_os = "linux"))]
fn map_open_i2c_error(bus: &str, err: impl std::fmt::Display) -> String {
    format!("failed to open i2c bus '{bus}': {err}")
}

#[cfg(any(test, target_os = "linux"))]
fn validate_bus_open_with<F, E>(bus: &str, open_fn: F) -> Result<(), String>
where
    F: FnOnce(&str) -> Result<(), E>,
    E: std::fmt::Display,
{
    open_fn(bus).map_err(|err| map_open_i2c_error(bus, err))
}

#[cfg(target_os = "linux")]
fn validate_bus_open_sync(bus: &str) -> Result<(), String> {
    use linux_embedded_hal::I2cdev;

    validate_bus_open_with(bus, |path| I2cdev::new(path).map(|_| ()))
}

#[cfg(not(target_os = "linux"))]
fn validate_bus_open_sync(_bus: &str) -> Result<(), String> {
    Err(unsupported_i2c_error())
}

#[cfg(target_os = "linux")]
mod linux_backend {
    use embedded_hal::i2c::I2c as _;
    use linux_embedded_hal::I2cdev;

    use super::{
        I2cErrorCode, I2cOperation, NoAcknowledgeSource, collect_transaction_read_lengths,
        validate_address, validate_io_len, validate_write_len,
    };

    fn map_linux_error(err: impl std::fmt::Display) -> I2cErrorCode {
        let lower = err.to_string().to_ascii_lowercase();
        if lower.contains("arbitration") {
            I2cErrorCode::ArbitrationLoss
        } else if lower.contains("overrun") {
            I2cErrorCode::Overrun
        } else if lower.contains("ack") || lower.contains("nack") {
            I2cErrorCode::NoAcknowledge(NoAcknowledgeSource::Unknown)
        } else if lower.contains("bus") {
            I2cErrorCode::Bus
        } else {
            I2cErrorCode::Other
        }
    }

    pub(super) fn read(bus: &str, address: u16, len: u64) -> Result<Vec<u8>, I2cErrorCode> {
        let mut dev = I2cdev::new(bus).map_err(map_linux_error)?;
        let address = validate_address(address)?;
        let len = validate_io_len(len)?;
        if len == 0 {
            return Ok(Vec::new());
        }
        let mut out = vec![0u8; len];
        dev.read(address, &mut out).map_err(map_linux_error)?;
        Ok(out)
    }

    pub(super) fn write(bus: &str, address: u16, data: &[u8]) -> Result<(), I2cErrorCode> {
        let mut dev = I2cdev::new(bus).map_err(map_linux_error)?;
        let address = validate_address(address)?;
        validate_write_len(data.len())?;
        dev.write(address, data).map_err(map_linux_error)
    }

    pub(super) fn write_read(
        bus: &str,
        address: u16,
        write: &[u8],
        read_len: u64,
    ) -> Result<Vec<u8>, I2cErrorCode> {
        let mut dev = I2cdev::new(bus).map_err(map_linux_error)?;
        let address = validate_address(address)?;
        validate_write_len(write.len())?;
        let read_len = validate_io_len(read_len)?;
        if read_len == 0 {
            dev.write(address, write).map_err(map_linux_error)?;
            return Ok(Vec::new());
        }
        let mut out = vec![0u8; read_len];
        dev.write_read(address, write, &mut out)
            .map_err(map_linux_error)?;
        Ok(out)
    }

    pub(super) fn transaction(
        bus: &str,
        address: u16,
        operations: &[I2cOperation],
    ) -> Result<Vec<Vec<u8>>, I2cErrorCode> {
        let mut dev = I2cdev::new(bus).map_err(map_linux_error)?;
        let address = validate_address(address)?;
        let read_lengths = collect_transaction_read_lengths(operations)?;
        let mut read_results = read_lengths
            .into_iter()
            .map(|len| vec![0u8; len])
            .collect::<Vec<_>>();

        {
            let mut read_buffers = read_results
                .iter_mut()
                .map(|buffer| buffer.as_mut_slice())
                .collect::<Vec<_>>();
            let mut read_iter = read_buffers.drain(..);
            let mut hal_operations = Vec::with_capacity(operations.len());

            for operation in operations {
                match operation {
                    I2cOperation::Read(_) => {
                        let read_buffer = read_iter.next().ok_or(I2cErrorCode::Other)?;
                        hal_operations.push(embedded_hal::i2c::Operation::Read(read_buffer));
                    }
                    I2cOperation::Write(data) => {
                        hal_operations.push(embedded_hal::i2c::Operation::Write(data));
                    }
                }
            }

            dev.transaction(address, &mut hal_operations)
                .map_err(map_linux_error)?;
        }

        Ok(read_results)
    }
}

#[cfg(not(target_os = "linux"))]
mod linux_backend {
    use super::{I2cErrorCode, I2cOperation};

    pub(super) fn read(_bus: &str, _address: u16, _len: u64) -> Result<Vec<u8>, I2cErrorCode> {
        Err(I2cErrorCode::Other)
    }

    pub(super) fn write(_bus: &str, _address: u16, _data: &[u8]) -> Result<(), I2cErrorCode> {
        Err(I2cErrorCode::Other)
    }

    pub(super) fn write_read(
        _bus: &str,
        _address: u16,
        _write: &[u8],
        _read_len: u64,
    ) -> Result<Vec<u8>, I2cErrorCode> {
        Err(I2cErrorCode::Other)
    }

    pub(super) fn transaction(
        _bus: &str,
        _address: u16,
        _operations: &[I2cOperation],
    ) -> Result<Vec<Vec<u8>>, I2cErrorCode> {
        Err(I2cErrorCode::Other)
    }
}

impl imago_experimental_i2c_plugin_bindings::imago::experimental_i2c::provider::Host for WasiState {
    async fn open_i2c(&mut self, bus: String) -> Result<Resource<I2cResource>, String> {
        ensure_i2c_supported()?;
        let bus = validate_bus_path(&bus)?;
        let bus_for_validation = bus.clone();
        run_blocking_string(move || validate_bus_open_sync(&bus_for_validation)).await?;
        Ok(Resource::new_own(register_i2c_handle(I2cHandle { bus })))
    }

    async fn open_default_i2c(&mut self) -> Result<Resource<I2cResource>, String> {
        ensure_i2c_supported()?;
        let bus = resolve_default_bus_path()?;
        let bus_for_validation = bus.clone();
        run_blocking_string(move || validate_bus_open_sync(&bus_for_validation)).await?;
        Ok(Resource::new_own(register_i2c_handle(I2cHandle { bus })))
    }

    async fn open_delay(&mut self) -> Resource<DelayResource> {
        Resource::new_own(register_delay_handle())
    }
}

impl imago_experimental_i2c_plugin_bindings::imago::experimental_i2c::i2c::Host for WasiState {}

impl imago_experimental_i2c_plugin_bindings::imago::experimental_i2c::i2c::HostI2c for WasiState {
    async fn transaction(
        &mut self,
        self_: Resource<I2cResource>,
        address: u16,
        operations: Vec<I2cOperation>,
    ) -> Result<Vec<Vec<u8>>, I2cErrorCode> {
        let handle = lookup_i2c_handle(self_.rep()).map_err(map_missing_resource_to_i2c_error)?;
        validate_transaction_request(address, &operations)?;

        let bus = handle.bus;
        run_i2c_blocking(move || linux_backend::transaction(&bus, address, &operations)).await
    }

    async fn read(
        &mut self,
        self_: Resource<I2cResource>,
        address: u16,
        len: u64,
    ) -> Result<Vec<u8>, I2cErrorCode> {
        let handle = lookup_i2c_handle(self_.rep()).map_err(map_missing_resource_to_i2c_error)?;
        validate_read_request(address, len)?;

        let bus = handle.bus;
        run_i2c_blocking(move || linux_backend::read(&bus, address, len)).await
    }

    async fn write(
        &mut self,
        self_: Resource<I2cResource>,
        address: u16,
        data: Vec<u8>,
    ) -> Result<(), I2cErrorCode> {
        let handle = lookup_i2c_handle(self_.rep()).map_err(map_missing_resource_to_i2c_error)?;
        validate_write_request(address, &data)?;

        let bus = handle.bus;
        run_i2c_blocking(move || linux_backend::write(&bus, address, &data)).await
    }

    async fn write_read(
        &mut self,
        self_: Resource<I2cResource>,
        address: u16,
        write: Vec<u8>,
        read_len: u64,
    ) -> Result<Vec<u8>, I2cErrorCode> {
        let handle = lookup_i2c_handle(self_.rep()).map_err(map_missing_resource_to_i2c_error)?;
        validate_write_read_request(address, &write, read_len)?;

        let bus = handle.bus;
        run_i2c_blocking(move || linux_backend::write_read(&bus, address, &write, read_len)).await
    }

    async fn drop(&mut self, resource: Resource<I2cResource>) -> wasmtime::Result<()> {
        remove_i2c_handle(resource.rep());
        Ok(())
    }
}

impl imago_experimental_i2c_plugin_bindings::imago::experimental_i2c::delay::Host for WasiState {}

impl imago_experimental_i2c_plugin_bindings::imago::experimental_i2c::delay::HostDelay
    for WasiState
{
    async fn delay_ns(&mut self, self_: Resource<DelayResource>, ns: u32) {
        if !has_delay_handle(self_.rep()) {
            return;
        }
        tokio::time::sleep(Duration::from_nanos(u64::from(ns))).await;
    }

    async fn drop(&mut self, resource: Resource<DelayResource>) -> wasmtime::Result<()> {
        remove_delay_handle(resource.rep());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn validate_bus_path_accepts_standard_path() {
        let parsed = validate_bus_path("/dev/i2c-1").expect("path should be valid");
        assert_eq!(parsed, "/dev/i2c-1");
    }

    #[test]
    fn validate_bus_path_rejects_invalid_paths() {
        let empty = validate_bus_path(" ").expect_err("empty path must fail");
        assert!(
            empty.contains("must not be empty"),
            "unexpected error: {empty}"
        );

        let wrong_prefix = validate_bus_path("i2c-1").expect_err("wrong prefix must fail");
        assert!(
            wrong_prefix.contains("must start"),
            "unexpected error: {wrong_prefix}"
        );

        let wrong_suffix =
            validate_bus_path("/dev/i2c-one").expect_err("non-numeric suffix must fail");
        assert!(
            wrong_suffix.contains("numeric"),
            "unexpected error: {wrong_suffix}"
        );
    }

    #[test]
    fn default_bus_uses_env_when_present() {
        let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
        unsafe {
            std::env::set_var(DEFAULT_I2C_BUS_ENV, "/dev/i2c-7");
        }
        let bus = resolve_default_bus_path().expect("env value should be valid");
        assert_eq!(bus, "/dev/i2c-7");
        unsafe {
            std::env::remove_var(DEFAULT_I2C_BUS_ENV);
        }
    }

    #[test]
    fn default_bus_falls_back_when_env_is_missing() {
        let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
        unsafe {
            std::env::remove_var(DEFAULT_I2C_BUS_ENV);
        }
        let bus = resolve_default_bus_path().expect("fallback bus should be valid");
        assert_eq!(bus, DEFAULT_I2C_BUS);
    }

    #[test]
    fn default_bus_rejects_invalid_env_value() {
        let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
        unsafe {
            std::env::set_var(DEFAULT_I2C_BUS_ENV, "invalid");
        }
        let err = resolve_default_bus_path().expect_err("invalid env must fail");
        assert!(err.contains("must start"), "unexpected error: {err}");
        unsafe {
            std::env::remove_var(DEFAULT_I2C_BUS_ENV);
        }
    }

    #[test]
    fn i2c_registry_lifecycle_roundtrip() {
        let rep = register_i2c_handle(I2cHandle {
            bus: "/dev/i2c-3".to_string(),
        });
        let looked_up = lookup_i2c_handle(rep).expect("registered handle should be found");
        assert_eq!(looked_up.bus, "/dev/i2c-3");

        remove_i2c_handle(rep);
        let err = lookup_i2c_handle(rep).expect_err("removed handle should not exist");
        assert!(err.contains("not found"), "unexpected error: {err}");
    }

    #[test]
    fn delay_registry_lifecycle_roundtrip() {
        let rep = register_delay_handle();
        assert!(has_delay_handle(rep), "handle should exist after register");
        remove_delay_handle(rep);
        assert!(!has_delay_handle(rep), "handle should be removed");
    }

    #[test]
    fn validates_io_and_transaction_limits() {
        assert_eq!(
            validate_io_len(MAX_IO_BYTES as u64).expect("max len should pass"),
            MAX_IO_BYTES
        );
        assert!(
            matches!(
                validate_io_len((MAX_IO_BYTES + 1) as u64).expect_err("too large len should fail"),
                I2cErrorCode::Other
            ),
            "oversized io length should map to error-code::other"
        );
        assert!(
            validate_write_len(MAX_IO_BYTES).is_ok(),
            "max write length should pass"
        );
        assert!(
            matches!(
                validate_write_len(MAX_IO_BYTES + 1).expect_err("too large write should fail"),
                I2cErrorCode::Other
            ),
            "oversized write length should map to error-code::other"
        );

        assert!(
            validate_transaction_len(MAX_TRANSACTION_OPERATIONS).is_ok(),
            "max operation count should pass"
        );
        assert!(
            matches!(
                validate_transaction_len(MAX_TRANSACTION_OPERATIONS + 1)
                    .expect_err("too many operations should fail"),
                I2cErrorCode::Other
            ),
            "too many operations should map to error-code::other"
        );
    }

    #[test]
    fn collect_transaction_read_lengths_preserves_read_order_in_mixed_operations() {
        let operations = vec![
            I2cOperation::Write(vec![0x10, 0x11]),
            I2cOperation::Read(2),
            I2cOperation::Write(vec![0x12]),
            I2cOperation::Read(4),
        ];
        let lengths =
            collect_transaction_read_lengths(&operations).expect("mixed operations should pass");
        assert_eq!(lengths, vec![2, 4]);
    }

    #[test]
    fn collect_transaction_read_lengths_rejects_oversized_read() {
        let operations = vec![I2cOperation::Read((MAX_IO_BYTES + 1) as u64)];
        assert!(
            matches!(
                collect_transaction_read_lengths(&operations)
                    .expect_err("oversized read should fail"),
                I2cErrorCode::Other
            ),
            "oversized read should map to error-code::other"
        );
    }

    #[test]
    fn validates_transaction_request_rejects_too_many_operations() {
        let operations = vec![I2cOperation::Read(1); MAX_TRANSACTION_OPERATIONS + 1];
        assert!(
            matches!(
                validate_transaction_request(0x20, &operations)
                    .expect_err("too many operations should fail"),
                I2cErrorCode::Other
            ),
            "too many operations should map to error-code::other"
        );
    }

    #[test]
    fn blocking_join_error_maps_to_other() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("tokio runtime must build");

        let err = runtime.block_on(async {
            run_i2c_blocking(|| -> Result<(), I2cErrorCode> {
                panic!("intentional panic for join-error mapping test");
            })
            .await
            .expect_err("join error must map to i2c error")
        });

        assert!(
            matches!(err, I2cErrorCode::Other),
            "join error should map to error-code::other"
        );
    }

    #[test]
    fn validate_bus_open_with_formats_contextual_error() {
        let err = validate_bus_open_with("/dev/i2c-9", |_path| -> Result<(), &str> {
            Err("permission denied")
        })
        .expect_err("open failure should propagate as contextual error");
        assert!(err.contains("/dev/i2c-9"), "unexpected error: {err}");
        assert!(err.contains("permission denied"), "unexpected error: {err}");
    }

    #[test]
    fn validate_bus_open_with_accepts_success() {
        validate_bus_open_with("/dev/i2c-1", |_path| -> Result<(), &str> { Ok(()) })
            .expect("open helper should succeed");
    }

    #[test]
    fn validates_7bit_address_only() {
        assert_eq!(validate_address(0x7f).expect("7-bit max should pass"), 0x7f);
        assert!(
            matches!(
                validate_address(0x80).expect_err("10-bit addresses are currently unsupported"),
                I2cErrorCode::Other
            ),
            "unsupported address should map to error-code::other"
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_reports_i2c_unsupported() {
        let err = ensure_i2c_supported().expect_err("non-linux should be unsupported");
        assert!(err.contains("only on Linux"), "unexpected error: {err}");
    }
}
