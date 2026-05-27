use sha2::{Digest, Sha256};

pub fn stable_id(prefix: &str, schema_version: u32, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(schema_version.to_string().as_bytes());
    hasher.update(b"\0");
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b"\0");
    }
    let digest = hasher.finalize();
    format!("{prefix}_{}", hex::encode(&digest[..8]))
}

pub fn safe_slug(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_are_repeatable_and_prefixed() {
        let a = stable_id("D", 1, &["app/main.py", "x", "1:0"]);
        let b = stable_id("D", 1, &["app/main.py", "x", "1:0"]);
        assert_eq!(a, b);
        assert!(a.starts_with("D_"));
    }

    #[test]
    fn safe_slug_replaces_dot_unsafe_characters() {
        assert_eq!(
            safe_slug("app/routers/tests.py::create-test"),
            "app_routers_tests_py__create_test"
        );
    }
}
