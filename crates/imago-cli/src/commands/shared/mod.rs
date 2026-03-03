pub(crate) mod dependency;

#[cfg(test)]
mod tests {
    use super::dependency::StandardDependencyResolver;

    #[test]
    fn dependency_module_exports_default_resolver() {
        let resolver = StandardDependencyResolver;
        let _ = resolver;
    }
}
