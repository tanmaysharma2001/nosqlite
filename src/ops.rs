//! Pure storage operations. These functions take a borrowed `Connection`
//! and contain all the SQL plumbing — both `Collection` (which locks the
//! database mutex) and `TxCollection` (which uses a connection already held
//! by an active `Transaction`) call into them.

use crate::cursor::apply_projection;
use crate::error::Result;
use crate::query;
use crate::update;
use crate::util::{ensure_id, mongo_path, now_ms, validate_identifier};
use crate::validation::Validator;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, Connection};
use serde_json::{Map, Value};

#[derive(Default, Clone)]
pub struct FindOptions {
    pub sort: Option<Value>,
    pub projection: Option<Value>,
    pub limit: Option<i64>,
    pub skip: Option<i64>,
}

#[derive(Default, Clone, Debug)]
pub struct UpdateOptions {
    pub upsert: bool,
}

#[derive(Default, Clone, Debug)]
pub struct UpdateResult {
    pub matched_count: u64,
    pub modified_count: u64,
    pub upserted_id: Option<String>,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReturnDocument {
    #[default]
    Before,
    After,
}

#[derive(Default, Clone)]
pub struct FindOneAndUpdateOptions {
    pub upsert: bool,
    pub return_document: ReturnDocument,
    pub sort: Option<Value>,
    pub projection: Option<Value>,
}

#[derive(Default, Clone)]
pub struct FindOneAndDeleteOptions {
    pub sort: Option<Value>,
    pub projection: Option<Value>,
}

#[derive(Clone)]
pub enum WriteOp {
    InsertOne {
        document: Value,
    },
    UpdateOne {
        filter: Value,
        update: Value,
        upsert: bool,
    },
    UpdateMany {
        filter: Value,
        update: Value,
        upsert: bool,
    },
    ReplaceOne {
        filter: Value,
        replacement: Value,
        upsert: bool,
    },
    DeleteOne {
        filter: Value,
    },
    DeleteMany {
        filter: Value,
    },
}

#[derive(Clone, Debug)]
pub struct BulkWriteOptions {
    pub ordered: bool,
}

impl Default for BulkWriteOptions {
    fn default() -> Self {
        Self { ordered: true }
    }
}

#[derive(Default, Debug, Clone)]
pub struct BulkWriteResult {
    pub inserted_count: u64,
    pub matched_count: u64,
    pub modified_count: u64,
    pub deleted_count: u64,
    pub upserted_ids: Vec<(usize, String)>,
}

pub fn ensure_table(conn: &Connection, name: &str) -> Result<()> {
    validate_identifier(name)?;
    conn.execute(
        &format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" (
                _id        TEXT PRIMARY KEY,
                doc        TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            name
        ),
        [],
    )?;
    Ok(())
}

pub fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
        params![name],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

pub fn drop_table(conn: &Connection, name: &str) -> Result<()> {
    validate_identifier(name)?;
    conn.execute(&format!("DROP TABLE IF EXISTS \"{}\"", name), [])?;
    Ok(())
}

pub fn insert_one(
    conn: &Connection,
    name: &str,
    mut doc: Value,
    validator: Option<&Validator>,
) -> Result<String> {
    ensure_table(conn, name)?;
    let id = ensure_id(&mut doc)?;
    if let Some(v) = validator {
        v.validate(&doc)?;
    }
    let now = now_ms();
    conn.execute(
        &format!(
            "INSERT INTO \"{}\" (_id, doc, created_at, updated_at) VALUES (?, ?, ?, ?)",
            name
        ),
        params![&id, doc.to_string(), now, now],
    )?;
    crate::fts::reindex_one(conn, name, &id, &doc)?;
    Ok(id)
}

pub fn insert_many(
    conn: &mut Connection,
    name: &str,
    docs: Vec<Value>,
    validator: Option<&Validator>,
) -> Result<Vec<String>> {
    ensure_table(conn, name)?;
    if conn.is_autocommit() {
        let tx = conn.transaction()?;
        let ids = insert_many_in(&tx, name, docs, validator)?;
        tx.commit()?;
        Ok(ids)
    } else {
        insert_many_in(conn, name, docs, validator)
    }
}

