use imagod_common::ImagodError;
use imagod_ipc::CapabilityPolicy;
use imagod_runtime_internal::CapabilityChecker;
use wasmtime::{Engine, component::types};

use crate::map_runtime_unauthorized_error;

#[derive(Default)]
pub(crate) struct DefaultCapabilityChecker;

impl CapabilityChecker for DefaultCapabilityChecker {
    fn is_dependency_function_allowed(
        &self,
        policy: &CapabilityPolicy,
        dependency_name: &str,
        interface_name: &str,
        function_name: &str,
    ) -> bool {
        if policy.privileged {
            return true;
        }
        let Some(rules) = policy.deps.get(dependency_name) else {
            return false;
        };
        rules
            .iter()
            .any(|rule| rule_matches(rule, interface_name, function_name))
    }

    fn ensure_dependency_function_allowed(
        &self,
        caller_name: &str,
        policy: &CapabilityPolicy,
        dependency_name: &str,
        interface_name: &str,
        function_name: &str,
    ) -> Result<(), ImagodError> {
        if self.is_dependency_function_allowed(
            policy,
            dependency_name,
            interface_name,
            function_name,
        ) {
            return Ok(());
        }

        Err(map_runtime_unauthorized_error(format!(
            "capability denied caller '{}' -> dependency '{}' function '{}.{}'",
            caller_name, dependency_name, interface_name, function_name
        )))
    }

    fn is_wasi_function_allowed(
        &self,
        policy: &CapabilityPolicy,
        interface_name: &str,
        function_name: &str,
    ) -> bool {
        if policy.privileged {
            return true;
        }
        let Some(rules) = policy.wasi.get(interface_name) else {
            return false;
        };
        rules
            .iter()
            .any(|rule| rule_matches(rule, interface_name, function_name))
    }

    fn ensure_wasi_function_allowed(
        &self,
        caller_name: &str,
        policy: &CapabilityPolicy,
        interface_name: &str,
        function_name: &str,
    ) -> Result<(), ImagodError> {
        if self.is_wasi_function_allowed(policy, interface_name, function_name) {
            return Ok(());
        }

        Err(map_runtime_unauthorized_error(format!(
            "capability denied caller '{}' -> wasi '{}' function '{}'",
            caller_name, interface_name, function_name
        )))
    }
}

pub(crate) fn enforce_wasi_import_capabilities<C>(
    checker: &C,
    caller_name: &str,
    policy: &CapabilityPolicy,
    interface_name: &str,
    instance_ty: &types::ComponentInstance,
    engine: &Engine,
) -> Result<(), ImagodError>
where
    C: CapabilityChecker + ?Sized,
{
    for (function_name, item) in instance_ty.exports(engine) {
        let types::ComponentItem::ComponentFunc(_) = item else {
            continue;
        };
        checker.ensure_wasi_function_allowed(caller_name, policy, interface_name, function_name)?;
    }
    Ok(())
}

fn rule_matches(rule: &str, interface_name: &str, function_name: &str) -> bool {
    rule == "*"
        || rule == function_name
        || rule == format!("{interface_name}.{function_name}")
        || rule == format!("{interface_name}/{function_name}")
        || rule == format!("{interface_name}#{function_name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasi_capability_denies_when_policy_is_empty() {
        let checker = DefaultCapabilityChecker;
        let allowed = checker.is_wasi_function_allowed(
            &CapabilityPolicy::default(),
            "wasi:cli/environment",
            "get-environment",
        );
        assert!(!allowed, "empty policy should deny wasi function");
    }

    #[test]
    fn wasi_capability_allows_when_privileged() {
        let checker = DefaultCapabilityChecker;
        let policy = CapabilityPolicy {
            privileged: true,
            deps: std::collections::BTreeMap::new(),
            wasi: std::collections::BTreeMap::new(),
        };
        let allowed =
            checker.is_wasi_function_allowed(&policy, "wasi:cli/environment", "get-environment");
        assert!(allowed, "privileged policy should allow all wasi calls");
    }

    #[test]
    fn wasi_capability_allows_when_rule_is_wildcard() {
        let checker = DefaultCapabilityChecker;
        let policy = CapabilityPolicy {
            privileged: false,
            deps: std::collections::BTreeMap::new(),
            wasi: std::collections::BTreeMap::from([(
                "wasi:cli/environment".to_string(),
                vec!["*".to_string()],
            )]),
        };
        let allowed =
            checker.is_wasi_function_allowed(&policy, "wasi:cli/environment", "get-environment");
        assert!(allowed, "wildcard rule should allow wasi function");
    }

    #[test]
    fn wasi_capability_rejects_unlisted_function() {
        let checker = DefaultCapabilityChecker;
        let policy = CapabilityPolicy {
            privileged: false,
            deps: std::collections::BTreeMap::new(),
            wasi: std::collections::BTreeMap::from([(
                "wasi:cli/environment".to_string(),
                vec!["get-arguments".to_string()],
            )]),
        };
        let allowed =
            checker.is_wasi_function_allowed(&policy, "wasi:cli/environment", "get-environment");
        assert!(!allowed, "unlisted function should be denied");
    }

    #[test]
    fn wasi_capability_denial_maps_to_unauthorized() {
        let checker = DefaultCapabilityChecker;
        let err = checker
            .ensure_wasi_function_allowed(
                "app",
                &CapabilityPolicy::default(),
                "wasi:cli/environment",
                "get-environment",
            )
            .expect_err("empty policy should deny wasi function");
        assert_eq!(err.code, imago_protocol::ErrorCode::Unauthorized);
        assert!(
            err.message.contains("capability denied caller 'app'"),
            "unexpected message: {}",
            err.message
        );
    }
}
