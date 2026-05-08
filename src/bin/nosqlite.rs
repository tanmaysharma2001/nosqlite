//! `nosqlite` — interactive shell for a NoSQLite database file.
//!
//! Usage:
//!     nosqlite <file.nosqlite>
//!
//! Built-in commands (one per line):
//!     .help                                 — show this help
//!     .collections                          — list collections
//!     .indexes <coll>                       — list indexes on a collection
//!     .drop <coll>                          — drop a collection
//!     .count <coll> [<filter-json>]         — count documents
//!     .find <coll> [<filter-json>]          — print matching documents
//!     .insert <coll> <doc-json>             — insert a single document
//!     .delete <coll> <filter-json>          — delete matching documents
//!     .update <coll> <filter-json> <upd>    — apply update operators
//!     .aggregate <coll> <pipeline-json>     — run an aggregation pipeline
//!     .text-index <coll> <field> [field...] — create an FTS5 index
//!     .explain <coll> <filter-json>         — print SQLite query plan
//!     .import <coll> <path>                 — import JSON or JSONL
//!     .export <coll> <path>                 — export to JSON or JSONL
//!     .quit                                 — exit
//!
//! Anything that doesn't start with `.` is parsed as JSON and echoed back —
//! handy for sanity-checking syntax before pasting into a `.find` etc.

use nosqlite::{Database, Format};
use serde_json::{json, Value};
use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let path = match args.next() {
        Some(s) if s == "--help" || s == "-h" => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        Some(s) => PathBuf::from(s),
        None => {
            eprintln!("usage: nosqlite <file.nosqlite>");
            return ExitCode::from(2);
        }
    };

    let db = match Database::open(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error opening {}: {}", path.display(), e);
            return ExitCode::FAILURE;
        }
    };

    println!("nosqlite shell — type .help for commands, .quit to exit");
    let stdin = io::stdin();
    let stdout = io::stdout();
    loop {
        print!("> ");
        if stdout.lock().flush().is_err() {
            break;
        }
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match dispatch(&db, line) {
            Ok(Action::Continue) => {}
            Ok(Action::Quit) => break,
            Err(e) => eprintln!("error: {}", e),
        }
    }
    ExitCode::SUCCESS
}

enum Action {
    Continue,
    Quit,
}

fn dispatch(db: &Database, line: &str) -> Result<Action, String> {
    if !line.starts_with('.') {
        // Echo parsed JSON for sanity-checking.
        match serde_json::from_str::<Value>(line) {
            Ok(v) => {
                println!("{}", pretty(&v));
                return Ok(Action::Continue);
            }
            Err(e) => return Err(format!("not a command and not valid JSON: {}", e)),
        }
    }

    let mut parts = split_command(line);
    let cmd = parts.next().unwrap_or("");
    match cmd {
        ".help" => {
            print_usage();
            Ok(Action::Continue)
        }
        ".quit" | ".exit" => Ok(Action::Quit),
        ".collections" => {
            for c in db.list_collections().map_err(|e| e.to_string())? {
                println!("{}", c);
            }
            Ok(Action::Continue)
        }
        ".indexes" => {
            let coll = parts.next().ok_or("usage: .indexes <coll>")?;
            for i in db
                .collection(coll)
                .list_indexes()
                .map_err(|e| e.to_string())?
            {
                println!(
                    "{}{}{}",
                    i.name,
                    if i.unique { "  (UNIQUE)" } else { "" },
                    i.sql.map(|s| format!("\n  {}", s)).unwrap_or_default()
                );
            }
            Ok(Action::Continue)
        }
        ".drop" => {
            let coll = parts.next().ok_or("usage: .drop <coll>")?;
            db.drop_collection(coll).map_err(|e| e.to_string())?;
            println!("dropped {}", coll);
            Ok(Action::Continue)
        }
        ".count" => {
            let coll = parts.next().ok_or("usage: .count <coll> [filter]")?;
            let filter = parse_rest(&mut parts).unwrap_or_else(|| json!({}));
            let n = db
                .collection(coll)
                .count(filter)
                .map_err(|e| e.to_string())?;
            println!("{}", n);
            Ok(Action::Continue)
        }
        ".find" => {
            let coll = parts.next().ok_or("usage: .find <coll> [filter]")?;
            let filter = parse_rest(&mut parts).unwrap_or_else(|| json!({}));
            let docs = db
                .collection(coll)
                .find(filter)
                .into_vec()
                .map_err(|e| e.to_string())?;
            for d in docs {
                println!("{}", pretty(&d));
            }
            Ok(Action::Continue)
        }
        ".insert" => {
            let coll = parts.next().ok_or("usage: .insert <coll> <doc>")?;
            let doc = parse_rest(&mut parts).ok_or("missing document JSON")?;
            let id = db
                .collection(coll)
                .insert_one(doc)
                .map_err(|e| e.to_string())?;
            println!("inserted {}", id);
            Ok(Action::Continue)
        }
        ".delete" => {
            let coll = parts.next().ok_or("usage: .delete <coll> <filter>")?;
            let filter = parse_rest(&mut parts).ok_or("missing filter JSON")?;
            let n = db
                .collection(coll)
                .delete_many(filter)
                .map_err(|e| e.to_string())?;
            println!("{} deleted", n);
            Ok(Action::Continue)
        }
        ".update" => {
            let coll = parts
                .next()
                .ok_or("usage: .update <coll> <filter> <update>")?;
            let rest = parts.collect::<Vec<_>>().join(" ");
            let (filter, upd) = split_two_jsons(&rest)?;
            let n = db
                .collection(coll)
                .update_many(filter, upd)
                .map_err(|e| e.to_string())?;
            println!("{} updated", n);
            Ok(Action::Continue)
        }
        ".aggregate" => {
            let coll = parts.next().ok_or("usage: .aggregate <coll> <pipeline>")?;
            let pipeline = parse_rest(&mut parts).ok_or("missing pipeline JSON")?;
            let arr = pipeline
                .as_array()
                .ok_or("pipeline must be a JSON array")?
                .clone();
            let docs = db
                .collection(coll)
                .aggregate(arr)
                .map_err(|e| e.to_string())?;
            for d in docs {
                println!("{}", pretty(&d));
            }
            Ok(Action::Continue)
        }
        ".text-index" => {
            let coll = parts
                .next()
                .ok_or("usage: .text-index <coll> <field> [field...]")?;
            let fields: Vec<String> = parts.map(|s| s.to_string()).collect();
            if fields.is_empty() {
                return Err("at least one field is required".into());
            }
            db.collection(coll)
                .create_text_index(&fields)
                .map_err(|e| e.to_string())?;
            println!("indexed {} on {:?}", coll, fields);
            Ok(Action::Continue)
        }
        ".explain" => {
            let coll = parts.next().ok_or("usage: .explain <coll> <filter>")?;
            let filter = parse_rest(&mut parts).unwrap_or_else(|| json!({}));
            let plan = db
                .collection(coll)
                .find(filter)
                .explain()
                .map_err(|e| e.to_string())?;
            println!("{}", plan);
            Ok(Action::Continue)
        }
        ".import" => {
            let coll = parts.next().ok_or("usage: .import <coll> <path>")?;
            let p = parts.next().ok_or("missing path")?;
            let path = PathBuf::from(p);
            let fmt = Format::from_path(&path);
            let n = db
                .collection(coll)
                .import_file(&path, fmt)
                .map_err(|e| e.to_string())?;
            println!("imported {} document(s)", n);
            Ok(Action::Continue)
        }
        ".export" => {
            let coll = parts.next().ok_or("usage: .export <coll> <path>")?;
            let p = parts.next().ok_or("missing path")?;
            let path = PathBuf::from(p);
            let fmt = Format::from_path(&path);
            let n = db
                .collection(coll)
                .export_file(&path, fmt, json!({}))
                .map_err(|e| e.to_string())?;
            println!("exported {} document(s) to {}", n, path.display());
            Ok(Action::Continue)
        }
        other => Err(format!("unknown command: {}", other)),
    }
}