fn insert_many_in(
    conn: &Connection,
    name: &str,
    docs: Vec<Value>,
    validator: Option<&Validator>,
) -> Result<Vec<String>> {
    let now = now_ms();
    let sql = format!(
        "INSERT INTO \"{}\" (_id, doc, created_at, updated_at) VALUES (?, ?, ?, ?)",
        name
    );
    let mut ids = Vec::with_capacity(docs.len());
    {
        let mut stmt = conn.prepare(&sql)?;
        for mut doc in docs {
            let id = ensure_id(&mut doc)?;
            if let Some(v) = validator {
                v.validate(&doc)?;
            }
            stmt.execute(params![&id, doc.to_string(), now, now])?;
            crate::fts::reindex_one(conn, name, &id, &doc)?;
            ids.push(id);
        }
    }
    Ok(ids)
}

pub fn count(conn: &Connection, name: &str, filter: &Value) -> Result<i64> {
    if !table_exists(conn, name)? {
        return Ok(0);
    }
    let post = post_filter_of(filter);
    let (where_clause, params) = build_where(name, filter)?;
    if post.is_none() {
        let sql = format!("SELECT COUNT(*) FROM \"{}\" WHERE {}", name, where_clause);
        let mut stmt = conn.prepare(&sql)?;
        let n: i64 = stmt.query_row(rusqlite::params_from_iter(params.iter()), |r| r.get(0))?;
        return Ok(n);
    }
    // Fall back to scanning candidate rows when $expr is present.
    let sql = format!("SELECT doc FROM \"{}\" WHERE {}", name, where_clause);
    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<String> = {
        let collected: rusqlite::Result<Vec<String>> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, String>(0)
            })?
            .collect();
        collected?
    };
    let post = post.unwrap();
    let mut n = 0i64;
    for s in rows {
        let v: Value = serde_json::from_str(&s)?;
        if crate::matcher::matches(&v, &post)? {
            n += 1;
        }
    }
    Ok(n)
}

pub fn find_into_vec(
    conn: &Connection,
    name: &str,
    filter: &Value,
    opts: &FindOptions,
) -> Result<Vec<Value>> {
    if !table_exists(conn, name)? {
        return Ok(Vec::new());
    }
    let post = post_filter_of(filter);
    // If $expr is present, defer LIMIT/SKIP to after the in-Rust filter.
    let (sql_opts, post_skip, post_limit) = if post.is_none() {
        (opts.clone(), None, None)
    } else {
        (
            FindOptions {
                sort: opts.sort.clone(),
                projection: opts.projection.clone(),
                limit: None,
                skip: None,
            },
            opts.skip,
            opts.limit,
        )
    };
    let (sql, params) = build_find_sql(name, filter, &sql_opts)?;
    let mut stmt = conn.prepare(&sql)?;
    let raw: Vec<String> = {
        let collected: rusqlite::Result<Vec<String>> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, String>(0)
            })?
            .collect();
        collected?
    };
    let mut out = Vec::with_capacity(raw.len());
    let mut skipped = 0i64;
    for s in raw {
        let v: Value = serde_json::from_str(&s)?;
        if let Some(p) = &post {
            if !crate::matcher::matches(&v, p)? {
                continue;
            }
        }
        if let Some(n) = post_skip {
            if skipped < n {
                skipped += 1;
                continue;
            }
        }
        let projected = if let Some(p) = &opts.projection {
            apply_projection(&v, p)?
        } else {
            v
        };
        out.push(projected);
        if let Some(n) = post_limit {
            if (out.len() as i64) >= n {
                break;
            }
        }
    }
    Ok(out)
}

pub fn explain(
    conn: &Connection,
    name: &str,
    filter: &Value,
    opts: &FindOptions,
) -> Result<crate::cursor::ExplainPlan> {
    let (sql, params) = build_find_sql(name, filter, opts)?;
    let explain_sql = format!("EXPLAIN QUERY PLAN {}", sql);
    let mut stmt = conn.prepare(&explain_sql)?;
    let rows = {
        let collected: rusqlite::Result<Vec<crate::cursor::ExplainRow>> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok(crate::cursor::ExplainRow {
                    id: row.get(0)?,
                    parent: row.get(1)?,
                    detail: row.get(3)?,
                })
            })?
            .collect();
        collected?
    };
    Ok(crate::cursor::ExplainPlan { sql, rows })
}

