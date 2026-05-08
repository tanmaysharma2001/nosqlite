//! Minimal JSON Schema validator.
//!
//! Supported keywords: `type`, `required`, `properties`, `additionalProperties`,
//! `items`, `enum`, `minimum`, `maximum`, `exclusiveMinimum`,
//! `exclusiveMaximum`, `minLength`, `maxLength`, `minItems`, `maxItems`,
//! `multipleOf`, `const`. Other keywords are ignored.
//!
//! Validation is invoked from `Collection::insert_*` / `update_*` /
//! `replace_one` when a validator is registered for that collection.

use crate::error::{Error, Result};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ValidationLevel {
    /// Reject inserts/updates that fail validation.
    #[default]
    Strict,
    /// Allow them through but the failures can be retrieved via the error type
    /// when needed. We currently still surface errors but tag them differently.
    Warn,
}

#[derive(Debug, Clone)]
pub struct Validator {
    pub schema: Value,
    pub level: ValidationLevel,
}

impl Validator {
    pub fn new(schema: Value, level: ValidationLevel) -> Self {
        Self { schema, level }
    }

    /// Run the schema against `doc`. In `Strict` mode any failure is
    /// returned as `Error::ValidationFailed`; in `Warn` mode the document
    /// passes through and the failure messages are collected via
    /// [`Validator::validate_collect`] if the caller cares about them.
    pub fn validate(&self, doc: &Value) -> Result<()> {
        let errors = check(&self.schema, doc, "");
        if errors.is_empty() {
            return Ok(());
        }
        match self.level {
            ValidationLevel::Strict => Err(Error::ValidationFailed(errors.join("; "))),
            ValidationLevel::Warn => Ok(()),
        }
    }

    /// Run the schema against `doc` and return the list of failure messages,
    /// regardless of validation level. Useful in `Warn` mode where the caller
    /// wants to log violations without rejecting writes.
    pub fn validate_collect(&self, doc: &Value) -> Vec<String> {
        check(&self.schema, doc, "")
    }
}

