# NoSQLite

A MongoDB-style document database that stores everything in a single SQLite
file. Implemented as an embeddable Rust crate.

```rust
use nosqlite::Database;
use serde_json::json;

let db = Database::open("app.nosqlite")?;
let users = db.collection("users");

users.insert_many(vec![
    json!({ "name": "Alice", "age": 30 }),
    json!({ "name": "Bob",   "age": 22 }),
])?;

users.create_index(json!({ "age": 1 }))?;

let adults = users
    .find(json!({ "age": { "$gte": 25 } }))
    .sort(json!({ "age": -1 }))
    .project(json!({ "name": 1, "_id": 0 }))
    .into_vec()?;
```

## Status

Phases 1–4 are implemented:

- **Storage** — one SQLite table per collection, ULID-based `_id` generation,
  atomic batch inserts, in-memory or file-backed.
- **Query engine** — full MQL filter compiler covering `$eq`, `$ne`, `$gt`,
  `$gte`, `$lt`, `$lte`, `$in`, `$nin`, `$exists`, `$type`, `$size`, `$not`,
  with logical combinators `$and`, `$or`, `$nor`, `$not` at any nesting depth.
  Supports dotted-field access (`addr.city`).
- **Updates** — `$set`, `$unset`, `$inc`, `$mul`, `$min`, `$max`, `$rename`,
  `$push` (with `$each`), `$pull`, `$pop`, `$addToSet`. Replacement-style
  updates also supported.
- **Cursor** — `sort`, `limit`, `skip`, `project` (include/exclude with
  dotted paths), plus `count()`, `first()`, `into_vec()`, `explain()`.
- **Indexes** — single-field and compound expression indexes over
  `json_extract`. The MQL compiler inlines JSON paths as literals so
  SQLite's planner picks them up automatically. Run `.explain()` on any
  cursor to see the chosen plan.
- **Aggregation pipeline** — `$match`, `$project`, `$addFields` / `$set`,
  `$sort`, `$limit`, `$skip`, `$count`, `$group` (with `$sum`/`$avg`/`$min`/
  `$max`/`$first`/`$last`/`$push`/`$addToSet`/`$count` accumulators),
  `$unwind`, `$lookup`. Expression operators include `$add`/`$subtract`/
  `$multiply`/`$divide`, `$concat`, `$toUpper`/`$toLower`, `$ifNull`, and
  field references via `"$field"`.
- **JSON-Schema validation** — register a per-collection schema with
  `db.set_validator(...)`; enforced on insert/update/replace. Persists in a
  meta table so it survives reopens. Supports `type`, `required`,
  `properties`, `additionalProperties`, `items`, `enum`, `const`, `minimum`/
  `maximum`/`exclusiveMinimum`/`exclusiveMaximum`, `minLength`/`maxLength`,
  `minItems`/`maxItems`, `multipleOf`.
- **Import / export** — JSON array files (`coll.import_file(p, Format::Json)`)
  and streaming JSONL (`Format::Jsonl`). Batched inserts during import keep
  memory bounded.
- **Transactions** — `db.transaction(|tx| { ... })` runs the closure in a
  SQLite `BEGIN IMMEDIATE`; returning `Err` rolls back. The `tx.collection(name)`
  handle exposes the same CRUD API but without the cursor builder.
- **Full-text search** — `coll.create_text_index(&["title", "body"])` builds
  an FTS5 virtual table that's auto-synced on insert / update / delete. Query
  with the standard MQL `$text` operator: `find(json!({ "$text": { "$search": "..." } }))`.
- **CLI shell** — `cargo install` builds a `nosqlite <file>` binary that
  exposes all of the above through a small REPL (`.find`, `.aggregate`,
  `.text-index`, `.import`, `.explain`, …).
- **BSON / `mongodump` import** — behind the `bson` feature flag:
  `coll.import_bson_file("dump/app/users.bson")?` ingests a stream of
  concatenated BSON documents (the format `mongodump` writes), mapping
  `ObjectId` to its hex string and preserving nested documents and arrays.
- **Python SDK** — a thin PyO3 wrapper at [python/](python/) that exposes
  the full Rust API to Python. Build with `cd python && maturin develop --release`.
- **Node.js SDK** — a napi-rs wrapper at [node/](node/) that exposes the same
  surface to Node, with auto-generated TypeScript types. Build with
  `cd node && npm install && npx napi build --release`. A Mongoose-style
  ODM (Schema + Model with hooks, indexes, validation) ships in
  [node/odm.js](node/odm.js).
