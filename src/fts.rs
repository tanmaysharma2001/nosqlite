//! FTS5-backed full-text search.
//!
//! When `coll.create_text_index(&["title", "body"])` is called, NoSQLite
//! creates an FTS5 virtual table named `<coll>_fts` with one column per
//! indexed field plus an external content-id column (`_id`). The list of
//! indexed fields is recorded in the `_nosqlite_meta` table so that the
//! ops layer can keep the FTS rows in sync with the source collection on
//! insert / update / delete.
//!
//! Querying uses the standard MQL `$text` operator:
//!
//! ```text
//! coll.find(json!({ "$text": { "$search": "quick brown fox" } }))
//! ```
//!
//! `$text` rewrites to `_id IN (SELECT _id FROM <coll>_fts WHERE <coll>_fts MATCH ?)`.

use crate::error::{Error, Result};
use crate::util::validate_identifier;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

const META_TABLE: &str = "_nosqlite_meta";

/// Map of collection name -> ordered list of indexed fields, cached after
/// the first lookup. The cache is process-local; it's regenerated when a
/// new `Database` opens.
fn cache() -> &'static std::sync::Mutex<HashMap<String, Vec<String>>> {
    static C: OnceLock<std::sync::Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn cache_key(conn_addr: usize, coll: &str) -> String {
    format!("{}::{}", conn_addr, coll)
}

pub fn fts_table_name(coll: &str) -> String {
    format!("{}_fts", coll)
}

/// Create the FTS5 virtual table for `coll` over the given fields, replacing
/// any prior text index. Existing rows are reindexed.
pub fn create_text_index(conn: &mut Connection, coll: &str, fields: &[String]) -> Result<()> {
    validate_identifier(coll)?;
    if fields.is_empty() {
        return Err(Error::InvalidIndex(
            "create_text_index requires at least one field".into(),
        ));
    }
    for f in fields {
        if f.contains('"') {
            return Err(Error::InvalidIndex(format!("invalid field name: {}", f)));
        }
    }

    let fts = fts_table_name(coll);
    let cols: Vec<String> = fields.iter().map(|f| format!("\"{}\"", f)).collect();

    let tx = conn.transaction()?;
    tx.execute(&format!("DROP TABLE IF EXISTS \"{}\"", fts), [])?;
    tx.execute(
        &format!(
            "CREATE VIRTUAL TABLE \"{}\" USING fts5(_id UNINDEXED, {}, tokenize='porter unicode61')",
            fts,
            cols.join(", ")
        ),
        [],
    )?;
    tx.execute(
        &format!(
            "INSERT INTO \"{0}\" (key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            META_TABLE
        ),
        params![
            format!("text_index:{}", coll),
            serde_json::to_string(fields)?
        ],
    )?;

    // Backfill from existing rows in the source collection (if it exists).
    let exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
        params![coll],
        |r| r.get(0),
    )?;
    if exists > 0 {
        let rows: Vec<(String, String)> = {
            let mut stmt = tx.prepare(&format!("SELECT _id, doc FROM \"{}\"", coll))?;
            let collected: rusqlite::Result<Vec<(String, String)>> = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect();
            collected?
        };
        let placeholders = std::iter::repeat_n("?", fields.len())
            .collect::<Vec<_>>()
            .join(", ");
        let insert_sql = format!(
            "INSERT INTO \"{}\" (_id, {}) VALUES (?, {})",
            fts,
            cols.join(", "),
            placeholders
        );
        let mut ins = tx.prepare(&insert_sql)?;
        for (id, raw) in rows {
            let doc: Value = serde_json::from_str(&raw)?;
            let mut p: Vec<rusqlite::types::Value> = Vec::with_capacity(fields.len() + 1);
            p.push(rusqlite::types::Value::Text(id));
            for f in fields {
                p.push(rusqlite::types::Value::Text(extract_text(&doc, f)));
            }
            ins.execute(rusqlite::params_from_iter(p.iter()))?;
        }
    }

    tx.commit()?;

    let mut c = cache().lock().unwrap();
    c.insert(cache_key(conn as *const _ as usize, coll), fields.to_vec());
    Ok(())
}

pub fn drop_text_index(conn: &Connection, coll: &str) -> Result<()> {
    validate_identifier(coll)?;
    let fts = fts_table_name(coll);
    conn.execute(&format!("DROP TABLE IF EXISTS \"{}\"", fts), [])?;
    conn.execute(
        &format!("DELETE FROM \"{}\" WHERE key=?", META_TABLE),
        params![format!("text_index:{}", coll)],
    )?;
    let mut c = cache().lock().unwrap();
    c.remove(&cache_key(conn as *const _ as usize, coll));
    Ok(())
}

/// Look up the indexed fields for a collection, hitting the in-process cache
/// before falling back to the meta table.
pub fn fields_for(conn: &Connection, coll: &str) -> Result<Option<Vec<String>>> {
    {
        let c = cache().lock().unwrap();
        if let Some(v) = c.get(&cache_key(conn as *const _ as usize, coll)) {
            return Ok(Some(v.clone()));
        }
    }
    // Fall back to the meta table.
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
        params![META_TABLE],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Ok(None);
    }
    let val: rusqlite::Result<String> = conn.query_row(
        &format!("SELECT value FROM \"{}\" WHERE key=?", META_TABLE),
        params![format!("text_index:{}", coll)],
        |r| r.get(0),
    );
    match val {
        Ok(s) => {
            let fields: Vec<String> = serde_json::from_str(&s)?;
            let mut c = cache().lock().unwrap();
            c.insert(cache_key(conn as *const _ as usize, coll), fields.clone());
            Ok(Some(fields))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Sync a single document into the FTS table. No-op if no text index exists.
pub fn reindex_one(conn: &Connection, coll: &str, id: &str, doc: &Value) -> Result<()> {
    let fields = match fields_for(conn, coll)? {
        Some(f) => f,
        None => return Ok(()),
    };
    let fts = fts_table_name(coll);
    conn.execute(
        &format!("DELETE FROM \"{}\" WHERE _id = ?", fts),
        params![id],
    )?;
    let cols: Vec<String> = fields.iter().map(|f| format!("\"{}\"", f)).collect();
    let placeholders = std::iter::repeat_n("?", fields.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut p: Vec<rusqlite::types::Value> = Vec::with_capacity(fields.len() + 1);
    p.push(rusqlite::types::Value::Text(id.to_string()));
    for f in &fields {
        p.push(rusqlite::types::Value::Text(extract_text(doc, f)));
    }
    conn.execute(
        &format!(
            "INSERT INTO \"{}\" (_id, {}) VALUES (?, {})",
            fts,
            cols.join(", "),
            placeholders
        ),
        rusqlite::params_from_iter(p.iter()),
    )?;
    Ok(())
}

/// Remove FTS rows whose source document no longer exists.
pub fn clear_orphans(conn: &Connection, coll: &str) -> Result<()> {
    if fields_for(conn, coll)?.is_none() {
        return Ok(());
    }
    let fts = fts_table_name(coll);
    conn.execute(
        &format!(
            "DELETE FROM \"{0}\" WHERE _id NOT IN (SELECT _id FROM \"{1}\")",
            fts, coll
        ),
        [],
    )?;
    Ok(())
}

fn extract_text(doc: &Value, field: &str) -> String {
    let mut cur = doc;
    for seg in field.split('.') {
        cur = match cur {
            Value::Object(o) => match o.get(seg) {
                Some(v) => v,
                None => return String::new(),
            },
            _ => return String::new(),
        };
    }
    match cur {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join(" "),
        other => other.to_string(),
    }
}
