'use strict'

const assert = require('assert')
const { Database } = require('..')
const { Schema, model } = require('../odm')

const tests = []
const test = (name, fn) => tests.push({ name, fn })

test('schema + model basic CRUD', () => {
  const db = new Database()
  const userSchema = new Schema({
    name: { type: String, required: true, minLength: 1 },
    age: { type: Number, min: 0 },
  })
  const User = model('User', userSchema, db)

  const alice = User.create({ name: 'Alice', age: 30 })
  assert.ok(alice._id, 'should have generated _id')
  assert.strictEqual(alice.constructor.modelName, 'User')

  const found = User.findOne({ name: 'Alice' })
  assert.strictEqual(found.age, 30)

  assert.strictEqual(User.countDocuments(), 1)
})

test('validator rejects bad inputs', () => {
  const db = new Database()
  const Item = model(
    'Item',
    new Schema({
      qty: { type: Number, required: true, min: 0 },
    }),
    db,
  )

  let threw = false
  try {
    Item.create({ qty: -1 })
  } catch (e) {
    threw = true
  }
  assert.ok(threw, 'expected validator to reject qty=-1')
})

test('unique index is enforced', () => {
  const db = new Database()
  const Email = model(
    'Email',
    new Schema({
      addr: { type: String, required: true, unique: true },
    }),
    db,
  )

  Email.create({ addr: 'a@b.com' })
  let threw = false
  try {
    Email.create({ addr: 'a@b.com' })
  } catch (_) {
    threw = true
  }
  assert.ok(threw, 'expected duplicate insert to fail unique constraint')
})

test('save() inserts new and updates existing', () => {
  const db = new Database()
  const Note = model('Note', new Schema({ title: String, n: Number }), db)

  const n = new Note({ title: 'first', n: 1 })
  n.save()
  assert.ok(n._id)

  n.set('n', 99)
  n.save()

  const fresh = Note.findById(n._id)
  assert.strictEqual(fresh.n, 99)
})

test('static find/sort/limit/projection', () => {
  const db = new Database()
  const Item = model('Item', new Schema({ score: Number }), db)
  Item.create([{ score: 3 }, { score: 1 }, { score: 4 }, { score: 1 }, { score: 5 }])

  const top = Item.find({}, { sort: { score: -1 }, limit: 3 })
  assert.deepStrictEqual(
    top.map((t) => t.score),
    [5, 4, 3],
  )
})

test('hooks run on save', () => {
  const db = new Database()
  const schema = new Schema({ name: String, slug: String })
  let preCount = 0
  let postCount = 0
  schema.pre('save', function () {
    preCount++
    this.slug = (this.name || '').toLowerCase().replace(/\s+/g, '-')
  })
  schema.post('save', function () {
    postCount++
  })
  const Page = model('Page', schema, db)

  const p = new Page({ name: 'Hello World' })
  p.save()
  assert.strictEqual(p.slug, 'hello-world')
  assert.strictEqual(preCount, 1)
  assert.strictEqual(postCount, 1)
})

test('instance.remove() deletes it', () => {
  const db = new Database()
  const X = model('X', new Schema({ v: Number }), db)
  const a = X.create({ v: 1 })
  X.create({ v: 2 })
  a.remove()
  assert.strictEqual(X.countDocuments(), 1)
})

test('aggregate is forwarded', () => {
  const db = new Database()
  const Sale = model('Sale', new Schema({ category: String, price: Number }), db)
  Sale.create([
    { category: 'A', price: 10 },
    { category: 'A', price: 20 },
    { category: 'B', price: 30 },
  ])
  const out = Sale.aggregate([
    { $group: { _id: '$category', total: { $sum: '$price' } } },
    { $sort: { _id: 1 } },
  ])
  assert.deepStrictEqual(out, [
    { _id: 'A', total: 30 },
    { _id: 'B', total: 30 },
  ])
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
