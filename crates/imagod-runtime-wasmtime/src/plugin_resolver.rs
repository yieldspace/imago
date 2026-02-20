use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use imagod_common::ImagodError;
use imagod_ipc::{CapabilityPolicy, PluginDependency, PluginKind, ServiceBinding};
use imagod_runtime_internal::{
    CapabilityChecker, PluginComponentInterfaces, PluginImportProvider, PluginResolver,
};
use wasmtime::{
    Engine, Store,
    component::{Component, Func, Linker, Val, types},
};
use wasmtime_wasi::p2::add_to_linker_async;
use wasmtime_wasi_http::add_only_http_to_linker_async;

use crate::{
    WasiState,
    capability_checker::enforce_wasi_import_capabilities,
    map_runtime_error,
    native_plugins::NativePluginRegistry,
    rpc_bridge,
    rpc_values::{decode_payload_values, encode_payload_values},
};

#[derive(Clone)]
pub(crate) struct AvailablePlugin {
    pub(crate) kind: PluginKind,
    pub(crate) instance: Option<wasmtime::component::Instance>,
}

#[derive(Default)]
pub(crate) struct DefaultPluginResolver;

impl PluginResolver for DefaultPluginResolver {
    fn all_dependency_names(&self, dependencies: &[PluginDependency]) -> BTreeSet<String> {
        dependencies.iter().map(|dep| dep.name.clone()).collect()
    }

    fn dependency_topological_order(
        &self,
        dependencies: &[PluginDependency],
        component_interfaces_by_dependency: &BTreeMap<String, PluginComponentInterfaces>,
    ) -> Result<Vec<usize>, ImagodError> {
        let mut by_name = BTreeMap::<String, usize>::new();
        for (index, dep) in dependencies.iter().enumerate() {
            if by_name.insert(dep.name.clone(), index).is_some() {
                return Err(map_runtime_error(format!(
                    "duplicate plugin dependency name '{}'",
                    dep.name
                )));
            }
        }

        let mut indegree = vec![0usize; dependencies.len()];
        let mut edges = vec![BTreeSet::<usize>::new(); dependencies.len()];
        for (index, dep) in dependencies.iter().enumerate() {
            for req in &dep.requires {
                let req_index = by_name.get(req).ok_or_else(|| {
                    map_runtime_error(format!(
                        "plugin dependency '{}' requires unknown dependency '{}'",
                        dep.name, req
                    ))
                })?;
                add_dependency_edge(&mut edges, &mut indegree, *req_index, index);
            }

            let Some(interfaces) = component_interfaces_by_dependency.get(&dep.name) else {
                continue;
            };
            let implicit_provider_indices = collect_implicit_provider_indices(
                &dep.name,
                &interfaces.imports,
                &interfaces.exports,
                &by_name,
            );
            for provider_index in implicit_provider_indices {
                add_dependency_edge(&mut edges, &mut indegree, provider_index, index);
            }
        }

        let mut ready = dependencies
            .iter()
            .enumerate()
            .filter_map(|(idx, _)| (indegree[idx] == 0).then_some(idx))
            .collect::<Vec<_>>();
        ready.sort_unstable_by(|a, b| dependencies[*a].name.cmp(&dependencies[*b].name));

        let mut order = Vec::with_capacity(dependencies.len());
        while let Some(next) = ready.pop() {
            order.push(next);
            for edge in &edges[next] {
                indegree[*edge] = indegree[*edge].saturating_sub(1);
                if indegree[*edge] == 0 {
                    ready.push(*edge);
                }
            }
            ready.sort_unstable_by(|a, b| dependencies[*a].name.cmp(&dependencies[*b].name));
        }

        if order.len() != dependencies.len() {
            return Err(map_runtime_error(
                "plugin dependency graph contains a cycle".to_string(),
            ));
        }
        Ok(order)
    }

