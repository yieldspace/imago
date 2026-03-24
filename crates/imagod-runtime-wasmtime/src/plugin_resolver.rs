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
    AsContextMut, Engine, Store,
    component::{Component, Func, Linker, ResourceDynamic, Val, types},
};
use wasmtime_wasi::p2::add_to_linker_async;
use wasmtime_wasi_http::p2::add_only_http_to_linker_async;

use crate::{
    WasiState, WasmDependencyResourceKey,
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

#[derive(Debug, Clone)]
struct DependencyResourceBinding {
    import_resource: types::ResourceType,
    callee_resource: types::ResourceType,
    host_dynamic_type_id: u32,
    resource_name: String,
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
    let mut preloaded_components = BTreeMap::<String, Component>::new();
    let mut component_interfaces = BTreeMap::<String, PluginComponentInterfaces>::new();
    for dep in dependencies {
        if dep.kind != PluginKind::Wasm {
            continue;
        }
        let interfaces =
            component_interfaces_for_dependency(dep, engine, &mut preloaded_components)?;
        component_interfaces.insert(dep.name.clone(), interfaces);
    }

    let order = resolver.dependency_topological_order(dependencies, &component_interfaces)?;
    let explicit_dependency_names = resolver.all_dependency_names(dependencies);
    let mut available = BTreeMap::<String, AvailablePlugin>::new();

    for dep_index in order {
        let dep = &dependencies[dep_index];
        match dep.kind {
            PluginKind::Native => {
                let native_plugin = native_plugins.plugin(&dep.name).ok_or_else(|| {
                    map_runtime_error(format!(
                        "native plugin '{}' is declared in manifest but not registered in runtime",
                        dep.name
                    ))
                })?;
                native_plugin
                    .validate_resources(store.data().native_plugin_context().resources())?;
                available.insert(
                    dep.name.clone(),
                    AvailablePlugin {
                        kind: PluginKind::Native,
                        instance: None,
                    },
                );
            }
            PluginKind::Wasm => {
                let component =
                    load_component_for_instantiation(dep, engine, &mut preloaded_components)?;

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
                    &component,
                    &dep.name,
                    &explicit_dependency_names,
                    &dep.capabilities,
                    &[],
                    &available,
                    Some(self_instance.clone()),
                )?;

                let instance = linker
                    .instantiate_async(&mut *store, &component)
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

fn component_interfaces_for_dependency(
    dep: &PluginDependency,
    engine: &Engine,
    preloaded_components: &mut BTreeMap<String, Component>,
) -> Result<PluginComponentInterfaces, ImagodError> {
    let component = dep.component.as_ref().ok_or_else(|| {
        map_runtime_error(format!(
            "plugin dependency '{}' missing component definition",
            dep.name
        ))
    })?;
    if let (Some(imports), Some(exports)) = (&component.imports, &component.exports) {
        return Ok(PluginComponentInterfaces {
            imports: imports.clone(),
            exports: exports.iter().cloned().collect(),
        });
    }
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
    preloaded_components.insert(dep.name.clone(), loaded);
    Ok(interfaces)
}

fn load_component_for_instantiation(
    dep: &PluginDependency,
    engine: &Engine,
    preloaded_components: &mut BTreeMap<String, Component>,
) -> Result<Component, ImagodError> {
    if let Some(preloaded) = preloaded_components.remove(&dep.name) {
        return Ok(preloaded);
    }
    let component_meta = dep.component.as_ref().ok_or_else(|| {
        map_runtime_error(format!(
            "plugin dependency '{}' missing component definition",
            dep.name
        ))
    })?;
    Component::from_file(engine, &component_meta.path).map_err(|e| {
        map_runtime_error(format!(
            "failed to load plugin component '{}' from {}: {e}",
            dep.name,
            component_meta.path.display()
        ))
    })
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
    let mut linked_native_plugin_packages = BTreeSet::<String>::new();
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
            if mark_native_plugin_linked(
                &mut linked_native_plugin_packages,
                native_plugin.package_name(),
            ) {
                linker.allow_shadowing(true);
                let link_result = (|| -> Result<(), ImagodError> {
                    native_plugin.add_to_linker(linker)?;
                    add_to_linker_async(linker).map_err(|e| {
                        map_runtime_error(format!(
                            "failed to refresh WASI linker after native plugin '{}' link: {e}",
                            native_plugin.package_name()
                        ))
                    })?;
                    Ok(())
                })();
                linker.allow_shadowing(false);
                link_result?;
            }
            continue;
        }

        let mut import_instance = linker.instance(import_name).map_err(|e| {
            map_runtime_error(format!(
                "failed to define plugin import namespace '{}': {e}",
                import_name
            ))
        })?;
        let dependency_resource_bindings = match &provider {
            PluginImportProvider::Dependency(target_dependency) => {
                let plugin = available_plugins.get(target_dependency).ok_or_else(|| {
                    map_runtime_error(format!(
                        "missing target dependency '{}' for plugin import '{}'",
                        target_dependency, import_name
                    ))
                })?;
                if plugin.kind != PluginKind::Wasm {
                    Vec::new()
                } else {
                    let dependency_instance = plugin.instance.as_ref().ok_or_else(|| {
                        map_runtime_error(format!(
                            "internal error: wasm plugin dependency '{}' is missing instance during bridge registration",
                            target_dependency
                        ))
                    })?;
                    let bindings = collect_dependency_resource_bindings(
                        store,
                        target_dependency,
                        import_name,
                        &instance_ty,
                        dependency_instance,
                        engine,
                    )?;
                    for binding in &bindings {
                        let interface_name = import_name.to_string();
                        let resource_name = binding.resource_name.clone();
                        let host_dynamic_type_id = binding.host_dynamic_type_id;
                        import_instance
                            .resource(
                                &resource_name.clone(),
                                wasmtime::component::ResourceType::host_dynamic(
                                    host_dynamic_type_id,
                                ),
                                move |mut store, rep| {
                                    let stored = store
                                        .data_mut()
                                        .remove_wasm_dependency_resource(rep)
                                        .ok_or_else(|| {
                                            wasmtime::Error::msg(format!(
                                                "missing bridged dependency resource '{}' rep={} for '{}'",
                                                resource_name, rep, interface_name
                                            ))
                                        })?;
                                    if stored.type_id != host_dynamic_type_id {
                                        return Err(wasmtime::Error::msg(format!(
                                            "bridged dependency resource '{}' rep={} for '{}' has unexpected host type {}",
                                            resource_name,
                                            rep,
                                            interface_name,
                                            stored.type_id
                                        )));
                                    }
                                    stored.resource.resource_drop(store.as_context_mut())?;
                                    Ok(())
                                },
                            )
                            .map_err(|e| {
                                map_runtime_error(format!(
                                    "failed to define bridged dependency resource '{}.{}': {e}",
                                    import_name, binding.resource_name
                                ))
                            })?;
                    }
                    bindings
                }
            }
            PluginImportProvider::SelfComponent => Vec::new(),
        };

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
                        None,
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
                                Some(&dependency_resource_bindings),
                            )?;
                            let import_param_types =
                                import_ty.params().map(|(_, ty)| ty).collect::<Vec<_>>();
                            let import_result_types = import_ty.results().collect::<Vec<_>>();
                            let resource_bindings = dependency_resource_bindings.clone();

                            import_instance
                                .func_new_async(
                                    func_name,
                                    move |mut store, _ty, params, results| {
                                        let import_param_types = import_param_types.clone();
                                        let import_result_types = import_result_types.clone();
                                        let resource_bindings = resource_bindings.clone();
                                        Box::new(async move {
                                            let callee_params = import_param_types
                                                .iter()
                                                .zip(params.iter())
                                                .map(|(param_ty, param)| {
                                                    map_value_to_wasm_dependency(
                                                        &mut store,
                                                        param.clone(),
                                                        param_ty,
                                                        &resource_bindings,
                                                    )
                                                    .map_err(|err| {
                                                        wasmtime::Error::msg(err.to_string())
                                                    })
                                                })
                                                .collect::<Result<Vec<_>, _>>()?;
                                            callee
                                                .call_async(&mut store, &callee_params, results)
                                                .await?;
                                            for (result_ty, result) in
                                                import_result_types.iter().zip(results.iter_mut())
                                            {
                                                *result = map_value_from_wasm_dependency(
                                                    &mut store,
                                                    result.clone(),
                                                    result_ty,
                                                    &resource_bindings,
                                                )
                                                .map_err(|err| {
                                                    wasmtime::Error::msg(err.to_string())
                                                })?;
                                            }
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
                        args_cbor,
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

fn mark_native_plugin_linked(
    linked_native_plugin_packages: &mut BTreeSet<String>,
    package_name: &str,
) -> bool {
    linked_native_plugin_packages.insert(package_name.to_string())
}

fn ensure_component_signatures_match(
    import_ty: &types::ComponentFunc,
    callee_ty: &types::ComponentFunc,
    interface_name: &str,
    function_name: &str,
    resource_bindings: Option<&[DependencyResourceBinding]>,
) -> Result<(), ImagodError> {
    let import_params = import_ty.params().map(|(_, ty)| ty).collect::<Vec<_>>();
    let callee_params = callee_ty.params().map(|(_, ty)| ty).collect::<Vec<_>>();
    if import_params.len() != callee_params.len()
        || !import_params
            .iter()
            .zip(callee_params.iter())
            .all(|(import_ty, callee_ty)| {
                component_types_match(import_ty, callee_ty, resource_bindings)
            })
    {
        return Err(map_runtime_error(format!(
            "plugin import type mismatch for '{}.{}': parameter types differ",
            interface_name, function_name
        )));
    }

    let import_results = import_ty.results().collect::<Vec<_>>();
    let callee_results = callee_ty.results().collect::<Vec<_>>();
    if import_results.len() != callee_results.len()
        || !import_results
            .iter()
            .zip(callee_results.iter())
            .all(|(import_ty, callee_ty)| {
                component_types_match(import_ty, callee_ty, resource_bindings)
            })
    {
        return Err(map_runtime_error(format!(
            "plugin import type mismatch for '{}.{}': result types differ",
            interface_name, function_name
        )));
    }

    Ok(())
}

fn component_types_match(
    import_ty: &types::Type,
    callee_ty: &types::Type,
    resource_bindings: Option<&[DependencyResourceBinding]>,
) -> bool {
    match (import_ty, callee_ty) {
        (types::Type::Bool, types::Type::Bool)
        | (types::Type::S8, types::Type::S8)
        | (types::Type::U8, types::Type::U8)
        | (types::Type::S16, types::Type::S16)
        | (types::Type::U16, types::Type::U16)
        | (types::Type::S32, types::Type::S32)
        | (types::Type::U32, types::Type::U32)
        | (types::Type::S64, types::Type::S64)
        | (types::Type::U64, types::Type::U64)
        | (types::Type::Float32, types::Type::Float32)
        | (types::Type::Float64, types::Type::Float64)
        | (types::Type::Char, types::Type::Char)
        | (types::Type::String, types::Type::String)
        | (types::Type::ErrorContext, types::Type::ErrorContext) => true,
        (types::Type::Own(import_resource), types::Type::Own(callee_resource))
        | (types::Type::Borrow(import_resource), types::Type::Borrow(callee_resource)) => {
            dependency_resource_types_match(import_resource, callee_resource, resource_bindings)
        }
        (types::Type::List(import_list), types::Type::List(callee_list)) => {
            component_types_match(&import_list.ty(), &callee_list.ty(), resource_bindings)
        }
        (types::Type::Record(import_record), types::Type::Record(callee_record)) => {
            let import_fields = import_record.fields().collect::<Vec<_>>();
            let callee_fields = callee_record.fields().collect::<Vec<_>>();
            import_fields.len() == callee_fields.len()
                && import_fields.iter().zip(callee_fields.iter()).all(
                    |(import_field, callee_field)| {
                        import_field.name == callee_field.name
                            && component_types_match(
                                &import_field.ty,
                                &callee_field.ty,
                                resource_bindings,
                            )
                    },
                )
        }
        (types::Type::Tuple(import_tuple), types::Type::Tuple(callee_tuple)) => {
            let import_fields = import_tuple.types().collect::<Vec<_>>();
            let callee_fields = callee_tuple.types().collect::<Vec<_>>();
            import_fields.len() == callee_fields.len()
                && import_fields.iter().zip(callee_fields.iter()).all(
                    |(import_field, callee_field)| {
                        component_types_match(import_field, callee_field, resource_bindings)
                    },
                )
        }
        (types::Type::Variant(import_variant), types::Type::Variant(callee_variant)) => {
            let import_cases = import_variant.cases().collect::<Vec<_>>();
            let callee_cases = callee_variant.cases().collect::<Vec<_>>();
            import_cases.len() == callee_cases.len()
                && import_cases
                    .iter()
                    .zip(callee_cases.iter())
                    .all(|(import_case, callee_case)| {
                        import_case.name == callee_case.name
                            && option_component_types_match(
                                import_case.ty.clone(),
                                callee_case.ty.clone(),
                                resource_bindings,
                            )
                    })
        }
        (types::Type::Enum(import_enum), types::Type::Enum(callee_enum)) => {
            import_enum.names().eq(callee_enum.names())
        }
        (types::Type::Option(import_option), types::Type::Option(callee_option)) => {
            component_types_match(&import_option.ty(), &callee_option.ty(), resource_bindings)
        }
        (types::Type::Result(import_result), types::Type::Result(callee_result)) => {
            option_component_types_match(import_result.ok(), callee_result.ok(), resource_bindings)
                && option_component_types_match(
                    import_result.err(),
                    callee_result.err(),
                    resource_bindings,
                )
        }
        (types::Type::Flags(import_flags), types::Type::Flags(callee_flags)) => {
            import_flags.names().eq(callee_flags.names())
        }
        (types::Type::Future(import_future), types::Type::Future(callee_future)) => {
            option_component_types_match(import_future.ty(), callee_future.ty(), resource_bindings)
        }
        (types::Type::Stream(import_stream), types::Type::Stream(callee_stream)) => {
            option_component_types_match(import_stream.ty(), callee_stream.ty(), resource_bindings)
        }
        _ => false,
    }
}

// Component methods carry an implicit self resource parameter. When a wasm
// dependency bridge pairs import/export resources by name, accept the paired
// identities even though instantiation freshens the guest resource type.
fn dependency_resource_types_match(
    import_resource: &types::ResourceType,
    callee_resource: &types::ResourceType,
    resource_bindings: Option<&[DependencyResourceBinding]>,
) -> bool {
    if import_resource == callee_resource {
        return true;
    }
    resource_bindings.is_some_and(|bindings| {
        bindings.iter().any(|binding| {
            binding.import_resource == *import_resource
                && binding.callee_resource == *callee_resource
        })
    })
}

fn option_component_types_match(
    import_ty: Option<types::Type>,
    callee_ty: Option<types::Type>,
    resource_bindings: Option<&[DependencyResourceBinding]>,
) -> bool {
    match (import_ty, callee_ty) {
        (Some(import_ty), Some(callee_ty)) => {
            component_types_match(&import_ty, &callee_ty, resource_bindings)
        }
        (None, None) => true,
        _ => false,
    }
}

fn collect_dependency_resource_bindings(
    store: &mut Store<WasiState>,
    target_dependency: &str,
    interface_name: &str,
    instance_ty: &types::ComponentInstance,
    dependency_instance: &wasmtime::component::Instance,
    engine: &Engine,
) -> Result<Vec<DependencyResourceBinding>, ImagodError> {
    let mut bindings = Vec::new();
    for (resource_name, item) in instance_ty.exports(engine) {
        let types::ComponentItem::Resource(resource_ty) = item else {
            continue;
        };
        bindings.push(DependencyResourceBinding {
            import_resource: resource_ty,
            callee_resource: resolve_dependency_resource_type(
                &mut *store,
                dependency_instance,
                interface_name,
                resource_name,
            )?,
            host_dynamic_type_id: store.data_mut().wasm_dependency_resource_type_id(
                WasmDependencyResourceKey {
                    dependency_name: target_dependency.to_string(),
                    interface_name: interface_name.to_string(),
                    resource_name: resource_name.to_string(),
                },
            ),
            resource_name: resource_name.to_string(),
        });
    }
    Ok(bindings)
}

fn resolve_dependency_resource_type(
    mut store: impl wasmtime::AsContextMut<Data = WasiState>,
    dependency_instance: &wasmtime::component::Instance,
    interface_name: &str,
    resource_name: &str,
) -> Result<types::ResourceType, ImagodError> {
    let interface_export = dependency_instance
        .get_export_index(store.as_context_mut(), None, interface_name)
        .ok_or_else(|| {
            map_runtime_error(format!(
                "wasm dependency export interface '{}' was not found",
                interface_name
            ))
        })?;
    let resource_export = dependency_instance
        .get_export_index(
            store.as_context_mut(),
            Some(&interface_export),
            resource_name,
        )
        .ok_or_else(|| {
            map_runtime_error(format!(
                "wasm dependency export resource '{}.{}' was not found",
                interface_name, resource_name
            ))
        })?;
    dependency_instance
        .get_resource(store.as_context_mut(), resource_export)
        .ok_or_else(|| {
            map_runtime_error(format!(
                "wasm dependency export '{}.{}' is not a resource",
                interface_name, resource_name
            ))
        })
}

fn dependency_resource_binding<'a>(
    bindings: &'a [DependencyResourceBinding],
    resource_ty: &types::ResourceType,
) -> Result<&'a DependencyResourceBinding, ImagodError> {
    bindings
        .iter()
        .find(|binding| binding.import_resource == *resource_ty)
        .ok_or_else(|| {
            map_runtime_error("missing bridged wasm dependency resource binding".to_string())
        })
}

fn map_value_to_wasm_dependency(
    store: &mut wasmtime::StoreContextMut<'_, WasiState>,
    value: Val,
    ty: &types::Type,
    bindings: &[DependencyResourceBinding],
) -> Result<Val, ImagodError> {
    match (ty, value) {
        (
            types::Type::Bool
            | types::Type::S8
            | types::Type::U8
            | types::Type::S16
            | types::Type::U16
            | types::Type::S32
            | types::Type::U32
            | types::Type::S64
            | types::Type::U64
            | types::Type::Float32
            | types::Type::Float64
            | types::Type::Char
            | types::Type::String
            | types::Type::Enum(_)
            | types::Type::Flags(_)
            | types::Type::Future(_)
            | types::Type::Stream(_)
            | types::Type::ErrorContext,
            value,
        ) => Ok(value),
        (types::Type::List(list_ty), Val::List(values)) => Ok(Val::List(
            values
                .into_iter()
                .map(|value| map_value_to_wasm_dependency(store, value, &list_ty.ty(), bindings))
                .collect::<Result<_, _>>()?,
        )),
        (types::Type::Record(record_ty), Val::Record(values)) => {
            let fields = record_ty.fields().collect::<Vec<_>>();
            if values.len() != fields.len() {
                return Err(map_runtime_error(
                    "record field count mismatch in wasm dependency bridge".to_string(),
                ));
            }
            let mut mapped = Vec::with_capacity(values.len());
            for ((field_name, field_value), field_ty) in values.into_iter().zip(fields.iter()) {
                if field_name != field_ty.name {
                    return Err(map_runtime_error(format!(
                        "record field mismatch in wasm dependency bridge: expected '{}', got '{}'",
                        field_ty.name, field_name
                    )));
                }
                mapped.push((
                    field_name,
                    map_value_to_wasm_dependency(store, field_value, &field_ty.ty, bindings)?,
                ));
            }
            Ok(Val::Record(mapped))
        }
        (types::Type::Tuple(tuple_ty), Val::Tuple(values)) => {
            let types = tuple_ty.types().collect::<Vec<_>>();
            if values.len() != types.len() {
                return Err(map_runtime_error(
                    "tuple field count mismatch in wasm dependency bridge".to_string(),
                ));
            }
            Ok(Val::Tuple(
                values
                    .into_iter()
                    .zip(types.iter())
                    .map(|(value, value_ty)| {
                        map_value_to_wasm_dependency(store, value, value_ty, bindings)
                    })
                    .collect::<Result<_, _>>()?,
            ))
        }
        (types::Type::Variant(variant_ty), Val::Variant(case_name, payload)) => {
            let case_ty = variant_ty
                .cases()
                .find(|case_ty| case_ty.name == case_name)
                .ok_or_else(|| {
                    map_runtime_error(format!(
                        "variant case '{}' is not defined in wasm dependency bridge",
                        case_name
                    ))
                })?;
            Ok(Val::Variant(
                case_name,
                map_optional_value_to_wasm_dependency(
                    store,
                    payload,
                    case_ty.ty.clone(),
                    bindings,
                )?,
            ))
        }
        (types::Type::Option(option_ty), Val::Option(value)) => Ok(Val::Option(
            map_optional_value_to_wasm_dependency(store, value, Some(option_ty.ty()), bindings)?,
        )),
        (types::Type::Result(result_ty), Val::Result(value)) => Ok(Val::Result(match value {
            Ok(ok) => Ok(map_optional_value_to_wasm_dependency(
                store,
                ok,
                result_ty.ok(),
                bindings,
            )?),
            Err(err) => Err(map_optional_value_to_wasm_dependency(
                store,
                err,
                result_ty.err(),
                bindings,
            )?),
        })),
        (types::Type::Own(resource_ty), Val::Resource(resource)) => {
            map_resource_to_wasm_dependency(store, resource, resource_ty, true, bindings)
        }
        (types::Type::Borrow(resource_ty), Val::Resource(resource)) => {
            map_resource_to_wasm_dependency(store, resource, resource_ty, false, bindings)
        }
        _ => Err(map_runtime_error(
            "unsupported value/type pair in wasm dependency bridge".to_string(),
        )),
    }
}

fn map_optional_value_to_wasm_dependency(
    store: &mut wasmtime::StoreContextMut<'_, WasiState>,
    value: Option<Box<Val>>,
    ty: Option<types::Type>,
    bindings: &[DependencyResourceBinding],
) -> Result<Option<Box<Val>>, ImagodError> {
    match (value, ty) {
        (Some(value), Some(ty)) => Ok(Some(Box::new(map_value_to_wasm_dependency(
            store, *value, &ty, bindings,
        )?))),
        (None, None) => Ok(None),
        (None, Some(_)) => Ok(None),
        (Some(_), None) => Err(map_runtime_error(
            "unexpected optional payload in wasm dependency bridge".to_string(),
        )),
    }
}

fn map_value_from_wasm_dependency(
    store: &mut wasmtime::StoreContextMut<'_, WasiState>,
    value: Val,
    ty: &types::Type,
    bindings: &[DependencyResourceBinding],
) -> Result<Val, ImagodError> {
    match (ty, value) {
        (
            types::Type::Bool
            | types::Type::S8
            | types::Type::U8
            | types::Type::S16
            | types::Type::U16
            | types::Type::S32
            | types::Type::U32
            | types::Type::S64
            | types::Type::U64
            | types::Type::Float32
            | types::Type::Float64
            | types::Type::Char
            | types::Type::String
            | types::Type::Enum(_)
            | types::Type::Flags(_)
            | types::Type::Future(_)
            | types::Type::Stream(_)
            | types::Type::ErrorContext,
            value,
        ) => Ok(value),
        (types::Type::List(list_ty), Val::List(values)) => Ok(Val::List(
            values
                .into_iter()
                .map(|value| map_value_from_wasm_dependency(store, value, &list_ty.ty(), bindings))
                .collect::<Result<_, _>>()?,
        )),
        (types::Type::Record(record_ty), Val::Record(values)) => {
            let fields = record_ty.fields().collect::<Vec<_>>();
            if values.len() != fields.len() {
                return Err(map_runtime_error(
                    "record field count mismatch in wasm dependency bridge".to_string(),
                ));
            }
            let mut mapped = Vec::with_capacity(values.len());
            for ((field_name, field_value), field_ty) in values.into_iter().zip(fields.iter()) {
                if field_name != field_ty.name {
                    return Err(map_runtime_error(format!(
                        "record field mismatch in wasm dependency bridge: expected '{}', got '{}'",
                        field_ty.name, field_name
                    )));
                }
                mapped.push((
                    field_name,
                    map_value_from_wasm_dependency(store, field_value, &field_ty.ty, bindings)?,
                ));
            }
            Ok(Val::Record(mapped))
        }
        (types::Type::Tuple(tuple_ty), Val::Tuple(values)) => {
            let types = tuple_ty.types().collect::<Vec<_>>();
            if values.len() != types.len() {
                return Err(map_runtime_error(
                    "tuple field count mismatch in wasm dependency bridge".to_string(),
                ));
            }
            Ok(Val::Tuple(
                values
                    .into_iter()
                    .zip(types.iter())
                    .map(|(value, value_ty)| {
                        map_value_from_wasm_dependency(store, value, value_ty, bindings)
                    })
                    .collect::<Result<_, _>>()?,
            ))
        }
        (types::Type::Variant(variant_ty), Val::Variant(case_name, payload)) => {
            let case_ty = variant_ty
                .cases()
                .find(|case_ty| case_ty.name == case_name)
                .ok_or_else(|| {
                    map_runtime_error(format!(
                        "variant case '{}' is not defined in wasm dependency bridge",
                        case_name
                    ))
                })?;
            Ok(Val::Variant(
                case_name,
                map_optional_value_from_wasm_dependency(
                    store,
                    payload,
                    case_ty.ty.clone(),
                    bindings,
                )?,
            ))
        }
        (types::Type::Option(option_ty), Val::Option(value)) => Ok(Val::Option(
            map_optional_value_from_wasm_dependency(store, value, Some(option_ty.ty()), bindings)?,
        )),
        (types::Type::Result(result_ty), Val::Result(value)) => Ok(Val::Result(match value {
            Ok(ok) => Ok(map_optional_value_from_wasm_dependency(
                store,
                ok,
                result_ty.ok(),
                bindings,
            )?),
            Err(err) => Err(map_optional_value_from_wasm_dependency(
                store,
                err,
                result_ty.err(),
                bindings,
            )?),
        })),
        (types::Type::Own(resource_ty), Val::Resource(resource)) => {
            map_resource_from_wasm_dependency(store, resource, resource_ty, true, bindings)
        }
        (types::Type::Borrow(resource_ty), Val::Resource(resource)) => {
            map_resource_from_wasm_dependency(store, resource, resource_ty, false, bindings)
        }
        _ => Err(map_runtime_error(
            "unsupported value/type pair in wasm dependency bridge".to_string(),
        )),
    }
}

fn map_optional_value_from_wasm_dependency(
    store: &mut wasmtime::StoreContextMut<'_, WasiState>,
    value: Option<Box<Val>>,
    ty: Option<types::Type>,
    bindings: &[DependencyResourceBinding],
) -> Result<Option<Box<Val>>, ImagodError> {
    match (value, ty) {
        (Some(value), Some(ty)) => Ok(Some(Box::new(map_value_from_wasm_dependency(
            store, *value, &ty, bindings,
        )?))),
        (None, None) => Ok(None),
        (None, Some(_)) => Ok(None),
        (Some(_), None) => Err(map_runtime_error(
            "unexpected optional payload in wasm dependency bridge".to_string(),
        )),
    }
}

fn map_resource_to_wasm_dependency(
    store: &mut wasmtime::StoreContextMut<'_, WasiState>,
    resource: wasmtime::component::ResourceAny,
    resource_ty: &types::ResourceType,
    take_ownership: bool,
    bindings: &[DependencyResourceBinding],
) -> Result<Val, ImagodError> {
    let binding = dependency_resource_binding(bindings, resource_ty)?;
    let dynamic = ResourceDynamic::try_from_resource_any(resource, store.as_context_mut())
        .map_err(|e| {
            map_runtime_error(format!("failed to unwrap bridged dependency resource: {e}"))
        })?;
    if dynamic.ty() != binding.host_dynamic_type_id {
        return Err(map_runtime_error(format!(
            "bridged dependency resource '{}' expected host type {} but saw {}",
            binding.resource_name,
            binding.host_dynamic_type_id,
            dynamic.ty()
        )));
    }

    let stored = if take_ownership {
        store
            .data_mut()
            .remove_wasm_dependency_resource(dynamic.rep())
    } else {
        store.data().wasm_dependency_resource(dynamic.rep())
    }
    .ok_or_else(|| {
        map_runtime_error(format!(
            "missing bridged dependency resource '{}' rep={}",
            binding.resource_name,
            dynamic.rep()
        ))
    })?;

    Ok(Val::Resource(stored.resource))
}

fn map_resource_from_wasm_dependency(
    store: &mut wasmtime::StoreContextMut<'_, WasiState>,
    resource: wasmtime::component::ResourceAny,
    resource_ty: &types::ResourceType,
    is_owned: bool,
    bindings: &[DependencyResourceBinding],
) -> Result<Val, ImagodError> {
    if !is_owned {
        return Err(map_runtime_error(
            "borrowed resource results are unsupported for wasm dependency bridge".to_string(),
        ));
    }

    let binding = dependency_resource_binding(bindings, resource_ty)?;
    let rep = store
        .data_mut()
        .store_wasm_dependency_resource(binding.host_dynamic_type_id, resource)?;
    let bridged = ResourceDynamic::new_own(rep, binding.host_dynamic_type_id)
        .try_into_resource_any(store.as_context_mut())
        .map_err(|e| {
            map_runtime_error(format!("failed to wrap bridged dependency resource: {e}"))
        })?;
    Ok(Val::Resource(bridged))
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
    use crate::native_plugins::{
        HasSelf, NativePlugin, NativePluginLinker, NativePluginResult,
        map_native_plugin_linker_error,
    };
    use imago_protocol::ErrorCode;
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::{fs, path::PathBuf};
    use tempfile::TempDir;

    /// Test-only inline WIT bindings used to reproduce a same-package
    /// multi-import native plugin fixture with minimal surface area.
    mod test_multi_import_bindings {
        wasmtime::component::bindgen!({
            inline: r#"
                package test:multi@0.1.0;

                interface a {
                    ping: func();
                }

                interface b {
                    ping: func();
                }

                world host {
                    import a;
                    import b;
                }
            "#,
            world: "host",
        });
    }

    /// Native plugin fixture that exposes multiple imports in one package and
    /// is used to validate package-scoped `mark_native_plugin_linked` dedup.
    #[derive(Clone, Default)]
    struct TestMultiImportPlugin {
        link_calls: Arc<AtomicUsize>,
    }

    impl TestMultiImportPlugin {
        const PACKAGE_NAME: &'static str = "test:multi";
        const IMPORTS: [&'static str; 2] = ["test:multi/a@0.1.0", "test:multi/b@0.1.0"];
        const SYMBOLS: [&'static str; 2] = ["test:multi/a@0.1.0.ping", "test:multi/b@0.1.0.ping"];

        fn link_call_count(&self) -> usize {
            self.link_calls.load(Ordering::Relaxed)
        }
    }

    fn paired_dependency_binding(
        import_resource: types::ResourceType,
        callee_resource: types::ResourceType,
    ) -> Vec<DependencyResourceBinding> {
        vec![DependencyResourceBinding {
            import_resource,
            callee_resource,
            host_dynamic_type_id: 99,
            resource_name: "session".to_string(),
        }]
    }

    #[test]
    fn component_types_match_rejects_distinct_own_resource_types() {
        let import_ty = types::Type::Own(wasmtime::component::ResourceType::host_dynamic(1));
        let callee_ty = types::Type::Own(wasmtime::component::ResourceType::host_dynamic(2));
        assert!(
            !component_types_match(&import_ty, &callee_ty, None),
            "distinct own resource identities must not compare equal"
        );
    }

    #[test]
    fn component_types_match_accepts_matching_borrow_resource_types() {
        let resource_ty = wasmtime::component::ResourceType::host_dynamic(7);
        let import_ty = types::Type::Borrow(resource_ty);
        let callee_ty = types::Type::Borrow(resource_ty);
        assert!(
            component_types_match(&import_ty, &callee_ty, None),
            "identical borrow resource identities should compare equal"
        );
    }

    #[test]
    fn component_types_match_accepts_paired_own_resource_types_for_dependency_bridge() {
        let import_resource = wasmtime::component::ResourceType::host_dynamic(11);
        let callee_resource = wasmtime::component::ResourceType::host_dynamic(22);
        let bindings = paired_dependency_binding(import_resource, callee_resource);
        let import_ty = types::Type::Own(import_resource);
        let callee_ty = types::Type::Own(callee_resource);
        assert!(
            component_types_match(&import_ty, &callee_ty, Some(&bindings)),
            "paired own resource identities should compare equal for dependency bridges"
        );
    }

    #[test]
    fn component_types_match_accepts_paired_borrow_resource_types_for_dependency_bridge() {
        let import_resource = wasmtime::component::ResourceType::host_dynamic(33);
        let callee_resource = wasmtime::component::ResourceType::host_dynamic(44);
        let bindings = paired_dependency_binding(import_resource, callee_resource);
        let import_ty = types::Type::Borrow(import_resource);
        let callee_ty = types::Type::Borrow(callee_resource);
        assert!(
            component_types_match(&import_ty, &callee_ty, Some(&bindings)),
            "paired borrow resource identities should compare equal for dependency bridges"
        );
    }

    #[derive(Clone)]
    struct TestResourceValidationPlugin {
        validate_calls: Arc<AtomicUsize>,
        saw_gpio_resource: Arc<AtomicBool>,
        fail_validation: bool,
    }

    impl TestResourceValidationPlugin {
        const PACKAGE_NAME: &'static str = "test:resource-validation";
        const SYMBOLS: [&'static str; 0] = [];

        fn new(fail_validation: bool) -> Self {
            Self {
                validate_calls: Arc::new(AtomicUsize::new(0)),
                saw_gpio_resource: Arc::new(AtomicBool::new(false)),
                fail_validation,
            }
        }

        fn validate_call_count(&self) -> usize {
            self.validate_calls.load(Ordering::Relaxed)
        }

        fn saw_gpio_resource(&self) -> bool {
            self.saw_gpio_resource.load(Ordering::Relaxed)
        }
    }

    impl NativePlugin for TestResourceValidationPlugin {
        fn package_name(&self) -> &'static str {
            Self::PACKAGE_NAME
        }

        fn supports_import(&self, _import_name: &str) -> bool {
            false
        }

        fn symbols(&self) -> &'static [&'static str] {
            &Self::SYMBOLS
        }

        fn add_to_linker(&self, _linker: &mut NativePluginLinker) -> NativePluginResult<()> {
            Ok(())
        }

        fn validate_resources(
            &self,
            resources: &imagod_ipc::ResourceMap,
        ) -> NativePluginResult<()> {
            self.validate_calls.fetch_add(1, Ordering::Relaxed);
            self.saw_gpio_resource
                .store(resources.contains_key("gpio"), Ordering::Relaxed);
            if self.fail_validation {
                return Err(ImagodError::new(
                    ErrorCode::Internal,
                    "runtime.native_plugin",
                    "resource validation failed",
                ));
            }
            Ok(())
        }
    }

    fn native_plugin_dependency(package_name: &str) -> PluginDependency {
        PluginDependency {
            name: package_name.to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::Native,
            wit: format!("{package_name}/api@0.1.0"),
            requires: Vec::new(),
            component: None,
            capabilities: CapabilityPolicy::default(),
        }
    }

    fn test_store_with_resources(
        engine: &Engine,
        resources: imagod_ipc::ResourceMap,
    ) -> Store<WasiState> {
        let state = WasiState::new(
            wasmtime_wasi::WasiCtxBuilder::new().build(),
            wasmtime_wasi_http::WasiHttpCtx::new(),
            Vec::new(),
            crate::NativePluginContext::new(
                "svc-test".to_string(),
                "release-test".to_string(),
                "runner-test".to_string(),
                imagod_ipc::RunnerAppType::Cli,
                PathBuf::from("/tmp/manager.sock"),
                "secret".to_string(),
                resources,
            ),
        );
        Store::new(engine, state)
    }

    fn imported_instance_type(
        component: &Component,
        engine: &Engine,
        interface_name: &str,
    ) -> types::ComponentInstance {
        component
            .component_type()
            .imports(engine)
            .find_map(|(name, item)| match item {
                types::ComponentItem::ComponentInstance(instance_ty) if name == interface_name => {
                    Some(instance_ty)
                }
                _ => None,
            })
            .expect("imported interface should exist")
    }

    fn write_minimal_component_file(name: &str) -> (TempDir, PathBuf) {
        let tempdir = tempfile::Builder::new()
            .prefix(name)
            .tempdir()
            .expect("tempdir should be created");
        let component_path = tempdir.path().join("plugin.wasm");
        let bytes = wat::parse_str("(component)").expect("wat component should compile");
        fs::write(&component_path, bytes).expect("component bytes should be written");
        (tempdir, component_path)
    }

    fn wasm_plugin_dependency_with_component(
        name: &str,
        component_path: PathBuf,
        imports: Option<Vec<String>>,
        exports: Option<Vec<String>>,
    ) -> PluginDependency {
        PluginDependency {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            kind: PluginKind::Wasm,
            wit: format!("warg://{name}@0.1.0"),
            requires: Vec::new(),
            component: Some(imagod_ipc::PluginComponent {
                path: component_path,
                sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
                imports,
                exports,
            }),
            capabilities: CapabilityPolicy::default(),
        }
    }

    #[test]
    fn collect_dependency_resource_bindings_resolves_callee_resource_types_from_instance() {
        let engine = Engine::default();
        let import_component = Component::new(
            &engine,
            r#"
                (component
                  (import "test:camera/provider@0.1.0"
                    (instance
                      (export "session" (type $session (sub resource)))
                    )
                  )
                )
            "#,
        )
        .expect("import component should compile");
        let import_instance_ty =
            imported_instance_type(&import_component, &engine, "test:camera/provider@0.1.0");
        let dependency_component = Component::new(
            &engine,
            r#"
                (component
                  (type $session (resource (rep i32)))
                  (component $provider
                    (import "import-type-session" (type $import-session (sub resource)))
                    (export "session" (type $import-session))
                  )
                  (instance $provider-instance
                    (instantiate $provider
                      (with "import-type-session" (type $session))
                    )
                  )
                  (export "test:camera/provider@0.1.0" (instance $provider-instance))
                )
            "#,
        )
        .expect("dependency component should compile");
        let mut store = test_store_with_resources(&engine, imagod_ipc::ResourceMap::default());
        let linker = Linker::new(&engine);
        let dependency_instance = linker
            .instantiate(&mut store, &dependency_component)
            .expect("dependency component should instantiate");

        let bindings = collect_dependency_resource_bindings(
            &mut store,
            "test:camera",
            "test:camera/provider@0.1.0",
            &import_instance_ty,
            &dependency_instance,
            &engine,
        )
        .expect("resource bindings should be collected");

        assert_eq!(bindings.len(), 1, "expected one session resource binding");
        let binding = &bindings[0];
        assert_eq!(binding.resource_name, "session");
        assert_ne!(
            binding.import_resource, binding.callee_resource,
            "instantiated dependency resource should be fresh"
        );
        assert!(
            component_types_match(
                &types::Type::Own(binding.import_resource),
                &types::Type::Own(binding.callee_resource),
                Some(&bindings),
            ),
            "paired own resource types should compare equal once the binding is known"
        );
        assert!(
            component_types_match(
                &types::Type::Borrow(binding.import_resource),
                &types::Type::Borrow(binding.callee_resource),
                Some(&bindings),
            ),
            "paired borrow resource types should compare equal once the binding is known"
        );
    }

    impl NativePlugin for TestMultiImportPlugin {
        fn package_name(&self) -> &'static str {
            Self::PACKAGE_NAME
        }

        fn supports_import(&self, import_name: &str) -> bool {
            Self::IMPORTS.contains(&import_name)
        }

        fn symbols(&self) -> &'static [&'static str] {
            &Self::SYMBOLS
        }

        fn add_to_linker(&self, linker: &mut NativePluginLinker) -> NativePluginResult<()> {
            self.link_calls.fetch_add(1, Ordering::Relaxed);
            test_multi_import_bindings::Host_::add_to_linker::<_, HasSelf<_>>(linker, |state| state)
                .map_err(|err| map_native_plugin_linker_error(Self::PACKAGE_NAME, err))
        }
    }

    impl test_multi_import_bindings::test::multi::a::Host for WasiState {
        fn ping(&mut self) {}
    }

    impl test_multi_import_bindings::test::multi::b::Host for WasiState {
        fn ping(&mut self) {}
    }

    #[tokio::test]
    async fn instantiate_plugin_dependencies_validates_native_plugin_resources() {
        let plugin = TestResourceValidationPlugin::new(false);
        let mut registry_builder = crate::native_plugins::NativePluginRegistryBuilder::new();
        registry_builder
            .register_plugin(Arc::new(plugin.clone()))
            .expect("resource validation plugin should register");
        let registry = registry_builder.build();

        let engine = Engine::default();
        let mut store = test_store_with_resources(
            &engine,
            std::collections::BTreeMap::from([("gpio".to_string(), json!({ "digital_pins": [] }))]),
        );
        let dependencies = vec![native_plugin_dependency(
            TestResourceValidationPlugin::PACKAGE_NAME,
        )];
        let checker = crate::capability_checker::DefaultCapabilityChecker;

        let available = instantiate_plugin_dependencies(
            &DefaultPluginResolver,
            &checker,
            &registry,
            &engine,
            &mut store,
            &dependencies,
        )
        .await
        .expect("native dependency resolution should succeed");

        assert!(
            available.contains_key(TestResourceValidationPlugin::PACKAGE_NAME),
            "native plugin should be available after dependency resolution"
        );
        assert_eq!(
            plugin.validate_call_count(),
            1,
            "native plugin resources should be validated exactly once"
        );
        assert!(
            plugin.saw_gpio_resource(),
            "native plugin validation should receive runner resources"
        );
    }

    #[tokio::test]
    async fn instantiate_plugin_dependencies_propagates_resource_validation_error() {
        let plugin = TestResourceValidationPlugin::new(true);
        let mut registry_builder = crate::native_plugins::NativePluginRegistryBuilder::new();
        registry_builder
            .register_plugin(Arc::new(plugin.clone()))
            .expect("resource validation plugin should register");
        let registry = registry_builder.build();

        let engine = Engine::default();
        let mut store = test_store_with_resources(
            &engine,
            std::collections::BTreeMap::from([("gpio".to_string(), json!({ "digital_pins": [] }))]),
        );
        let dependencies = vec![native_plugin_dependency(
            TestResourceValidationPlugin::PACKAGE_NAME,
        )];
        let checker = crate::capability_checker::DefaultCapabilityChecker;

        let err = match instantiate_plugin_dependencies(
            &DefaultPluginResolver,
            &checker,
            &registry,
            &engine,
            &mut store,
            &dependencies,
        )
        .await
        {
            Ok(_) => panic!("resource validation failure should stop dependency resolution"),
            Err(err) => err,
        };

        assert_eq!(err.stage, "runtime.native_plugin");
        assert!(
            err.message.contains("resource validation failed"),
            "unexpected validation message: {}",
            err.message
        );
        assert_eq!(
            plugin.validate_call_count(),
            1,
            "failing resource validation should still execute exactly once"
        );
    }

    #[test]
    fn component_interfaces_for_dependency_uses_metadata_without_loading_file() {
        let engine = Engine::default();
        let dependency = wasm_plugin_dependency_with_component(
            "yieldspace:plugin/metadata-only",
            PathBuf::from("/nonexistent/plugin.wasm"),
            Some(vec!["yieldspace:plugin/provider".to_string()]),
            Some(vec!["yieldspace:plugin/metadata-only".to_string()]),
        );
        let mut preloaded_components = BTreeMap::new();

        let interfaces =
            component_interfaces_for_dependency(&dependency, &engine, &mut preloaded_components)
                .expect("interfaces should come from metadata");

        assert_eq!(
            interfaces.imports,
            vec!["yieldspace:plugin/provider".to_string()]
        );
        assert_eq!(
            interfaces.exports,
            BTreeSet::from(["yieldspace:plugin/metadata-only".to_string()])
        );
        assert!(
            preloaded_components.is_empty(),
            "metadata path should avoid preload inserts"
        );
    }

    #[test]
    fn load_component_for_instantiation_reuses_preloaded_component() {
        let engine = Engine::default();
        let (_tempdir, component_path) = write_minimal_component_file("plugin-preload-reuse");
        let dependency = wasm_plugin_dependency_with_component(
            "yieldspace:plugin/preload",
            component_path,
            None,
            None,
        );
        let mut preloaded_components = BTreeMap::new();

        let interfaces =
            component_interfaces_for_dependency(&dependency, &engine, &mut preloaded_components)
                .expect("fallback path should preload component");
        assert!(
            preloaded_components.contains_key(&dependency.name),
            "legacy fallback should preload loaded component"
        );

        let _component =
            load_component_for_instantiation(&dependency, &engine, &mut preloaded_components)
                .expect("instantiation should reuse preloaded component");
        assert!(
            preloaded_components.is_empty(),
            "preloaded component should be consumed by instantiation"
        );
        assert!(
            interfaces.imports.is_empty() && interfaces.exports.is_empty(),
            "minimal component should have no instance imports/exports"
        );
    }

    #[test]
    fn multi_import_plugin_link_twice_fails_with_duplicate_entry() {
        let plugin = TestMultiImportPlugin::default();
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);

        plugin
            .add_to_linker(&mut linker)
            .expect("first linker registration should succeed");

        let err = plugin
            .add_to_linker(&mut linker)
            .expect_err("second linker registration should fail");
        assert!(
            err.message.contains("defined twice")
                || err.message.contains("already defined")
                || err.message.contains("defined more than once"),
            "unexpected error: {}",
            err.message
        );
        assert_eq!(
            plugin.link_call_count(),
            2,
            "duplicate test must invoke add_to_linker twice"
        );
    }

    #[test]
    fn package_scoped_guard_links_multi_import_plugin_once() {
        let plugin = TestMultiImportPlugin::default();
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        let mut linked_native_plugin_packages = BTreeSet::new();

        for import_name in TestMultiImportPlugin::IMPORTS {
            assert!(
                plugin.supports_import(import_name),
                "test plugin must support import '{}'",
                import_name
            );
            if mark_native_plugin_linked(&mut linked_native_plugin_packages, plugin.package_name())
            {
                plugin
                    .add_to_linker(&mut linker)
                    .expect("guarded linker registration should succeed");
            }
        }

        assert_eq!(
            plugin.link_call_count(),
            1,
            "package-scoped guard must prevent relinking for second import"
        );
        assert!(
            linked_native_plugin_packages.contains(TestMultiImportPlugin::PACKAGE_NAME),
            "linked package set should include test plugin package"
        );
    }

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

    #[test]
    fn mark_native_plugin_linked_is_package_scoped() {
        let mut linked_native_plugin_packages = BTreeSet::new();

        assert!(
            mark_native_plugin_linked(&mut linked_native_plugin_packages, "imago:nanokvm"),
            "first import from same package should link once"
        );
        assert!(
            !mark_native_plugin_linked(&mut linked_native_plugin_packages, "imago:nanokvm"),
            "second import from same package should not relink"
        );
        assert!(
            mark_native_plugin_linked(&mut linked_native_plugin_packages, "imago:node"),
            "another package should still link"
        );
    }
}
