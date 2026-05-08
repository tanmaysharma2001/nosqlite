# Migrating from MongoDB

NoSQLite's API tracks MongoDB closely on purpose. This document is a
side-by-side cheatsheet for the most common operations and a guide to the
small number of behavioral differences.

## Connecting

| MongoDB                                    | NoSQLite (Rust)                                |
|--------------------------------------------|------------------------------------------------|
| `mongoclient.connect("mongodb://...")`     | `Database::open("path.nosqlite")?`             |
| `client.db("app")`                         | (no separate database namespace; one file = one DB) |
| `db.users` / `db.collection("users")`      | `db.collection("users")`                       |

There's no client/server. Open the file, work, drop the handle.

## CRUD

| MongoDB                                                | NoSQLite                                                                |
|--------------------------------------------------------|-------------------------------------------------------------------------|
| `db.users.insertOne({ name: "Alice" })`                | `users.insert_one(json!({ "name": "Alice" }))?`                         |
| `db.users.insertMany([...])`                           | `users.insert_many(vec![...])?`                                         |
| `db.users.findOne({ name: "Alice" })`                  | `users.find_one(json!({ "name": "Alice" }))?`                           |
| `db.users.find({ age: { $gt: 25 } }).sort({ age: -1 })` | `users.find(json!({ "age": { "$gt": 25 } })).sort(json!({ "age": -1 }))` |
| `db.users.countDocuments({...})`                       | `users.count(json!({...}))?`                                            |
| `db.users.updateOne(filter, { $set: {...} })`          | `users.update_one(filter, json!({ "$set": {...} }))?`                   |
| `db.users.replaceOne(filter, doc)`                     | `users.replace_one(filter, doc)?`                                       |
| `db.users.deleteMany(filter)`                          | `users.delete_many(filter)?`                                            |
| `cursor.toArray()`                                     | `cursor.into_vec()?`                                                    |

## Operators

Every operator below compiles to SQLite via `json_extract`, with paths
inlined as literals so SQLite's expression-index planner can pick up
matching indexes.

**Comparison:** `$eq` `$ne` `$gt` `$gte` `$lt` `$lte` `$in` `$nin`
**Existence / type:** `$exists` `$type` `$size`
**Logical:** `$and` `$or` `$nor` `$not`
**Updates:** `$set` `$unset` `$inc` `$mul` `$min` `$max` `$rename` `$push`
(with `$each`) `$pull` `$pop` `$addToSet`
**Aggregation stages:** `$match` `$project` `$addFields` (`$set`) `$sort`
`$limit` `$skip` `$count` `$group` `$unwind` `$lookup`
**Group accumulators:** `$sum` `$avg` `$min` `$max` `$first` `$last` `$push`
`$addToSet` `$count`
**Aggregation expressions:** field references (`"$field"`), `$add`,
`$subtract`, `$multiply`, `$divide`, `$concat`, `$toUpper`, `$toLower`,
`$ifNull`, `$literal`
**Full-text:** `$text` (`$search`) — backed by FTS5

## Indexes

```js
// MongoDB
db.users.createIndex({ tenant: 1, created: -1 });
db.users.createIndex({ email: 1 }, { unique: true });
```

```rust
// NoSQLite
users.create_index(json!({ "tenant": 1, "created": -1 }))?;
users.create_index_with_options(
    json!({ "email": 1 }),
    Some(json!({ "unique": true })),
)?;
```

Run `users.find(filter).explain()?` and look for `USING INDEX` to confirm
the planner is picking up your index.

## Transactions

```js
// MongoDB
const session = client.startSession();
session.withTransaction(async () => {
    await accts.updateOne({ _id: "alice" }, { $inc: { bal: -25 } }, { session });
    await accts.updateOne({ _id: "bob"   }, { $inc: { bal:  25 } }, { session });
});
```

```rust
// NoSQLite
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

Returning `Err` from the closure rolls everything back. Inside a
transaction, the cursor builder is unavailable — use `find_one`,
`find_into_vec(filter)`, or `find_with(filter, FindOptions { ... })`.

## Schema validation

```js
db.createCollection("users", { validator: { $jsonSchema: { ... } } });
```

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

The validator is persisted in the database file itself and applied to every
insert/update/replace.

## Importing existing MongoDB data

```sh
# 1. Dump from MongoDB
mongodump --collection=users --db=app --out=dump
```

```rust
// 2. Import in Rust (requires the `bson` feature)
let users = db.collection("users");
users.import_bson_file("dump/app/users.bson")?;
```

BSON `ObjectId` values are mapped to their hex string form on import.
Embedded documents and arrays round-trip naturally.

For JSON / JSONL dumps:

```rust
use nosqlite::Format;

