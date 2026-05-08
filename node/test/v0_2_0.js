'use strict'

const assert = require('assert')
const { Database } = require('..')

const tests = []
const test = (name, fn) => tests.push({ name, fn })

test('upsert inserts when no match', () => {
  const db = new Database()
  const c = db.collection('u')
  const r = c.updateOneWithOptions(
    { name: 'Alice' },
    { $set: { age: 30 } },
    { upsert: true },
  )
  assert.strictEqual(r.matchedCount, 0)
  assert.strictEqual(r.modifiedCount, 0)
  assert.ok(r.upsertedId)
  const d = c.findOne({ _id: r.upsertedId })
  assert.strictEqual(d.name, 'Alice')
  assert.strictEqual(d.age, 30)
})

test('upsert existing returns no upserted id', () => {
  const db = new Database()
  const c = db.collection('u')
  c.insertOne({ _id: 'x', n: 1 })
  const r = c.updateOneWithOptions(
    { _id: 'x' },
    { $inc: { n: 1 } },
    { upsert: true },
  )
  assert.strictEqual(r.matchedCount, 1)
  assert.strictEqual(r.modifiedCount, 1)
  assert.ok(r.upsertedId == null, `expected null/undefined, got ${r.upsertedId}`)
  assert.strictEqual(c.findOne({ _id: 'x' }).n, 2)
})

test('findOneAndUpdate default returns before', () => {
  const db = new Database()
  const c = db.collection('u')
  c.insertOne({ _id: 'x', n: 1 })
  const before = c.findOneAndUpdate({ _id: 'x' }, { $inc: { n: 1 } })
  assert.strictEqual(before.n, 1)
  assert.strictEqual(c.findOne({ _id: 'x' }).n, 2)
})

test('findOneAndUpdate after returns new', () => {
  const db = new Database()
  const c = db.collection('u')
  c.insertOne({ _id: 'x', n: 1 })
  const after = c.findOneAndUpdate(
    { _id: 'x' },
    { $inc: { n: 1 } },
    { returnDocument: 'after' },
  )
  assert.strictEqual(after.n, 2)
})

test('findOneAndUpdate upsert + after returns inserted', () => {
  const db = new Database()
  const c = db.collection('u')
  const r = c.findOneAndUpdate(
    { name: 'Z' },
    { $set: { n: 5 } },
    { upsert: true, returnDocument: 'after' },
  )
  assert.strictEqual(r.name, 'Z')
  assert.strictEqual(r.n, 5)
})

test('findOneAndReplace swaps doc', () => {
  const db = new Database()
  const c = db.collection('u')
  c.insertOne({ _id: 'k', old: true })
  c.findOneAndReplace({ _id: 'k' }, { fresh: true })
  const d = c.findOne({ _id: 'k' })
  assert.strictEqual(d.old, undefined)
  assert.strictEqual(d.fresh, true)
  assert.strictEqual(d._id, 'k')
})

test('findOneAndDelete removes and returns', () => {
  const db = new Database()
  const c = db.collection('u')
  c.insertOne({ _id: 'gone', v: 7 })
  const r = c.findOneAndDelete({ _id: 'gone' })
  assert.strictEqual(r.v, 7)
  assert.strictEqual(c.count(), 0)
})

test('distinct returns unique values', () => {
  const db = new Database()
  const c = db.collection('p')
  for (const color of ['red', 'blue', 'red', 'green', 'blue']) {
    c.insertOne({ color })
  }
  const got = c.distinct('color').sort()
  assert.deepStrictEqual(got, ['blue', 'green', 'red'])
})

test('distinct unrolls arrays', () => {
  const db = new Database()
  const c = db.collection('p')
  c.insertOne({ tags: ['a', 'b'] })
  c.insertOne({ tags: ['b', 'c'] })
  const got = c.distinct('tags').sort()
  assert.deepStrictEqual(got, ['a', 'b', 'c'])
})

test('bulkWrite mixed ops', () => {
  const db = new Database()
  const c = db.collection('b')
  c.insertOne({ _id: 'a', n: 1 })
  const r = c.bulkWrite([
    { insertOne: { document: { _id: 'b', n: 2 } } },
    { updateOne: { filter: { _id: 'a' }, update: { $inc: { n: 10 } } } },
    {
      updateOne: {
        filter: { _id: 'c' },
        update: { $set: { n: 99 } },
        upsert: true,
      },
    },
    { deleteOne: { filter: { _id: 'b' } } },
  ])
  assert.strictEqual(r.insertedCount, 1)
  assert.strictEqual(r.matchedCount, 1)
  assert.strictEqual(r.modifiedCount, 1)
  assert.strictEqual(r.deletedCount, 1)
  assert.strictEqual(r.upsertedIds.length, 1)
  assert.strictEqual(r.upsertedIds[0].index, 2)
  assert.strictEqual(r.upsertedIds[0].id, 'c')
  assert.strictEqual(c.findOne({ _id: 'a' }).n, 11)
  assert.strictEqual(c.findOne({ _id: 'c' }).n, 99)
})

test('bulkWrite unordered continues past failure', () => {
  const db = new Database()
  const c = db.collection('b')
  c.insertOne({ _id: 'x', n: 1 })
  const r = c.bulkWrite(
    [
      { insertOne: { document: { _id: 'x', dup: true } } }, // fails
      { insertOne: { document: { _id: 'y', n: 2 } } }, // succeeds
    ],
    { ordered: false },
  )
  assert.strictEqual(r.insertedCount, 1)
  assert.ok(c.findOne({ _id: 'y' }))
})

test('$expr filter', () => {
  const db = new Database()
  const c = db.collection('e')
  c.insertMany([
    { a: 5, b: 3 },
    { a: 2, b: 4 },
    { a: 7, b: 7 },
  ])
  assert.strictEqual(c.count({ $expr: { $gt: ['$a', '$b'] } }), 1)
  assert.strictEqual(c.count({ $expr: { $eq: ['$a', '$b'] } }), 1)
})

test('v0.2.0 methods inside transaction', () => {
  const db = new Database()
  db.collection('b').insertOne({ _id: 'a', n: 1 })
  const tx = db.beginTransaction()
  try {
    const c = tx.collection('b')
    const r = c.bulkWrite([
      { insertOne: { document: { _id: 'b', n: 2 } } },
      { updateOne: { filter: { _id: 'a' }, update: { $inc: { n: 10 } } } },
    ])
    assert.strictEqual(r.insertedCount, 1)
    tx.commit()
  } catch (e) {
    tx.rollback()
    throw e
  }
  const c = db.collection('b')
  assert.strictEqual(c.findOne({ _id: 'a' }).n, 11)
  assert.strictEqual(c.findOne({ _id: 'b' }).n, 2)
})

;(async () => {
  let failures = 0
  for (const { name, fn } of tests) {
    try {
      await fn()
      console.log(`  ok   ${name}`)
    } catch (e) {
      failures++
      console.log(`  FAIL ${name}: ${e.message}`)
      if (process.env.VERBOSE) console.error(e.stack)
    }
  }
  process.exit(failures ? 1 : 0)
})()