- **Python ODM** — Pydantic-backed `Document[T]` wrapper at
  [python/src_py/nosqlite/orm.py](python/src_py/nosqlite/orm.py). Pydantic
  schemas auto-translate to JSON-Schema validators.
- **`#[document]` macro** — Rust attribute macro that derives `Serialize`/
  `Deserialize` and the `Document` trait, removing the
  `#[serde(rename = "_id", ...)]` boilerplate from typed collections. Lives
  in [derive/](derive/).

The `.nosqlite` file format is identical across all three runtimes — a file
written by the Rust crate, the CLI shell, the Python module, or the Node
module is readable and writable by any of the others.

Not yet implemented (Phase 5): hosted docs site.

## API tour

### Connecting

```rust
let db = Database::open("path.nosqlite")?;     // file-backed (WAL mode)
let db = Database::open_in_memory()?;          // ephemeral
db.list_collections()?;
db.drop_collection("users")?;
```

### CRUD

```rust
let id = users.insert_one(json!({ "name": "Alice" }))?;
let ids = users.insert_many(vec![json!({...}), json!({...})])?;

let one = users.find_one(json!({ "_id": id }))?;
let n   = users.count(json!({ "age": { "$gt": 25 } }))?;
let all = users.find(json!({})).into_vec()?;

users.update_one(filter, json!({ "$set": { "name": "Alicia" } }))?;
users.update_many(filter, json!({ "$inc": { "logins": 1 } }))?;
users.replace_one(filter, json!({ "name": "new", "v": 1 }))?;

users.delete_one(filter)?;
users.delete_many(filter)?;
```

### Typed access (Rust)

```rust
use nosqlite::{document, Database, TypedCollection};
use serde_json::json;

#[document]
#[derive(Debug, Clone)]
struct User {
    #[id]
    id: Option<String>,
    name: String,
    age: u32,
}

let users: TypedCollection<User> = db.typed_collection("users");
let mut alice = User { id: None, name: "Alice".into(), age: 30 };
users.insert(&mut alice)?;                        // alice.id is now Some(...)
let same: User = users.get(alice.id.as_ref().unwrap())?.unwrap();
```

The `#[document]` attribute macro derives `Serialize`/`Deserialize`, renames
the marked field to `_id` on the wire, and implements the `Document` trait.
Filters and updates stay JSON for full MQL access; drop down to the
underlying `Collection` via `users.untyped()` for indexes, aggregation,
FTS, or validators.

### Aggregation

```rust
let revenue = orders.aggregate(vec![
    json!({ "$match": { "status": "shipped" } }),
    json!({ "$group": {
        "_id": "$customer_id",
        "total": { "$sum": "$amount" },
        "n":     { "$sum": 1 },
    }}),
    json!({ "$sort": { "total": -1 } }),
    json!({ "$limit": 10 }),
])?;

// Joins are spelled `$lookup`, just like MongoDB:
users.aggregate(vec![
    json!({ "$lookup": {
        "from": "orders",
        "localField": "_id",
        "foreignField": "user",
        "as": "orders",
    }}),
])?;
```

### Schema validation

```rust
use nosqlite::ValidationLevel;

db.set_validator(
    "users",
    json!({
        "type": "object",
        "required": ["name", "age"],
        "properties": {
            "name": { "type": "string", "minLength": 1 },
            "age":  { "type": "integer", "minimum": 0 }
        }
    }),
    ValidationLevel::Strict,
)?;
```

### Import / export

```rust
use nosqlite::Format;

users.import_file("seed.jsonl", Format::Jsonl)?;     // streamed
users.export_file("dump.json",  Format::Json, json!({}))?;
```

### Transactions

```rust
db.transaction(|tx| {
    tx.collection("accts").update_one(
        json!({ "_id": "alice" }), json!({ "$inc": { "bal": -25 } }),
    )?;
    tx.collection("accts").update_one(
        json!({ "_id": "bob"   }), json!({ "$inc": { "bal":  25 } }),
    )?;
    Ok::<_, nosqlite::Error>(())
})?;
```

### Full-text search

```rust
posts.create_text_index(&["title", "body"])?;
let hits = posts
    .find(json!({ "$text": { "$search": "rust crab" }, "lang": "en" }))
    .into_vec()?;
```

### CLI shell

