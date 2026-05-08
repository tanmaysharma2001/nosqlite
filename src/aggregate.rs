//! Aggregation pipeline. A pipeline is a sequence of stage objects, each of
//! which transforms the stream of documents flowing through it. Stages we
//! implement here:
//!
//! - `$match`     — filter (uses the in-memory matcher)
//! - `$project`   — include/exclude fields
//! - `$addFields` / `$set` — compute & insert new fields
//! - `$sort`      — order by one or more fields
//! - `$limit`     — keep first N
//! - `$skip`      — drop first N
//! - `$count`     — replace stream with `{ <name>: count }`
//! - `$group`     — group by an expression and accumulate
//!   ($sum/$avg/$min/$max/$first/$last/$push/$addToSet/$count)
//! - `$unwind`    — emit one document per element of an array field
//! - `$lookup`    — left-outer join against another collection
//!
//! For the leading stage, `$match` is pushed down to SQL so indexes apply.
//! Other stages run in Rust over the streamed-back documents.

use crate::cursor::apply_projection;
use crate::database::{Collection, Database};
use crate::error::{Error, Result};
use crate::matcher::{self, lookup_path};
use serde_json::{json, Map, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;

pub fn run(coll: &Collection<'_>, db: &Database, pipeline: &[Value]) -> Result<Vec<Value>> {
    if pipeline.is_empty() {
        return coll.find(json!({})).into_vec();
    }

    let (leading_match, rest_start) = leading_match(pipeline);
    let mut docs = match leading_match {
        Some(filter) => coll.find(filter).into_vec()?,
        None => coll.find(json!({})).into_vec()?,
    };

    for stage in &pipeline[rest_start..] {
        docs = apply_stage(docs, stage, db)?;
    }
    Ok(docs)
}

fn leading_match(pipeline: &[Value]) -> (Option<Value>, usize) {
    if let Some(first) = pipeline.first() {
        if let Some(o) = first.as_object() {
            if o.len() == 1 {
                if let Some(m) = o.get("$match") {
                    return (Some(m.clone()), 1);
                }
            }
        }
    }
    (None, 0)
}

fn apply_stage(docs: Vec<Value>, stage: &Value, db: &Database) -> Result<Vec<Value>> {
    let obj = stage
        .as_object()
        .ok_or_else(|| Error::InvalidQuery("pipeline stage must be an object".into()))?;
    if obj.len() != 1 {
        return Err(Error::InvalidQuery(
            "pipeline stage must have exactly one operator".into(),
        ));
    }
    let (op, val) = obj.iter().next().unwrap();
    match op.as_str() {
        "$match" => stage_match(docs, val),
        "$project" => stage_project(docs, val),
        "$addFields" | "$set" => stage_add_fields(docs, val),
        "$sort" => stage_sort(docs, val),
        "$limit" => stage_limit(docs, val),
        "$skip" => stage_skip(docs, val),
        "$count" => stage_count(docs, val),
        "$group" => stage_group(docs, val),
        "$unwind" => stage_unwind(docs, val),
        "$lookup" => stage_lookup(docs, val, db),
        other => Err(Error::InvalidQuery(format!("unknown stage {}", other))),
    }
}

fn stage_match(docs: Vec<Value>, filter: &Value) -> Result<Vec<Value>> {
    let mut out = Vec::with_capacity(docs.len());
    for d in docs {
        if matcher::matches(&d, filter)? {
            out.push(d);
        }
    }
    Ok(out)
}

fn stage_project(docs: Vec<Value>, spec: &Value) -> Result<Vec<Value>> {
    docs.iter().map(|d| apply_projection(d, spec)).collect()
}

fn stage_add_fields(docs: Vec<Value>, spec: &Value) -> Result<Vec<Value>> {
    let spec_obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidQuery("$addFields spec must be object".into()))?;
    let mut out = Vec::with_capacity(docs.len());
    for d in docs {
        let mut doc = d;
        for (path, expr) in spec_obj {
            let v = eval_expr(&doc, expr)?;
            set_path(&mut doc, path, v);
        }
        out.push(doc);
    }
    Ok(out)
}