    fn resolve_import_provider(
        &self,
        caller_name: &str,
        import_name: &str,
        self_export_interfaces: &BTreeSet<String>,
        explicit_dependency_names: &BTreeSet<String>,
        available_dependency_names: &BTreeSet<String>,
        allow_self_provider: bool,
    ) -> Result<PluginImportProvider, ImagodError> {
        if allow_self_provider && self_export_interfaces.contains(import_name) {
            return Ok(PluginImportProvider::SelfComponent);
        }

        if let Some(explicit_dep) = parse_import_package_name(import_name)
            .filter(|name| explicit_dependency_names.contains(*name))
        {
            if available_dependency_names.contains(explicit_dep) {
                return Ok(PluginImportProvider::Dependency(explicit_dep.to_string()));
            }
            return Err(map_runtime_error(format!(
                "failed to resolve plugin import provider for caller='{}', import='{}': checked=self, explicit_dep='{}' exists but is not instantiated yet",
                caller_name, import_name, explicit_dep
            )));
        }

        let explicit_dep_label = parse_import_package_name(import_name).unwrap_or("<none>");
        Err(map_runtime_error(format!(
            "failed to resolve plugin import provider for caller='{}', import='{}': checked=self, explicit_dep='{}', result=not-found",
            caller_name, import_name, explicit_dep_label
        )))
    }
}

