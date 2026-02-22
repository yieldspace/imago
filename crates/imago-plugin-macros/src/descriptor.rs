use std::path::Path;

use wit_parser::{Resolve, TypeDefKind, WorldItem};

#[derive(Debug)]
pub(crate) struct WitDescriptor {
    pub(crate) package_name: String,
    pub(crate) import_names: Vec<String>,
    pub(crate) symbols: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ParseOptions {
    pub(crate) allow_multiple_imports: bool,
    pub(crate) allow_non_resource_types: bool,
}

pub(crate) fn parse_wit_descriptor(
    wit_path: &Path,
    world_name: &str,
    options: ParseOptions,
) -> std::result::Result<WitDescriptor, String> {
    let mut resolve = Resolve::default();
    let (top_package, _) = resolve
        .push_path(wit_path)
        .map_err(|err| format!("failed to parse WIT path: {err}"))?;
    let top_package_name = resolve.packages[top_package].name.clone();
    let world_id = resolve
        .select_world(&[top_package], Some(world_name))
        .map_err(|err| format!("world '{world_name}' was not found: {err}"))?;
    let world = &resolve.worlds[world_id];

    if !world.exports.is_empty() {
        return Err(
            "world exports are not supported for native plugin descriptor generation".to_string(),
        );
    }

    let mut imported_interfaces = Vec::new();
    for (_key, item) in &world.imports {
        match item {
            WorldItem::Interface { id, .. } => {
                let interface = &resolve.interfaces[*id];
                let Some(package_id) = interface.package else {
                    continue;
                };
                let package_name = &resolve.packages[package_id].name;
                if package_name.namespace != top_package_name.namespace
                    || package_name.name != top_package_name.name
                {
                    continue;
                }
                let import_name = resolve
                    .id_of(*id)
                    .ok_or_else(|| "anonymous interface import is not supported".to_string())?;
                imported_interfaces.push((*id, import_name));
            }
            WorldItem::Function(function) => {
                return Err(format!(
                    "imported function '{}' is not supported; import one interface only",
                    function.name
                ));
            }
            WorldItem::Type(_) => {
                return Err("imported type is not supported; import one interface only".to_string());
            }
        }
    }

    if options.allow_multiple_imports {
        if imported_interfaces.is_empty() {
            return Err("world must import at least one interface".to_string());
        }
    } else if imported_interfaces.len() != 1 {
        return Err(format!(
            "world must import exactly one interface, found {}",
            imported_interfaces.len()
        ));
    }

    let mut package_name: Option<String> = None;
    let mut import_names = Vec::with_capacity(imported_interfaces.len());
    let mut symbols = Vec::new();

    for (interface_id, import_name) in imported_interfaces {
        let interface = &resolve.interfaces[interface_id];
        import_names.push(import_name.to_string());

        if !options.allow_non_resource_types {
            for (type_name, type_id) in &interface.types {
                let type_def = &resolve.types[*type_id];
                if !matches!(type_def.kind, TypeDefKind::Resource) {
                    return Err(format!(
                        "imported interface type '{type_name}' is not supported; only resource types are allowed"
                    ));
                }
            }
        }

        if interface.functions.is_empty() {
            return Err("imported interface must define at least one function".to_string());
        }

        let package_id = interface
            .package
            .ok_or_else(|| "imported interface package metadata is missing".to_string())?;
        let interface_package = &resolve.packages[package_id].name;
        let interface_package =
            format!("{}:{}", interface_package.namespace, interface_package.name);

        if let Some(expected_package) = &package_name {
            if expected_package != &interface_package {
                return Err(format!(
                    "all imported interfaces must belong to the same package; expected '{expected_package}', found '{interface_package}'",
                ));
            }
        } else {
            package_name = Some(interface_package);
        }

        symbols.extend(
            interface
                .functions
                .keys()
                .map(|function_name| format!("{import_name}.{function_name}")),
        );
    }

    let package_name =
        package_name.ok_or_else(|| "failed to determine package name from imports".to_string())?;
    Ok(WitDescriptor {
        package_name,
        import_names,
        symbols,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn new_temp_dir(test_name: &str) -> PathBuf {
        let unique = format!(
            "imago-plugin-macros-tests-{}-{}-{}",
            test_name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after UNIX_EPOCH")
                .as_nanos(),
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root).expect("temp dir should be created");
        root
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent should be created");
        }
        std::fs::write(path, text).expect("file should be written");
    }

    #[test]
    fn parse_wit_descriptor_reads_package_import_and_symbols() {
        let root = new_temp_dir("valid");
        write(
            &root.join("wit/package.wit"),
            r#"
package imago:admin@0.1.0;

interface runtime {
    service-name: func() -> string;
    release-hash: func() -> string;
}

world host {
    import runtime;
}
"#,
        );

        let descriptor = parse_wit_descriptor(&root.join("wit"), "host", ParseOptions::default())
            .expect("valid wit should produce descriptor");

        assert_eq!(descriptor.package_name, "imago:admin");
        assert_eq!(
            descriptor.import_names,
            vec!["imago:admin/runtime@0.1.0".to_string()]
        );
        assert_eq!(
            descriptor.symbols,
            vec![
                "imago:admin/runtime@0.1.0.service-name".to_string(),
                "imago:admin/runtime@0.1.0.release-hash".to_string(),
            ]
        );
    }

    #[test]
    fn parse_wit_descriptor_rejects_multiple_import_interfaces() {
        let root = new_temp_dir("multiple-imports");
        write(
            &root.join("wit/package.wit"),
            r#"
package imago:admin@0.1.0;

interface runtime {
    ping: func() -> string;
}

interface extra {
    pong: func() -> string;
}

world host {
    import runtime;
    import extra;
}
"#,
        );

        let err = parse_wit_descriptor(&root.join("wit"), "host", ParseOptions::default())
            .expect_err("multiple imports should fail");
        assert!(
            err.contains("exactly one interface"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_wit_descriptor_allows_resource_types() {
        let root = new_temp_dir("resource-type");
        write(
            &root.join("wit/package.wit"),
            r#"
package imago:node@0.1.0;

interface rpc {
    resource connection {
        disconnect: func();
    }

    connect: func(addr: string) -> result<connection, string>;
}

world host {
    import rpc;
}
"#,
        );

        let descriptor = parse_wit_descriptor(&root.join("wit"), "host", ParseOptions::default())
            .expect("resource types should be accepted");
        assert_eq!(descriptor.package_name, "imago:node");
        assert_eq!(
            descriptor.import_names,
            vec!["imago:node/rpc@0.1.0".to_string()]
        );
        assert!(
            descriptor
                .symbols
                .iter()
                .any(|symbol| symbol.ends_with(".connect")),
            "expected connect symbol in {:?}",
            descriptor.symbols
        );
    }

    #[test]
    fn parse_wit_descriptor_rejects_non_function_types() {
        let root = new_temp_dir("non-function-type");
        write(
            &root.join("wit/package.wit"),
            r#"
package imago:admin@0.1.0;

interface runtime {
    type app-state = string;
    service-name: func() -> string;
}

world host {
    import runtime;
}
"#,
        );

        let err = parse_wit_descriptor(&root.join("wit"), "host", ParseOptions::default())
            .expect_err("interface types should fail");
        assert!(
            err.contains("only resource types are allowed"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_wit_descriptor_allows_multiple_import_interfaces_when_enabled() {
        let root = new_temp_dir("multiple-imports-enabled");
        write(
            &root.join("wit/package.wit"),
            r#"
package imago:admin@0.1.0;

interface runtime {
    ping: func() -> string;
}

interface extra {
    pong: func() -> string;
}

world host {
    import runtime;
    import extra;
}
"#,
        );

        let descriptor = parse_wit_descriptor(
            &root.join("wit"),
            "host",
            ParseOptions {
                allow_multiple_imports: true,
                allow_non_resource_types: true,
            },
        )
        .expect("multiple imports should be supported when enabled");

        assert_eq!(descriptor.package_name, "imago:admin");
        assert_eq!(
            descriptor.import_names,
            vec![
                "imago:admin/runtime@0.1.0".to_string(),
                "imago:admin/extra@0.1.0".to_string()
            ]
        );
    }

    #[test]
    fn parse_wit_descriptor_allows_non_resource_types_when_enabled() {
        let root = new_temp_dir("non-resource-type-enabled");
        write(
            &root.join("wit/package.wit"),
            r#"
package imago:admin@0.1.0;

interface runtime {
    enum state {
        started,
        stopped,
    }
    service-name: func() -> string;
}

world host {
    import runtime;
}
"#,
        );

        let descriptor = parse_wit_descriptor(
            &root.join("wit"),
            "host",
            ParseOptions {
                allow_multiple_imports: false,
                allow_non_resource_types: true,
            },
        )
        .expect("non-resource types should be supported when enabled");

        assert_eq!(descriptor.package_name, "imago:admin");
        assert_eq!(descriptor.symbols.len(), 1);
        assert_eq!(
            descriptor.symbols[0],
            "imago:admin/runtime@0.1.0.service-name"
        );
    }
}