fn stage_sort(mut docs: Vec<Value>, spec: &Value) -> Result<Vec<Value>> {
    let obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidQuery("$sort spec must be object".into()))?;
    let keys: Vec<(String, i32)> = obj
        .iter()
        .map(|(k, v)| {
            v.as_i64()
                .ok_or_else(|| {
                    Error::InvalidQuery(format!("sort direction for {} must be 1 or -1", k))
                })
                .map(|d| (k.clone(), if d >= 0 { 1 } else { -1 }))
        })
        .collect::<Result<_>>()?;

    docs.sort_by(|a, b| {
        for (key, dir) in &keys {
            let av = lookup_path(a, key);
            let bv = lookup_path(b, key);
            let ord = match (av, bv) {
                (Some(x), Some(y)) => matcher::compare(Some(x), y).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Greater,
                (None, Some(_)) => Ordering::Less,
                (None, None) => Ordering::Equal,
            };
            if ord != Ordering::Equal {
                return if *dir == 1 { ord } else { ord.reverse() };
            }
        }
        Ordering::Equal
    });
    Ok(docs)
}

fn stage_limit(docs: Vec<Value>, val: &Value) -> Result<Vec<Value>> {
    let n = val
        .as_i64()
        .ok_or_else(|| Error::InvalidQuery("$limit requires integer".into()))?;
    Ok(docs.into_iter().take(n.max(0) as usize).collect())
}

fn stage_skip(docs: Vec<Value>, val: &Value) -> Result<Vec<Value>> {
    let n = val
        .as_i64()
        .ok_or_else(|| Error::InvalidQuery("$skip requires integer".into()))?;
    Ok(docs.into_iter().skip(n.max(0) as usize).collect())
}

fn stage_count(docs: Vec<Value>, val: &Value) -> Result<Vec<Value>> {
    let name = val
        .as_str()
        .ok_or_else(|| Error::InvalidQuery("$count requires field name string".into()))?;
    let mut m = Map::new();
    m.insert(name.to_string(), Value::Number((docs.len() as i64).into()));
    Ok(vec![Value::Object(m)])
}

fn stage_unwind(docs: Vec<Value>, val: &Value) -> Result<Vec<Value>> {
    let (path, preserve_null, include_index) = match val {
        Value::String(s) => (strip_dollar(s)?.to_string(), false, None),
        Value::Object(o) => {
            let p = o
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::InvalidQuery("$unwind requires path".into()))?;
            let preserve = o
                .get("preserveNullAndEmptyArrays")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let idx = o
                .get("includeArrayIndex")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (strip_dollar(p)?.to_string(), preserve, idx)
        }
        _ => {
            return Err(Error::InvalidQuery(
                "$unwind requires string or object".into(),
            ))
        }
    };

    let mut out = Vec::new();
    for d in docs {
        let arr_opt = lookup_path(&d, &path).cloned();
        match arr_opt {
            Some(Value::Array(items)) => {
                if items.is_empty() {
                    if preserve_null {
                        out.push(d);
                    }
                    continue;
                }
                for (i, item) in items.into_iter().enumerate() {
                    let mut clone = d.clone();
                    set_path(&mut clone, &path, item);
                    if let Some(idx_field) = &include_index {
                        set_path(&mut clone, idx_field, Value::Number((i as i64).into()));
                    }
                    out.push(clone);
                }
            }
            Some(Value::Null) | None => {
                if preserve_null {
                    out.push(d);
                }
            }
            Some(other) => {
                // Treat non-array, non-null scalar as a single-element array.
                let mut clone = d.clone();
                set_path(&mut clone, &path, other);
                out.push(clone);
            }
        }
    }
    Ok(out)
}

fn stage_lookup(docs: Vec<Value>, spec: &Value, db: &Database) -> Result<Vec<Value>> {
    let obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidQuery("$lookup requires object".into()))?;
    let from = obj
        .get("from")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::InvalidQuery("$lookup.from required".into()))?;
    let local_field = obj
        .get("localField")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::InvalidQuery("$lookup.localField required".into()))?;
    let foreign_field = obj
        .get("foreignField")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::InvalidQuery("$lookup.foreignField required".into()))?;
    let as_name = obj
        .get("as")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::InvalidQuery("$lookup.as required".into()))?;

    let foreign_coll = db.collection(from);
    let mut out = Vec::with_capacity(docs.len());
    for mut d in docs {
        let local_val = lookup_path(&d, local_field).cloned().unwrap_or(Value::Null);
        let filter = json!({ foreign_field: local_val });
        let matched = foreign_coll.find(filter).into_vec()?;
        if let Some(o) = d.as_object_mut() {
            o.insert(as_name.to_string(), Value::Array(matched));
        }
        out.push(d);
    }
    Ok(out)
}

