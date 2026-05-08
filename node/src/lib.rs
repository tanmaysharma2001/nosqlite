//! Node.js bindings for the NoSQLite document database.
//!
//! Mirrors the Python SDK shape — same `.nosqlite` file format, same MQL,
//! same operators. The Rust core does all the work; this crate is a thin
//! N-API shim that converts JS values to/from `serde_json::Value`.

#![deny(clippy::all)]

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;
use std::sync::Arc;

fn map_err<E: std::fmt::Display>(e: E) -> Error {
    Error::from_reason(format!("{}", e))
}

fn or_empty(v: Option<Value>) -> Value {
    v.unwrap_or_else(|| serde_json::json!({}))
}

#[napi]
pub struct Database {
    inner: Arc<nosqlite::Database>,
}

#[napi]
impl Database {
    /// Open or create a NoSQLite database. Pass no argument for in-memory.
    #[napi(constructor)]
    pub fn new(path: Option<String>) -> Result<Self> {
        let db = match path {
            None => nosqlite::Database::open_in_memory(),
            Some(p) => nosqlite::Database::open(p),
        }
        .map_err(map_err)?;
        Ok(Self {
            inner: Arc::new(db),
        })
    }

    #[napi]
    pub fn collection(&self, name: String) -> Collection {
        Collection {
            db: self.inner.clone(),
            name,
        }
    }

    #[napi]
    pub fn list_collections(&self) -> Result<Vec<String>> {
        self.inner.list_collections().map_err(map_err)
    }

    #[napi]
    pub fn drop_collection(&self, name: String) -> Result<()> {
        self.inner.drop_collection(&name).map_err(map_err)
    }

    #[napi]
    pub fn set_validator(
        &self,
        collection: String,
        schema: Value,
        level: Option<String>,
    ) -> Result<()> {
        let lvl = match level.as_deref().unwrap_or("strict") {
            "strict" => nosqlite::ValidationLevel::Strict,
            "warn" => nosqlite::ValidationLevel::Warn,
            other => {
                return Err(Error::from_reason(format!("unknown level: {}", other)));
            }
        };
        self.inner
            .set_validator(&collection, schema, lvl)
            .map_err(map_err)
    }

    #[napi]
    pub fn remove_validator(&self, collection: String) -> Result<()> {
        self.inner.remove_validator(&collection).map_err(map_err)
    }

    /// Begin a transaction. The returned handle exposes the same CRUD
    /// surface as a regular `Collection`. Call `.commit()` to persist or
    /// `.rollback()` to discard.
    #[napi]
    pub fn begin_transaction(&self) -> Result<Transaction> {
        self.inner.begin().map_err(map_err)?;
        Ok(Transaction {
            db: self.inner.clone(),
            done: false,
        })
    }
}

#[napi]
pub struct Collection {
    db: Arc<nosqlite::Database>,
    name: String,
}

