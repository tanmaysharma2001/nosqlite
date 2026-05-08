use crate::aggregate;
use crate::cursor::FindCursor;
use crate::error::{Error, Result};
use crate::fts;
use crate::index::{IndexInfo, IndexSpec};
use crate::ops;
use crate::transaction::Transaction;
use crate::util::validate_identifier;
use crate::validation::{ValidationLevel, Validator};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

pub(crate) const META_TABLE: &str = "_nosqlite_meta";

/// A NoSQLite database — a single SQLite file (or in-memory database)
/// holding one table per collection.
pub struct Database {
    conn: Mutex<Connection>,
    validators: Mutex<HashMap<String, Validator>>,
}

impl Database {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::configure(&conn)?;
        let validators = load_validators(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            validators: Mutex::new(validators),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            validators: Mutex::new(HashMap::new()),
        })
    }

    fn configure(conn: &Connection) -> Result<()> {
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS \"{}\" (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
                META_TABLE
            ),
            [],
        )?;
        Ok(())
    }

    /// Begin a SQLite transaction explicitly. Most callers should prefer
    /// [`Database::transaction`] which handles commit/rollback automatically.
    /// Manual `begin`/`commit`/`rollback` is provided so non-Rust callers
    /// (e.g. the Python bindings' `with db.transaction(): ...` context
    /// manager) can scope a transaction across an external code block.
    pub fn begin(&self) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("BEGIN IMMEDIATE", [])?;
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("COMMIT", [])?;
        Ok(())
    }

    pub fn rollback(&self) -> Result<()> {
        let conn = self.lock()?;
        conn.execute("ROLLBACK", [])?;
        Ok(())
    }

    /// Run `f` inside a SQLite transaction. The closure receives a
    /// [`Transaction`] handle whose `collection(name)` operations execute on
    /// the same connection — if the closure returns `Err`, the changes are
    /// rolled back.
    pub fn transaction<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Transaction<'_>) -> Result<R>,
    {
        let mut guard = self.lock()?;
        guard.execute("BEGIN IMMEDIATE", [])?;
        let result = {
            let tx = Transaction::new(self, &mut guard);
            f(&tx)
        };
        match result {
            Ok(r) => {
                guard.execute("COMMIT", [])?;
                Ok(r)
            }
            Err(e) => {
                let _ = guard.execute("ROLLBACK", []);
                Err(e)
            }
        }
    }

    pub fn set_validator(
        &self,
        collection: &str,
        schema: Value,
        level: ValidationLevel,
    ) -> Result<()> {
        validate_identifier(collection)?;
        let payload = serde_json::json!({
            "schema": schema,
            "level": match level {
                ValidationLevel::Strict => "strict",
                ValidationLevel::Warn => "warn",
            },
        });
        let conn = self.lock()?;
        conn.execute(
            &format!(
                "INSERT INTO \"{0}\" (key, value) VALUES (?, ?) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                META_TABLE
            ),
            params![format!("validator:{}", collection), payload.to_string()],
        )?;
        drop(conn);
        let mut v = self.validators.lock().map_err(|_| Error::Poisoned)?;
        v.insert(collection.to_string(), Validator::new(schema, level));
        Ok(())
    }

    pub fn remove_validator(&self, collection: &str) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            &format!("DELETE FROM \"{}\" WHERE key=?", META_TABLE),
            params![format!("validator:{}", collection)],
        )?;
        drop(conn);
        let mut v = self.validators.lock().map_err(|_| Error::Poisoned)?;
        v.remove(collection);
        Ok(())
    }

    pub(crate) fn validator_for(&self, collection: &str) -> Result<Option<Validator>> {
        let v = self.validators.lock().map_err(|_| Error::Poisoned)?;
        Ok(v.get(collection).cloned())
    }

    pub fn collection<'a>(&'a self, name: &str) -> Collection<'a> {
        Collection {
            db: self,
            name: name.to_string(),
        }
    }

    pub fn list_collections(&self) -> Result<Vec<String>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master \
             WHERE type='table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )?;
        let all: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(all
            .into_iter()
            .filter(|n| n != META_TABLE && !is_fts_internal(n))
            .collect())
    }

    pub fn drop_collection(&self, name: &str) -> Result<()> {
        let conn = self.lock()?;
        ops::drop_table(&conn, name)?;
        // Best-effort: also drop any FTS table associated with this collection.
        let _ = fts::drop_text_index(&conn, name);
        Ok(())
    }

    pub(crate) fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| Error::Poisoned)
    }
}

fn is_fts_internal(name: &str) -> bool {
    name.ends_with("_fts")
        || name.ends_with("_fts_data")
        || name.ends_with("_fts_idx")
        || name.ends_with("_fts_config")
        || name.ends_with("_fts_docsize")
        || name.ends_with("_fts_content")
}

