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

    /// Update a single document, with options. With `{ upsert: true }`,
    /// inserts a new document if none match. Returns
    /// `{ matchedCount, modifiedCount, upsertedId }`.
    #[napi]
    pub fn update_one_with_options(
        &self,
        filter: Value,
        update: Value,
        options: Option<UpdateOpts>,
    ) -> Result<UpdateResult> {
        let upsert = options.and_then(|o| o.upsert).unwrap_or(false);
        let r = self
            .db
            .collection(&self.name)
            .update_one_with_options(filter, update, nosqlite::UpdateOptions { upsert })
            .map_err(map_err)?;
        Ok(update_result_from(r))
    }

    #[napi]
    pub fn update_many_with_options(
        &self,
        filter: Value,
        update: Value,
        options: Option<UpdateOpts>,
    ) -> Result<UpdateResult> {
        let upsert = options.and_then(|o| o.upsert).unwrap_or(false);
        let r = self
            .db
            .collection(&self.name)
            .update_many_with_options(filter, update, nosqlite::UpdateOptions { upsert })
            .map_err(map_err)?;
        Ok(update_result_from(r))
    }

    #[napi]
    pub fn replace_one_with_options(
        &self,
        filter: Value,
        replacement: Value,
        options: Option<UpdateOpts>,
    ) -> Result<UpdateResult> {
        let upsert = options.and_then(|o| o.upsert).unwrap_or(false);
        let r = self
            .db
            .collection(&self.name)
            .replace_one_with_options(filter, replacement, nosqlite::UpdateOptions { upsert })
            .map_err(map_err)?;
        Ok(update_result_from(r))
    }

    /// Atomically find a document matching `filter` and apply `update`.
    /// `options.returnDocument` is `"before"` (default) or `"after"`.
    #[napi]
    pub fn find_one_and_update(
        &self,
        filter: Value,
        update: Value,
        options: Option<FindOneAndUpdateOpts>,
    ) -> Result<Option<Value>> {
        let opts = parse_find_one_update_opts(options)?;
        self.db
            .collection(&self.name)
            .find_one_and_update_with_options(filter, update, opts)
            .map_err(map_err)
    }

    #[napi]
    pub fn find_one_and_replace(
        &self,
        filter: Value,
        replacement: Value,
        options: Option<FindOneAndUpdateOpts>,
    ) -> Result<Option<Value>> {
        let opts = parse_find_one_update_opts(options)?;
        self.db
            .collection(&self.name)
            .find_one_and_replace_with_options(filter, replacement, opts)
            .map_err(map_err)
    }

    #[napi]
    pub fn find_one_and_delete(
        &self,
        filter: Value,
        options: Option<FindOneAndDeleteOpts>,
    ) -> Result<Option<Value>> {
        let opts = nosqlite::FindOneAndDeleteOptions {
            sort: options.as_ref().and_then(|o| o.sort.clone()),
            projection: options.as_ref().and_then(|o| o.projection.clone()),
        };
        self.db
            .collection(&self.name)
            .find_one_and_delete_with_options(filter, opts)
            .map_err(map_err)
    }

    /// Return the unique values of `field` across documents matching
    /// `filter`. Array fields contribute each element separately.
    #[napi]
    pub fn distinct(&self, field: String, filter: Option<Value>) -> Result<Vec<Value>> {
        self.db
            .collection(&self.name)
            .distinct(&field, or_empty(filter))
            .map_err(map_err)
    }

    /// Execute a sequence of writes in one transaction. `ops` is an array
    /// of objects shaped like `{ insertOne: { document } }`,
    /// `{ updateOne: { filter, update, upsert } }`, etc.
    #[napi]
    pub fn bulk_write(
        &self,
        ops: Vec<Value>,
        options: Option<BulkWriteOpts>,
    ) -> Result<BulkResult> {
        let ordered = options.and_then(|o| o.ordered).unwrap_or(true);
        let mut write_ops = Vec::with_capacity(ops.len());
        for op in ops {
            write_ops.push(parse_write_op(op)?);
        }
        let r = self
            .db
            .collection(&self.name)
            .bulk_write_with_options(write_ops, nosqlite::BulkWriteOptions { ordered })
            .map_err(map_err)?;
        Ok(bulk_result_from(r))
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

#[napi(object)]
pub struct UpdateOpts {
    pub upsert: Option<bool>,
}

#[napi(object)]
pub struct UpdateResult {
    pub matched_count: u32,
    pub modified_count: u32,
    pub upserted_id: Option<String>,
}

#[napi(object)]
pub struct FindOneAndUpdateOpts {
    pub upsert: Option<bool>,
    pub return_document: Option<String>,
    pub sort: Option<Value>,
    pub projection: Option<Value>,
}

#[napi(object)]
pub struct FindOneAndDeleteOpts {
    pub sort: Option<Value>,
    pub projection: Option<Value>,
}

#[napi(object)]
pub struct BulkWriteOpts {
    pub ordered: Option<bool>,
}

#[napi(object)]
pub struct UpsertedIndex {
    pub index: u32,
    pub id: String,
}

#[napi(object)]
pub struct BulkResult {
    pub inserted_count: u32,
    pub matched_count: u32,
    pub modified_count: u32,
    pub deleted_count: u32,
    pub upserted_ids: Vec<UpsertedIndex>,
}

fn update_result_from(r: nosqlite::UpdateResult) -> UpdateResult {
    UpdateResult {
        matched_count: r.matched_count as u32,
        modified_count: r.modified_count as u32,
        upserted_id: r.upserted_id,
    }
}

fn bulk_result_from(r: nosqlite::BulkWriteResult) -> BulkResult {
    BulkResult {
        inserted_count: r.inserted_count as u32,
        matched_count: r.matched_count as u32,
        modified_count: r.modified_count as u32,
        deleted_count: r.deleted_count as u32,
        upserted_ids: r
            .upserted_ids
            .into_iter()
            .map(|(i, id)| UpsertedIndex {
                index: i as u32,
                id,
            })
            .collect(),
    }
}

fn parse_find_one_update_opts(
    options: Option<FindOneAndUpdateOpts>,
) -> Result<nosqlite::FindOneAndUpdateOptions> {
    let o = options.unwrap_or(FindOneAndUpdateOpts {
        upsert: None,
        return_document: None,
        sort: None,
        projection: None,
    });
    let return_document = match o.return_document.as_deref() {
        Some("before") | None => nosqlite::ReturnDocument::Before,
        Some("after") => nosqlite::ReturnDocument::After,
        Some(other) => {
            return Err(Error::from_reason(format!(
                "returnDocument must be 'before' or 'after', got {:?}",
                other
            )))
        }
    };
    Ok(nosqlite::FindOneAndUpdateOptions {
        upsert: o.upsert.unwrap_or(false),
        return_document,
        sort: o.sort,
        projection: o.projection,
    })
}

fn parse_write_op(v: Value) -> Result<nosqlite::WriteOp> {
    let obj = v
        .as_object()
        .ok_or_else(|| Error::from_reason("bulkWrite op must be an object"))?;
    if obj.len() != 1 {
        return Err(Error::from_reason(
            "bulkWrite op must have exactly one key (e.g. 'insertOne')",
        ));
    }
    let (kind, body) = obj.iter().next().unwrap();
    let body_obj = body
        .as_object()
        .ok_or_else(|| Error::from_reason("bulkWrite op body must be an object"))?;
    let take = |k: &str| -> Result<Value> {
        body_obj
            .get(k)
            .cloned()
            .ok_or_else(|| Error::from_reason(format!("{}.{} required", kind, k)))
    };
    let upsert_flag = || -> bool {
        body_obj
            .get("upsert")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    Ok(match kind.as_str() {
        "insertOne" => nosqlite::WriteOp::InsertOne {
            document: take("document")?,
        },
        "updateOne" => nosqlite::WriteOp::UpdateOne {
            filter: take("filter")?,
            update: take("update")?,
            upsert: upsert_flag(),
        },
        "updateMany" => nosqlite::WriteOp::UpdateMany {
            filter: take("filter")?,
            update: take("update")?,
            upsert: upsert_flag(),
        },
        "replaceOne" => nosqlite::WriteOp::ReplaceOne {
            filter: take("filter")?,
            replacement: take("replacement")?,
            upsert: upsert_flag(),
        },
        "deleteOne" => nosqlite::WriteOp::DeleteOne {
            filter: take("filter")?,
        },
        "deleteMany" => nosqlite::WriteOp::DeleteMany {
            filter: take("filter")?,
        },
        other => {
            return Err(Error::from_reason(format!(
                "unknown bulkWrite op '{}'",
                other
            )))
        }
    })
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
