/// Built-in native plugin metadata shared by config loading and runner startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinNativePluginDescriptor {
    /// Canonical package name used in manifests and runtime registries.
    pub package_name: &'static str,
    /// Whether the plugin is enabled when `[runtime].features` is unset/false/empty.
    pub default_enabled: bool,
}

/// Canonical descriptor table for all built-in native plugins linked into `imagod`.
pub const BUILTIN_NATIVE_PLUGIN_DESCRIPTORS: [BuiltinNativePluginDescriptor; 6] = [
    BuiltinNativePluginDescriptor {
        package_name: "imago:admin",
        default_enabled: true,
    },
    BuiltinNativePluginDescriptor {
        package_name: "imago:node",
        default_enabled: true,
    },
    BuiltinNativePluginDescriptor {
        package_name: "imago:experimental-gpio",
        default_enabled: false,
    },
    BuiltinNativePluginDescriptor {
        package_name: "imago:experimental-i2c",
        default_enabled: false,
    },
    BuiltinNativePluginDescriptor {
        package_name: "imago:usb",
        default_enabled: false,
    },
    BuiltinNativePluginDescriptor {
        package_name: "imago:v4l2",
        default_enabled: false,
    },
];

/// Returns true when `package_name` matches one of the built-in native plugins.
pub fn is_builtin_native_plugin_package_name(package_name: &str) -> bool {
    BUILTIN_NATIVE_PLUGIN_DESCRIPTORS
        .iter()
        .any(|descriptor| descriptor.package_name == package_name)
}