users.import_file("seed.jsonl", Format::Jsonl)?;
users.export_file("dump.json",  Format::Json, json!({}))?;
```

## Behavioral differences from MongoDB

These are the places where NoSQLite intentionally diverges:

- **`_id` defaults to a [ULID](https://github.com/ulid/spec) string**, not an
  ObjectId. ULIDs sort lexicographically by creation time, which keeps the
  primary key clustering friendly. Explicit `_id`s in inserted documents
  are honoured.
- **No `$where` / server-side JS.** NoSQLite is an embedded library; there's
  no server to run JS in. Use the aggregation expression operators instead.
- **No change streams / oplog.** SQLite has its own update hook mechanism
  but it isn't surfaced yet.
- **Single-writer.** SQLite serializes writes per-database, so very high
  write concurrency from multiple processes is best handled with WAL mode
  (default on file-backed databases) and short transactions.
- **Database = file.** There's no concept of multiple databases inside one
  client, replica sets, or sharding. One process, one file.
- **`$regex` is not currently implemented** (would require pulling in a
  regex extension to SQLite). For substring matching, fall back to
  `$text` + FTS5 or filter in Rust after `find_into_vec`.
- **Cursor-only inside transactions:** `db.transaction(|tx| { ... })`
  exposes simple read methods (`find_one`, `find_into_vec`,
  `find_with(filter, FindOptions)`, `count`) instead of the chained
  `FindCursor` builder. Use the regular `Collection::find()` outside
  transactions if you need lazy iteration with `sort/limit/skip`.

## Python

The Python bindings track the Rust API closely. Same operators, same file
format, same MQL.

```python
import nosqlite

db = nosqlite.Database("app.nosqlite")
users = db.collection("users")
users.insert_many([{"name": "Alice", "age": 30}, {"name": "Bob", "age": 22}])

users.create_index({"email": 1}, unique=True)
users.create_text_index(["title", "body"])
users.find({"$text": {"$search": "fox"}, "draft": False})

with db.transaction() as tx:
    tx.collection("accts").update_one({"_id": "alice"}, {"$inc": {"bal": -25}})
    tx.collection("accts").update_one({"_id": "bob"},   {"$inc": {"bal":  25}})
```

A few cosmetic differences from PyMongo:

- The chained cursor builder is replaced by keyword arguments:
  `users.find(filter, sort=..., limit=..., skip=..., projection=...)`.
- `find()` returns a `list`, not a lazy cursor. For very large result sets,
  fall back to `aggregate(...)` with `$limit` / `$skip` for paging, or use
  the Rust crate.
- Errors raise `RuntimeError` with the Rust error message attached.
- `nosqlite.Database()` with no argument opens an in-memory database.

## Node.js / TypeScript

```js
const { Database } = require('nosqlite')

const db = new Database('app.nosqlite')
const users = db.collection('users')

users.insertMany([{ name: 'Alice', age: 30 }, { name: 'Bob', age: 22 }])
users.createIndex({ email: 1 }, { unique: true })

const tx = db.beginTransaction()
try {
  tx.collection('accts').updateOne({ _id: 'alice' }, { $inc: { bal: -25 } })
  tx.collection('accts').updateOne({ _id: 'bob' },   { $inc: { bal:  25 } })
  tx.commit()
} catch (e) { tx.rollback(); throw e }
```

Differences from the official `mongodb` driver:

- Synchronous calls — no Promises. SQLite is local; there's nothing to
  await. If you need async, wrap calls in your own queue.
- `find(...)` returns a plain `Array`, not a cursor.
- Transactions use a `try/finally` pattern with `beginTransaction()` →
  `commit()` / `rollback()`, instead of MongoDB sessions.
- TypeScript types are bundled (`index.d.ts`).

## When to reach for MongoDB anyway

NoSQLite covers the embedded / single-node use cases very well, but if you
need any of these, MongoDB (or another distributed document database) is
still the right answer:

- Horizontal sharding across many machines
- Replication with automatic failover
- Multi-document distributed transactions
- Change streams powering external reactive systems
- Geo-replicated reads with read preferences