fn build_find_sql(
    name: &str,
    filter: &Value,
    opts: &FindOptions,
) -> Result<(String, Vec<SqlValue>)> {
    let (where_clause, params) = build_where(name, filter)?;
    let mut sql = format!("SELECT doc FROM \"{}\" WHERE {}", name, where_clause);
    if let Some(spec) = &opts.sort {
        let order = compile_order_by(spec)?;
        if !order.is_empty() {
            sql.push_str(" ORDER BY ");
            sql.push_str(&order);
        }
    }
    if let Some(n) = opts.limit {
        sql.push_str(&format!(" LIMIT {}", n));
    } else if opts.skip.is_some() {
        sql.push_str(" LIMIT -1");
    }
    if let Some(n) = opts.skip {
        sql.push_str(&format!(" OFFSET {}", n));
    }
    Ok((sql, params))
}

/// Compile `filter` into a SQL `WHERE`-fragment and parameter list,
/// handling any top-level `$text` operators by merging in an FTS subquery.
fn build_where(name: &str, filter: &Value) -> Result<(String, Vec<SqlValue>)> {
    let (rest, text_clauses, _post) = split_special(filter);
    let compiled = query::compile(&rest)?;
    let mut where_clause = compiled.sql;
    let mut params = compiled.params;
    for search in text_clauses {
        let fts = crate::fts::fts_table_name(name);
        where_clause = format!(
            "({}) AND _id IN (SELECT _id FROM \"{}\" WHERE \"{}\" MATCH ?)",
            where_clause, fts, fts
        );
        params.push(SqlValue::Text(search));
    }
    Ok((where_clause, params))
}

/// Returns `Some(post)` when the filter has any `$expr`, where `post`
/// must be checked against each fetched row via the in-memory matcher.
fn post_filter_of(filter: &Value) -> Option<Value> {
    split_special(filter).2
}

/// Walk `filter` building a SQL-compilable version: top-level `$text` is
/// extracted into `texts` for an FTS subquery, and `$expr` is stripped
/// recursively (replaced with always-true placeholders inside `$and` /
/// `$or` / `$nor`). The third return value is the original filter with
/// `$text` removed — passed to the in-memory matcher to enforce `$expr`
/// (and any other clauses the SQL filter loosened) post-fetch.
///
/// A `$expr` removed from a single-key object (`{$expr: ...}`) becomes
/// `{}`, which compiles to `1=1` and matches every row — false positives
/// are then pruned by the matcher post-filter. This means $expr inside
/// $and tightens nothing but $expr inside $or correctly widens the SQL
/// scan to include candidates the matcher will keep.
fn split_special(filter: &Value) -> (Value, Vec<String>, Option<Value>) {
    let mut texts = Vec::new();
    let mut has_expr = false;
    let sql_filter = strip_for_sql(filter, &mut texts, true, &mut has_expr);
    let post = if has_expr {
        Some(strip_text_only(filter))
    } else {
        None
    };
    (sql_filter, texts, post)
}

fn strip_for_sql(
    v: &Value,
    texts: &mut Vec<String>,
    top_level: bool,
    has_expr: &mut bool,
) -> Value {
    match v {
        Value::Object(o) => {
            let mut new_obj = serde_json::Map::new();
            for (k, vv) in o {
                if top_level && k == "$text" {
                    if let Some(s) = vv.get("$search").and_then(|x| x.as_str()) {
                        texts.push(s.to_string());
                        continue;
                    }
                }
                if k == "$expr" {
                    *has_expr = true;
                    continue;
                }
                new_obj.insert(k.clone(), strip_for_sql(vv, texts, false, has_expr));
            }
            Value::Object(new_obj)
        }
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|x| strip_for_sql(x, texts, false, has_expr))
                .collect(),
        ),
        _ => v.clone(),
    }
}

fn strip_text_only(filter: &Value) -> Value {
    if let Value::Object(o) = filter {
        let mut new_obj = serde_json::Map::new();
        for (k, v) in o {
            if k == "$text" {
                continue;
            }
            new_obj.insert(k.clone(), v.clone());
        }
        return Value::Object(new_obj);
    }
    filter.clone()
}

