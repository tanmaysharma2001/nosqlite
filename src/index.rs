//! Index management. Indexes are SQLite expression indexes over
//! `json_extract(doc, '<path>')`.

use crate::error::{Error, Result};
use crate::util::{mongo_path, validate_identifier};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct IndexSpec {
    pub fields: Vec<(String, i32)>,
    pub unique: bool,
    pub name: Option<String>,
}

impl IndexSpec {
    pub fn parse(keys: &Value, options: Option<&Value>) -> Result<Self> {
        let obj = keys
            .as_object()
            .ok_or_else(|| Error::InvalidIndex("index spec must be an object".into()))?;
        if obj.is_empty() {
            return Err(Error::InvalidIndex(
                "index spec must have at least one field".into(),
            ));
        }
        let mut fields = Vec::with_capacity(obj.len());
        for (k, v) in obj {
            let dir = v.as_i64().ok_or_else(|| {
                Error::InvalidIndex(format!("direction for {} must be 1 or -1", k))
            })?;
            if dir != 1 && dir != -1 {
                return Err(Error::InvalidIndex(format!(
                    "direction for {} must be 1 or -1",
                    k
                )));
            }
            fields.push((k.clone(), dir as i32));
        }

        let mut unique = false;
        let mut name = None;
        if let Some(opts) = options {
            if let Some(o) = opts.as_object() {
                if let Some(u) = o.get("unique") {
                    unique = u.as_bool().unwrap_or(false);
                }
                if let Some(n) = o.get("name") {
                    name = n.as_str().map(|s| s.to_string());
                }
            }
        }

        Ok(IndexSpec {
            fields,
            unique,
            name,
        })
    }

    pub fn auto_name(&self, collection: &str) -> String {
        let parts: Vec<String> = self
            .fields
            .iter()
            .map(|(f, d)| format!("{}_{}", f.replace('.', "__"), d))
            .collect();
        format!("nsl_{}_{}", collection, parts.join("_"))
    }

    pub fn create_sql(&self, collection: &str) -> Result<String> {
        validate_identifier(collection)?;
        let name = match &self.name {
            Some(n) => {
                validate_identifier(n)?;
                n.clone()
            }
            None => self.auto_name(collection),
        };
        let cols: Vec<String> = self
            .fields
            .iter()
            .map(|(f, dir)| {
                let path = mongo_path(f);
                let direction = if *dir == 1 { "ASC" } else { "DESC" };
                format!(
                    "json_extract(doc, '{}') {}",
                    path.replace('\'', "''"),
                    direction
                )
            })
            .collect();
        let unique = if self.unique { "UNIQUE " } else { "" };
        Ok(format!(
            "CREATE {unique}INDEX IF NOT EXISTS \"{name}\" ON \"{coll}\" ({cols})",
            unique = unique,
            name = name,
            coll = collection,
            cols = cols.join(", ")
        ))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub sql: Option<String>,
}
