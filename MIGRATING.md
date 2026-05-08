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
| `db.users.updateOne(filter, upd, { upsert: true })`    | `users.update_one_with_options(filter, upd, UpdateOptions { upsert: true })?` |
| `db.users.replaceOne(filter, doc)`                     | `users.replace_one(filter, doc)?`                                       |
| `db.users.deleteMany(filter)`                          | `users.delete_many(filter)?`                                            |
| `db.users.findOneAndUpdate(filter, upd)`               | `users.find_one_and_update(filter, upd)?`                               |
| `db.users.findOneAndReplace(filter, doc)`              | `users.find_one_and_replace(filter, doc)?`                              |
| `db.users.findOneAndDelete(filter)`                    | `users.find_one_and_delete(filter)?`                                    |
| `db.users.distinct("color", filter)`                   | `users.distinct("color", filter)?`                                      |
| `db.users.bulkWrite([...])`                            | `users.bulk_write(vec![WriteOp::InsertOne { document }, ...])?`         |
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
`$ifNull`, `$literal`, `$eq`, `$ne`, `$gt`, `$gte`, `$lt`, `$lte`, `$and`,
`$or`, `$not`, `$cond`
**Computed filters:** `$expr` (use any aggregation expression as a filter,
e.g. `{ "$expr": { "$gt": ["$a", "$b"] } }`)
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
- Mongo-style **upsert** is on the `_with_options` variants:
  `users.update_one_with_options(filter, update, upsert=True)` returns
  `{matched_count, modified_count, upserted_id}`. **find_one_and_** *
  methods take kwargs:
  `users.find_one_and_update(filter, update, upsert=False, return_document="before"|"after", sort=..., projection=...)`.
- **bulk_write** takes a list of single-key dicts:
  `users.bulk_write([{"insertOne": {"document": {...}}}, {"updateOne": {"filter": {...}, "update": {...}, "upsert": True}}, ...])`
  and returns
  `{inserted_count, matched_count, modified_count, deleted_count, upserted_ids: [{"index": int, "_id": str}]}`.

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
- Mongo-style **upsert** is on the `*WithOptions` variants:
  `users.updateOneWithOptions(filter, update, { upsert: true })` returns
  `{ matchedCount, modifiedCount, upsertedId }`. **findOneAnd*** methods
  take an options object:
  `users.findOneAndUpdate(filter, update, { upsert, returnDocument, sort, projection })`
  where `returnDocument` is `'before'` (default) or `'after'`.
- **bulkWrite** takes an array of single-key objects:
  `users.bulkWrite([{ insertOne: { document } }, { updateOne: { filter, update, upsert: true } }, { deleteOne: { filter } }])`
  and returns counters plus `upsertedIds: [{ index, id }]`.

## When to reach for MongoDB anyway

NoSQLite covers the embedded / single-node use cases very well, but if you
need any of these, MongoDB (or another distributed document database) is
still the right answer:

- Horizontal sharding across many machines
- Replication with automatic failover
- Multi-document distributed transactions
- Change streams powering external reactive systems
- Geo-replicated reads with read preferences
