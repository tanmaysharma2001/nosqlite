//! Update operator engine. Applied to documents in-memory using
//! read–modify–write semantics so all operators share one code path.

use crate::error::{Error, Result};
use serde_json::{Map, Value};

/// Apply a Mongo-style update document to `doc` in place.
pub fn apply(doc: &mut Value, update: &Value) -> Result<()> {
    let obj = update
        .as_object()
        .ok_or_else(|| Error::InvalidUpdate("update must be a JSON object".into()))?;

    let any_op = obj.keys().any(|k| k.starts_with('$'));
    let any_non_op = obj.keys().any(|k| !k.starts_with('$'));
    if any_op && any_non_op {
        return Err(Error::InvalidUpdate(
            "cannot mix operator and non-operator keys".into(),
        ));
    }

    if !any_op {
        // Replacement-style update: replace doc but keep _id from original.
        let id = doc.as_object().and_then(|o| o.get("_id")).cloned();
        *doc = update.clone();
        if let Some(id) = id {
            if let Some(o) = doc.as_object_mut() {
                o.insert("_id".into(), id);
            }
        }
        return Ok(());
    }

    for (op, val) in obj {
        match op.as_str() {
            "$set" => apply_set(doc, val)?,
            "$unset" => apply_unset(doc, val)?,
            "$inc" => apply_inc(doc, val)?,
            "$mul" => apply_mul(doc, val)?,
            "$rename" => apply_rename(doc, val)?,
            "$push" => apply_push(doc, val)?,
            "$pull" => apply_pull(doc, val)?,
            "$pop" => apply_pop(doc, val)?,
            "$addToSet" => apply_add_to_set(doc, val)?,
            "$min" => apply_min_max(doc, val, true)?,
            "$max" => apply_min_max(doc, val, false)?,
            other => return Err(Error::InvalidUpdate(format!("unknown operator {}", other))),
        }
    }
    Ok(())
}

fn require_obj<'a>(val: &'a Value, op: &str) -> Result<&'a Map<String, Value>> {
    val.as_object()
        .ok_or_else(|| Error::InvalidUpdate(format!("{} requires an object", op)))
}

/// Walk `path` (dot-separated), creating intermediate objects as needed,
/// and return a mutable reference to the parent map and the leaf key.
fn walk_mut<'a>(doc: &'a mut Value, path: &str) -> Result<(&'a mut Map<String, Value>, String)> {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
        return Err(Error::InvalidUpdate(format!("invalid path {:?}", path)));
    }
    let mut cur = doc;
    for seg in &segments[..segments.len() - 1] {
        let obj = cur
            .as_object_mut()
            .ok_or_else(|| Error::InvalidUpdate(format!("path traverses non-object at {}", seg)))?;
        if !obj.contains_key(*seg) {
            obj.insert((*seg).to_string(), Value::Object(Map::new()));
        }
        cur = obj.get_mut(*seg).unwrap();
    }
    let parent = cur
        .as_object_mut()
        .ok_or_else(|| Error::InvalidUpdate("path traverses non-object".into()))?;
    Ok((parent, segments.last().unwrap().to_string()))
}

/// Like walk_mut but returns None if any intermediate segment is missing or
/// not an object — used by operators that should not auto-create paths.
fn walk_existing_mut<'a>(
    doc: &'a mut Value,
    path: &str,
) -> Result<Option<(&'a mut Map<String, Value>, String)>> {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() || segments.iter().any(|s| s.is_empty()) {
        return Err(Error::InvalidUpdate(format!("invalid path {:?}", path)));
    }
    let mut cur = doc;
    for seg in &segments[..segments.len() - 1] {
        let obj = match cur.as_object_mut() {
            Some(o) => o,
            None => return Ok(None),
        };
        if !obj.contains_key(*seg) {
            return Ok(None);
        }
        cur = obj.get_mut(*seg).unwrap();
    }
    match cur.as_object_mut() {
        Some(parent) => Ok(Some((parent, segments.last().unwrap().to_string()))),
        None => Ok(None),
    }
}

fn apply_set(doc: &mut Value, val: &Value) -> Result<()> {
    for (path, v) in require_obj(val, "$set")? {
        let (parent, key) = walk_mut(doc, path)?;
        parent.insert(key, v.clone());
    }
    Ok(())
}

fn apply_unset(doc: &mut Value, val: &Value) -> Result<()> {
    for (path, _) in require_obj(val, "$unset")? {
        if let Some((parent, key)) = walk_existing_mut(doc, path)? {
            parent.remove(&key);
        }
    }
    Ok(())
}

fn apply_inc(doc: &mut Value, val: &Value) -> Result<()> {
    for (path, v) in require_obj(val, "$inc")? {
        let delta = v.as_f64().ok_or_else(|| {
            Error::InvalidUpdate(format!("$inc value for {} must be numeric", path))
        })?;
        let (parent, key) = walk_mut(doc, path)?;
        let cur = parent.get(&key);
        let new = match cur {
            None | Some(Value::Null) => delta,
            Some(n) => {
                n.as_f64().ok_or_else(|| {
                    Error::InvalidUpdate(format!("$inc target {} not numeric", path))
                })? + delta
            }
        };
        parent.insert(
            key,
            num_value(new, v.is_i64() && cur.is_none_or(is_int_like)),
        );
    }
    Ok(())
}

