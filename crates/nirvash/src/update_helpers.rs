use std::collections::BTreeMap;

/// Returns a cloned sequence with one in-bounds element replaced.
///
/// Panics when `index` is out of bounds. The DSL uses this helper for
/// immutable sequence-update expressions where the index is already guarded.
pub fn sequence_update<T>(mut base: Vec<T>, index: usize, value: T) -> Vec<T> {
    assert!(
        index < base.len(),
        "sequence_update index {index} out of bounds for len {}",
        base.len()
    );
    base[index] = value;
    base
}

/// Returns a cloned finite function with one key inserted or replaced.
pub fn function_update<K, V>(mut base: BTreeMap<K, V>, key: K, value: V) -> BTreeMap<K, V>
where
    K: Ord,
{
    base.insert(key, value);
    base
}

#[cfg(test)]
mod tests {
    use super::{function_update, sequence_update};
    use std::collections::BTreeMap;

    #[test]
    fn sequence_update_replaces_selected_index() {
        assert_eq!(sequence_update(vec![1, 2, 3], 1, 9), vec![1, 9, 3]);
    }

    #[test]
    fn function_update_inserts_and_replaces_key() {
        let map = BTreeMap::from([(1, "one"), (2, "two")]);
        let inserted = function_update(map.clone(), 3, "three");
        let replaced = function_update(map, 2, "TWO");

        assert_eq!(inserted.get(&3), Some(&"three"));
        assert_eq!(replaced.get(&2), Some(&"TWO"));
    }
}
