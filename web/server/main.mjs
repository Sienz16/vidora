import { once } from 'node:events'
import { fileURLToPath } from 'node:url'
import { createWeb, readConfig } from './http.mjs'

try {
  const config = readConfig(process.env)
  const server = await createWeb({ config, dist: fileURLToPath(new URL('../dist/', import.meta.url)) })
  server.listen(config.port, config.bind)
  await once(server, 'listening')
  const stop = () => {
    server.close()
    server.closeAllConnections()
  }
  process.once('SIGTERM', stop)
  process.once('SIGINT', stop)
} catch {
  process.stderr.write('web_startup_failed\n')
  process.exitCode = 1
}