fn stage_group(docs: Vec<Value>, spec: &Value) -> Result<Vec<Value>> {
    let obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidQuery("$group spec must be object".into()))?;
    let id_expr = obj
        .get("_id")
        .ok_or_else(|| Error::InvalidQuery("$group requires _id".into()))?;
    let accumulators: Vec<(String, &Value)> = obj
        .iter()
        .filter(|(k, _)| k.as_str() != "_id")
        .map(|(k, v)| (k.clone(), v))
        .collect();

    let mut groups: BTreeMap<String, GroupBucket> = BTreeMap::new();

    for d in &docs {
        let key_val = eval_expr(d, id_expr)?;
        let key_str = serde_json::to_string(&key_val).unwrap_or_default();
        let bucket = groups.entry(key_str).or_insert_with(|| GroupBucket {
            id: key_val,
            accs: vec![Accumulator::default(); accumulators.len()],
        });
        for (i, (_, expr)) in accumulators.iter().enumerate() {
            update_accumulator(&mut bucket.accs[i], expr, d)?;
        }
    }

    let mut out = Vec::with_capacity(groups.len());
    for (_, bucket) in groups {
        let mut m = Map::new();
        m.insert("_id".to_string(), bucket.id);
        for (i, (name, _)) in accumulators.iter().enumerate() {
            m.insert(name.clone(), bucket.accs[i].finalize());
        }
        out.push(Value::Object(m));
    }
    Ok(out)
}

struct GroupBucket {
    id: Value,
    accs: Vec<Accumulator>,
}

#[derive(Default, Clone)]
struct Accumulator {
    op: String,
    sum: f64,
    count: i64,
    min: Option<Value>,
    max: Option<Value>,
    first: Option<Value>,
    last: Option<Value>,
    items: Vec<Value>,
    set: Vec<Value>,
}

impl Accumulator {
    fn finalize(&self) -> Value {
        match self.op.as_str() {
            "$sum" => {
                if self.sum.fract() == 0.0 && self.sum.abs() < 9.2e18 {
                    Value::Number((self.sum as i64).into())
                } else {
                    serde_json::Number::from_f64(self.sum)
                        .map(Value::Number)
                        .unwrap_or(Value::Null)
                }
            }
            "$avg" => {
                if self.count == 0 {
                    Value::Null
                } else {
                    serde_json::Number::from_f64(self.sum / self.count as f64)
                        .map(Value::Number)
                        .unwrap_or(Value::Null)
                }
            }
            "$count" => Value::Number(self.count.into()),
            "$min" => self.min.clone().unwrap_or(Value::Null),
            "$max" => self.max.clone().unwrap_or(Value::Null),
            "$first" => self.first.clone().unwrap_or(Value::Null),
            "$last" => self.last.clone().unwrap_or(Value::Null),
            "$push" => Value::Array(self.items.clone()),
            "$addToSet" => Value::Array(self.set.clone()),
            _ => Value::Null,
        }
    }
}

fn update_accumulator(acc: &mut Accumulator, expr: &Value, doc: &Value) -> Result<()> {
    let obj = expr
        .as_object()
        .ok_or_else(|| Error::InvalidQuery("accumulator must be {$op: expr}".into()))?;
    if obj.len() != 1 {
        return Err(Error::InvalidQuery(
            "accumulator must have exactly one operator".into(),
        ));
    }
    let (op, arg) = obj.iter().next().unwrap();
    if acc.op.is_empty() {
        acc.op = op.clone();
    }
    match op.as_str() {
        "$sum" => {
            // {$sum: 1} counts; {$sum: "$field"} sums.
            let v = eval_expr(doc, arg)?;
            if let Some(n) = v.as_f64() {
                acc.sum += n;
            }
            acc.count += 1;
        }
        "$avg" => {
            let v = eval_expr(doc, arg)?;
            if let Some(n) = v.as_f64() {
                acc.sum += n;
                acc.count += 1;
            }
        }
        "$count" => {
            acc.count += 1;
        }
        "$min" => {
            let v = eval_expr(doc, arg)?;
            if !v.is_null() {
                acc.min = Some(match acc.min.take() {
                    None => v,
                    Some(prev) => {
                        if matches!(matcher::compare(Some(&v), &prev), Some(Ordering::Less)) {
                            v
                        } else {
                            prev
                        }
                    }
                });
            }
        }
        "$max" => {
            let v = eval_expr(doc, arg)?;
            if !v.is_null() {
                acc.max = Some(match acc.max.take() {
                    None => v,
                    Some(prev) => {
                        if matches!(matcher::compare(Some(&v), &prev), Some(Ordering::Greater)) {
                            v
                        } else {
                            prev
                        }
                    }
                });
            }
        }
        "$first" => {
            let v = eval_expr(doc, arg)?;
            if acc.first.is_none() {
                acc.first = Some(v);
            }
        }
        "$last" => {
            let v = eval_expr(doc, arg)?;
            acc.last = Some(v);
        }
        "$push" => {
            let v = eval_expr(doc, arg)?;
            acc.items.push(v);
        }
        "$addToSet" => {
            let v = eval_expr(doc, arg)?;
            if !acc.set.iter().any(|x| x == &v) {
                acc.set.push(v);
            }
        }
        other => {
            return Err(Error::InvalidQuery(format!(
                "unknown accumulator {}",
                other
            )))
        }
    }
    Ok(())
}

