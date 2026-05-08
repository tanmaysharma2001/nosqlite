//! MQL filter compiler. Translates a JSON filter document into a SQL `WHERE`
//! fragment plus a parameter list.
//!
//! JSON paths are inlined as SQL string literals (with single-quote escaping)
//! so that SQLite's expression-index planner can match them. Values are always
//! parameterized.

use crate::error::{Error, Result};
use crate::util::{json_to_sql, mongo_path};
use rusqlite::types::Value as SqlValue;
use serde_json::Value;

pub struct CompiledFilter {
    pub sql: String,
    pub params: Vec<SqlValue>,
}

pub fn compile(filter: &Value) -> Result<CompiledFilter> {
    let mut c = Compiler::default();
    let sql = c.compile_filter(filter)?;
    Ok(CompiledFilter {
        sql,
        params: c.params,
    })
}

#[derive(Default)]
struct Compiler {
    params: Vec<SqlValue>,
}

fn path_lit(path: &str) -> String {
    format!("'{}'", path.replace('\'', "''"))
}

fn json_extract_lit(path: &str) -> String {
    format!("json_extract(doc, {})", path_lit(path))
}

fn json_type_lit(path: &str) -> String {
    format!("json_type(doc, {})", path_lit(path))
}

impl Compiler {
    fn compile_filter(&mut self, filter: &Value) -> Result<String> {
        let obj = match filter {
            Value::Object(o) => o,
            Value::Null => return Ok("1=1".to_string()),
            _ => return Err(Error::InvalidQuery("filter must be a JSON object".into())),
        };

        if obj.is_empty() {
            return Ok("1=1".to_string());
        }

        let mut clauses = Vec::with_capacity(obj.len());
        for (key, val) in obj {
            let clause = if let Some(stripped) = key.strip_prefix('$') {
                self.compile_top_logical(stripped, val)?
            } else {
                self.compile_field(key, val)?
            };
            clauses.push(clause);
        }
        Ok(clauses.join(" AND "))
    }

    fn compile_top_logical(&mut self, op: &str, val: &Value) -> Result<String> {
        match op {
            "and" | "or" | "nor" => {
                let arr = val
                    .as_array()
                    .ok_or_else(|| Error::InvalidQuery(format!("${} requires an array", op)))?;
                if arr.is_empty() {
                    return Ok("1=1".to_string());
                }
                let parts: Vec<String> = arr
                    .iter()
                    .map(|f| self.compile_filter(f).map(|s| format!("({})", s)))
                    .collect::<Result<_>>()?;
                Ok(match op {
                    "and" => parts.join(" AND "),
                    "or" => parts.join(" OR "),
                    "nor" => format!("NOT ({})", parts.join(" OR ")),
                    _ => unreachable!(),
                })
            }
            "not" => {
                let inner = self.compile_filter(val)?;
                Ok(format!("NOT ({})", inner))
            }
            other => Err(Error::InvalidQuery(format!(
                "unknown top-level operator $${}",
                other
            ))),
        }
    }

    fn compile_field(&mut self, field: &str, val: &Value) -> Result<String> {
        let path = mongo_path(field);

        if let Value::Object(obj) = val {
            let has_op = obj.keys().any(|k| k.starts_with('$'));
            let has_non_op = obj.keys().any(|k| !k.starts_with('$'));
            if has_op {
                if has_non_op {
                    return Err(Error::InvalidQuery(format!(
                        "cannot mix operators and fields in value for {}",
                        field
                    )));
                }
                let mut clauses = Vec::with_capacity(obj.len());
                for (op, opval) in obj {
                    clauses.push(self.compile_field_op(&path, op, opval)?);
                }
                return Ok(format!("({})", clauses.join(" AND ")));
            }
        }

        Ok(self.eq_clause(&path, val))
    }

