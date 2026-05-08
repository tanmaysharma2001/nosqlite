//! Strongly-typed wrappers over `Collection` and `FindCursor`.
//!
//! ```no_run
//! use nosqlite::{Database, TypedCollection};
//! use serde::{Deserialize, Serialize};
//! use serde_json::json;
//!
//! #[derive(Debug, Serialize, Deserialize)]
//! struct User {
//!     #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
//!     id: Option<String>,
//!     name: String,
//!     age: u32,
//! }
//!
//! let db = Database::open_in_memory().unwrap();
//! let users: TypedCollection<User> = db.typed_collection("users");
//!
//! users.insert_one(&User { id: None, name: "Alice".into(), age: 30 }).unwrap();
//! let alice: User = users.find_one(json!({ "name": "Alice" })).unwrap().unwrap();
//! assert_eq!(alice.age, 30);
//! ```

use crate::cursor::{ExplainPlan, FindCursor};
use crate::database::{Collection, Database};
use crate::error::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::marker::PhantomData;

/// A type that can be stored as a NoSQLite document.
///
/// Implemented automatically by the `#[document]` attribute macro for any
/// struct with an `Option<String>` `id` field. Implement it manually if your
/// struct shape requires it.
pub trait Document {
    fn id(&self) -> Option<&str>;
    fn set_id(&mut self, id: String);
}

pub struct TypedCollection<'a, T> {
    inner: Collection<'a>,
    _phantom: PhantomData<fn() -> T>,
}

impl Database {
    /// Bind a collection to a Rust type. The type must implement
    /// `Serialize + DeserializeOwned`. Documents inserted go through
    /// `serde_json` for round-tripping; queries use the standard MQL
    /// surface (filters and projections still take JSON values).
    pub fn typed_collection<'a, T>(&'a self, name: &str) -> TypedCollection<'a, T> {
        TypedCollection {
            inner: self.collection(name),
            _phantom: PhantomData,
        }
    }
}

impl<'a, T> TypedCollection<'a, T> {
    /// Return the underlying untyped `Collection` so callers can drop down
    /// to raw JSON when needed (indexes, validators, FTS, aggregation).
    pub fn untyped(&self) -> &Collection<'a> {
        &self.inner
    }
}

impl<'a, T> TypedCollection<'a, T>
where
    T: Serialize + DeserializeOwned,
{
    pub fn insert_one(&self, doc: &T) -> Result<String> {
        self.inner.insert_one(serde_json::to_value(doc)?)
    }

    pub fn insert_many(&self, docs: &[T]) -> Result<Vec<String>> {
        let vs: std::result::Result<Vec<Value>, serde_json::Error> =
            docs.iter().map(serde_json::to_value).collect();
        self.inner.insert_many(vs?)
    }

    pub fn find(&self, filter: Value) -> TypedFindCursor<'a, T> {
        TypedFindCursor {
            inner: self.inner.find(filter),
            _phantom: PhantomData,
        }
    }

    pub fn find_one(&self, filter: Value) -> Result<Option<T>> {
        match self.inner.find_one(filter)? {
            None => Ok(None),
            Some(v) => Ok(Some(serde_json::from_value(v)?)),
        }
    }

    pub fn count(&self, filter: Value) -> Result<i64> {
        self.inner.count(filter)
    }

    pub fn count_all(&self) -> Result<i64> {
        self.inner.count_all()
    }

    pub fn update_one(&self, filter: Value, update: Value) -> Result<u64> {
        self.inner.update_one(filter, update)
    }

    pub fn update_many(&self, filter: Value, update: Value) -> Result<u64> {
        self.inner.update_many(filter, update)
    }

    pub fn replace_one(&self, filter: Value, replacement: &T) -> Result<u64> {
        self.inner
            .replace_one(filter, serde_json::to_value(replacement)?)
    }

    pub fn delete_one(&self, filter: Value) -> Result<u64> {
        self.inner.delete_one(filter)
    }

    pub fn delete_many(&self, filter: Value) -> Result<u64> {
        self.inner.delete_many(filter)
    }
}

impl<'a, T> TypedCollection<'a, T>
where
    T: Document + Serialize + DeserializeOwned,
{
    /// Insert `doc` and write the generated id back into it. If `doc.id()`
    /// already returns `Some(_)`, that id is preserved.
    pub fn insert(&self, doc: &mut T) -> Result<()> {
        let id = self.inner.insert_one(serde_json::to_value(&*doc)?)?;
        if doc.id().is_none() {
            doc.set_id(id);
        }
        Ok(())
    }

    /// Look up a document by id. Equivalent to `find_one(json!({ "_id": id }))`.
    pub fn get(&self, id: &str) -> Result<Option<T>> {
        self.find_one(serde_json::json!({ "_id": id }))
    }
}

pub struct TypedFindCursor<'a, T> {
    inner: FindCursor<'a>,
    _phantom: PhantomData<fn() -> T>,
}

impl<'a, T> TypedFindCursor<'a, T>
where
    T: DeserializeOwned,
{
    pub fn sort(self, spec: Value) -> Self {
        Self {
            inner: self.inner.sort(spec),
            _phantom: PhantomData,
        }
    }

    pub fn limit(self, n: i64) -> Self {
        Self {
            inner: self.inner.limit(n),
            _phantom: PhantomData,
        }
    }

    pub fn skip(self, n: i64) -> Self {
        Self {
            inner: self.inner.skip(n),
            _phantom: PhantomData,
        }
    }

    pub fn into_vec(self) -> Result<Vec<T>> {
        self.inner
            .into_vec()?
            .into_iter()
            .map(|v| serde_json::from_value(v).map_err(Into::into))
            .collect()
    }

    pub fn first(self) -> Result<Option<T>> {
        match self.inner.first()? {
            None => Ok(None),
            Some(v) => Ok(Some(serde_json::from_value(v)?)),
        }
    }

    pub fn count(self) -> Result<i64> {
        self.inner.count()
    }

    pub fn explain(self) -> Result<ExplainPlan> {
        self.inner.explain()
    }
}
