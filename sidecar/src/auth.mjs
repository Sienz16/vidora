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
      save: () => put('creds', '', creds),
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