fn compile_order_by(spec: &Value) -> Result<String> {
    let obj = spec
        .as_object()
        .ok_or_else(|| crate::Error::InvalidQuery("sort spec must be an object".into()))?;
    let mut parts = Vec::with_capacity(obj.len());
    for (field, dir) in obj {
        let d = dir.as_i64().ok_or_else(|| {
            crate::Error::InvalidQuery(format!("sort direction for {} must be 1 or -1", field))
        })?;
        let direction = if d >= 0 { "ASC" } else { "DESC" };
        let path = mongo_path(field);
        parts.push(format!(
            "json_extract(doc, '{}') {}",
            path.replace('\'', "''"),
            direction
        ));
    }
    Ok(parts.join(", "))
}

pub fn update_internal(
    conn: &mut Connection,
    name: &str,
    filter: &Value,
    update_doc: &Value,
    only_one: bool,
    validator: Option<&Validator>,
) -> Result<u64> {
    let r = update_with_options(
        conn,
        name,
        filter,
        update_doc,
        only_one,
        &UpdateOptions::default(),
        validator,
    )?;
    Ok(r.modified_count)
}

pub fn update_with_options(
    conn: &mut Connection,
    name: &str,
    filter: &Value,
    update_doc: &Value,
    only_one: bool,
    options: &UpdateOptions,
    validator: Option<&Validator>,
) -> Result<UpdateResult> {
    if options.upsert {
        ensure_table(conn, name)?;
    } else if !table_exists(conn, name)? {
        return Ok(UpdateResult::default());
    }
    if conn.is_autocommit() {
        let tx = conn.transaction()?;
        let r =
            update_with_options_in(&tx, name, filter, update_doc, only_one, options, validator)?;
        tx.commit()?;
        Ok(r)
    } else {
        update_with_options_in(conn, name, filter, update_doc, only_one, options, validator)
    }
}

fn update_with_options_in(
    conn: &Connection,
    name: &str,
    filter: &Value,
    update_doc: &Value,
    only_one: bool,
    options: &UpdateOptions,
    validator: Option<&Validator>,
) -> Result<UpdateResult> {
    let modified = update_rows_in(conn, name, filter, update_doc, only_one, validator)?;
    let mut result = UpdateResult {
        matched_count: modified,
        modified_count: modified,
        upserted_id: None,
    };
    if modified == 0 && options.upsert {
        let mut doc = upsert_baseline_from_filter(filter);
        update::apply(&mut doc, update_doc)?;
        let id = ensure_id(&mut doc)?;
        if let Some(v) = validator {
            v.validate(&doc)?;
        }
        let now = now_ms();
        conn.execute(
            &format!(
                "INSERT INTO \"{}\" (_id, doc, created_at, updated_at) VALUES (?, ?, ?, ?)",
                name
            ),
            params![&id, doc.to_string(), now, now],
        )?;
        crate::fts::reindex_one(conn, name, &id, &doc)?;
        result.upserted_id = Some(id);
    }
    Ok(result)
}

/// Build a baseline document from a filter's top-level equality clauses,
/// to seed an upsert insert. Operator keys (`$and`, `$or`, `$expr`, …) and
/// operator-only field values (`{age: {$gt: 18}}`) are skipped. Dotted
/// paths are walked to set nested fields.
fn upsert_baseline_from_filter(filter: &Value) -> Value {
    let mut base = Map::new();
    if let Value::Object(o) = filter {
        for (k, v) in o {
            if k.starts_with('$') {
                continue;
            }
            if let Value::Object(inner) = v {
                if inner.keys().any(|kk| kk.starts_with('$')) {
                    continue;
                }
            }
            set_in_map(&mut base, k, v.clone());
        }
    }
    Value::Object(base)
}

fn set_in_map(map: &mut Map<String, Value>, path: &str, val: Value) {
    let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return;
    }
    if segments.len() == 1 {
        map.insert(segments[0].to_string(), val);
        return;
    }
    let entry = map
        .entry(segments[0].to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Value::Object(inner) = entry {
        let rest = segments[1..].join(".");
        set_in_map(inner, &rest, val);
    }
}

