//! Atomic multi-collection transactions.
//!
//! ```no_run
//! # use nosqlite::Database;
//! # use serde_json::json;
//! # let db = Database::open_in_memory().unwrap();
//! db.transaction(|tx| {
//!     tx.collection("from").update_one(json!({"_id": "a"}), json!({"$inc": {"bal": -10}}))?;
//!     tx.collection("to").update_one(json!({"_id": "b"}), json!({"$inc": {"bal":  10}}))?;
//!     Ok(())
//! }).unwrap();
//! ```
//!
//! Returning `Err` from the closure rolls everything back. Inside a
//! transaction, the cursor builder is not available — use the
//! `find_into_vec` / `find_one` / `count` shortcuts instead.

use crate::database::Database;
use crate::error::Result;
use crate::ops::{self, FindOptions};
use rusqlite::Connection;
use serde_json::Value;
use std::cell::RefCell;

pub struct Transaction<'db> {
    db: &'db Database,
    conn: RefCell<&'db mut Connection>,
}

impl<'db> Transaction<'db> {
    pub(crate) fn new(db: &'db Database, conn: &'db mut Connection) -> Self {
        Self {
            db,
            conn: RefCell::new(conn),
        }
    }

    pub fn collection<'tx>(&'tx self, name: &str) -> TxCollection<'tx, 'db> {
        TxCollection {
            tx: self,
            name: name.to_string(),
        }
    }
}

pub struct TxCollection<'tx, 'db> {
    tx: &'tx Transaction<'db>,
    name: String,
}

impl<'tx, 'db> TxCollection<'tx, 'db> {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn insert_one(&self, doc: Value) -> Result<String> {
        let validator = self.tx.db.validator_for(&self.name)?;
        let conn = self.tx.conn.borrow();
        ops::insert_one(*conn, &self.name, doc, validator.as_ref())
    }

    pub fn insert_many(&self, docs: Vec<Value>) -> Result<Vec<String>> {
        let validator = self.tx.db.validator_for(&self.name)?;
        let mut conn = self.tx.conn.borrow_mut();
        ops::insert_many(*conn, &self.name, docs, validator.as_ref())
    }

    pub fn find_into_vec(&self, filter: Value) -> Result<Vec<Value>> {
        self.find_with(filter, FindOptions::default())
    }

    pub fn find_with(&self, filter: Value, opts: FindOptions) -> Result<Vec<Value>> {
        let conn = self.tx.conn.borrow();
        ops::find_into_vec(*conn, &self.name, &filter, &opts)
    }

    pub fn find_one(&self, filter: Value) -> Result<Option<Value>> {
        let mut docs = self.find_with(
            filter,
            FindOptions {
                limit: Some(1),
                ..Default::default()
            },
        )?;
        Ok(docs.pop())
    }

    pub fn count(&self, filter: Value) -> Result<i64> {
        let conn = self.tx.conn.borrow();
        ops::count(*conn, &self.name, &filter)
    }

    pub fn count_all(&self) -> Result<i64> {
        self.count(Value::Object(Default::default()))
    }

    pub fn update_one(&self, filter: Value, update: Value) -> Result<u64> {
        let validator = self.tx.db.validator_for(&self.name)?;
        let mut conn = self.tx.conn.borrow_mut();
        ops::update_internal(
            *conn,
            &self.name,
            &filter,
            &update,
            true,
            validator.as_ref(),
        )
    }

    pub fn update_many(&self, filter: Value, update: Value) -> Result<u64> {
        let validator = self.tx.db.validator_for(&self.name)?;
        let mut conn = self.tx.conn.borrow_mut();
        ops::update_internal(
            *conn,
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

    pub fn delete_one(&self, filter: Value) -> Result<u64> {
        let conn = self.tx.conn.borrow();
        ops::delete_internal(*conn, &self.name, &filter, true)
    }

    pub fn delete_many(&self, filter: Value) -> Result<u64> {
        let conn = self.tx.conn.borrow();
        ops::delete_internal(*conn, &self.name, &filter, false)
    }
}
