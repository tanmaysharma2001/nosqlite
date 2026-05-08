# nosqlite — Node.js bindings

MongoDB-style document database for Node.js, backed by a single SQLite file
via the [nosqlite Rust crate](../README.md).

```js
const { Database } = require('nosqlite')

const db = new Database('app.nosqlite')   // omit arg for in-memory
const users = db.collection('users')

users.insertMany([
  { name: 'Alice', age: 30 },
  { name: 'Bob', age: 22 },
])

users.createIndex({ age: 1 })

for (const u of users.find({ age: { $gt: 25 } }, { sort: { age: -1 }, limit: 10 })) {
  console.log(u)
}

const tx = db.beginTransaction()
try {
  tx.collection('a').insertOne({ v: 1 })
  tx.collection('b').insertOne({ v: 2 })
  tx.commit()
} catch (e) {
  tx.rollback()
  throw e
}
```

## Mongoose-style ODM

For users coming from Mongoose, an opinionated ODM ships at
`require('nosqlite/odm')`:

```js
const { Database } = require('nosqlite')
const { Schema, model } = require('nosqlite/odm')

const db = new Database()

const User = model('User', new Schema({
  name:  { type: String, required: true, minLength: 1 },
  email: { type: String, unique: true },
  age:   { type: Number, min: 0 },
}), db)

const alice = User.create({ name: 'Alice', email: 'a@b.com', age: 30 })
alice.set('age', 31)
alice.save()

const adults = User.find({ age: { $gt: 25 } }, { sort: { age: -1 } })
```

The Schema's `required` / `min` / `max` / `minLength` / `enum` rules
translate to a JSON-Schema validator stored alongside the collection.
`unique: true` and `index: true` create the corresponding SQLite indexes.
`schema.pre('save', fn)` and `schema.post('save', fn)` hooks run around
every `save()`.

## Building from source

```sh
cd node
npm install
npx napi build --release --strip
npm test
```

This produces `index.node` (native module) and `index.d.ts` (TypeScript
types) in the package directory. Both Rust crate, Python module, and Node
module read and write the same `.nosqlite` file format.