fn update_rows_in(
    conn: &Connection,
    name: &str,
    filter: &Value,
    update_doc: &Value,
    only_one: bool,
    validator: Option<&Validator>,
) -> Result<u64> {
    let post = post_filter_of(filter);
    let (where_clause, where_params) = build_where(name, filter)?;
    // When $expr is present we can't push LIMIT down to SQL, so load
    // candidates and filter in Rust.
    let select_sql = if only_one && post.is_none() {
        format!(
            "SELECT _id, doc FROM \"{}\" WHERE {} LIMIT 1",
            name, where_clause
        )
    } else {
        format!("SELECT _id, doc FROM \"{}\" WHERE {}", name, where_clause)
    };

    let now = now_ms();
    let mut updated: u64 = 0;

    let rows: Vec<(String, String)> = {
        let mut stmt = conn.prepare(&select_sql)?;
        let collected: rusqlite::Result<Vec<(String, String)>> = stmt
            .query_map(rusqlite::params_from_iter(where_params.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect();
        collected?
    };

    let upd_sql = format!(
        "UPDATE \"{}\" SET doc = ?, updated_at = ? WHERE _id = ?",
        name
    );
    {
        let mut stmt = conn.prepare(&upd_sql)?;
        for (id, raw) in rows {
            let mut doc: Value = serde_json::from_str(&raw)?;
            if let Some(p) = &post {
                if !crate::matcher::matches(&doc, p)? {
                    continue;
                }
            }
            update::apply(&mut doc, update_doc)?;
            if let Some(o) = doc.as_object_mut() {
                o.insert("_id".into(), Value::String(id.clone()));
            }
            if let Some(v) = validator {
                v.validate(&doc)?;
            }
            stmt.execute(params![doc.to_string(), now, id])?;
            crate::fts::reindex_one(conn, name, &id, &doc)?;
            updated += 1;
            if only_one {
                break;
            }
        }
    }
    Ok(updated)
}

pub fn delete_internal(
    conn: &Connection,
    name: &str,
    filter: &Value,
    only_one: bool,
) -> Result<u64> {
    if !table_exists(conn, name)? {
        return Ok(0);
    }
    let post = post_filter_of(filter);
    let (where_clause, where_params) = build_where(name, filter)?;

    if post.is_none() {
        let sql = if only_one {
            format!(
                "DELETE FROM \"{0}\" WHERE _id IN \
                 (SELECT _id FROM \"{0}\" WHERE {1} LIMIT 1)",
                name, where_clause
            )
        } else {
            format!("DELETE FROM \"{}\" WHERE {}", name, where_clause)
        };
        let n = conn.execute(&sql, rusqlite::params_from_iter(where_params.iter()))?;
        crate::fts::clear_orphans(conn, name)?;
        return Ok(n as u64);
    }
    let post = post.unwrap();

    // $expr: identify ids in Rust, then delete by id list.
    let select_sql = format!("SELECT _id, doc FROM \"{}\" WHERE {}", name, where_clause);
    let mut stmt = conn.prepare(&select_sql)?;
    let candidates: Vec<(String, String)> = {
        let collected: rusqlite::Result<Vec<(String, String)>> = stmt
            .query_map(rusqlite::params_from_iter(where_params.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect();
        collected?
    };
    drop(stmt);
    let mut to_delete: Vec<String> = Vec::new();
    for (id, raw) in candidates {
        let v: Value = serde_json::from_str(&raw)?;
        if crate::matcher::matches(&v, &post)? {
            to_delete.push(id);
            if only_one {
                break;
            }
        }
    }
    let mut n = 0u64;
    if !to_delete.is_empty() {
        let del_sql = format!("DELETE FROM \"{}\" WHERE _id = ?", name);
        let mut stmt = conn.prepare(&del_sql)?;
        for id in &to_delete {
            n += stmt.execute(params![id])? as u64;
        }
    }
    crate::fts::clear_orphans(conn, name)?;
    Ok(n)
}

pub fn find_one_and_update(
    conn: &mut Connection,
    name: &str,
    filter: &Value,
    update_doc: &Value,
    options: &FindOneAndUpdateOptions,
    validator: Option<&Validator>,
) -> Result<Option<Value>> {
    if options.upsert {
        ensure_table(conn, name)?;
    } else if !table_exists(conn, name)? {
        return Ok(None);
    }
    if conn.is_autocommit() {
        let tx = conn.transaction()?;
        let r = find_one_and_update_in(&tx, name, filter, update_doc, options, validator)?;
        tx.commit()?;
        Ok(r)
    } else {
        find_one_and_update_in(conn, name, filter, update_doc, options, validator)
    }
}

fn find_one_and_update_in(
    conn: &Connection,
    name: &str,
    filter: &Value,
    update_doc: &Value,
    options: &FindOneAndUpdateOptions,
    validator: Option<&Validator>,
) -> Result<Option<Value>> {
    let find_opts = FindOptions {
        sort: options.sort.clone(),
        projection: None,
        limit: Some(1),
        skip: None,
    };
    let original = find_into_vec(conn, name, filter, &find_opts)?
        .into_iter()
        .next();

    let after = if let Some(orig) = original.as_ref() {
        let id = orig
            .get("_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::Error::InvalidQuery("matched doc has no _id".into()))?
            .to_string();
        let mut new_doc = orig.clone();
        update::apply(&mut new_doc, update_doc)?;
        if let Some(o) = new_doc.as_object_mut() {
            o.insert("_id".into(), Value::String(id.clone()));
        }
        if let Some(v) = validator {
            v.validate(&new_doc)?;
        }
        let now = now_ms();
        conn.execute(
            &format!(
                "UPDATE \"{}\" SET doc = ?, updated_at = ? WHERE _id = ?",
                name
            ),
            params![new_doc.to_string(), now, id],
        )?;
        crate::fts::reindex_one(conn, name, &id, &new_doc)?;
        Some(new_doc)
    } else if options.upsert {
        let mut doc = upsert_baseline_from_filter(filter);
        update::apply(&mut doc, update_doc)?;
        let id = ensure_id(&mut doc)?;
        if let Some(v) = validator {
            v.validate(&doc)?;
        }
        let now = now_ms();
        conn.execute(
            &format!(
                "INSERT INTO \"{}\" (_id, doc, created_at, updated_at) VALUES (?, ?, ?, ?)",
                name
            ),
            params![&id, doc.to_string(), now, now],
        )?;
        crate::fts::reindex_one(conn, name, &id, &doc)?;
        Some(doc)
    } else {
        None
    };

    let returned = match options.return_document {
        ReturnDocument::Before => original,
        ReturnDocument::After => after,
    };
    if let (Some(d), Some(p)) = (returned.as_ref(), options.projection.as_ref()) {
        return Ok(Some(apply_projection(d, p)?));
    }
    Ok(returned)
}

pub fn find_one_and_delete(
    conn: &mut Connection,
    name: &str,
    filter: &Value,
    options: &FindOneAndDeleteOptions,
) -> Result<Option<Value>> {
    if !table_exists(conn, name)? {
        return Ok(None);
    }
    if conn.is_autocommit() {
        let tx = conn.transaction()?;
        let r = find_one_and_delete_in(&tx, name, filter, options)?;
        tx.commit()?;
        Ok(r)
    } else {
        find_one_and_delete_in(conn, name, filter, options)
    }
}

fn find_one_and_delete_in(
    conn: &Connection,
    name: &str,
    filter: &Value,
    options: &FindOneAndDeleteOptions,
) -> Result<Option<Value>> {
    let find_opts = FindOptions {
        sort: options.sort.clone(),
        projection: None,
        limit: Some(1),
        skip: None,
    };
    let docs = find_into_vec(conn, name, filter, &find_opts)?;
    let original = docs.into_iter().next();
    if let Some(o) = original.as_ref() {
        let id = o
            .get("_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::Error::InvalidQuery("matched doc has no _id".into()))?
            .to_string();
        conn.execute(
            &format!("DELETE FROM \"{}\" WHERE _id = ?", name),
            params![&id],
        )?;
        crate::fts::clear_orphans(conn, name)?;
    }
    if let (Some(d), Some(p)) = (original.as_ref(), options.projection.as_ref()) {
        return Ok(Some(apply_projection(d, p)?));
    }
    Ok(original)
}

pub fn distinct(conn: &Connection, name: &str, field: &str, filter: &Value) -> Result<Vec<Value>> {
    if !table_exists(conn, name)? {
        return Ok(Vec::new());
    }
    let post = post_filter_of(filter);
    let (where_clause, where_params) = build_where(name, filter)?;
    let sql = format!("SELECT doc FROM \"{}\" WHERE {}", name, where_clause);
    let mut stmt = conn.prepare(&sql)?;
    let raw: Vec<String> = {
        let collected: rusqlite::Result<Vec<String>> = stmt
            .query_map(rusqlite::params_from_iter(where_params.iter()), |row| {
                row.get::<_, String>(0)
            })?
            .collect();
        collected?
    };
    let mut seen: Vec<Value> = Vec::new();
    for s in raw {
        let v: Value = serde_json::from_str(&s)?;
        if let Some(p) = &post {
            if !crate::matcher::matches(&v, p)? {
                continue;
            }
        }
        let extracted = crate::matcher::lookup_path(&v, field).cloned();
        match extracted {
            Some(Value::Array(items)) => {
                for item in items {
                    if !seen.iter().any(|x| x == &item) {
                        seen.push(item);
                    }
                }
            }
            Some(other) => {
                if !seen.iter().any(|x| x == &other) {
                    seen.push(other);
                }
            }
            None => {}
        }
    }
    Ok(seen)
}

pub fn bulk_write(
    conn: &mut Connection,
    name: &str,
    ops: Vec<WriteOp>,
    options: &BulkWriteOptions,
    validator: Option<&Validator>,
) -> Result<BulkWriteResult> {
    ensure_table(conn, name)?;
    if conn.is_autocommit() {
        let tx = conn.transaction()?;
        let r = bulk_write_in(&tx, name, ops, options, validator);
        match r {
            Ok(r) => {
                tx.commit()?;
                Ok(r)
            }
            Err(e) => {
                let _ = tx.rollback();
                Err(e)
            }
        }
    } else {
        bulk_write_in(conn, name, ops, options, validator)
    }
}

fn bulk_write_in(
    conn: &Connection,
    name: &str,
    ops: Vec<WriteOp>,
    options: &BulkWriteOptions,
    validator: Option<&Validator>,
) -> Result<BulkWriteResult> {
    let mut result = BulkWriteResult::default();
    for (i, op) in ops.into_iter().enumerate() {
        let outcome: Result<()> = (|| {
            match op {
                WriteOp::InsertOne { document } => {
                    insert_one(conn, name, document, validator)?;
                    result.inserted_count += 1;
                }
                WriteOp::UpdateOne {
                    filter,
                    update,
                    upsert,
                } => {
                    let r = update_with_options_in(
                        conn,
                        name,
                        &filter,
                        &update,
                        true,
                        &UpdateOptions { upsert },
                        validator,
                    )?;
                    result.matched_count += r.matched_count;
                    result.modified_count += r.modified_count;
                    if let Some(id) = r.upserted_id {
                        result.upserted_ids.push((i, id));
                    }
                }
                WriteOp::UpdateMany {
                    filter,
                    update,
                    upsert,
                } => {
                    let r = update_with_options_in(
                        conn,
                        name,
                        &filter,
                        &update,
                        false,
                        &UpdateOptions { upsert },
                        validator,
                    )?;
                    result.matched_count += r.matched_count;
                    result.modified_count += r.modified_count;
                    if let Some(id) = r.upserted_id {
                        result.upserted_ids.push((i, id));
                    }
                }
                WriteOp::ReplaceOne {
                    filter,
                    replacement,
                    upsert,
                } => {
                    let r = update_with_options_in(
                        conn,
                        name,
                        &filter,
                        &replacement,
                        true,
                        &UpdateOptions { upsert },
                        validator,
                    )?;
                    result.matched_count += r.matched_count;
                    result.modified_count += r.modified_count;
                    if let Some(id) = r.upserted_id {
                        result.upserted_ids.push((i, id));
                    }
                }
                WriteOp::DeleteOne { filter } => {
                    let n = delete_internal(conn, name, &filter, true)?;
                    result.deleted_count += n;
                }
                WriteOp::DeleteMany { filter } => {
                    let n = delete_internal(conn, name, &filter, false)?;
                    result.deleted_count += n;
                }
            }
            Ok(())
        })();
        if let Err(e) = outcome {
            if options.ordered {
                return Err(e);
            }
            // Unordered: continue past the failed op. The caller's transaction
            // rollback semantics still apply, since we're inside a single tx.
            // Best-effort: report as no-op contribution to counters.
        }
    }
    Ok(result)
}
