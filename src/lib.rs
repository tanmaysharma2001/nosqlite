//! NoSQLite — a MongoDB-style document database backed by SQLite.
//!
//! ```no_run
//! use nosqlite::Database;
//! use serde_json::json;
//!
//! let db = Database::open_in_memory().unwrap();
//! let users = db.collection("users");
//!
//! users.insert_one(json!({ "name": "Alice", "age": 30 })).unwrap();
//! let alice = users.find_one(json!({ "name": "Alice" })).unwrap();
//! assert!(alice.is_some());
//! ```

mod aggregate;
mod cursor;
mod database;
mod error;
mod fts;
mod index;
mod io;
mod matcher;
mod ops;
mod query;
mod transaction;
mod typed;
mod update;
mod util;
mod validation;

pub use cursor::{ExplainPlan, FindCursor};
pub use database::{Collection, Database};
pub use error::{Error, Result};
pub use index::{IndexInfo, IndexSpec};
pub use io::Format;
pub use ops::{
    BulkWriteOptions, BulkWriteResult, FindOneAndDeleteOptions, FindOneAndUpdateOptions,
    FindOptions, ReturnDocument, UpdateOptions, UpdateResult, WriteOp,
};
pub use transaction::{Transaction, TxCollection};
pub use typed::{Document, TypedCollection, TypedFindCursor};
pub use validation::{ValidationLevel, Validator};

#[cfg(feature = "derive")]
pub use nosqlite_derive::document;
