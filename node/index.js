'use strict'

const { existsSync } = require('fs')
const { join } = require('path')

const localBuild = join(__dirname, 'index.node')
if (!existsSync(localBuild)) {
  throw new Error(
    'nosqlite native binding not found. Build it with `npx napi build --release`.'
  )
}

const native = require('./index.node')

module.exports = {
  Database: native.Database,
  Collection: native.Collection,
  Transaction: native.Transaction,
}
