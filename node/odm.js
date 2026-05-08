'use strict'

// Mongoose-style ODM for nosqlite. Wraps a Database with Schema/Model
// helpers — schemas translate to JSON-Schema validators, indexes are
// created automatically, and Model instances expose Mongoose-ish save() /
// remove() / set() methods.

const { Database } = require('./')

// Map a Mongoose-ish "type" to a JSON Schema "type" string.
function jsonSchemaType(t) {
  if (t === String) return 'string'
  if (t === Number) return 'number'
  if (t === Boolean) return 'boolean'
  if (t === Date) return 'string' // dates serialize as ISO strings
  if (Array.isArray(t)) return 'array'
  if (typeof t === 'function') return 'object'
  return undefined
}

// Take a Mongoose-style schema definition and produce a JSON-Schema object
// plus a list of indexes to create.
function compileSchema(def) {
  const properties = {}
  const required = []
  const indexes = []
  let hasRequired = false

  for (const [name, raw] of Object.entries(def)) {
    const spec = typeof raw === 'function' || Array.isArray(raw) ? { type: raw } : raw
    const prop = {}

    if ('type' in spec) {
      if (Array.isArray(spec.type)) {
        prop.type = 'array'
        const inner = jsonSchemaType(spec.type[0])
        if (inner) prop.items = { type: inner }
      } else {
        const t = jsonSchemaType(spec.type)
        if (t) prop.type = t
      }
    }
    if (spec.minLength !== undefined) prop.minLength = spec.minLength
    if (spec.maxLength !== undefined) prop.maxLength = spec.maxLength
    if (spec.min !== undefined) prop.minimum = spec.min
    if (spec.max !== undefined) prop.maximum = spec.max
    if (spec.enum !== undefined) prop.enum = spec.enum

    properties[name] = prop

    if (spec.required) {
      required.push(name)
      hasRequired = true
    }

    if (spec.unique) {
      indexes.push({ keys: { [name]: 1 }, unique: true })
    } else if (spec.index) {
      indexes.push({ keys: { [name]: 1 } })
    }
  }

  const jsonSchema = {
    type: 'object',
    properties,
  }
  if (hasRequired) jsonSchema.required = required
  return { jsonSchema, indexes }
}

class Schema {
  constructor(definition, options = {}) {
    this.definition = definition
    this.options = options
    const { jsonSchema, indexes } = compileSchema(definition)
    this.jsonSchema = jsonSchema
    this.indexes = indexes
    this.hooks = { preSave: [], postSave: [] }
  }

  pre(event, fn) {
    if (event === 'save') this.hooks.preSave.push(fn)
    return this
  }

  post(event, fn) {
    if (event === 'save') this.hooks.postSave.push(fn)
    return this
  }
}

function model(name, schema, db, options = {}) {
  if (!(db instanceof Database)) {
    throw new TypeError('model() requires a nosqlite.Database instance')
  }
  const collectionName =
    options.collection || schema.options.collection || pluralize(name)
  const coll = db.collection(collectionName)

  // Apply schema (validator + indexes) once per process. The validator
  // itself is persisted in the meta table, so this is idempotent.
  try {
    db.setValidator(collectionName, schema.jsonSchema, 'strict')
  } catch (_) {
    /* setValidator can fail before the underlying table exists; ignore. */
  }
  for (const ix of schema.indexes) {
    try {
      coll.createIndex(ix.keys, { unique: !!ix.unique })
    } catch (_) {
      /* createIndex creates the table; safe to retry on next call. */
    }
  }

  class Model {
    constructor(doc) {
      Object.assign(this, doc || {})
    }

    static get collectionName() {
      return collectionName
    }

    static get db() {
      return db
    }

    static get coll() {
      return coll
    }

    // ---- Static query methods (Mongoose API) -----------------------------
    static find(filter = {}, options = {}) {
      const docs = coll.find(filter, options)
      return docs.map((d) => new Model(d))
    }

    static findOne(filter = {}) {
      const d = coll.findOne(filter)
      return d ? new Model(d) : null
    }

    static findById(id) {
      return Model.findOne({ _id: id })
    }

    static countDocuments(filter = {}) {
      return coll.count(filter)
    }

    static create(docOrDocs) {
      if (Array.isArray(docOrDocs)) {
        return docOrDocs.map((d) => Model.createOne(d))
      }
      return Model.createOne(docOrDocs)
    }

    static createOne(doc) {
      const m = new Model(doc)
      m.save()
      return m
    }

    static updateOne(filter, update) {
      return coll.updateOne(filter, update)
    }

    static updateMany(filter, update) {
      return coll.updateMany(filter, update)
    }

    static deleteOne(filter) {
      return coll.deleteOne(filter)
    }

    static deleteMany(filter) {
      return coll.deleteMany(filter)
    }

    static aggregate(pipeline) {
      return coll.aggregate(pipeline)
    }

    static index(keys, options = {}) {
      return coll.createIndex(keys, options)
    }

    static get modelName() {
      return name
    }

    // ---- Instance methods ----------------------------------------------
    save() {
      for (const h of schema.hooks.preSave) h.call(this)
      const plain = { ...this }
      if (plain._id == null) {
        delete plain._id
        const id = coll.insertOne(plain)
        this._id = id
      } else {
        coll.replaceOne({ _id: plain._id }, plain)
      }
      for (const h of schema.hooks.postSave) h.call(this)
      return this
    }

    remove() {
      if (this._id == null) return 0
      return coll.deleteOne({ _id: this._id })
    }

    set(field, value) {
      this[field] = value
      return this
    }

    toObject() {
      return { ...this }
    }
  }

  Object.defineProperty(Model, 'name', { value: name })
  return Model
}

function pluralize(name) {
  // Tiny lower-case+s pluraliser. Good enough; users can override with
  // `model(..., schema, db, { collection: 'mything' })`.
  const lower = name.toLowerCase()
  if (lower.endsWith('s')) return lower
  if (lower.endsWith('y')) return lower.slice(0, -1) + 'ies'
  return lower + 's'
}

module.exports = { Schema, model, Database }