#[napi]
impl Collection {
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.name.clone()
    }

    #[napi]
    pub fn insert_one(&self, doc: Value) -> Result<String> {
        self.db
            .collection(&self.name)
            .insert_one(doc)
            .map_err(map_err)
    }

    #[napi]
    pub fn insert_many(&self, docs: Vec<Value>) -> Result<Vec<String>> {
        self.db
            .collection(&self.name)
            .insert_many(docs)
            .map_err(map_err)
    }

    #[napi]
    pub fn find(&self, filter: Option<Value>, options: Option<FindOptions>) -> Result<Vec<Value>> {
        let coll = self.db.collection(&self.name);
        let mut cur = coll.find(or_empty(filter));
        if let Some(opts) = options {
            if let Some(s) = opts.sort {
                cur = cur.sort(s);
            }
            if let Some(p) = opts.projection {
                cur = cur.project(p);
            }
            if let Some(n) = opts.limit {
                cur = cur.limit(n);
            }
            if let Some(n) = opts.skip {
                cur = cur.skip(n);
            }
        }
        cur.into_vec().map_err(map_err)
    }

    #[napi]
    pub fn find_one(&self, filter: Option<Value>) -> Result<Option<Value>> {
        self.db
            .collection(&self.name)
            .find(or_empty(filter))
            .first()
            .map_err(map_err)
    }

    #[napi]
    pub fn count(&self, filter: Option<Value>) -> Result<i64> {
        self.db
            .collection(&self.name)
            .count(or_empty(filter))
            .map_err(map_err)
    }

    #[napi]
    pub fn update_one(&self, filter: Value, update: Value) -> Result<u32> {
        self.db
            .collection(&self.name)
            .update_one(filter, update)
            .map_err(map_err)
            .map(|n| n as u32)
    }

    #[napi]
    pub fn update_many(&self, filter: Value, update: Value) -> Result<u32> {
        self.db
            .collection(&self.name)
            .update_many(filter, update)
            .map_err(map_err)
            .map(|n| n as u32)
    }

    #[napi]
    pub fn replace_one(&self, filter: Value, replacement: Value) -> Result<u32> {
        self.db
            .collection(&self.name)
            .replace_one(filter, replacement)
            .map_err(map_err)
            .map(|n| n as u32)
    }

    #[napi]
    pub fn delete_one(&self, filter: Value) -> Result<u32> {
        self.db
            .collection(&self.name)
            .delete_one(filter)
            .map_err(map_err)
            .map(|n| n as u32)
    }

    #[napi]
    pub fn delete_many(&self, filter: Value) -> Result<u32> {
        self.db
            .collection(&self.name)
            .delete_many(filter)
            .map_err(map_err)
            .map(|n| n as u32)
    }

    #[napi]
    pub fn aggregate(&self, pipeline: Vec<Value>) -> Result<Vec<Value>> {
        self.db
            .collection(&self.name)
            .aggregate(pipeline)
            .map_err(map_err)
    }

    #[napi]
    pub fn create_index(&self, keys: Value, options: Option<IndexOptions>) -> Result<String> {
        let mut opts = serde_json::json!({});
        if let Some(o) = options {
            if let Some(u) = o.unique {
                opts["unique"] = serde_json::Value::Bool(u);
            }
            if let Some(n) = o.name {
                opts["name"] = serde_json::Value::String(n);
            }
        }
        self.db
            .collection(&self.name)
            .create_index_with_options(keys, Some(opts))
            .map_err(map_err)
    }

    #[napi]
    pub fn drop_index(&self, name: String) -> Result<()> {
        self.db
            .collection(&self.name)
            .drop_index(&name)
            .map_err(map_err)
    }

    #[napi]
    pub fn list_indexes(&self) -> Result<Vec<Value>> {
        let infos = self
            .db
            .collection(&self.name)
            .list_indexes()
            .map_err(map_err)?;
        Ok(infos
            .into_iter()
            .map(|i| {
                serde_json::json!({
                    "name": i.name,
                    "unique": i.unique,
                    "sql": i.sql,
                })
            })
            .collect())
    }

    #[napi]
    pub fn create_text_index(&self, fields: Vec<String>) -> Result<()> {
        self.db
            .collection(&self.name)
            .create_text_index(&fields)
            .map_err(map_err)
    }

    #[napi]
    pub fn drop_text_index(&self) -> Result<()> {
        self.db
            .collection(&self.name)
            .drop_text_index()
            .map_err(map_err)
    }

    #[napi]
    pub fn explain(&self, filter: Option<Value>) -> Result<String> {
        self.db
            .collection(&self.name)
            .find(or_empty(filter))
            .explain()
            .map_err(map_err)
            .map(|p| p.to_string())
    }

    #[napi]
    pub fn import_file(&self, path: String, format: Option<String>) -> Result<u32> {
        let p = std::path::PathBuf::from(&path);
        let fmt = match format.as_deref() {
            Some("jsonl") | Some("ndjson") => nosqlite::Format::Jsonl,
            Some("json") => nosqlite::Format::Json,
            _ => nosqlite::Format::from_path(&p),
        };
        self.db
            .collection(&self.name)
            .import_file(&p, fmt)
            .map_err(map_err)
            .map(|n| n as u32)
    }

    #[napi]
    pub fn import_bson_file(&self, path: String) -> Result<u32> {
        self.db
            .collection(&self.name)
            .import_bson_file(&path)
            .map_err(map_err)
            .map(|n| n as u32)
    }

    #[napi]
    pub fn export_file(
        &self,
        path: String,
        format: Option<String>,
        filter: Option<Value>,
    ) -> Result<u32> {
        let p = std::path::PathBuf::from(&path);
        let fmt = match format.as_deref() {
            Some("jsonl") | Some("ndjson") => nosqlite::Format::Jsonl,
            Some("json") => nosqlite::Format::Json,
            _ => nosqlite::Format::from_path(&p),
        };
        self.db
            .collection(&self.name)
            .export_file(&p, fmt, or_empty(filter))
            .map_err(map_err)
            .map(|n| n as u32)
    }
}

#[napi(object)]
pub struct FindOptions {
    pub sort: Option<Value>,
    pub projection: Option<Value>,
    pub limit: Option<i64>,
    pub skip: Option<i64>,
}

#[napi(object)]
pub struct IndexOptions {
    pub unique: Option<bool>,
    pub name: Option<String>,
}

#[napi]
pub struct Transaction {
    db: Arc<nosqlite::Database>,
    done: bool,
}

#[napi]
impl Transaction {
    #[napi]
    pub fn collection(&self, name: String) -> Collection {
        Collection {
            db: self.db.clone(),
            name,
        }
    }

    #[napi]
    pub fn commit(&mut self) -> Result<()> {
        if self.done {
            return Err(Error::from_reason("transaction already finished"));
        }
        self.done = true;
        self.db.commit().map_err(map_err)
    }

    #[napi]
    pub fn rollback(&mut self) -> Result<()> {
        if self.done {
            return Err(Error::from_reason("transaction already finished"));
        }
        self.done = true;
        self.db.rollback().map_err(map_err)
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if !self.done {
            // Defensive: if a JS user forgets to commit/rollback, undo so we
            // don't leave the connection in an open transaction.
            let _ = self.db.rollback();
        }
    }
}
