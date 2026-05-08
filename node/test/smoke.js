'use strict'

const assert = require('assert')
const fs = require('fs')
const os = require('os')
const path = require('path')
const { Database } = require('..')

const tests = []
const test = (name, fn) => tests.push({ name, fn })

test('insert / find / count in-memory', () => {
  const db = new Database()
  const users = db.collection('users')
  users.insertMany([
    { name: 'Alice', age: 30, tags: ['admin'] },
    { name: 'Bob', age: 22, tags: ['editor'] },
    { name: 'Carol', age: 41, tags: ['editor', 'admin'] },
  ])
  assert.strictEqual(users.count(), 3)
  const adults = users.find(
    { age: { $gte: 25 } },
    { sort: { age: -1 } },
  )
  assert.deepStrictEqual(adults.map((u) => u.name), ['Carol', 'Alice'])
})

test('dotted path + projection', () => {
  const db = new Database()
  const c = db.collection('c')
  c.insertOne({ addr: { city: 'NYC', zip: '10001' }, name: 'A' })
  c.insertOne({ addr: { city: 'SF' }, name: 'B' })
  const nyc = c.find({ 'addr.city': 'NYC' }, { projection: { name: 1, _id: 0 } })
  assert.deepStrictEqual(nyc, [{ name: 'A' }])
})

test('update operators', () => {
  const db = new Database()
  const c = db.collection('c')
  const id = c.insertOne({ n: 1, stale: true })
  c.updateOne({ _id: id }, { $set: { n: 5 }, $unset: { stale: '' } })
  c.updateOne({ _id: id }, { $inc: { n: 3 } })
  c.updateOne({ _id: id }, { $push: { tags: 'ok' } })
  c.updateOne({ _id: id }, { $push: { tags: { $each: ['x', 'y'] } } })
  const d = c.findOne({ _id: id })
  assert.strictEqual(d.n, 8)
  assert.strictEqual(d.stale, undefined)
  assert.deepStrictEqual(d.tags, ['ok', 'x', 'y'])
})

test('index + explain', () => {
  const db = new Database()
  const c = db.collection('c')
  for (let i = 0; i < 200; i++) c.insertOne({ i, tenant: i % 5 })
  c.createIndex({ i: 1 })
  const plan = c.explain({ i: 137 })
  assert.ok(plan.includes('USING INDEX'), plan)
})

test('aggregation', () => {
  const db = new Database()
  const sales = db.collection('sales')
  sales.insertMany([
    { category: 'A', price: 10 },
    { category: 'A', price: 20 },
    { category: 'B', price: 30 },
    { category: 'B', price: 30 },
    { category: 'B', price: 40 },
  ])
  const out = sales.aggregate([
    {
      $group: {
        _id: '$category',
        total: { $sum: '$price' },
        n: { $sum: 1 },
      },
    },
    { $sort: { _id: 1 } },
  ])
  assert.deepStrictEqual(out, [
    { _id: 'A', total: 30, n: 2 },
    { _id: 'B', total: 100, n: 3 },
  ])
})

test('full-text search', () => {
  const db = new Database()
  const posts = db.collection('posts')
  posts.createTextIndex(['title', 'body'])
  posts.insertMany([
    { title: 'the quick brown fox', body: 'jumps over the lazy dog' },
    { title: 'foxes are clever', body: 'they hunt at night' },
    { title: 'hello world', body: 'first post' },
  ])
  const hits = posts.find({ $text: { $search: 'fox' } })
  assert.strictEqual(hits.length, 2)
})

test('transaction commit + rollback', () => {
  const db = new Database()
  const accts = db.collection('accts')
  accts.insertMany([
    { _id: 'alice', bal: 100 },
    { _id: 'bob', bal: 0 },
  ])

  const tx = db.beginTransaction()
  try {
    tx.collection('accts').updateOne({ _id: 'alice' }, { $inc: { bal: -25 } })
    tx.collection('accts').updateOne({ _id: 'bob' }, { $inc: { bal: 25 } })
    tx.commit()
  } catch (e) {
    tx.rollback()
    throw e
  }
  assert.strictEqual(accts.findOne({ _id: 'alice' }).bal, 75)
  assert.strictEqual(accts.findOne({ _id: 'bob' }).bal, 25)

  // Explicit rollback discards changes.
  const tx2 = db.beginTransaction()
  tx2.collection('accts').updateOne({ _id: 'alice' }, { $inc: { bal: -1000 } })
  tx2.rollback()
  assert.strictEqual(accts.findOne({ _id: 'alice' }).bal, 75)
})

test('schema validator', () => {
  const db = new Database()
  db.setValidator('users', {
    type: 'object',
    required: ['name', 'age'],
    properties: {
      name: { type: 'string', minLength: 1 },
      age: { type: 'integer', minimum: 0 },
    },
  })
  const users = db.collection('users')
  users.insertOne({ name: 'Alice', age: 30 })
  let threw = false
  try {
    users.insertOne({ name: '' })
  } catch (e) {
    threw = true
  }
  assert.ok(threw, 'expected validator to reject')
  assert.strictEqual(users.count(), 1)
})

test('persistence across reopen', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'nosqlite-'))
  const file = path.join(dir, 'test.nosqlite')
  let db = new Database(file)
  db.collection('c').insertOne({ v: 42 })
  db = null
  // Forcing GC isn't reliable without flags, but Database closes its handle
  // on drop via SQLite's auto-checkpoint. Reopening should still see the data.
  const db2 = new Database(file)
  const d = db2.collection('c').findOne()
  assert.strictEqual(d.v, 42)
  fs.rmSync(dir, { recursive: true })
})

test('import / export round-trip', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'nosqlite-'))
  const file = path.join(dir, 'dump.jsonl')
  const dba = new Database()
  const ca = dba.collection('c')
  ca.insertMany([{ n: 1 }, { n: 2 }, { n: 3 }])
  ca.exportFile(file, 'jsonl')

  const dbb = new Database()
  const n = dbb.collection('c').importFile(file, 'jsonl')
  assert.strictEqual(n, 3)
  assert.strictEqual(dbb.collection('c').count(), 3)
  fs.rmSync(dir, { recursive: true })
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
