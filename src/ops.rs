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
use serde_json::Value;

#[derive(Default, Clone)]
pub struct FindOptions {
    pub sort: Option<Value>,
    pub projection: Option<Value>,
    pub limit: Option<i64>,
    pub skip: Option<i64>,
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
    let (where_clause, params) = build_where(name, filter)?;
    let sql = format!("SELECT COUNT(*) FROM \"{}\" WHERE {}", name, where_clause);
    let mut stmt = conn.prepare(&sql)?;
    let n: i64 = stmt.query_row(rusqlite::params_from_iter(params.iter()), |r| r.get(0))?;
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
    let (sql, params) = build_find_sql(name, filter, opts)?;
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
    for s in raw {
        let mut v: Value = serde_json::from_str(&s)?;
        if let Some(p) = &opts.projection {
            v = apply_projection(&v, p)?;
        }
        out.push(v);
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
    let (rest, text_clauses) = split_text(filter);
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

/// Walk `filter` removing top-level `$text` operators and returning the
/// rewritten filter plus a list of search strings to AND in.
fn split_text(filter: &Value) -> (Value, Vec<String>) {
    let mut texts = Vec::new();
    let rest = match filter {
        Value::Object(o) => {
            let mut new_obj = serde_json::Map::new();
            for (k, v) in o {
                if k == "$text" {
                    if let Some(s) = v.get("$search").and_then(|x| x.as_str()) {
                        texts.push(s.to_string());
                        continue;
                    }
                }
                new_obj.insert(k.clone(), v.clone());
            }
            Value::Object(new_obj)
        }
        _ => filter.clone(),
    };
    (rest, texts)
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
    if !table_exists(conn, name)? {
        return Ok(0);
    }
    if conn.is_autocommit() {
        let tx = conn.transaction()?;
        let n = update_rows_in(&tx, name, filter, update_doc, only_one, validator)?;
        tx.commit()?;
        Ok(n)
    } else {
        update_rows_in(conn, name, filter, update_doc, only_one, validator)
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
    let (where_clause, where_params) = build_where(name, filter)?;
    let select_sql = if only_one {
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
    let (where_clause, where_params) = build_where(name, filter)?;
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
    // Remove orphaned FTS rows. The FTS table is keyed by _id, so a fresh
    // sync on next insert/update is fine; we just clear here.
    crate::fts::clear_orphans(conn, name)?;
    Ok(n as u64)
}
