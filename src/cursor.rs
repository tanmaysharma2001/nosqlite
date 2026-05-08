//! `FindCursor` — fluent builder for `find()` queries on a `Database`.

use crate::database::Database;
use crate::error::{Error, Result};
use crate::ops::{self, FindOptions};
use serde::Serialize;
use serde_json::{Map, Value};

pub struct FindCursor<'a> {
    db: &'a Database,
    coll: String,
    filter: Value,
    opts: FindOptions,
}

impl<'a> FindCursor<'a> {
    pub(crate) fn new(db: &'a Database, coll: String, filter: Value) -> Self {
        Self {
            db,
            coll,
            filter,
            opts: FindOptions::default(),
        }
    }

    pub fn sort(mut self, spec: Value) -> Self {
        self.opts.sort = Some(spec);
        self
    }

    pub fn project(mut self, spec: Value) -> Self {
        self.opts.projection = Some(spec);
        self
    }

    pub fn limit(mut self, n: i64) -> Self {
        self.opts.limit = Some(n);
        self
    }

    pub fn skip(mut self, n: i64) -> Self {
        self.opts.skip = Some(n);
        self
    }

    pub fn into_vec(self) -> Result<Vec<Value>> {
        let conn = self.db.lock()?;
        ops::find_into_vec(&conn, &self.coll, &self.filter, &self.opts)
    }

    pub fn first(self) -> Result<Option<Value>> {
        let mut v = self.limit(1).into_vec()?;
        Ok(v.pop())
    }

    pub fn count(self) -> Result<i64> {
        let conn = self.db.lock()?;
        ops::count(&conn, &self.coll, &self.filter)
    }

    pub fn explain(self) -> Result<ExplainPlan> {
        let conn = self.db.lock()?;
        ops::explain(&conn, &self.coll, &self.filter, &self.opts)
    }
}

pub fn apply_projection(doc: &Value, spec: &Value) -> Result<Value> {
    let spec_obj = spec
        .as_object()
        .ok_or_else(|| Error::InvalidQuery("projection must be an object".into()))?;
    if spec_obj.is_empty() {
        return Ok(doc.clone());
    }

    let mut include_mode = false;
    let mut id_explicit: Option<bool> = None;
    for (k, v) in spec_obj {
        let on = match v {
            Value::Bool(b) => *b,
            Value::Number(n) => n.as_i64().map(|i| i != 0).unwrap_or(false),
            _ => {
                return Err(Error::InvalidQuery(
                    "projection values must be 0/1 or bool".into(),
                ))
            }
        };
        if k == "_id" {
            id_explicit = Some(on);
        } else if on {
            include_mode = true;
        }
    }

    let src = doc
        .as_object()
        .ok_or_else(|| Error::InvalidQuery("cannot project non-object".into()))?;

    if include_mode {
        let mut out = Map::new();
        if id_explicit.unwrap_or(true) {
            if let Some(id) = src.get("_id") {
                out.insert("_id".into(), id.clone());
            }
        }
        for (k, v) in spec_obj {
            if k == "_id" {
                continue;
            }
            let on = matches!(v, Value::Bool(true))
                || matches!(v, Value::Number(n) if n.as_i64().unwrap_or(0) != 0);
            if !on {
                continue;
            }
            if let Some(val) = lookup_path(doc, k) {
                set_path(&mut out, k, val.clone());
            }
        }
        return Ok(Value::Object(out));
    }

    let mut out = src.clone();
    for (k, v) in spec_obj {
        let off = matches!(v, Value::Bool(false))
            || matches!(v, Value::Number(n) if n.as_i64().unwrap_or(0) == 0);
        if !off {
            continue;
        }
        remove_path(&mut out, k);
    }
    Ok(Value::Object(out))
}

fn lookup_path<'a>(doc: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = doc;
    for seg in path.split('.') {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

fn set_path(out: &mut Map<String, Value>, path: &str, val: Value) {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.len() == 1 {
        out.insert(segments[0].to_string(), val);
        return;
    }
    let mut cur = out;
    for (i, seg) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            cur.insert((*seg).to_string(), val);
            return;
        }
        let entry = cur
            .entry((*seg).to_string())
            .or_insert(Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        cur = entry.as_object_mut().unwrap();
    }
}

fn remove_path(out: &mut Map<String, Value>, path: &str) {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.len() == 1 {
        out.remove(segments[0]);
        return;
    }
    let mut cur = out;
    for (i, seg) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            cur.remove(*seg);
            return;
        }
        match cur.get_mut(*seg).and_then(|v| v.as_object_mut()) {
            Some(next) => cur = next,
            None => return,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ExplainPlan {
    pub sql: String,
    pub rows: Vec<ExplainRow>,
}

#[derive(Debug, Serialize)]
pub struct ExplainRow {
    pub id: i64,
    pub parent: i64,
    pub detail: String,
}

impl std::fmt::Display for ExplainPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}", self.sql)?;
        for row in &self.rows {
            writeln!(f, "  [{}] {}", row.id, row.detail)?;
        }
        Ok(())
    }
}
