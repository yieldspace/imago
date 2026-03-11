use super::*;

pub(in crate::commands::build) fn parse_root_capabilities(
    root: &toml::Table,
) -> anyhow::Result<ManifestCapabilityPolicy> {
    parse_capability_policy(root.get("capabilities"), "capabilities")
}

pub(in crate::commands::build) fn parse_capability_policy(
    value: Option<&TomlValue>,
    field_name: &str,
) -> anyhow::Result<ManifestCapabilityPolicy> {
    let Some(value) = value else {
        return Ok(ManifestCapabilityPolicy::default());
    };
    let table = value
        .as_table()
        .ok_or_else(|| anyhow!("{field_name} must be a table"))?;

    for key in table.keys() {
        if !matches!(key.as_str(), "privileged" | "deps" | "wasi") {
            return Err(anyhow!("{field_name}.{key} is not supported"));
        }
    }

    let privileged = match table.get("privileged") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| anyhow!("{field_name}.privileged must be a boolean"))?,
    };

    let deps = parse_deps_capability_rules(table.get("deps"), &format!("{field_name}.deps"))?;
    let wasi = parse_wasi_capability_rules(table.get("wasi"), &format!("{field_name}.wasi"))?;

    Ok(ManifestCapabilityPolicy {
        privileged,
        deps,
        wasi,
    })
}

pub(in crate::commands::build) fn parse_deps_capability_rules(
    value: Option<&TomlValue>,
    field_name: &str,
) -> anyhow::Result<BTreeMap<String, Vec<String>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };

    if let Some(rule) = value.as_str() {
        if rule.trim() == "*" {
            return Ok(BTreeMap::from([("*".to_string(), vec!["*".to_string()])]));
        }
        return Err(anyhow!("{field_name} must be \"*\" or a table"));
    }

    if value.as_table().is_none() {
        return Err(anyhow!("{field_name} must be \"*\" or a table"));
    }

    parse_capability_rule_table(Some(value), field_name)
}

pub(in crate::commands::build) fn parse_wasi_capability_rules(
    value: Option<&TomlValue>,
    field_name: &str,
) -> anyhow::Result<BTreeMap<String, Vec<String>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };

    if let Some(allow_all) = value.as_bool() {
        if allow_all {
            return Ok(BTreeMap::from([("*".to_string(), vec!["*".to_string()])]));
        }
        return Ok(BTreeMap::new());
    }

    parse_capability_rule_table(Some(value), field_name)
}

pub(in crate::commands::build) fn parse_capability_rule_table(
    value: Option<&TomlValue>,
    field_name: &str,
) -> anyhow::Result<BTreeMap<String, Vec<String>>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let table = value
        .as_table()
        .ok_or_else(|| anyhow!("{field_name} must be a table"))?;

    let mut normalized = BTreeMap::new();
    for (key, value) in table {
        if key.trim().is_empty() {
            return Err(anyhow!("{field_name} contains an empty key"));
        }
        let rules = parse_capability_rule_list(value, &format!("{field_name}.{key}"))?;
        if !rules.is_empty() {
            normalized.insert(key.clone(), rules);
        }
    }
    Ok(normalized)
}

pub(in crate::commands::build) fn parse_capability_rule_list(
    value: &TomlValue,
    field_name: &str,
) -> anyhow::Result<Vec<String>> {
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("{field_name} must be an array of strings"))?;
    let mut rules = Vec::with_capacity(array.len());
    for (index, value) in array.iter().enumerate() {
        let text = value
            .as_str()
            .ok_or_else(|| anyhow!("{field_name}[{index}] must be a string"))?
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(anyhow!("{field_name}[{index}] must not be empty"));
        }
        rules.push(text);
    }
    Ok(normalize_string_list(rules))
}

pub(in crate::commands::build) fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            set.insert(value.to_string());
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toml::Value as TomlValue;

    use super::{
        parse_capability_rule_list, parse_deps_capability_rules, parse_wasi_capability_rules,
    };

    #[test]
    fn parse_deps_capability_rules_accepts_allow_all() {
        let parsed =
            parse_deps_capability_rules(Some(&TomlValue::String("*".to_string())), "cap.deps")
                .expect("'*' rules should parse");
        assert_eq!(
            parsed,
            BTreeMap::from([("*".to_string(), vec!["*".to_string()])])
        );
    }

    #[test]
    fn parse_wasi_capability_rules_false_returns_empty_rules() {
        let parsed = parse_wasi_capability_rules(Some(&TomlValue::Boolean(false)), "cap.wasi")
            .expect("false boolean should parse");
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_capability_rule_list_trims_and_deduplicates_values() {
        let value = TomlValue::Array(vec![
            TomlValue::String("streams".to_string()),
            TomlValue::String(" streams ".to_string()),
            TomlValue::String("poll".to_string()),
        ]);

        let rules =
            parse_capability_rule_list(&value, "cap.wasi.io").expect("rule list should normalize");
        assert_eq!(rules, vec!["poll".to_string(), "streams".to_string()]);
    }
}