```text
$ nosqlite mydb.nosqlite
nosqlite shell — type .help for commands, .quit to exit
> .insert users {"name":"Alice","age":30}
inserted 01HZ...
> .find users {"age": {"$gt": 25}}
{"name": "Alice", "age": 30, "_id": "01HZ..."}
> .aggregate orders [{"$group":{"_id":"$customer","total":{"$sum":"$amt"}}}]
...
```

### Python

```python
import nosqlite

db = nosqlite.Database("app.nosqlite")
users = db.collection("users")

users.insert_many([
    {"name": "Alice", "age": 30},
    {"name": "Bob",   "age": 22},
])
users.create_index({"age": 1})

for u in users.find({"age": {"$gt": 25}}, sort={"age": -1}, limit=10):
    print(u)

with db.transaction() as tx:
    tx.collection("a").insert_one({"v": 1})
    tx.collection("b").insert_one({"v": 2})
```

Build the Python extension from a checkout with `cd python && maturin develop`.
The same `.nosqlite` file is readable and writable by the Rust crate, the
Python module, and the CLI.

### Node.js / TypeScript

```js
const { Database } = require('nosqlite')

const db = new Database('app.nosqlite')
const users = db.collection('users')

users.insertMany([
  { name: 'Alice', age: 30 },
  { name: 'Bob',   age: 22 },
])
users.createIndex({ age: 1 })

const adults = users.find({ age: { $gt: 25 } }, { sort: { age: -1 }, limit: 10 })

const tx = db.beginTransaction()
try {
  tx.collection('a').insertOne({ v: 1 })
  tx.commit()
} catch (e) { tx.rollback(); throw e }
```

TypeScript types ship with the module — `index.d.ts` is auto-generated from
the Rust signatures by napi-rs.

### Migrating from MongoDB

```toml
[dependencies]
nosqlite = { version = "0.1", features = ["bson"] }
```

```rust
db.collection("users").import_bson_file("dump/app/users.bson")?;
```

See [MIGRATING.md](MIGRATING.md) for a side-by-side cheatsheet covering
operators, indexes, transactions, validation, and the behavioral
differences you should know about.

### Indexes

```rust
users.create_index(json!({ "email": 1 }))?;
users.create_index_with_options(
    json!({ "tenant": 1, "created_at": -1 }),
    Some(json!({ "name": "tenant_recent", "unique": false })),
)?;
users.list_indexes()?;
users.drop_index("tenant_recent")?;

println!("{}", users.find(filter).explain()?);
```

## Performance

`cargo run --release --example bench` against an in-memory database with
100,000 documents (six fields, one nested object, one tag array) on an
M-series Mac:

| Operation                                                  | Time           |
|------------------------------------------------------------|----------------|
| `insert_many` (single batched transaction)                 | 555 ms (~180k ops/s) |
| `find` equality filter — full scan, no index               | 38.9 ms        |
| `find` equality filter — with index on the queried field   | **0.028 ms**   |
| `find` range filter `{$gte, $lt}` — full scan              | 43.9 ms        |
| `find` range filter — with index                           | **0.226 ms**   |
| `update_one` with `$inc` (1000 keyed updates)              | 215 ms (~4.7k ops/s) |
| `aggregate` `$match`+`$group`+`$sort`+`$limit` over 100k   | 97.8 ms        |
| `delete_many` range filter (10k rows removed)              | 15.3 ms        |

The roadmap target was **indexed `find()` on 500k documents under 5 ms** —
indexed equality at 100k is 28 µs, two orders of magnitude under target.

## Storage layout

Each collection maps to a SQLite table:

```sql
CREATE TABLE "<name>" (
    _id        TEXT PRIMARY KEY,
    doc        TEXT NOT NULL,        -- JSON text
    created_at INTEGER NOT NULL,     -- ms since epoch
    updated_at INTEGER NOT NULL
);
```

Indexes are created as expression indexes:

```sql
CREATE INDEX "<auto-name>" ON "<name>" (json_extract(doc, '$.<field>') ASC);
```

Because the query compiler inlines JSON paths as SQL string literals, the
SQLite planner can match these indexes structurally — no per-index hint
plumbing required.

## Roadmap

| Phase | Focus                                        | Status |
|-------|----------------------------------------------|--------|
| 1     | Foundation — storage & basic CRUD            | done   |
| 2     | Query engine — MQL compiler & updates        | done   |
| 3     | Indexes, expression-index planner, benchmarks | done   |
| 4     | Aggregation, validation, transactions, FTS5, BSON import | done |
| 5     | CLI, migration guide, Python SDK, Node SDK, ODMs, derive macro done; hosted docs site pending | done bar docs hosting |

## License

MIT
