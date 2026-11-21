import { spawn, spawnSync } from 'node:child_process'
import { createHash, randomBytes } from 'node:crypto'
import { once } from 'node:events'
import { existsSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('../', import.meta.url))
const data = resolve(root, process.env.VIDORA_LOCAL_DATA_DIR || 'data/local')
const credentials = join(data, 'credentials.json')
const host = '127.0.0.1'
const origin = `http://${host}:3000`
const api = `http://${host}:8080`
const control = `http://${host}:8090`
const services = []
const stopping = new AbortController()

function command(program, args) {
  const result = spawnSync(program, args, { cwd: root, stdio: 'inherit' })
  if (result.error || result.status !== 0) throw new Error(`${program} failed. See build output above.`)
}

function install(directory) {
  const base = join(root, directory)
  const receipt = join(base, 'node_modules', '.vidora-install')
  const signature = createHash('sha256').update(process.version).update(readFileSync(join(base, 'package-lock.json'))).digest('hex')
  if (existsSync(receipt) && readFileSync(receipt, 'utf8') === signature) return
  command('npm', ['ci', '--prefix', directory, '--ignore-scripts', '--prefer-offline'])
  writeFileSync(receipt, signature)
}

function secrets() {
  mkdirSync(data, { recursive: true, mode: 0o700 })
  const directory = lstatSync(data)
  if (!directory.isDirectory() || (directory.mode & 0o077)) {
    throw new Error('Local data directory must be private (mode 0700).')
  }
  if (!existsSync(credentials)) {
    const random = () => randomBytes(32).toString('hex')
    writeFileSync(credentials, JSON.stringify({
      WEB_ADMIN_TOKEN: random(), INTERNAL_API_TOKEN: random(), SIDECAR_CONTROL_TOKEN: random(),
    }, null, 2) + '\n', { mode: 0o600, flag: 'wx', flush: true })
  }
  const file = lstatSync(credentials)
  if (!file.isFile() || (file.mode & 0o077)) throw new Error('Local credentials file must be private and contain valid tokens.')
  const { WEB_ADMIN_TOKEN, INTERNAL_API_TOKEN, SIDECAR_CONTROL_TOKEN } = JSON.parse(readFileSync(credentials, 'utf8'))
  const values = [WEB_ADMIN_TOKEN, INTERNAL_API_TOKEN, SIDECAR_CONTROL_TOKEN]
  if (values.some((token) => typeof token !== 'string' || !/^[a-f0-9]{64}$/.test(token))
    || new Set(values).size !== values.length) throw new Error('Local credentials file must be private and contain valid tokens.')
  return { WEB_ADMIN_TOKEN, INTERNAL_API_TOKEN, SIDECAR_CONTROL_TOKEN }
}

function available(port) {
  return new Promise((resolve, reject) => {
    const listener = createServer()
    listener.once('error', () => reject(new Error(`Port ${port} is unavailable on loopback.`)))
    listener.listen(port, host, () => listener.close(resolve))
  })
}

function start(name, program, args, settings) {
  const child = spawn(program, args, {
    cwd: root, stdio: ['ignore', 'inherit', 'inherit'],
    env: { PATH: process.env.PATH, ...settings },
  })
  const done = new Promise((resolve) => {
    child.once('error', () => resolve(`${name} could not start.`))
    child.once('exit', (code, signal) => resolve(`${name} exited (${code ?? signal}).`))
  })
  services.push({ child, done })
}

async function ready(url, headers = {}) {
  const deadline = AbortSignal.any([stopping.signal, AbortSignal.timeout(10_000)])
  while (!deadline.aborted) {
    try {
      const response = await fetch(url, {
        headers, signal: AbortSignal.any([deadline, AbortSignal.timeout(750)]), redirect: 'error',
      })
      await response.body?.cancel()
      if (response.ok) return
    } catch {}
    await delay(100, undefined, { signal: deadline }).catch(() => {})
  }
  throw new Error('Local services did not become ready.')
}

async function stop() {
  for (const { child } of services) child.kill('SIGTERM')
  let timeout
  try {
    await Promise.race([
      Promise.all(services.map(({ done }) => done)),
      new Promise((resolve) => {
        timeout = setTimeout(() => {
          for (const { child } of services) child.kill('SIGKILL')
          resolve()
        }, 25_000)
      }),
    ])
  } finally {
    clearTimeout(timeout)
  }
}

function openDashboard() {
  if (process.env.CI) return
  const opener = process.platform === 'darwin' ? 'open'
    : process.env.DISPLAY || process.env.WAYLAND_DISPLAY ? 'xdg-open' : null
  if (opener) spawn(opener, [origin], { stdio: 'ignore' }).on('error', () => {}).unref()
}

async function run() {
  if (process.getuid?.() === 0) throw new Error('Run Vidora as a regular user.')
  process.umask(0o077)
  await Promise.all([3000, 8080, 8090].map(available))
  const profile = process.env.VIDORA_VIDEO_PROFILE ?? 'hd29'
  if (!['hd29', 'status720'].includes(profile)) throw new Error('Invalid video profile.')
  const { INTERNAL_API_TOKEN, WEB_ADMIN_TOKEN, SIDECAR_CONTROL_TOKEN } = secrets()
  install('sidecar')
  install('web')
  command('npm', ['run', 'build', '--prefix', 'web'])
  const target = join(root, 'target')
  command('cargo', ['build', '--locked', '--bin', 'api', '--target-dir', target])

  start('Rust API', join(target, 'debug', 'api'), [], {
    DATA_DIR: join(data, 'api'), VIDORA_BIND_IP: host, INTERNAL_API_TOKEN, VIDORA_VIDEO_PROFILE: profile,
  })
  start('WhatsApp sidecar', process.execPath, ['sidecar/src/main.mjs'], {
    BAILEYS_AUTH_DIR: join(data, 'session'), RUST_API_URL: api, INTERNAL_API_TOKEN,
    SIDECAR_CONTROL_BIND: host, SIDECAR_CONTROL_TOKEN,
  })
  start('Admin UI', process.execPath, ['web/server/main.mjs'], {
    PORT: '3000', WEB_BIND_IP: host, WEB_PUBLIC_ORIGIN: origin, WEB_ADMIN_TOKEN,
    RUST_API_URL: api, INTERNAL_API_TOKEN, SIDECAR_CONTROL_URL: control, SIDECAR_CONTROL_TOKEN,
  })
  const exited = Promise.race(services.map(({ done }) => done)).then((error) => { throw new Error(error) })
  await Promise.race([exited, Promise.all([
    ready(api + '/health'),
    ready(control + '/control/status', { Authorization: 'Bearer ' + SIDECAR_CONTROL_TOKEN }),
    ready(origin + '/healthz'),
  ])])
  console.log(`Vidora is ready at ${origin}`)
  console.log(`Unlock with WEB_ADMIN_TOKEN from ${credentials}`)
  console.log('Press Ctrl+C to stop all three services.')
  openDashboard()
  if (!stopping.signal.aborted) await Promise.race([exited, once(stopping.signal, 'abort')])
}

for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => stopping.abort())
try {
  await run()
} catch (error) {
  if (!stopping.signal.aborted) {
    console.error(error instanceof SyntaxError ? 'Invalid local credentials file.' : error.message)
    process.exitCode = 1
  }
} finally {
  await stop()
}
