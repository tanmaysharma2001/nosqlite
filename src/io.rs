//! JSON / JSONL import and export.
//!
//! Two on-disk formats are supported:
//!
//! - **JSON** — a single top-level array of documents.
//! - **JSONL** — one document per line (more memory-efficient for large
//!   datasets since records are streamed).

use crate::database::Collection;
use crate::error::{Error, Result};
use serde_json::Value;
use std::fs::File;
#[cfg(feature = "bson")]
use std::io::Read;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `[doc1, doc2, ...]`
    Json,
    /// One JSON document per line.
    Jsonl,
}

impl Format {
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|s| s.to_str()) {
            Some("jsonl") | Some("ndjson") => Format::Jsonl,
            _ => Format::Json,
        }
    }
}

/// Convert a BSON value into the equivalent `serde_json::Value`. ObjectId,
/// dates, binary data, and the Decimal128 type are mapped to strings.
#[cfg(feature = "bson")]
fn bson_to_json(b: bson::Bson) -> Value {
    use bson::Bson;
    use serde_json::Map;
    match b {
        Bson::Double(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Bson::String(s) => Value::String(s),
        Bson::Array(arr) => Value::Array(arr.into_iter().map(bson_to_json).collect()),
        Bson::Document(d) => {
            let mut m = Map::new();
            for (k, v) in d {
                m.insert(k, bson_to_json(v));
            }
            Value::Object(m)
        }
        Bson::Boolean(b) => Value::Bool(b),
        Bson::Null | Bson::Undefined => Value::Null,
        Bson::Int32(n) => Value::Number(n.into()),
        Bson::Int64(n) => Value::Number(n.into()),
        Bson::ObjectId(oid) => Value::String(oid.to_hex()),
        Bson::DateTime(dt) => Value::String(dt.to_string()),
        Bson::Decimal128(d) => Value::String(d.to_string()),
        Bson::Symbol(s) => Value::String(s),
        Bson::Timestamp(ts) => {
            serde_json::json!({ "$timestamp": { "t": ts.time, "i": ts.increment } })
        }
        Bson::Binary(b) => {
            let sub: u8 = b.subtype.into();
            serde_json::json!({ "$binary": { "subType": sub as i64, "base64": base64_lite(&b.bytes) } })
        }
        Bson::RegularExpression(re) => {
            serde_json::json!({ "$regex": re.pattern, "$options": re.options })
        }
        Bson::JavaScriptCode(c) => serde_json::json!({ "$code": c }),
        Bson::JavaScriptCodeWithScope(c) => {
            serde_json::json!({ "$code": c.code, "$scope": bson_to_json(Bson::Document(c.scope)) })
        }
        Bson::DbPointer(_) | Bson::MinKey | Bson::MaxKey => Value::Null,
    }
}

#[cfg(feature = "bson")]
fn base64_lite(bytes: &[u8]) -> String {
    // Tiny no-dep base64 encoder for the binary subtype payload. Not used on
    // hot paths.
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut chunks = bytes.chunks_exact(3);
    for c in &mut chunks {
        let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(TABLE[((n >> 18) & 63) as usize] as char);
            out.push(TABLE[((n >> 12) & 63) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(TABLE[((n >> 18) & 63) as usize] as char);
            out.push(TABLE[((n >> 12) & 63) as usize] as char);
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

impl<'a> Collection<'a> {
    /// Append documents from a JSON or JSONL file. Returns the number of
    /// documents inserted.
    pub fn import_file<P: AsRef<Path>>(&self, path: P, format: Format) -> Result<usize> {
        let file = File::open(path.as_ref())?;
        match format {
            Format::Json => {
                let docs: Value = serde_json::from_reader(BufReader::new(file))?;
                let arr = docs.as_array().ok_or_else(|| {
                    Error::InvalidQuery("JSON import file must be an array of documents".into())
                })?;
                let docs: Vec<Value> = arr.clone();
                let n = docs.len();
                self.insert_many(docs)?;
                Ok(n)
            }
            Format::Jsonl => {
                let mut buf = Vec::new();
                let mut total = 0usize;
                for line in BufReader::new(file).lines() {
                    let line = line?;
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let doc: Value = serde_json::from_str(trimmed)?;
                    buf.push(doc);
                    if buf.len() >= 1_000 {
                        let batch = std::mem::take(&mut buf);
                        total += batch.len();
                        self.insert_many(batch)?;
                    }
                }
                if !buf.is_empty() {
                    total += buf.len();
                    self.insert_many(buf)?;
                }
                Ok(total)
            }
        }
    }

    /// Append documents from a stream of concatenated BSON documents (the
    /// format produced by `mongodump`). Returns the number of documents
    /// inserted. Available only with the `bson` feature.
    #[cfg(feature = "bson")]
    pub fn import_bson<R: Read>(&self, mut r: R) -> Result<usize> {
        let mut buf = Vec::with_capacity(1024);
        let mut total = 0usize;
        loop {
            // BSON documents are length-prefixed: read the 4-byte length,
            // then the rest of the document, then decode.
            let mut len_bytes = [0u8; 4];
            match r.read_exact(&mut len_bytes) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
            let len = i32::from_le_bytes(len_bytes);
            if !(5..=64 * 1024 * 1024).contains(&len) {
                return Err(Error::InvalidQuery(format!(
                    "implausible BSON document length: {}",
                    len
                )));
            }
            buf.clear();
            buf.extend_from_slice(&len_bytes);
            let needed = (len as usize) - 4;
            buf.resize(buf.len() + needed, 0);
            r.read_exact(&mut buf[4..])?;
            let bson_doc = bson::Document::from_reader(&mut buf.as_slice())
                .map_err(|e| Error::InvalidQuery(format!("BSON decode failed: {}", e)))?;
            let json = bson_to_json(bson::Bson::Document(bson_doc));
            self.insert_one(json)?;
            total += 1;
        }
        Ok(total)
    }

    /// Convenience wrapper around [`Collection::import_bson`] for files.
    #[cfg(feature = "bson")]
    pub fn import_bson_file<P: AsRef<Path>>(&self, path: P) -> Result<usize> {
        let f = File::open(path.as_ref())?;
        self.import_bson(BufReader::new(f))
    }

    /// Write all documents matching `filter` to `path` in the chosen format.
    /// Pass `serde_json::json!({})` to export the whole collection.
    pub fn export_file<P: AsRef<Path>>(
        &self,
        path: P,
        format: Format,
        filter: Value,
    ) -> Result<usize> {
        let docs = self.find(filter).into_vec()?;
        let n = docs.len();
        let file = File::create(path.as_ref())?;
        let mut w = BufWriter::new(file);
        match format {
            Format::Json => {
                serde_json::to_writer(&mut w, &docs)?;
                w.write_all(b"\n")?;
            }
            Format::Jsonl => {
                for d in &docs {
                    serde_json::to_writer(&mut w, d)?;
                    w.write_all(b"\n")?;
                }
            }
        }
        w.flush()?;
        Ok(n)
    }
}