pub(crate) async fn instantiate_plugin_dependencies<R, C>(
    resolver: &R,
    capability_checker: &C,
    native_plugins: &NativePluginRegistry,
    engine: &Engine,
    store: &mut Store<WasiState>,
    dependencies: &[PluginDependency],
) -> Result<BTreeMap<String, AvailablePlugin>, ImagodError>
where
    R: PluginResolver,
    C: CapabilityChecker,
{
    let mut loaded_components = BTreeMap::<String, Component>::new();
    let mut component_interfaces = BTreeMap::<String, PluginComponentInterfaces>::new();
    for dep in dependencies {
        if dep.kind != PluginKind::Wasm {
            continue;
        }
        let component = dep.component.as_ref().ok_or_else(|| {
            map_runtime_error(format!(
                "plugin dependency '{}' missing component definition",
                dep.name
            ))
        })?;
        let loaded = Component::from_file(engine, &component.path).map_err(|e| {
            map_runtime_error(format!(
                "failed to load plugin component '{}' from {}: {e}",
                dep.name,
                component.path.display()
            ))
        })?;
        let interfaces = PluginComponentInterfaces {
            imports: collect_component_instance_import_names(&loaded, engine),
            exports: collect_component_instance_export_names(&loaded, engine),
        };
        component_interfaces.insert(dep.name.clone(), interfaces);
        loaded_components.insert(dep.name.clone(), loaded);
    }

    let order = resolver.dependency_topological_order(dependencies, &component_interfaces)?;
    let explicit_dependency_names = resolver.all_dependency_names(dependencies);
    let mut available = BTreeMap::<String, AvailablePlugin>::new();

    for dep_index in order {
        let dep = &dependencies[dep_index];
        match dep.kind {
            PluginKind::Native => {
                if !native_plugins.has_plugin(&dep.name) {
                    return Err(map_runtime_error(format!(
                        "native plugin '{}' is declared in manifest but not registered in runtime",
                        dep.name
                    )));
                }
                available.insert(
                    dep.name.clone(),
                    AvailablePlugin {
                        kind: PluginKind::Native,
                        instance: None,
                    },
                );
            }
            PluginKind::Wasm => {
                let component = loaded_components.get(&dep.name).ok_or_else(|| {
                    map_runtime_error(format!(
                        "internal error: loaded component for dependency '{}' is missing",
                        dep.name
                    ))
                })?;

                let mut linker = Linker::new(engine);
                add_to_linker_async(&mut linker).map_err(|e| {
                    map_runtime_error(format!(
                        "failed to add WASI linker for plugin '{}': {e}",
                        dep.name
                    ))
                })?;
                add_only_http_to_linker_async(&mut linker).map_err(|e| {
                    map_runtime_error(format!(
                        "failed to add WASI HTTP linker for plugin '{}': {e}",
                        dep.name
                    ))
                })?;

                let self_instance = Arc::new(tokio::sync::Mutex::new(None));
                register_plugin_import_shims(
                    resolver,
                    capability_checker,
                    native_plugins,
                    engine,
                    &mut linker,
                    store,
                    component,
                    &dep.name,
                    &explicit_dependency_names,
                    &dep.capabilities,
                    &[],
                    &available,
                    Some(self_instance.clone()),
                )?;

                let instance = linker
                    .instantiate_async(&mut *store, component)
                    .await
                    .map_err(|e| {
                        map_runtime_error(format!(
                            "failed to instantiate plugin component '{}': {e}",
                            dep.name
                        ))
                    })?;
                {
                    let mut guard = self_instance.lock().await;
                    *guard = Some(instance);
                }
                available.insert(
                    dep.name.clone(),
                    AvailablePlugin {
                        kind: PluginKind::Wasm,
                        instance: Some(instance),
                    },
                );
            }
        }
    }

    Ok(available)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn register_plugin_import_shims(
    resolver: &impl PluginResolver,
    capability_checker: &impl CapabilityChecker,
    native_plugins: &NativePluginRegistry,
    engine: &Engine,
    linker: &mut Linker<WasiState>,
    store: &mut Store<WasiState>,
    component: &Component,
    caller_name: &str,
    explicit_dependency_names: &BTreeSet<String>,
    capabilities: &CapabilityPolicy,
    bindings: &[ServiceBinding],
    available_plugins: &BTreeMap<String, AvailablePlugin>,
    self_instance: Option<Arc<tokio::sync::Mutex<Option<wasmtime::component::Instance>>>>,
) -> Result<(), ImagodError> {
    let component_ty = component.component_type();
    let self_export_interfaces = collect_component_instance_export_names(component, engine);
    let allow_self_provider = self_instance.is_some();
    let mut linked_native_imports = BTreeSet::<String>::new();
    let available_dependency_names = available_plugins.keys().cloned().collect::<BTreeSet<_>>();
    let binding_targets = build_binding_target_map(bindings)?;

    for (import_name, import_item) in component_ty.imports(engine) {
        let types::ComponentItem::ComponentInstance(instance_ty) = import_item else {
            continue;
        };
        if import_name.starts_with("wasi:") {
            enforce_wasi_import_capabilities(
                capability_checker,
                caller_name,
                capabilities,
                import_name,
                &instance_ty,
                engine,
            )?;
            continue;
        }

        if let Some((target_service, interface_id)) =
            resolve_binding_target_for_import(&binding_targets, import_name)
        {
            register_binding_import_shims(
                engine,
                linker,
                import_name,
                &instance_ty,
                target_service,
                interface_id,
            )?;
            continue;
        }

        let provider = resolver.resolve_import_provider(
            caller_name,
            import_name,
            &self_export_interfaces,
            explicit_dependency_names,
            &available_dependency_names,
            allow_self_provider,
        )?;

        let native_dependency = match &provider {
            PluginImportProvider::Dependency(target_dependency) => {
                available_plugins.get(target_dependency).and_then(|plugin| {
                    (plugin.kind == PluginKind::Native).then_some(target_dependency.as_str())
                })
            }
            PluginImportProvider::SelfComponent => None,
        };

        if let Some(target_dependency) = native_dependency {
            let native_plugin = native_plugins.plugin(target_dependency).ok_or_else(|| {
                map_runtime_error(format!(
                    "native plugin '{}' is not registered in runtime registry",
                    target_dependency
                ))
            })?;
            if !native_plugin.supports_import(import_name) {
                return Err(map_runtime_error(format!(
                    "native plugin '{}' does not support import '{}'",
                    target_dependency, import_name
                )));
            }
            for (func_name, item) in instance_ty.exports(engine) {
                let types::ComponentItem::ComponentFunc(_) = item else {
                    continue;
                };
                capability_checker.ensure_dependency_function_allowed(
                    caller_name,
                    capabilities,
                    target_dependency,
                    import_name,
                    func_name,
                )?;
                let native_symbol = format!("{import_name}.{func_name}");
                if !native_plugin.supports_symbol(&native_symbol) {
                    return Err(map_runtime_error(format!(
                        "native plugin '{}' does not expose symbol '{}'",
                        target_dependency, native_symbol
                    )));
                }
            }
            if linked_native_imports.insert(import_name.to_string()) {
                native_plugin.add_to_linker(linker)?;
            }
            continue;
        }

        let mut import_instance = linker.instance(import_name).map_err(|e| {
            map_runtime_error(format!(
                "failed to define plugin import namespace '{}': {e}",
                import_name
            ))
        })?;

        for (func_name, item) in instance_ty.exports(engine) {
            let types::ComponentItem::ComponentFunc(import_ty) = item else {
                continue;
            };

            match &provider {
                PluginImportProvider::SelfComponent => {
                    let self_instance = self_instance.clone().ok_or_else(|| {
                        map_runtime_error(format!(
                            "self provider is unavailable for caller='{}', import='{}'",
                            caller_name, import_name
                        ))
                    })?;
                    let self_export_ty =
                        resolve_component_export_type(component, engine, import_name, func_name)?;
                    ensure_component_signatures_match(
                        &import_ty,
                        &self_export_ty,
                        import_name,
                        func_name,
                    )?;

                    let interface_name = import_name.to_string();
                    let function_name = func_name.to_string();
                    import_instance
                        .func_new_async(func_name, move |mut store, _ty, params, results| {
                            let self_instance = self_instance.clone();
                            let interface_name = interface_name.clone();
                            let function_name = function_name.clone();
                            Box::new(async move {
                                let instance = {
                                    let guard = self_instance.lock().await;
                                    guard.as_ref().cloned()
                                }
                                .ok_or_else(|| {
                                    wasmtime::Error::msg(format!(
                                        "self provider instance is not ready for '{}.{}'",
                                        interface_name, function_name
                                    ))
                                })?;
                                let callee = resolve_wasm_export_from_instance(
                                    &mut store,
                                    &instance,
                                    &interface_name,
                                    &function_name,
                                )
                                .map_err(|err| wasmtime::Error::msg(err.to_string()))?;
                                callee.call_async(&mut store, params, results).await?;
                                callee.post_return_async(&mut store).await?;
                                Ok(())
                            })
                        })
                        .map_err(|e| {
                            map_runtime_error(format!(
                                "failed to define self plugin shim '{}.{}': {e}",
                                import_name, func_name
                            ))
                        })?;
                }
                PluginImportProvider::Dependency(target_dependency) => {
                    capability_checker.ensure_dependency_function_allowed(
                        caller_name,
                        capabilities,
                        target_dependency,
                        import_name,
                        func_name,
                    )?;

                    let plugin = available_plugins.get(target_dependency).ok_or_else(|| {
                        map_runtime_error(format!(
                            "missing target dependency '{}' for plugin import '{}.{}'",
                            target_dependency, import_name, func_name
                        ))
                    })?;
                    match plugin.kind {
                        PluginKind::Native => {
                            return Err(map_runtime_error(format!(
                                "internal error: native plugin dependency '{}' should have been linked before fallback bridge '{}.{}'",
                                target_dependency, import_name, func_name
                            )));
                        }
                        PluginKind::Wasm => {
                            let callee = resolve_wasm_plugin_export(
                                &mut *store,
                                plugin,
                                import_name,
                                func_name,
                            )?;
                            let callee_ty = callee.ty(&*store);
                            ensure_component_signatures_match(
                                &import_ty,
                                &callee_ty,
                                import_name,
                                func_name,
                            )?;

                            import_instance
                                .func_new_async(
                                    func_name,
                                    move |mut store, _ty, params, results| {
                                        Box::new(async move {
                                            callee.call_async(&mut store, params, results).await?;
                                            callee.post_return_async(&mut store).await?;
                                            Ok(())
                                        })
                                    },
                                )
                                .map_err(|e| {
                                    map_runtime_error(format!(
                                        "failed to define wasm plugin shim '{}.{}': {e}",
                                        import_name, func_name
                                    ))
                                })?;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct BindingShimSignature {
    param_types: Vec<types::Type>,
    ok_type: Option<types::Type>,
}

fn build_binding_target_map(
    bindings: &[ServiceBinding],
) -> Result<BTreeMap<String, String>, ImagodError> {
    let mut by_wit = BTreeMap::new();
    for binding in bindings {
        let normalized_wit = normalize_binding_wit_id(&binding.wit);
        if normalized_wit.is_empty() {
            return Err(map_runtime_error(
                "binding wit must not be empty".to_string(),
            ));
        }
        if let Some(existing) = by_wit.get(&normalized_wit) {
            if existing != &binding.name {
                return Err(map_runtime_error(format!(
                    "binding wit '{}' maps to multiple services ('{}' and '{}')",
                    normalized_wit, existing, binding.name
                )));
            }
            continue;
        }
        by_wit.insert(normalized_wit, binding.name.clone());
    }
    Ok(by_wit)
}

fn resolve_binding_target_for_import(
    binding_targets: &BTreeMap<String, String>,
    import_name: &str,
) -> Option<(String, String)> {
    let normalized = normalize_binding_wit_id(import_name);
    binding_targets
        .get(&normalized)
        .map(|target_service| (target_service.clone(), normalized))
}

fn normalize_binding_wit_id(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some((package, interface_with_version)) = trimmed.split_once('/') else {
        return trimmed.to_string();
    };
    let interface = interface_with_version
        .split('@')
        .next()
        .unwrap_or(interface_with_version);
    format!("{package}/{interface}")
}

fn register_binding_import_shims(
    engine: &Engine,
    linker: &mut Linker<WasiState>,
    import_name: &str,
    instance_ty: &types::ComponentInstance,
    target_service: String,
    interface_id: String,
) -> Result<(), ImagodError> {
    let exports = instance_ty.exports(engine).collect::<Vec<_>>();
    let mut import_instance = linker.instance(import_name).map_err(|e| {
        map_runtime_error(format!(
            "failed to define binding import namespace '{}': {e}",
            import_name
        ))
    })?;

    for (func_name, item) in exports {
        let types::ComponentItem::ComponentFunc(import_ty) = item else {
            continue;
        };
        let signature = parse_binding_shim_signature(&import_ty, import_name, func_name)?;
        let function_name = func_name.to_string();
        let param_types = signature.param_types.clone();
        let ok_type = signature.ok_type.clone();
        let target_service = target_service.clone();
        let interface_id = interface_id.clone();

        import_instance
            .func_new_async(func_name, move |mut store, _ty, params, results| {
                let function_name = function_name.clone();
                let param_types = param_types.clone();
                let ok_type = ok_type.clone();
                let target_service = target_service.clone();
                let interface_id = interface_id.clone();
                Box::new(async move {
                    if results.len() != 1 {
                        return Err(wasmtime::Error::msg(format!(
                            "binding shim '{}.{}' must return exactly one result",
                            interface_id, function_name
                        )));
                    }

                    let connection_rep =
                        extract_binding_connection_rep(&mut store, params, &param_types)
                            .map_err(|err| wasmtime::Error::msg(err.to_string()))?;
                    let args_cbor = encode_payload_values(&params[1..], &param_types[1..])
                        .map_err(|err| wasmtime::Error::msg(err.to_string()))?;
                    let invoke_result = rpc_bridge::invoke_connection(
                        store.data().native_plugin_context(),
                        connection_rep,
                        &target_service,
                        &interface_id,
                        &function_name,
                        &args_cbor,
                    );

                    results[0] = match invoke_result {
                        Ok(result_cbor) => {
                            let ok_values = match &ok_type {
                                Some(ok_ty) => {
                                    decode_payload_values(&result_cbor, std::slice::from_ref(ok_ty))
                                        .map_err(|err| wasmtime::Error::msg(err.to_string()))?
                                }
                                None => decode_payload_values(&result_cbor, &[])
                                    .map_err(|err| wasmtime::Error::msg(err.to_string()))?,
                            };
                            let ok_payload = ok_values.into_iter().next().map(Box::new);
                            Val::Result(Ok(ok_payload))
                        }
                        Err(message) => Val::Result(Err(Some(Box::new(Val::String(message))))),
                    };
                    Ok(())
                })
            })
            .map_err(|e| {
                map_runtime_error(format!(
                    "failed to define binding rpc shim '{}.{}': {e}",
                    import_name, func_name
                ))
            })?;
    }

    Ok(())
}

fn parse_binding_shim_signature(
    import_ty: &types::ComponentFunc,
    import_name: &str,
    func_name: &str,
) -> Result<BindingShimSignature, ImagodError> {
    let param_types = import_ty.params().map(|(_, ty)| ty).collect::<Vec<_>>();
    let first_param = param_types.first().ok_or_else(|| {
        map_runtime_error(format!(
            "binding import '{}.{}' must take connection as first argument",
            import_name, func_name
        ))
    })?;
    if !matches!(first_param, types::Type::Borrow(_)) {
        return Err(map_runtime_error(format!(
            "binding import '{}.{}' first argument must be borrow<connection>",
            import_name, func_name
        )));
    }

    let result_types = import_ty.results().collect::<Vec<_>>();
    if result_types.len() != 1 {
        return Err(map_runtime_error(format!(
            "binding import '{}.{}' must return exactly one result",
            import_name, func_name
        )));
    }
    let types::Type::Result(result_ty) = &result_types[0] else {
        return Err(map_runtime_error(format!(
            "binding import '{}.{}' must return result<_, string>",
            import_name, func_name
        )));
    };
    let err_ty = result_ty.err().ok_or_else(|| {
        map_runtime_error(format!(
            "binding import '{}.{}' result must define err type",
            import_name, func_name
        ))
    })?;
    if err_ty != types::Type::String {
        return Err(map_runtime_error(format!(
            "binding import '{}.{}' err type must be string",
            import_name, func_name
        )));
    }

    Ok(BindingShimSignature {
        param_types,
        ok_type: result_ty.ok(),
    })
}

fn extract_binding_connection_rep(
    mut store: impl wasmtime::AsContextMut<Data = WasiState>,
    params: &[Val],
    param_types: &[types::Type],
) -> Result<u32, ImagodError> {
    let first_type = param_types.first().ok_or_else(|| {
        map_runtime_error("binding shim requires connection parameter".to_string())
    })?;
    if !matches!(first_type, types::Type::Borrow(_)) {
        return Err(map_runtime_error(
            "binding shim first parameter must be borrow<connection>".to_string(),
        ));
    }

    let Some(connection_value) = params.first() else {
        return Err(map_runtime_error(
            "binding shim first runtime argument must be a connection resource".to_string(),
        ));
    };
    rpc_bridge::extract_connection_rep(store.as_context_mut(), connection_value)
}

fn add_dependency_edge(
    edges: &mut [BTreeSet<usize>],
    indegree: &mut [usize],
    from: usize,
    to: usize,
) {
    if edges[from].insert(to) {
        indegree[to] += 1;
    }
}

fn collect_component_instance_export_names(
    component: &Component,
    engine: &Engine,
) -> BTreeSet<String> {
    component
        .component_type()
        .exports(engine)
        .filter_map(|(name, item)| match item {
            types::ComponentItem::ComponentInstance(_) => Some(name.to_string()),
            _ => None,
        })
        .collect()
}

fn collect_component_instance_import_names(component: &Component, engine: &Engine) -> Vec<String> {
    component
        .component_type()
        .imports(engine)
        .filter_map(|(name, item)| {
            if name.starts_with("wasi:") {
                return None;
            }
            match item {
                types::ComponentItem::ComponentInstance(_) => Some(name.to_string()),
                _ => None,
            }
        })
        .collect()
}

fn collect_implicit_provider_indices(
    dependency_name: &str,
    import_names: &[String],
    self_export_interfaces: &BTreeSet<String>,
    by_name: &BTreeMap<String, usize>,
) -> BTreeSet<usize> {
    let mut providers = BTreeSet::new();
    for import_name in import_names {
        if self_export_interfaces.contains(import_name) {
            continue;
        }
        let Some(package_name) = parse_import_package_name(import_name) else {
            continue;
        };
        if package_name == dependency_name {
            continue;
        }
        if let Some(index) = by_name.get(package_name) {
            providers.insert(*index);
        }
    }
    providers
}

fn parse_import_package_name(import_name: &str) -> Option<&str> {
    import_name
        .rsplit_once('/')
        .map(|(package_name, _)| package_name)
}

fn ensure_component_signatures_match(
    import_ty: &types::ComponentFunc,
    callee_ty: &types::ComponentFunc,
    interface_name: &str,
    function_name: &str,
) -> Result<(), ImagodError> {
    let import_params = import_ty.params().map(|(_, ty)| ty).collect::<Vec<_>>();
    let callee_params = callee_ty.params().map(|(_, ty)| ty).collect::<Vec<_>>();
    if import_params != callee_params {
        return Err(map_runtime_error(format!(
            "plugin import type mismatch for '{}.{}': parameter types differ",
            interface_name, function_name
        )));
    }

    let import_results = import_ty.results().collect::<Vec<_>>();
    let callee_results = callee_ty.results().collect::<Vec<_>>();
    if import_results != callee_results {
        return Err(map_runtime_error(format!(
            "plugin import type mismatch for '{}.{}': result types differ",
            interface_name, function_name
        )));
    }

    Ok(())
}

fn resolve_component_export_type(
    component: &Component,
    engine: &Engine,
    interface_name: &str,
    function_name: &str,
) -> Result<types::ComponentFunc, ImagodError> {
    let component_ty = component.component_type();
    let interface_ty = component_ty
        .exports(engine)
        .find_map(|(export_name, export_item)| {
            if export_name != interface_name {
                return None;
            }
            match export_item {
                types::ComponentItem::ComponentInstance(instance_ty) => Some(instance_ty),
                _ => None,
            }
        })
        .ok_or_else(|| {
            map_runtime_error(format!(
                "self provider export interface '{}' was not found",
                interface_name
            ))
        })?;

    interface_ty
        .exports(engine)
        .find_map(|(export_name, export_item)| {
            if export_name != function_name {
                return None;
            }
            match export_item {
                types::ComponentItem::ComponentFunc(func_ty) => Some(func_ty),
                _ => None,
            }
        })
        .ok_or_else(|| {
            map_runtime_error(format!(
                "self provider export function '{}.{}' was not found",
                interface_name, function_name
            ))
        })
}

fn resolve_wasm_plugin_export(
    store: impl wasmtime::AsContextMut<Data = WasiState>,
    plugin: &AvailablePlugin,
    interface_name: &str,
    function_name: &str,
) -> Result<Func, ImagodError> {
    let instance = plugin.instance.as_ref().ok_or_else(|| {
        map_runtime_error("internal error: wasm plugin dependency is missing instance".to_string())
    })?;
    resolve_wasm_export_from_instance(store, instance, interface_name, function_name)
}

fn resolve_wasm_export_from_instance(
    mut store: impl wasmtime::AsContextMut<Data = WasiState>,
    instance: &wasmtime::component::Instance,
    interface_name: &str,
    function_name: &str,
) -> Result<Func, ImagodError> {
    let interface_index = instance
        .get_export_index(store.as_context_mut(), None, interface_name)
        .ok_or_else(|| {
            map_runtime_error(format!(
                "plugin export interface '{}' was not found",
                interface_name
            ))
        })?;
    let function_index = instance
        .get_export_index(
            store.as_context_mut(),
            Some(&interface_index),
            function_name,
        )
        .or_else(|| instance.get_export_index(store.as_context_mut(), None, function_name))
        .ok_or_else(|| {
            map_runtime_error(format!(
                "plugin export function '{}.{}' was not found",
                interface_name, function_name
            ))
        })?;
    instance
        .get_func(store.as_context_mut(), function_index)
        .ok_or_else(|| {
            map_runtime_error(format!(
                "plugin export '{}.{}' is not a function",
                interface_name, function_name
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_import_provider_prefers_self_component() {
        let resolver = DefaultPluginResolver;
        let self_exports = BTreeSet::from(["chikoski:name/name-provider".to_string()]);
        let explicit_names = BTreeSet::from(["chikoski:name".to_string()]);
        let available_names = BTreeSet::from(["chikoski:name".to_string()]);

        let provider = resolver
            .resolve_import_provider(
                "chikoski:hello",
                "chikoski:name/name-provider",
                &self_exports,
                &explicit_names,
                &available_names,
                true,
            )
            .expect("self provider should resolve");
        assert_eq!(provider, PluginImportProvider::SelfComponent);
    }

    #[test]
    fn resolve_import_provider_falls_back_to_explicit_dependency() {
        let resolver = DefaultPluginResolver;
        let provider = resolver
            .resolve_import_provider(
                "chikoski:hello",
                "chikoski:name/name-provider",
                &BTreeSet::new(),
                &BTreeSet::from(["chikoski:name".to_string()]),
                &BTreeSet::from(["chikoski:name".to_string()]),
                true,
            )
            .expect("explicit dependency fallback should resolve");
        assert_eq!(
            provider,
            PluginImportProvider::Dependency("chikoski:name".to_string())
        );
    }

    #[test]
    fn resolve_import_provider_reports_unresolved_import() {
        let resolver = DefaultPluginResolver;
        let err = resolver
            .resolve_import_provider(
                "chikoski:hello",
                "chikoski:name/name-provider",
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
                true,
            )
            .expect_err("missing provider should fail");
        assert!(
            err.message.contains("caller='chikoski:hello'")
                && err.message.contains("import='chikoski:name/name-provider'")
                && err.message.contains("checked=self")
                && err.message.contains("result=not-found"),
            "unexpected message: {}",
            err.message
        );
    }

    #[test]
    fn collect_implicit_provider_indices_uses_explicit_dep_when_self_missing() {
        let by_name = BTreeMap::from([
            ("chikoski:hello".to_string(), 0usize),
            ("chikoski:name".to_string(), 1usize),
        ]);
        let import_names = vec!["chikoski:name/name-provider".to_string()];
        let indices = collect_implicit_provider_indices(
            "chikoski:hello",
            &import_names,
            &BTreeSet::new(),
            &by_name,
        );
        assert_eq!(indices, BTreeSet::from([1usize]));
    }

    #[test]
    fn collect_implicit_provider_indices_skips_when_self_exports_interface() {
        let by_name = BTreeMap::from([
            ("chikoski:hello".to_string(), 0usize),
            ("chikoski:name".to_string(), 1usize),
        ]);
        let import_names = vec!["chikoski:name/name-provider".to_string()];
        let self_exports = BTreeSet::from(["chikoski:name/name-provider".to_string()]);
        let indices = collect_implicit_provider_indices(
            "chikoski:hello",
            &import_names,
            &self_exports,
            &by_name,
        );
        assert!(
            indices.is_empty(),
            "self export should suppress implicit edge"
        );
    }

    #[test]
    fn collect_implicit_provider_indices_supports_nested_dependency_package_name() {
        let by_name = BTreeMap::from([
            ("yieldspace:plugin/hello".to_string(), 0usize),
            ("yieldspace:plugin/example".to_string(), 1usize),
        ]);
        let import_names = vec!["yieldspace:plugin/example/admin".to_string()];
        let indices = collect_implicit_provider_indices(
            "yieldspace:plugin/hello",
            &import_names,
            &BTreeSet::new(),
            &by_name,
        );
        assert_eq!(indices, BTreeSet::from([1usize]));
    }

    #[test]
    fn normalize_binding_wit_id_drops_interface_version_suffix() {
        assert_eq!(
            normalize_binding_wit_id("yieldspace:svc/invoke@0.1.0"),
            "yieldspace:svc/invoke"
        );
        assert_eq!(
            normalize_binding_wit_id("yieldspace:svc/invoke"),
            "yieldspace:svc/invoke"
        );
    }

    #[test]
    fn build_binding_target_map_rejects_ambiguous_wit_mapping() {
        let bindings = vec![
            ServiceBinding {
                name: "svc-a".to_string(),
                wit: "yieldspace:svc/invoke".to_string(),
            },
            ServiceBinding {
                name: "svc-b".to_string(),
                wit: "yieldspace:svc/invoke@0.1.0".to_string(),
            },
        ];
        let err = build_binding_target_map(&bindings)
            .expect_err("same wit must not map to multiple services");
        assert!(
            err.message.contains("maps to multiple services"),
            "unexpected message: {}",
            err.message
        );
    }
}