fn print_usage() {
    println!(
        "nosqlite shell — commands:\n\
         \n\
         .help                                  show this help\n\
         .collections                           list collections\n\
         .indexes <coll>                        list indexes\n\
         .drop <coll>                           drop a collection\n\
         .count <coll> [filter]                 count documents\n\
         .find <coll> [filter]                  print matching documents\n\
         .insert <coll> <doc>                   insert one document\n\
         .delete <coll> <filter>                delete matching documents\n\
         .update <coll> <filter> <update>       apply update operators\n\
         .aggregate <coll> <pipeline>           run aggregation pipeline\n\
         .text-index <coll> <field> [field...]  create FTS5 index\n\
         .explain <coll> [filter]               show SQLite query plan\n\
         .import <coll> <path>                  import JSON or JSONL\n\
         .export <coll> <path>                  export to JSON or JSONL\n\
         .quit                                  exit"
    );
}

/// Parse `<command> <coll> <rest>` where `<rest>` is one piece of JSON.
fn parse_rest<'a, I: Iterator<Item = &'a str>>(parts: &mut I) -> Option<Value> {
    let rest = parts.collect::<Vec<_>>().join(" ");
    if rest.trim().is_empty() {
        return None;
    }
    serde_json::from_str(rest.trim()).ok()
}

/// Whitespace-aware split that preserves bracketed JSON tokens as one unit
/// so that `{"a":1}` doesn't get broken at the colon.
fn split_command(line: &str) -> std::vec::IntoIter<&str> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut depth = 0;
    let mut start: Option<usize> = None;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if depth == 0 && c.is_whitespace() {
            if let Some(s) = start.take() {
                out.push(&line[s..i]);
            }
        } else {
            if start.is_none() {
                start = Some(i);
            }
            if c == '{' || c == '[' {
                depth += 1;
            } else if (c == '}' || c == ']') && depth > 0 {
                depth -= 1;
            }
        }
        i += 1;
    }
    if let Some(s) = start {
        out.push(&line[s..]);
    }
    out.into_iter()
}

/// Split a string into two adjacent JSON values.
fn split_two_jsons(s: &str) -> Result<(Value, Value), String> {
    let s = s.trim();
    let mut iter = serde_json::Deserializer::from_str(s).into_iter::<Value>();
    let a = iter
        .next()
        .ok_or("expected two JSON values")?
        .map_err(|e| e.to_string())?;
    let b = iter
        .next()
        .ok_or("expected a second JSON value")?
        .map_err(|e| e.to_string())?;
    Ok((a, b))
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}
