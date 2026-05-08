use crate::error::{Error, Result};
use rusqlite::types::Value as SqlValue;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn validate_identifier(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::InvalidIdentifier("empty name".into()));
    }
    let first = name.chars().next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(Error::InvalidIdentifier(format!(
            "must start with letter or underscore: {}",
            name
        )));
    }
    for ch in name.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-') {
            return Err(Error::InvalidIdentifier(format!(
                "invalid character {:?} in {}",
                ch, name
            )));
        }
    }
    Ok(())
}

/// Convert a MongoDB-style dotted field path into a SQLite JSON path.
/// Example: `"address.city"` -> `"$.address.city"`.
pub fn mongo_path(field: &str) -> String {
    let mut out = String::with_capacity(field.len() + 2);
    out.push('$');
    for segment in field.split('.') {
        if segment.is_empty() {
            continue;
        }
        if segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            out.push('.');
            out.push_str(segment);
        } else {
            // Quote segments that contain special characters.
            out.push('.');
            out.push('"');
            for c in segment.chars() {
                if c == '"' {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push('"');
        }
    }
    out
}

/// Convert a serde_json `Value` into a `rusqlite` parameter value.
/// Scalars map naturally; arrays/objects are serialized as JSON text.
pub fn json_to_sql(v: &Value) -> Result<SqlValue> {
    Ok(match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                SqlValue::Real(f)
            } else {
                return Err(Error::InvalidQuery(format!("unsupported number: {}", n)));
            }
        }
        Value::String(s) => SqlValue::Text(s.clone()),
        Value::Array(_) | Value::Object(_) => SqlValue::Text(v.to_string()),
    })
}

/// Ensure the document has an `_id` field. Returns the id as a string.
/// If the document already has an `_id`, it is preserved (must be string).
pub fn ensure_id(doc: &mut Value) -> Result<String> {
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| Error::InvalidQuery("document must be a JSON object".into()))?;
    if let Some(existing) = obj.get("_id") {
        match existing {
            Value::String(s) => return Ok(s.clone()),
            Value::Number(n) => return Ok(n.to_string()),
            other => {
                return Err(Error::InvalidQuery(format!(
                    "_id must be string or number, got {:?}",
                    other
                )))
            }
        }
    }
    let id = ulid::Ulid::new().to_string();
    obj.insert("_id".into(), Value::String(id.clone()));
    Ok(id)
}