    fn eq_clause(&mut self, path: &str, val: &Value) -> String {
        let extr = json_extract_lit(path);
        match val {
            Value::Null => format!("{} IS NULL", extr),
            Value::Array(_) | Value::Object(_) => {
                self.params
                    .push(SqlValue::Text(serde_json::to_string(val).unwrap()));
                format!("json({}) = json(?)", extr)
            }
            _ => {
                self.params.push(json_to_sql(val).unwrap());
                format!("{} = ?", extr)
            }
        }
    }

    fn compile_field_op(&mut self, path: &str, op: &str, val: &Value) -> Result<String> {
        let extr = json_extract_lit(path);
        let typ = json_type_lit(path);

        match op {
            "$eq" => Ok(self.eq_clause(path, val)),
            "$ne" => match val {
                Value::Null => Ok(format!("{} IS NOT NULL", extr)),
                Value::Array(_) | Value::Object(_) => {
                    self.params
                        .push(SqlValue::Text(serde_json::to_string(val).unwrap()));
                    Ok(format!(
                        "(json({extr}) IS NOT json(?) OR {typ} IS NULL)",
                        extr = extr,
                        typ = typ
                    ))
                }
                _ => {
                    self.params.push(json_to_sql(val)?);
                    Ok(format!("{} IS NOT ?", extr))
                }
            },
            "$gt" | "$gte" | "$lt" | "$lte" => {
                let cmp = match op {
                    "$gt" => ">",
                    "$gte" => ">=",
                    "$lt" => "<",
                    "$lte" => "<=",
                    _ => unreachable!(),
                };
                self.params.push(json_to_sql(val)?);
                Ok(format!("{} {} ?", extr, cmp))
            }
            "$in" => {
                let arr = val
                    .as_array()
                    .ok_or_else(|| Error::InvalidQuery("$in requires array".into()))?;
                if arr.is_empty() {
                    return Ok("0=1".to_string());
                }
                let mut placeholders = Vec::with_capacity(arr.len());
                for v in arr {
                    self.params.push(json_to_sql(v)?);
                    placeholders.push("?");
                }
                Ok(format!("{} IN ({})", extr, placeholders.join(", ")))
            }
            "$nin" => {
                let arr = val
                    .as_array()
                    .ok_or_else(|| Error::InvalidQuery("$nin requires array".into()))?;
                if arr.is_empty() {
                    return Ok("1=1".to_string());
                }
                let mut placeholders = Vec::with_capacity(arr.len());
                for v in arr {
                    self.params.push(json_to_sql(v)?);
                    placeholders.push("?");
                }
                Ok(format!(
                    "({} NOT IN ({}) OR {} IS NULL)",
                    extr,
                    placeholders.join(", "),
                    typ
                ))
            }
            "$exists" => {
                let exists = val
                    .as_bool()
                    .ok_or_else(|| Error::InvalidQuery("$exists requires bool".into()))?;
                Ok(if exists {
                    format!("{} IS NOT NULL", typ)
                } else {
                    format!("{} IS NULL", typ)
                })
            }
            "$type" => {
                let tname = val
                    .as_str()
                    .ok_or_else(|| Error::InvalidQuery("$type requires string".into()))?;
                self.params.push(SqlValue::Text(tname.to_string()));
                Ok(format!("{} = ?", typ))
            }
            "$size" => {
                let n = val
                    .as_i64()
                    .ok_or_else(|| Error::InvalidQuery("$size requires integer".into()))?;
                self.params.push(SqlValue::Integer(n));
                // Two-arg form handles non-array values gracefully (returns 0/NULL).
                Ok(format!("json_array_length(doc, {}) = ?", path_lit(path)))
            }
            "$not" => {
                let inner_obj = val
                    .as_object()
                    .ok_or_else(|| Error::InvalidQuery("$not requires operator object".into()))?;
                let mut clauses = Vec::with_capacity(inner_obj.len());
                for (op2, val2) in inner_obj {
                    clauses.push(self.compile_field_op(path, op2, val2)?);
                }
                Ok(format!("NOT ({})", clauses.join(" AND ")))
            }
            other => Err(Error::InvalidQuery(format!("unknown operator {}", other))),
        }
    }
}
