//! A deliberately small JSON Schema checker for the test suite.
//!
//! It supports exactly the keywords `schemas/results.schema.json` uses — `type`, `const`, `enum`,
//! `required`, `properties`, `additionalProperties: false`, `items`, `minItems`, `minimum` and
//! the one `pattern` (a `sha256:` digest) — and ignores everything else. Pulling a full validator
//! crate in as a dev-dependency would test the validator; this tests the schema.

use serde_json::Value as J;

pub fn validate(instance: &J, schema: &J) -> Vec<String> {
    let mut errors = Vec::new();
    walk(instance, schema, "$", &mut errors);
    errors
}

fn type_name(v: &J) -> &'static str {
    match v {
        J::Null => "null",
        J::Bool(_) => "boolean",
        J::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        J::String(_) => "string",
        J::Array(_) => "array",
        J::Object(_) => "object",
    }
}

fn type_matches(v: &J, want: &str) -> bool {
    match want {
        "number" => matches!(v, J::Number(_)),
        "integer" => matches!(v, J::Number(n) if n.is_i64() || n.is_u64()),
        other => type_name(v) == other,
    }
}

fn walk(instance: &J, schema: &J, path: &str, errors: &mut Vec<String>) {
    let s = match schema.as_object() {
        Some(s) => s,
        None => return,
    };

    if let Some(want) = s.get("const") {
        if instance != want {
            errors.push(format!("{path}: expected const {want}, found {instance}"));
        }
    }

    if let Some(J::Array(options)) = s.get("enum") {
        if !options.contains(instance) {
            errors.push(format!("{path}: {instance} is not one of {options:?}"));
        }
    }

    match s.get("type") {
        Some(J::String(t)) => {
            if !type_matches(instance, t) {
                errors.push(format!(
                    "{path}: expected {t}, found {}",
                    type_name(instance)
                ));
            }
        }
        Some(J::Array(ts)) => {
            let ok = ts
                .iter()
                .filter_map(|t| t.as_str())
                .any(|t| type_matches(instance, t));
            if !ok {
                errors.push(format!(
                    "{path}: expected one of {ts:?}, found {}",
                    type_name(instance)
                ));
            }
        }
        _ => {}
    }

    if let (Some(J::String(pattern)), Some(text)) = (s.get("pattern"), instance.as_str()) {
        // The only pattern in this schema.
        if pattern == "^sha256:[0-9a-f]{64}$" {
            let ok = text.strip_prefix("sha256:").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
            });
            if !ok {
                errors.push(format!("{path}: `{text}` is not a sha256 digest"));
            }
        }
    }

    if let (Some(J::Number(min)), Some(n)) = (s.get("minimum"), instance.as_f64()) {
        if n < min.as_f64().unwrap_or(f64::MIN) {
            errors.push(format!("{path}: {n} is below the minimum"));
        }
    }

    if let Some(obj) = instance.as_object() {
        if let Some(J::Array(required)) = s.get("required") {
            for key in required.iter().filter_map(|k| k.as_str()) {
                if !obj.contains_key(key) {
                    errors.push(format!("{path}: missing required property `{key}`"));
                }
            }
        }
        let properties = s.get("properties").and_then(|p| p.as_object());
        if s.get("additionalProperties") == Some(&J::Bool(false)) {
            for key in obj.keys() {
                let known = properties.is_some_and(|p| p.contains_key(key));
                if !known {
                    errors.push(format!("{path}: unexpected property `{key}`"));
                }
            }
        }
        if let Some(properties) = properties {
            for (key, sub) in properties {
                if let Some(v) = obj.get(key) {
                    walk(v, sub, &format!("{path}.{key}"), errors);
                }
            }
        }
    }

    if let Some(items) = instance.as_array() {
        if let Some(J::Number(min)) = s.get("minItems") {
            if (items.len() as u64) < min.as_u64().unwrap_or(0) {
                errors.push(format!(
                    "{path}: expected at least {min} items, found {}",
                    items.len()
                ));
            }
        }
        if let Some(sub) = s.get("items") {
            for (i, item) in items.iter().enumerate() {
                walk(item, sub, &format!("{path}[{i}]"), errors);
            }
        }
    }
}