fn check(schema: &Value, value: &Value, path: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let s = match schema.as_object() {
        Some(o) => o,
        None => return errors,
    };

    if let Some(t) = s.get("type") {
        if !type_matches(t, value) {
            errors.push(format!(
                "{}: expected type {} but got {}",
                pretty_path(path),
                t,
                describe(value)
            ));
            // No point checking further constraints if type is wrong.
            return errors;
        }
    }

    if let Some(c) = s.get("const") {
        if c != value {
            errors.push(format!("{}: must equal const value", pretty_path(path)));
        }
    }

    if let Some(en) = s.get("enum").and_then(|v| v.as_array()) {
        if !en.iter().any(|x| x == value) {
            errors.push(format!("{}: not in enum", pretty_path(path)));
        }
    }

    match value {
        Value::Object(obj) => {
            if let Some(req) = s.get("required").and_then(|v| v.as_array()) {
                for r in req {
                    if let Some(name) = r.as_str() {
                        if !obj.contains_key(name) {
                            errors.push(format!(
                                "{} missing required field {}",
                                pretty_path(path),
                                name
                            ));
                        }
                    }
                }
            }
            if let Some(props) = s.get("properties").and_then(|v| v.as_object()) {
                for (k, prop_schema) in props {
                    if let Some(child) = obj.get(k) {
                        let child_path = if path.is_empty() {
                            k.clone()
                        } else {
                            format!("{}.{}", path, k)
                        };
                        errors.extend(check(prop_schema, child, &child_path));
                    }
                }
            }
            if let Some(addl) = s.get("additionalProperties") {
                let known: Vec<&String> = s
                    .get("properties")
                    .and_then(|v| v.as_object())
                    .map(|o| o.keys().collect())
                    .unwrap_or_default();
                if let Value::Bool(false) = addl {
                    for k in obj.keys() {
                        if !known.contains(&k) {
                            errors.push(format!(
                                "{}: additional property {} not allowed",
                                pretty_path(path),
                                k
                            ));
                        }
                    }
                } else if let Value::Object(_) = addl {
                    for (k, v) in obj {
                        if !known.contains(&k) {
                            let child_path = if path.is_empty() {
                                k.clone()
                            } else {
                                format!("{}.{}", path, k)
                            };
                            errors.extend(check(addl, v, &child_path));
                        }
                    }
                }
            }
        }
        Value::Array(arr) => {
            if let Some(items) = s.get("items") {
                for (i, item) in arr.iter().enumerate() {
                    let child_path = format!("{}[{}]", path, i);
                    errors.extend(check(items, item, &child_path));
                }
            }
            if let Some(n) = s.get("minItems").and_then(|v| v.as_u64()) {
                if (arr.len() as u64) < n {
                    errors.push(format!("{}: fewer than minItems {}", pretty_path(path), n));
                }
            }
            if let Some(n) = s.get("maxItems").and_then(|v| v.as_u64()) {
                if (arr.len() as u64) > n {
                    errors.push(format!("{}: more than maxItems {}", pretty_path(path), n));
                }
            }
        }
        Value::String(s_val) => {
            if let Some(n) = s.get("minLength").and_then(|v| v.as_u64()) {
                if (s_val.chars().count() as u64) < n {
                    errors.push(format!(
                        "{}: shorter than minLength {}",
                        pretty_path(path),
                        n
                    ));
                }
            }
            if let Some(n) = s.get("maxLength").and_then(|v| v.as_u64()) {
                if (s_val.chars().count() as u64) > n {
                    errors.push(format!(
                        "{}: longer than maxLength {}",
                        pretty_path(path),
                        n
                    ));
                }
            }
        }
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            if let Some(m) = s.get("minimum").and_then(|v| v.as_f64()) {
                if f < m {
                    errors.push(format!("{}: below minimum {}", pretty_path(path), m));
                }
            }
            if let Some(m) = s.get("maximum").and_then(|v| v.as_f64()) {
                if f > m {
                    errors.push(format!("{}: above maximum {}", pretty_path(path), m));
                }
            }
            if let Some(m) = s.get("exclusiveMinimum").and_then(|v| v.as_f64()) {
                if f <= m {
                    errors.push(format!(
                        "{}: not above exclusiveMinimum {}",
                        pretty_path(path),
                        m
                    ));
                }
            }
            if let Some(m) = s.get("exclusiveMaximum").and_then(|v| v.as_f64()) {
                if f >= m {
                    errors.push(format!(
                        "{}: not below exclusiveMaximum {}",
                        pretty_path(path),
                        m
                    ));
                }
            }
            if let Some(m) = s.get("multipleOf").and_then(|v| v.as_f64()) {
                if m != 0.0 && (f / m).fract() != 0.0 {
                    errors.push(format!("{}: not a multiple of {}", pretty_path(path), m));
                }
            }
        }
        _ => {}
    }

    errors
}

fn type_matches(t: &Value, v: &Value) -> bool {
    match t {
        Value::String(name) => single_type(name, v),
        Value::Array(arr) => arr
            .iter()
            .any(|n| n.as_str().map(|name| single_type(name, v)).unwrap_or(false)),
        _ => true,
    }
}

fn single_type(name: &str, v: &Value) -> bool {
    match (name, v) {
        ("object", Value::Object(_)) => true,
        ("array", Value::Array(_)) => true,
        ("string", Value::String(_)) => true,
        ("boolean", Value::Bool(_)) => true,
        ("null", Value::Null) => true,
        ("number", Value::Number(_)) => true,
        ("integer", Value::Number(n)) => n.is_i64() || n.is_u64(),
        _ => false,
    }
}

fn describe(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn pretty_path(p: &str) -> String {
    if p.is_empty() {
        "<root>".to_string()
    } else {
        p.to_string()
    }
}
