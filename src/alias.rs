use crate::ir::Place;

pub fn normalize_attribute(class_name: Option<&str>, base_expr: &str, attr: &str) -> Place {
    let base = match (class_name, base_expr) {
        (Some(class_name), "self") => format!("InstanceField({class_name})"),
        (Some(class_name), "cls") => format!("ClassField({class_name})"),
        (_, expr) if is_simple_name(expr) => expr.to_string(),
        _ => "*".to_string(),
    };

    Place::Attribute {
        base,
        attr: attr.to_string(),
    }
}

pub fn normalize_subscript(base_expr: &str, index_expr: Option<&str>) -> Place {
    let base = if is_simple_name(base_expr) {
        base_expr.to_string()
    } else {
        "*".to_string()
    };
    let index = index_expr
        .filter(|expr| is_simple_index(expr))
        .unwrap_or("*")
        .to_string();

    Place::Subscript { base, index }
}

fn is_simple_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn is_simple_index(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '"' || ch == '\'')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_attribute_is_class_field_sensitive() {
        let place = normalize_attribute(Some("ClassName"), "self", "token");
        assert_eq!(
            place,
            Place::Attribute {
                base: "InstanceField(ClassName)".to_string(),
                attr: "token".to_string()
            }
        );
    }

    #[test]
    fn unknown_attribute_uses_field_based_fallback() {
        let place = normalize_attribute(None, "factory()", "token");
        assert_eq!(
            place,
            Place::Attribute {
                base: "*".to_string(),
                attr: "token".to_string()
            }
        );
    }

    #[test]
    fn subscript_uses_field_sensitive_base_and_index() {
        let place = normalize_subscript("items", Some("index"));
        assert_eq!(
            place,
            Place::Subscript {
                base: "items".to_string(),
                index: "index".to_string()
            }
        );
    }
}