fn apply_mul(doc: &mut Value, val: &Value) -> Result<()> {
    for (path, v) in require_obj(val, "$mul")? {
        let factor = v.as_f64().ok_or_else(|| {
            Error::InvalidUpdate(format!("$mul value for {} must be numeric", path))
        })?;
        let (parent, key) = walk_mut(doc, path)?;
        let cur = parent.get(&key);
        let new = match cur {
            None | Some(Value::Null) => 0.0,
            Some(n) => {
                n.as_f64().ok_or_else(|| {
                    Error::InvalidUpdate(format!("$mul target {} not numeric", path))
                })? * factor
            }
        };
        parent.insert(
            key,
            num_value(new, v.is_i64() && cur.is_none_or(is_int_like)),
        );
    }
    Ok(())
}

fn is_int_like(v: &Value) -> bool {
    matches!(v, Value::Number(n) if n.is_i64() || n.is_u64())
}

fn num_value(n: f64, prefer_int: bool) -> Value {
    if prefer_int && n.fract() == 0.0 && n.is_finite() && n.abs() < 9.2e18 {
        Value::Number(serde_json::Number::from(n as i64))
    } else {
        serde_json::Number::from_f64(n)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

fn apply_rename(doc: &mut Value, val: &Value) -> Result<()> {
    for (from, to) in require_obj(val, "$rename")? {
        let to = to
            .as_str()
            .ok_or_else(|| Error::InvalidUpdate("$rename target must be string".into()))?;
        let removed = if let Some((parent, key)) = walk_existing_mut(doc, from)? {
            parent.remove(&key)
        } else {
            None
        };
        if let Some(value) = removed {
            let (parent, key) = walk_mut(doc, to)?;
            parent.insert(key, value);
        }
    }
    Ok(())
}

fn apply_push(doc: &mut Value, val: &Value) -> Result<()> {
    for (path, v) in require_obj(val, "$push")? {
        let (parent, key) = walk_mut(doc, path)?;
        let entry = parent.entry(key).or_insert(Value::Array(Vec::new()));
        let arr = entry.as_array_mut().ok_or_else(|| {
            Error::InvalidUpdate(format!("$push target {} is not an array", path))
        })?;
        // Support $each modifier.
        if let Value::Object(o) = v {
            if let Some(each) = o.get("$each") {
                let items = each
                    .as_array()
                    .ok_or_else(|| Error::InvalidUpdate("$each requires array".into()))?;
                arr.extend(items.iter().cloned());
                continue;
            }
        }
        arr.push(v.clone());
    }
    Ok(())
}

fn apply_pull(doc: &mut Value, val: &Value) -> Result<()> {
    for (path, criterion) in require_obj(val, "$pull")? {
        if let Some((parent, key)) = walk_existing_mut(doc, path)? {
            if let Some(Value::Array(arr)) = parent.get_mut(&key) {
                arr.retain(|item| !pull_matches(item, criterion));
            }
        }
    }
    Ok(())
}

fn pull_matches(item: &Value, criterion: &Value) -> bool {
    // Plain value: equality match.
    // Object: every (op or field) must match the item.
    if let Value::Object(crit) = criterion {
        if crit.is_empty() {
            return item == criterion;
        }
        if crit.keys().all(|k| !k.starts_with('$')) {
            // sub-document match — every key/value must match the item's matching field.
            if let Value::Object(item_obj) = item {
                return crit
                    .iter()
                    .all(|(k, v)| item_obj.get(k).map(|iv| iv == v).unwrap_or(false));
            }
            return false;
        }
    }
    item == criterion
}

fn apply_pop(doc: &mut Value, val: &Value) -> Result<()> {
    for (path, v) in require_obj(val, "$pop")? {
        let direction = v
            .as_i64()
            .ok_or_else(|| Error::InvalidUpdate("$pop value must be 1 or -1".into()))?;
        if let Some((parent, key)) = walk_existing_mut(doc, path)? {
            if let Some(Value::Array(arr)) = parent.get_mut(&key) {
                if arr.is_empty() {
                    continue;
                }
                if direction >= 0 {
                    arr.pop();
                } else {
                    arr.remove(0);
                }
            }
        }
    }
    Ok(())
}

fn apply_add_to_set(doc: &mut Value, val: &Value) -> Result<()> {
    for (path, v) in require_obj(val, "$addToSet")? {
        let (parent, key) = walk_mut(doc, path)?;
        let entry = parent.entry(key).or_insert(Value::Array(Vec::new()));
        let arr = entry.as_array_mut().ok_or_else(|| {
            Error::InvalidUpdate(format!("$addToSet target {} is not an array", path))
        })?;
        let items: Vec<Value> = if let Value::Object(o) = v {
            if let Some(each) = o.get("$each") {
                each.as_array()
                    .ok_or_else(|| Error::InvalidUpdate("$each requires array".into()))?
                    .clone()
            } else {
                vec![v.clone()]
            }
        } else {
            vec![v.clone()]
        };
        for item in items {
            if !arr.iter().any(|existing| existing == &item) {
                arr.push(item);
            }
        }
    }
    Ok(())
}

fn apply_min_max(doc: &mut Value, val: &Value, is_min: bool) -> Result<()> {
    for (path, v) in require_obj(val, if is_min { "$min" } else { "$max" })? {
        let (parent, key) = walk_mut(doc, path)?;
        let replace = match parent.get(&key) {
            None | Some(Value::Null) => true,
            Some(cur) => match (cur.as_f64(), v.as_f64()) {
                (Some(a), Some(b)) => {
                    if is_min {
                        b < a
                    } else {
                        b > a
                    }
                }
                _ => false,
            },
        };
        if replace {
            parent.insert(key, v.clone());
        }
    }
    Ok(())
}
