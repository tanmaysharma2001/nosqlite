//! In-memory MQL filter evaluation. Mirrors the SQL compiler in `query.rs`
//! but evaluates filters against `serde_json::Value` directly. Used by the
//! aggregation pipeline for `$match` stages that follow non-`$match` stages
//! (where there's no SQL row to compile against).

use crate::error::{Error, Result};
use serde_json::Value;
use std::cmp::Ordering;

pub fn matches(doc: &Value, filter: &Value) -> Result<bool> {
    let obj = match filter {
        Value::Object(o) => o,
        Value::Null => return Ok(true),
        _ => return Err(Error::InvalidQuery("filter must be an object".into())),
    };
    if obj.is_empty() {
        return Ok(true);
    }
    for (k, v) in obj {
        let ok = if let Some(stripped) = k.strip_prefix('$') {
            match_logical(doc, stripped, v)?
        } else {
            match_field(doc, k, v)?
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

fn match_logical(doc: &Value, op: &str, val: &Value) -> Result<bool> {
    match op {
        "expr" => {
            let v = crate::aggregate::eval_expr(doc, val)?;
            Ok(crate::aggregate::is_truthy(&v))
        }
        "and" => {
            let arr = val
                .as_array()
                .ok_or_else(|| Error::InvalidQuery("$and requires array".into()))?;
            for f in arr {
                if !matches(doc, f)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        "or" => {
            let arr = val
                .as_array()
                .ok_or_else(|| Error::InvalidQuery("$or requires array".into()))?;
            if arr.is_empty() {
                return Ok(true);
            }
            for f in arr {
                if matches(doc, f)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        "nor" => {
            let arr = val
                .as_array()
                .ok_or_else(|| Error::InvalidQuery("$nor requires array".into()))?;
            for f in arr {
                if matches(doc, f)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        "not" => Ok(!matches(doc, val)?),
        other => Err(Error::InvalidQuery(format!(
            "unknown top-level $${}",
            other
        ))),
    }
}

fn match_field(doc: &Value, field: &str, val: &Value) -> Result<bool> {
    let value_at = lookup_path(doc, field);

    if let Value::Object(obj) = val {
        let has_op = obj.keys().any(|k| k.starts_with('$'));
        let has_non_op = obj.keys().any(|k| !k.starts_with('$'));
        if has_op {
            if has_non_op {
                return Err(Error::InvalidQuery(format!(
                    "cannot mix operators and fields in {}",
                    field
                )));
            }
            for (op, opval) in obj {
                if !match_op(value_at, op, opval)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
    }

    Ok(values_equal(value_at, Some(val)))
}

fn match_op(value_at: Option<&Value>, op: &str, opval: &Value) -> Result<bool> {
    match op {
        "$eq" => Ok(values_equal(value_at, Some(opval))),
        "$ne" => Ok(!values_equal(value_at, Some(opval))),
        "$gt" => Ok(matches!(compare(value_at, opval), Some(Ordering::Greater))),
        "$gte" => Ok(matches!(
            compare(value_at, opval),
            Some(Ordering::Greater) | Some(Ordering::Equal)
        )),
        "$lt" => Ok(matches!(compare(value_at, opval), Some(Ordering::Less))),
        "$lte" => Ok(matches!(
            compare(value_at, opval),
            Some(Ordering::Less) | Some(Ordering::Equal)
        )),
        "$in" => {
            let arr = opval
                .as_array()
                .ok_or_else(|| Error::InvalidQuery("$in requires array".into()))?;
            Ok(arr.iter().any(|v| values_equal(value_at, Some(v))))
        }
        "$nin" => {
            let arr = opval
                .as_array()
                .ok_or_else(|| Error::InvalidQuery("$nin requires array".into()))?;
            Ok(!arr.iter().any(|v| values_equal(value_at, Some(v))))
        }
        "$exists" => {
            let want = opval
                .as_bool()
                .ok_or_else(|| Error::InvalidQuery("$exists requires bool".into()))?;
            Ok(value_at.is_some() == want)
        }
        "$type" => {
            let tname = opval
                .as_str()
                .ok_or_else(|| Error::InvalidQuery("$type requires string".into()))?;
            Ok(value_at.map(|v| type_name(v) == tname).unwrap_or(false))
        }
        "$size" => {
            let n = opval
                .as_i64()
                .ok_or_else(|| Error::InvalidQuery("$size requires integer".into()))?;
            Ok(value_at
                .and_then(|v| v.as_array())
                .map(|a| a.len() as i64 == n)
                .unwrap_or(false))
        }
        "$not" => {
            let inner = opval
                .as_object()
                .ok_or_else(|| Error::InvalidQuery("$not requires operator object".into()))?;
            for (op2, v2) in inner {
                if match_op(value_at, op2, v2)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        other => Err(Error::InvalidQuery(format!("unknown operator {}", other))),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(true) => "true",
        Value::Bool(false) => "false",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "real",
        Value::String(_) => "text",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub fn lookup_path<'a>(doc: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = doc;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(o) => o.get(seg)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn values_equal(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(Value::Null), Some(Value::Null)) | (None, Some(Value::Null)) | (None, None) => true,
        (Some(av), Some(bv)) => {
            // Numeric coercion: 1 == 1.0
            if let (Some(an), Some(bn)) = (av.as_f64(), bv.as_f64()) {
                if av.is_number() && bv.is_number() {
                    return an == bn;
                }
            }
            av == bv
        }
        _ => false,
    }
}

pub fn compare(a: Option<&Value>, b: &Value) -> Option<Ordering> {
    let av = a?;
    match (av, b) {
        (Value::Number(an), Value::Number(bn)) => an.as_f64()?.partial_cmp(&bn.as_f64()?),
        (Value::String(s1), Value::String(s2)) => Some(s1.cmp(s2)),
        (Value::Bool(b1), Value::Bool(b2)) => Some(b1.cmp(b2)),
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        _ => None,
    }
}
