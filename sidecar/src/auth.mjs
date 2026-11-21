import { chmodSync, lstatSync, mkdirSync } from 'node:fs'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { BufferJSON, initAuthCreds, proto } from '@whiskeysockets/baileys'

export function openAuth(directory) {
  mkdirSync(directory, { recursive: true, mode: 0o700 })
  if (!lstatSync(directory).isDirectory()) throw new Error('invalid session directory')
  chmodSync(directory, 0o700)
  const lock = new DatabaseSync(join(directory, 'instance.db'))
  let db
  try {
    lock.exec('BEGIN EXCLUSIVE')
    db = new DatabaseSync(join(directory, 'session.db'))
    db.exec(`
      PRAGMA journal_mode = WAL;
      PRAGMA synchronous = FULL;
      CREATE TABLE IF NOT EXISTS entries (
        category TEXT, id TEXT, value TEXT NOT NULL, PRIMARY KEY (category, id)
      );
    `)
    const read = db.prepare('SELECT value FROM entries WHERE category = ? AND id = ?')
    const write = db.prepare('INSERT OR REPLACE INTO entries VALUES (?, ?, ?)')
    const remove = db.prepare('DELETE FROM entries WHERE category = ? AND id = ?')
    const get = (category, id) => {
      const row = read.get(category, id)
      return row ? JSON.parse(row.value, BufferJSON.reviver) : null
    }
    const put = (category, id, value) => {
      if (value == null) remove.run(category, id)
      else write.run(category, id, JSON.stringify(value, BufferJSON.replacer))
    }
    const creds = get('creds', '') ?? initAuthCreds()
    delete creds.pairingCode
    const save = (value) => {
      const { pairingCode, ...persistent } = value
      put('creds', '', persistent)
    }
    return {
      state: {
        creds,
        keys: {
          async get(category, ids) {
            return Object.fromEntries(ids.map((id) => {
              const value = get(category, id)
              return [id, category === 'app-state-sync-key' && value
                ? proto.Message.AppStateSyncKeyData.fromObject(value) : value]
            }))
          },
          async set(entries) {
            db.exec('BEGIN')
            try {
              for (const [category, values] of Object.entries(entries)) {
                for (const [id, value] of Object.entries(values)) put(category, id, value)
              }
              db.exec('COMMIT')
            } catch (error) {
              db.exec('ROLLBACK')
              throw error
            }
          },
        },
      },
      save: () => save(creds),
      beginPairing() {
        if (creds.registered) throw new Error('session is registered')
        const pending = initAuthCreds()
        const keys = new Map()
        return {
          state: {
            creds: pending,
            keys: {
              async get(category, ids) {
                return Object.fromEntries(ids.map((id) => [id, keys.get(category)?.get(id)]))
              },
              async set(entries) {
                for (const [category, entriesById] of Object.entries(entries)) {
                  if (!keys.has(category)) keys.set(category, new Map())
                  const values = keys.get(category)
                  for (const [id, value] of Object.entries(entriesById)) {
                    if (value == null) values.delete(id)
                    else values.set(id, value)
                  }
                }
              },
            },
          },
          save() {},
          commit() {
            if (creds.registered || !pending.registered) throw new Error('invalid pairing state')
            db.exec('BEGIN')
            try {
              db.exec('DELETE FROM entries')
              save(pending)
              for (const [category, values] of keys) {
                for (const [id, value] of values) put(category, id, value)
              }
              db.exec('COMMIT')
            } catch (error) {
              db.exec('ROLLBACK')
              throw error
            }
            delete pending.pairingCode
            for (const key of Object.keys(creds)) delete creds[key]
            Object.assign(creds, pending)
          },
          discard() {
            keys.clear()
            delete pending.pairingCode
            if (!creds.registered) db.exec('DELETE FROM entries')
          },
        }
      },
      close() {
        try {
          db.close()
        } finally {
          lock.close()
        }
      },
    }
  } catch (error) {
    db?.close()
    lock.close()
    throw error
  }
}