fn load_validators(conn: &Connection) -> Result<HashMap<String, Validator>> {
    conn.execute(
        &format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            META_TABLE
        ),
        [],
    )?;
    let mut stmt = conn.prepare(&format!(
        "SELECT key, value FROM \"{}\" WHERE key LIKE 'validator:%'",
        META_TABLE
    ))?;
    let rows: Vec<(String, String)> = {
        let collected: rusqlite::Result<Vec<(String, String)>> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect();
        collected?
    };
    let mut out = HashMap::new();
    for (key, val) in rows {
        let coll = key.trim_start_matches("validator:").to_string();
        let parsed: Value = serde_json::from_str(&val)?;
        let schema = parsed.get("schema").cloned().unwrap_or(Value::Null);
        let level = match parsed.get("level").and_then(|v| v.as_str()) {
            Some("warn") => ValidationLevel::Warn,
            _ => ValidationLevel::Strict,
        };
        out.insert(coll, Validator::new(schema, level));
    }
    Ok(out)
}

/// A document collection. Cheap to construct from a `Database` reference;
/// the underlying SQLite table is created lazily on first write.
pub struct Collection<'a> {
    db: &'a Database,
    name: String,
}

impl<'a> Collection<'a> {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn insert_one(&self, doc: Value) -> Result<String> {
        let validator = self.db.validator_for(&self.name)?;
        let conn = self.db.lock()?;
        ops::insert_one(&conn, &self.name, doc, validator.as_ref())
    }

    pub fn insert_many(&self, docs: Vec<Value>) -> Result<Vec<String>> {
        let validator = self.db.validator_for(&self.name)?;
        let mut conn = self.db.lock()?;
        ops::insert_many(&mut conn, &self.name, docs, validator.as_ref())
    }

