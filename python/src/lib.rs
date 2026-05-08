//! Python bindings for the NoSQLite document database.
//!
//! This crate is a thin PyO3 shim — the actual storage, query compilation,
//! and aggregation logic all live in the parent `nosqlite` Rust crate, so
//! the Python and Rust APIs read and write a fully compatible file format.

use pyo3::exceptions::{PyKeyError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use pythonize::{depythonize, pythonize};
use std::path::PathBuf;
use std::sync::Arc;

fn map_err<E: std::fmt::Display>(e: E) -> PyErr {
    PyRuntimeError::new_err(format!("{}", e))
}

fn dep_value(obj: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    depythonize(obj).map_err(|e| PyValueError::new_err(format!("invalid JSON: {}", e)))
}

fn opt_value(obj: Option<&Bound<'_, PyAny>>) -> PyResult<Option<serde_json::Value>> {
    match obj {
        None => Ok(None),
        Some(o) if o.is_none() => Ok(None),
        Some(o) => Ok(Some(dep_value(o)?)),
    }
}

fn to_py(py: Python<'_>, v: &serde_json::Value) -> PyResult<PyObject> {
    pythonize(py, v)
        .map(|b| b.into_py(py))
        .map_err(|e| PyRuntimeError::new_err(format!("python conversion error: {}", e)))
}

fn parse_format(s: Option<&str>, path: &std::path::Path) -> nosqlite::Format {
    match s {
        Some("jsonl") | Some("ndjson") => nosqlite::Format::Jsonl,
        Some("json") => nosqlite::Format::Json,
        _ => nosqlite::Format::from_path(path),
    }
}

#[pyclass(name = "Database")]
struct PyDatabase {
    inner: Arc<nosqlite::Database>,
}

#[pymethods]
impl PyDatabase {
    #[new]
    #[pyo3(signature = (path=None))]
    fn new(path: Option<PathBuf>) -> PyResult<Self> {
        let db = match path {
            None => nosqlite::Database::open_in_memory(),
            Some(p) => nosqlite::Database::open(p),
        }
        .map_err(map_err)?;
        Ok(Self { inner: Arc::new(db) })
    }

    #[staticmethod]
    fn open_in_memory() -> PyResult<Self> {
        Ok(Self {
            inner: Arc::new(nosqlite::Database::open_in_memory().map_err(map_err)?),
        })
    }

    fn collection(&self, name: &str) -> PyCollection {
        PyCollection {
            db: self.inner.clone(),
            name: name.to_string(),
        }
    }

    fn list_collections(&self) -> PyResult<Vec<String>> {
        self.inner.list_collections().map_err(map_err)
    }

    fn drop_collection(&self, name: &str) -> PyResult<()> {
        self.inner.drop_collection(name).map_err(map_err)
    }

    #[pyo3(signature = (collection, schema, level="strict"))]
    fn set_validator(&self, collection: &str, schema: &Bound<'_, PyAny>, level: &str) -> PyResult<()> {
        let level = match level {
            "strict" => nosqlite::ValidationLevel::Strict,
            "warn" => nosqlite::ValidationLevel::Warn,
            other => return Err(PyValueError::new_err(format!("unknown level: {}", other))),
        };
        let v = dep_value(schema)?;
        self.inner
            .set_validator(collection, v, level)
            .map_err(map_err)
    }

    fn remove_validator(&self, collection: &str) -> PyResult<()> {
        self.inner.remove_validator(collection).map_err(map_err)
    }

    fn transaction(&self) -> PyTransaction {
        PyTransaction {
            db: self.inner.clone(),
            entered: false,
        }
    }
}

#[pyclass(name = "Collection")]
struct PyCollection {
    db: Arc<nosqlite::Database>,
    name: String,
}

#[pymethods]
impl PyCollection {
    #[getter]
    fn name(&self) -> &str {
        &self.name
    }

    fn insert_one(&self, doc: &Bound<'_, PyAny>) -> PyResult<String> {
        let v = dep_value(doc)?;
        self.db.collection(&self.name).insert_one(v).map_err(map_err)
    }

    fn insert_many(&self, docs: &Bound<'_, PyList>) -> PyResult<Vec<String>> {
        let mut vec = Vec::with_capacity(docs.len());
        for item in docs.iter() {
            vec.push(dep_value(&item)?);
        }
        self.db
            .collection(&self.name)
            .insert_many(vec)
            .map_err(map_err)
    }

    #[pyo3(signature = (filter=None, *, sort=None, limit=None, skip=None, projection=None))]
    fn find(
        &self,
        py: Python<'_>,
        filter: Option<&Bound<'_, PyAny>>,
        sort: Option<&Bound<'_, PyAny>>,
        limit: Option<i64>,
        skip: Option<i64>,
        projection: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        let f = match filter {
            Some(b) if !b.is_none() => dep_value(b)?,
            _ => serde_json::json!({}),
        };
        let coll = self.db.collection(&self.name);
        let mut cur = coll.find(f);
        if let Some(s) = opt_value(sort)? {
            cur = cur.sort(s);
        }
        if let Some(p) = opt_value(projection)? {
            cur = cur.project(p);
        }
        if let Some(n) = limit {
            cur = cur.limit(n);
        }
        if let Some(n) = skip {
            cur = cur.skip(n);
        }
        let docs = cur.into_vec().map_err(map_err)?;
        let list = PyList::empty_bound(py);
        for d in &docs {
            list.append(to_py(py, d)?)?;
        }
        Ok(list.into())
    }

    #[pyo3(signature = (filter=None, *, projection=None))]
    fn find_one(
        &self,
        py: Python<'_>,
        filter: Option<&Bound<'_, PyAny>>,
        projection: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Option<PyObject>> {
        let f = match filter {
            Some(b) if !b.is_none() => dep_value(b)?,
            _ => serde_json::json!({}),
        };
        let coll = self.db.collection(&self.name);
        let mut cur = coll.find(f).limit(1);
        if let Some(p) = opt_value(projection)? {
            cur = cur.project(p);
        }
        match cur.into_vec().map_err(map_err)?.into_iter().next() {
            None => Ok(None),
            Some(d) => Ok(Some(to_py(py, &d)?)),
        }
    }

    #[pyo3(signature = (filter=None))]
    fn count(&self, filter: Option<&Bound<'_, PyAny>>) -> PyResult<i64> {
        let f = match filter {
            Some(b) if !b.is_none() => dep_value(b)?,
            _ => serde_json::json!({}),
        };
        self.db
            .collection(&self.name)
            .count(f)
            .map_err(map_err)
    }

    fn update_one(&self, filter: &Bound<'_, PyAny>, update: &Bound<'_, PyAny>) -> PyResult<u64> {
        self.db
            .collection(&self.name)
            .update_one(dep_value(filter)?, dep_value(update)?)
            .map_err(map_err)
    }

    fn update_many(&self, filter: &Bound<'_, PyAny>, update: &Bound<'_, PyAny>) -> PyResult<u64> {
        self.db
            .collection(&self.name)
            .update_many(dep_value(filter)?, dep_value(update)?)
            .map_err(map_err)
    }

    fn replace_one(
        &self,
        filter: &Bound<'_, PyAny>,
        replacement: &Bound<'_, PyAny>,
    ) -> PyResult<u64> {
        self.db
            .collection(&self.name)
            .replace_one(dep_value(filter)?, dep_value(replacement)?)
            .map_err(map_err)
    }

    fn delete_one(&self, filter: &Bound<'_, PyAny>) -> PyResult<u64> {
        self.db
            .collection(&self.name)
            .delete_one(dep_value(filter)?)
            .map_err(map_err)
    }

    fn delete_many(&self, filter: &Bound<'_, PyAny>) -> PyResult<u64> {
        self.db
            .collection(&self.name)
            .delete_many(dep_value(filter)?)
            .map_err(map_err)
    }

    fn aggregate(&self, py: Python<'_>, pipeline: &Bound<'_, PyList>) -> PyResult<PyObject> {
        let mut stages = Vec::with_capacity(pipeline.len());
        for s in pipeline.iter() {
            stages.push(dep_value(&s)?);
        }
        let docs = self
            .db
            .collection(&self.name)
            .aggregate(stages)
            .map_err(map_err)?;
        let list = PyList::empty_bound(py);
        for d in &docs {
            list.append(to_py(py, d)?)?;
        }
        Ok(list.into())
    }

    #[pyo3(signature = (keys, *, unique=false, name=None))]
    fn create_index(
        &self,
        keys: &Bound<'_, PyAny>,
        unique: bool,
        name: Option<&str>,
    ) -> PyResult<String> {
        let mut opts = serde_json::json!({ "unique": unique });
        if let Some(n) = name {
            opts["name"] = serde_json::Value::String(n.to_string());
        }
        self.db
            .collection(&self.name)
            .create_index_with_options(dep_value(keys)?, Some(opts))
            .map_err(map_err)
    }

    fn drop_index(&self, name: &str) -> PyResult<()> {
        self.db
            .collection(&self.name)
            .drop_index(name)
            .map_err(map_err)
    }

    fn list_indexes(&self, py: Python<'_>) -> PyResult<PyObject> {
        let infos = self
            .db
            .collection(&self.name)
            .list_indexes()
            .map_err(map_err)?;
        let list = PyList::empty_bound(py);
        for i in infos {
            let dict = PyDict::new_bound(py);
            dict.set_item("name", i.name)?;
            dict.set_item("unique", i.unique)?;
            dict.set_item("sql", i.sql)?;
            list.append(dict)?;
        }
        Ok(list.into())
    }

    fn create_text_index(&self, fields: Vec<String>) -> PyResult<()> {
        self.db
            .collection(&self.name)
            .create_text_index(&fields)
            .map_err(map_err)
    }

    fn drop_text_index(&self) -> PyResult<()> {
        self.db
            .collection(&self.name)
            .drop_text_index()
            .map_err(map_err)
    }

    fn explain(&self, filter: &Bound<'_, PyAny>) -> PyResult<String> {
        let plan = self
            .db
            .collection(&self.name)
            .find(dep_value(filter)?)
            .explain()
            .map_err(map_err)?;
        Ok(plan.to_string())
    }

    #[pyo3(signature = (path, format=None))]
    fn import_file(&self, path: PathBuf, format: Option<&str>) -> PyResult<usize> {
        let fmt = parse_format(format, &path);
        self.db
            .collection(&self.name)
            .import_file(&path, fmt)
            .map_err(map_err)
    }

    fn import_bson_file(&self, path: PathBuf) -> PyResult<usize> {
        self.db
            .collection(&self.name)
            .import_bson_file(&path)
            .map_err(map_err)
    }

    #[pyo3(signature = (path, format=None, filter=None))]
    fn export_file(
        &self,
        path: PathBuf,
        format: Option<&str>,
        filter: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<usize> {
        let fmt = parse_format(format, &path);
        let f = match filter {
            Some(b) if !b.is_none() => dep_value(b)?,
            _ => serde_json::json!({}),
        };
        self.db
            .collection(&self.name)
            .export_file(&path, fmt, f)
            .map_err(map_err)
    }
}

#[pyclass(name = "Transaction")]
struct PyTransaction {
    db: Arc<nosqlite::Database>,
    entered: bool,
}

#[pymethods]
impl PyTransaction {
    fn __enter__(mut slf: PyRefMut<'_, Self>) -> PyResult<Py<Self>> {
        if slf.entered {
            return Err(PyRuntimeError::new_err("transaction already entered"));
        }
        slf.db.begin().map_err(map_err)?;
        slf.entered = true;
        let py = slf.py();
        Ok(slf.into_py(py).extract(py)?)
    }

    #[pyo3(signature = (exc_type=None, _exc_value=None, _tb=None))]
    fn __exit__(
        mut slf: PyRefMut<'_, Self>,
        exc_type: Option<&Bound<'_, PyAny>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _tb: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        if !slf.entered {
            return Ok(false);
        }
        let result = if exc_type.is_some() {
            slf.db.rollback()
        } else {
            slf.db.commit()
        };
        slf.entered = false;
        result.map_err(map_err)?;
        Ok(false)
    }

    fn collection(&self, name: &str) -> PyCollection {
        PyCollection {
            db: self.db.clone(),
            name: name.to_string(),
        }
    }

    fn commit(&mut self) -> PyResult<()> {
        if !self.entered {
            return Err(PyKeyError::new_err("transaction not active"));
        }
        self.db.commit().map_err(map_err)?;
        self.entered = false;
        Ok(())
    }

    fn rollback(&mut self) -> PyResult<()> {
        if !self.entered {
            return Err(PyKeyError::new_err("transaction not active"));
        }
        self.db.rollback().map_err(map_err)?;
        self.entered = false;
        Ok(())
    }
}

/// Module entry point — exposes `Database`, `Collection`, and `Transaction`.
#[pymodule]
fn _nosqlite(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDatabase>()?;
    m.add_class::<PyCollection>()?;
    m.add_class::<PyTransaction>()?;
    Ok(())
}