/// Evaluate an aggregation expression against a single document.
///
/// Supported forms:
///   * `"$field"` or `"$nested.field"` — field reference (returns Null if missing)
///   * literal scalar / array / object — returned as-is
///   * `{ "$add": [a, b] }` / `$subtract` / `$multiply` / `$divide` (binary or
///     n-ary numeric)
///   * `{ "$concat": [a, b, ...] }` for strings
///   * `{ "$toUpper": e }` / `$toLower`
///   * `{ "$ifNull": [a, b] }`
fn eval_expr(doc: &Value, expr: &Value) -> Result<Value> {
    match expr {
        Value::String(s) if s.starts_with('$') => {
            let path = &s[1..];
            if path.is_empty() {
                return Ok(Value::Null);
            }
            Ok(lookup_path(doc, path).cloned().unwrap_or(Value::Null))
        }
        Value::Object(o) if o.len() == 1 && o.keys().next().unwrap().starts_with('$') => {
            let (op, arg) = o.iter().next().unwrap();
            eval_op(doc, op, arg)
        }
        _ => Ok(expr.clone()),
    }
}

fn eval_op(doc: &Value, op: &str, arg: &Value) -> Result<Value> {
    match op {
        "$add" | "$subtract" | "$multiply" | "$divide" => {
            let args = eval_args(doc, arg)?;
            let nums: Vec<f64> = args.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect();
            let r = match op {
                "$add" => nums.iter().sum(),
                "$subtract" => {
                    if nums.len() != 2 {
                        return Err(Error::InvalidQuery("$subtract takes two args".into()));
                    }
                    nums[0] - nums[1]
                }
                "$multiply" => nums.iter().product(),
                "$divide" => {
                    if nums.len() != 2 {
                        return Err(Error::InvalidQuery("$divide takes two args".into()));
                    }
                    if nums[1] == 0.0 {
                        return Ok(Value::Null);
                    }
                    nums[0] / nums[1]
                }
                _ => unreachable!(),
            };
            Ok(if r.fract() == 0.0 && r.abs() < 9.2e18 {
                Value::Number((r as i64).into())
            } else {
                serde_json::Number::from_f64(r)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            })
        }
        "$concat" => {
            let args = eval_args(doc, arg)?;
            let mut s = String::new();
            for a in args {
                match a {
                    Value::Null => return Ok(Value::Null),
                    Value::String(t) => s.push_str(&t),
                    other => s.push_str(&other.to_string()),
                }
            }
            Ok(Value::String(s))
        }
        "$toUpper" => {
            let v = eval_expr(doc, arg)?;
            Ok(Value::String(v.as_str().unwrap_or("").to_uppercase()))
        }
        "$toLower" => {
            let v = eval_expr(doc, arg)?;
            Ok(Value::String(v.as_str().unwrap_or("").to_lowercase()))
        }
        "$ifNull" => {
            let args = eval_args(doc, arg)?;
            if args.len() != 2 {
                return Err(Error::InvalidQuery("$ifNull takes two args".into()));
            }
            Ok(if args[0].is_null() {
                args[1].clone()
            } else {
                args[0].clone()
            })
        }
        "$literal" => Ok(arg.clone()),
        other => Err(Error::InvalidQuery(format!(
            "unknown expression op {}",
            other
        ))),
    }
}

fn eval_args(doc: &Value, arg: &Value) -> Result<Vec<Value>> {
    if let Value::Array(arr) = arg {
        arr.iter().map(|a| eval_expr(doc, a)).collect()
    } else {
        Ok(vec![eval_expr(doc, arg)?])
    }
}

fn strip_dollar(s: &str) -> Result<&str> {
    s.strip_prefix('$')
        .ok_or_else(|| Error::InvalidQuery(format!("path expression must start with $: {}", s)))
}

fn set_path(doc: &mut Value, path: &str, val: Value) {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() {
        return;
    }
    if let Value::Object(map) = doc {
        set_path_in(map, &segments, val);
    }
}

fn set_path_in(map: &mut Map<String, Value>, segments: &[&str], val: Value) {
    if segments.len() == 1 {
        map.insert(segments[0].to_string(), val);
        return;
    }
    let entry = map
        .entry(segments[0].to_string())
        .or_insert(Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Value::Object(inner) = entry {
        set_path_in(inner, &segments[1..], val);
    }
}