    pub fn find(&self, filter: Value) -> FindCursor<'a> {
        FindCursor::new(self.db, self.name.clone(), filter)
    }

    pub fn find_one(&self, filter: Value) -> Result<Option<Value>> {
        self.find(filter).limit(1).first()
    }

    pub fn count(&self, filter: Value) -> Result<i64> {
        let conn = self.db.lock()?;
        ops::count(&conn, &self.name, &filter)
    }

    pub fn count_all(&self) -> Result<i64> {
        self.count(Value::Object(Default::default()))
    }

    pub fn aggregate(&self, pipeline: Vec<Value>) -> Result<Vec<Value>> {
        aggregate::run(self, self.db, &pipeline)
    }

    pub fn delete_one(&self, filter: Value) -> Result<u64> {
        let conn = self.db.lock()?;
        ops::delete_internal(&conn, &self.name, &filter, true)
    }

    pub fn delete_many(&self, filter: Value) -> Result<u64> {
        let conn = self.db.lock()?;
        ops::delete_internal(&conn, &self.name, &filter, false)
    }

    pub fn update_one(&self, filter: Value, update: Value) -> Result<u64> {
        let validator = self.db.validator_for(&self.name)?;
        let mut conn = self.db.lock()?;
        ops::update_internal(
            &mut conn,
            &self.name,
            &filter,
            &update,
            true,
            validator.as_ref(),
        )
    }

    pub fn update_many(&self, filter: Value, update: Value) -> Result<u64> {
        let validator = self.db.validator_for(&self.name)?;
        let mut conn = self.db.lock()?;
        ops::update_internal(
            &mut conn,
            &self.name,
            &filter,
            &update,
            false,
            validator.as_ref(),
        )
    }

    pub fn replace_one(&self, filter: Value, replacement: Value) -> Result<u64> {
        self.update_one(filter, replacement)
    }

    /// Update a single document with options (e.g. `{ upsert: true }`).
    /// Returns matched/modified counts and the upserted `_id` if a new
    /// document was inserted.
    pub fn update_one_with_options(
        &self,
        filter: Value,
        update: Value,
        options: ops::UpdateOptions,
    ) -> Result<ops::UpdateResult> {
        let validator = self.db.validator_for(&self.name)?;
        let mut conn = self.db.lock()?;
        ops::update_with_options(
            &mut conn,
            &self.name,
            &filter,
            &update,
            true,
            &options,
            validator.as_ref(),
        )
    }

    pub fn update_many_with_options(
        &self,
        filter: Value,
        update: Value,
        options: ops::UpdateOptions,
    ) -> Result<ops::UpdateResult> {
        let validator = self.db.validator_for(&self.name)?;
        let mut conn = self.db.lock()?;
        ops::update_with_options(
            &mut conn,
            &self.name,
            &filter,
            &update,
            false,
            &options,
            validator.as_ref(),
        )
    }

    pub fn replace_one_with_options(
        &self,
        filter: Value,
        replacement: Value,
        options: ops::UpdateOptions,
    ) -> Result<ops::UpdateResult> {
        self.update_one_with_options(filter, replacement, options)
    }

    /// Atomically find a document matching `filter` and apply `update` to it.
    /// Returns the document — `Before` (default) returns the pre-update doc;
    /// `After` returns the post-update doc. With `upsert: true` and no match,
    /// inserts a synthesized doc; `Before` returns `None`, `After` returns
    /// the inserted doc.
    pub fn find_one_and_update(&self, filter: Value, update: Value) -> Result<Option<Value>> {
        self.find_one_and_update_with_options(
            filter,
            update,
            ops::FindOneAndUpdateOptions::default(),
        )
    }

    pub fn find_one_and_update_with_options(
        &self,
        filter: Value,
        update: Value,
        options: ops::FindOneAndUpdateOptions,
    ) -> Result<Option<Value>> {
        let validator = self.db.validator_for(&self.name)?;
        let mut conn = self.db.lock()?;
        ops::find_one_and_update(
            &mut conn,
            &self.name,
            &filter,
            &update,
            &options,
            validator.as_ref(),
        )
    }

    pub fn find_one_and_replace(&self, filter: Value, replacement: Value) -> Result<Option<Value>> {
        self.find_one_and_update(filter, replacement)
    }

    pub fn find_one_and_replace_with_options(
        &self,
        filter: Value,
        replacement: Value,
        options: ops::FindOneAndUpdateOptions,
    ) -> Result<Option<Value>> {
        self.find_one_and_update_with_options(filter, replacement, options)
    }

    pub fn find_one_and_delete(&self, filter: Value) -> Result<Option<Value>> {
        self.find_one_and_delete_with_options(filter, ops::FindOneAndDeleteOptions::default())
    }

    pub fn find_one_and_delete_with_options(
        &self,
        filter: Value,
        options: ops::FindOneAndDeleteOptions,
    ) -> Result<Option<Value>> {
        let mut conn = self.db.lock()?;
        ops::find_one_and_delete(&mut conn, &self.name, &filter, &options)
    }

    /// Return the unique values of `field` across documents matching
    /// `filter`. Array values contribute each element separately, matching
    /// MongoDB's `distinct()` semantics. Missing fields are skipped.
    pub fn distinct(&self, field: &str, filter: Value) -> Result<Vec<Value>> {
        let conn = self.db.lock()?;
        ops::distinct(&conn, &self.name, field, &filter)
    }

    /// Execute a sequence of writes in a single SQLite transaction. With
    /// `ordered: true` (default), the first failing op aborts and rolls
    /// back; with `ordered: false`, individual op failures are tolerated
    /// while subsequent ops continue.
    pub fn bulk_write(&self, ops: Vec<ops::WriteOp>) -> Result<ops::BulkWriteResult> {
        self.bulk_write_with_options(ops, ops::BulkWriteOptions::default())
    }

    pub fn bulk_write_with_options(
        &self,
        write_ops: Vec<ops::WriteOp>,
        options: ops::BulkWriteOptions,
    ) -> Result<ops::BulkWriteResult> {
        let validator = self.db.validator_for(&self.name)?;
        let mut conn = self.db.lock()?;
        ops::bulk_write(
            &mut conn,
            &self.name,
            write_ops,
            &options,
            validator.as_ref(),
        )
    }

    pub fn create_index(&self, keys: Value) -> Result<String> {
        self.create_index_with_options(keys, None)
    }

    pub fn create_index_with_options(&self, keys: Value, options: Option<Value>) -> Result<String> {
        let conn = self.db.lock()?;
        ops::ensure_table(&conn, &self.name)?;
        let spec = IndexSpec::parse(&keys, options.as_ref())?;
        let sql = spec.create_sql(&self.name)?;
        let name = spec
            .name
            .clone()
            .unwrap_or_else(|| spec.auto_name(&self.name));
        conn.execute(&sql, [])?;
        Ok(name)
    }

    pub fn drop_index(&self, name: &str) -> Result<()> {
        validate_identifier(name)?;
        let conn = self.db.lock()?;
        conn.execute(&format!("DROP INDEX IF EXISTS \"{}\"", name), [])?;
        Ok(())
    }

    pub fn list_indexes(&self) -> Result<Vec<IndexInfo>> {
        let conn = self.db.lock()?;
        if !ops::table_exists(&conn, &self.name)? {
            return Ok(Vec::new());
        }
        let info_rows: Vec<(String, bool)> = {
            let mut stmt = conn.prepare(&format!("PRAGMA index_list(\"{}\")", self.name))?;
            let collected: rusqlite::Result<Vec<(String, bool)>> = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)? != 0))
                })?
                .collect();
            collected?
        };
        let mut out = Vec::with_capacity(info_rows.len());
        for (name, unique) in info_rows {
            let sql: Option<String> = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type='index' AND name=?",
                    params![&name],
                    |r| r.get(0),
                )
                .ok();
            out.push(IndexInfo { name, unique, sql });
        }
        Ok(out)
    }

    /// Build (or replace) a full-text-search index over the listed fields.
    /// After this returns, queries using `$text` against this collection
    /// will hit the FTS5 virtual table `<name>_fts`.
    pub fn create_text_index<S: AsRef<str>>(&self, fields: &[S]) -> Result<()> {
        let owned: Vec<String> = fields.iter().map(|s| s.as_ref().to_string()).collect();
        let mut conn = self.db.lock()?;
        ops::ensure_table(&conn, &self.name)?;
        fts::create_text_index(&mut conn, &self.name, &owned)
    }

    pub fn drop_text_index(&self) -> Result<()> {
        let conn = self.db.lock()?;
        fts::drop_text_index(&conn, &self.name)
    }
}
