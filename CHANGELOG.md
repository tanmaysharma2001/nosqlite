# Changelog

All notable changes are recorded here. The project follows
[Semantic Versioning](https://semver.org/) once the API stabilises at 1.0.

## [Unreleased]

## 0.2.0 — CRUD parity

Closes the most common "wait, this is missing?" gaps for callers migrating
from MongoDB. Tier 1 of the post-0.1 roadmap.

### Added
- **Upsert.** `update_one_with_options`, `update_many_with_options`, and
  `replace_one_with_options` accept `UpdateOptions { upsert: bool }` and
  return an `UpdateResult { matched_count, modified_count, upserted_id }`.
  When no document matches and `upsert: true`, a new document is built
  from the filter's top-level equality clauses (operator clauses are
  skipped) plus the update operators, then inserted.
- **`find_one_and_update` / `find_one_and_replace` / `find_one_and_delete`.**
  Atomic find-and-mutate that returns the document. Accepts
  `FindOneAndUpdateOptions { upsert, return_document, sort, projection }`
  and `FindOneAndDeleteOptions { sort, projection }`.
  `ReturnDocument::Before` (default) returns the pre-mutation doc;
  `ReturnDocument::After` returns the post-mutation doc.
- **`distinct(field, filter)`.** Returns the unique values of `field`
  across documents matching `filter`. Array-valued fields contribute each
  element separately, matching MongoDB's semantics.
- **`bulk_write(ops)`.** Executes a sequence of `WriteOp::{InsertOne,
  UpdateOne, UpdateMany, ReplaceOne, DeleteOne, DeleteMany}` in a single
  SQLite transaction. Returns a `BulkWriteResult` with per-op counters and
  the list of upserted ids. `BulkWriteOptions { ordered }` toggles ordered
  (default — abort on first error) vs. unordered (continue past op
  failures, e.g. unique-key conflicts).
- **`$expr` filter operator.** Filters can now reference computed
  expressions, e.g. `{ "$expr": { "$gt": ["$a", "$b"] } }`. Top-level
  `$expr` is post-filtered in Rust against rows returned by the SQL pass,
  so it composes with other clauses (`{kind: "A", $expr: ...}`).
- **More aggregation expression operators.** Added comparison (`$eq`,
  `$ne`, `$gt`, `$gte`, `$lt`, `$lte`), boolean (`$and`, `$or`, `$not`),
  and conditional (`$cond`) — usable both inside `$expr` filters and
  inside `$addFields` / `$group` accumulators.

All of the above are mirrored on `TxCollection` so they participate in
explicit `db.transaction(|tx| { ... })` blocks.

## 0.1.1

### Added
- **Typed Rust API.** `db.typed_collection::<User>("users")` returns a
  `TypedCollection<User>` whose `insert_one`/`find_one`/`find().into_vec()` /
  `replace_one` round-trip your `Serialize + DeserializeOwned` types through
  `serde_json` automatically. Filters, updates, and projections remain JSON
  values for full MQL access; drop down to the underlying untyped
  `Collection` via `.untyped()` for indexes / aggregation / FTS.
- New `Error::ValidationFailed` variant. Schema validation errors now have
  their own variant (previously misreported as `InvalidUpdate`).

### Fixed
- **`ValidationLevel::Warn` actually warns.** Previously it still rejected
  the write with a "schema validation warning:" prefix, which defeated the
  purpose of the level. Warn-mode writes now pass through; callers can use
  `Validator::validate_collect(&doc)` to retrieve the failure list when
  they want to log violations.

## 0.1.0 — initial release

The initial public release covers all five phases of the original roadmap
except for ORM adapters and a hosted docs site.

### Phase 1 — Foundation
- One SQLite table per collection, ULID-based `_id` generation, atomic
  batched inserts, in-memory or file-backed databases.

### Phase 2 — Query engine
- Full MQL filter compiler: `$eq` `$ne` `$gt` `$gte` `$lt` `$lte` `$in`
  `$nin` `$exists` `$type` `$size` `$not`; logical combinators `$and` `$or`
  `$nor` `$not` at any nesting depth; dotted-field access (`addr.city`).
- Update operators: `$set` `$unset` `$inc` `$mul` `$min` `$max` `$rename`
  `$push` (with `$each`) `$pull` `$pop` `$addToSet`; replacement-style
  updates.
- Cursor builder: `sort` / `limit` / `skip` / `project`; terminal
  `into_vec` / `first` / `count` / `explain`.

### Phase 3 — Indexes & performance
- Single-field and compound expression indexes; the MQL compiler inlines
  JSON paths as SQL literals so SQLite's planner picks them up.
- `cargo run --release --example bench` covers insert/find/update/delete
  with and without indexes. Indexed equality on 100k documents: 28 µs.

### Phase 4 — Aggregation & integrations
- Aggregation pipeline: `$match` `$project` `$addFields`/`$set` `$sort`
  `$limit` `$skip` `$count` `$group` `$unwind` `$lookup`. Group
  accumulators: `$sum` `$avg` `$min` `$max` `$first` `$last` `$push`
  `$addToSet` `$count`. Expression operators: field references, `$add`,
  `$subtract`, `$multiply`, `$divide`, `$concat`, `$toUpper`/`$toLower`,
  `$ifNull`, `$literal`.
- JSON-Schema validation persisted in a meta table.
- JSON / JSONL import + export with batched streaming.
- `db.transaction(|tx| { ... })` closure API for atomic multi-collection
  writes; manual `begin/commit/rollback` for non-Rust callers.
- FTS5-backed `$text` search auto-synced from the ops layer (no triggers).
- BSON / `mongodump` import behind the `bson` feature flag.

### Phase 5 — Multi-language reach
- `nosqlite <file>` interactive CLI shell (REPL with `.find`, `.aggregate`,
  `.text-index`, `.import`, `.explain`, `.indexes`, `.collections`, …).
- Python SDK via PyO3 + maturin at [python/](python/), exposing the full
  Rust API including transactions as a context manager.
- Node.js SDK via napi-rs at [node/](node/), with auto-generated
  TypeScript declarations.
- [MIGRATING.md](MIGRATING.md) — MongoDB → NoSQLite cheatsheet.

The `.nosqlite` file format is identical across the Rust crate, the CLI
shell, the Python module, and the Node module.

### Not yet
- Mongoose / SQLAlchemy ORM adapters.
- Hosted docs site.
