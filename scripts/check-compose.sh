#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# A dedicated Compose project keeps the smoke test away from deployed volumes.
project="vidora-smoke-$$"
export INTERNAL_API_TOKEN="$(openssl rand -hex 32)"
export OWNER_WHATSAPP_NUMBER=12025550101 BOT_WHATSAPP_NUMBER=12025550102
compose=(docker compose -p "$project")
cleanup() {
  if [[ -n "${sidecar_id:-}" ]]; then docker rm -f "$sidecar_id" >/dev/null || true; fi
  "${compose[@]}" down --volumes --remove-orphans
}
trap cleanup EXIT

check_limits() {
  local id=$1 uid=$2 pids=$3 cpu=$4 memory=$5 group
  test "$(docker inspect -f '{{.Config.User}} {{.HostConfig.ReadonlyRootfs}}' "$id")" = "$uid:$uid true"
  group=$(awk -F: '$1 == 0 {print $3}' "/proc/$(docker inspect -f '{{.State.Pid}}' "$id")/cgroup")
  test -n "$group"
  test "$(cat "/sys/fs/cgroup$group/pids.max")" = "$pids"
  test "$(cat "/sys/fs/cgroup$group/cpu.max")" = "$cpu 100000"
  test "$(cat "/sys/fs/cgroup$group/memory.max")" = "$memory"
}

"${compose[@]}" config --quiet
"${compose[@]}" -f compose.yaml -f compose.pairing.yaml config --quiet
"${compose[@]}" up --build --wait api
api_id=$("${compose[@]}" ps -q api)
check_limits "$api_id" 10001 128 175000 2147483648
test "$(docker inspect -f '{{len .NetworkSettings.Networks}} {{len .HostConfig.PortBindings}}' "$api_id")" = '1 0'
"${compose[@]}" exec -T api ffmpeg -hide_banner -protocols | awk '
  /^  / && $1 != "file" && $1 != "pipe" {fail=1}
  $1 == "file" {file=1}
  END {exit (fail || !file)}'
"${compose[@]}" build whatsapp
sidecar_id=$("${compose[@]}" run --detach --no-deps whatsapp node -e 'setInterval(() => {}, 30000)')
check_limits "$sidecar_id" 65532 64 50000 805306368
test "$(docker inspect -f '{{len .NetworkSettings.Networks}} {{len .HostConfig.PortBindings}}' "$sidecar_id")" = '2 0'
docker run --rm --network "container:$api_id" "$(docker inspect -f '{{.Image}}' "$sidecar_id")" node -e '
  const {readFileSync} = require("node:fs")
  const routes = readFileSync("/proc/net/route", "utf8").trim().split("\n")
  if (routes.some(line => line.trim().split(/\s+/)[1] === "00000000")) process.exitCode = 1
'
docker exec "$sidecar_id" node --input-type=module -e '
  import assert from "node:assert/strict"
  import { readFileSync, accessSync, constants } from "node:fs"
  assert.equal(process.getuid(), 65532)
  accessSync(process.env.BAILEYS_AUTH_DIR, constants.W_OK)
  const routes = readFileSync("/proc/net/route", "utf8").trim().split("\n")
  assert.ok(routes.some(line => line.trim().split(/\s+/)[1] === "00000000"))
  const response = await fetch("https://web.whatsapp.com/", { method: "HEAD", signal: AbortSignal.timeout(8000) })
  assert.ok(response.status >= 200)
'
if unlinked=$(docker exec "$sidecar_id" node src/main.mjs 2>&1); then
  printf 'An unpaired sidecar unexpectedly started\n' >&2
  exit 1
fi
test "$unlinked" = sidecar_pairing_required

"${compose[@]}" exec -T api ffmpeg -v error -f lavfi -i testsrc2=size=320x240:rate=24 \
  -t 1 -c:v libx264 -movflags +frag_keyframe+empty_moov -f mp4 pipe:1 |
docker exec -i "$sidecar_id" node --input-type=module -e '
  import assert from "node:assert/strict"
  import { createHash } from "node:crypto"
  import { Readable } from "node:stream"
  import { buffer } from "node:stream/consumers"
  import { setTimeout as delay } from "node:timers/promises"
  import { createApi } from "./src/api.mjs"
  const base = process.env.RUST_API_URL
  const stop = AbortSignal.timeout(30000)
  assert.equal((await fetch(base + "/internal/v1/jobs", {signal: stop})).status, 401)
  const api = createApi(base, process.env.INTERNAL_API_TOKEN)
  const bytes = await buffer(process.stdin)
  const input = {id: "compose-smoke", name: "generated.mp4", bytes: bytes.length,
    digest: createHash("sha256").update(bytes).digest("hex")}
  const first = await api.upload(Readable.from([bytes]), input, stop)
  assert.equal(first.created, true)
  let job = first.job
  while (job.state === "processing") {
    await delay(150, undefined, {signal: stop})
    job = await api.status(stop, input.id)
  }
  assert.equal(job.state, "ready")
  const clip = await buffer(await api.artifact(job.id, 0, job.sizes[0], stop))
  assert.equal(clip.length, job.sizes[0])
  await api.delivery(job.id, 0, "begin", stop)
  assert.equal((await api.delivery(job.id, 0, "ack", stop)).state, "delivered")
  assert.equal((await api.delivery(job.id, 0, "ack", stop)).state, "delivered")
  const duplicate = await api.upload(Readable.from([bytes]), input, stop)
  assert.equal(duplicate.created, false)
  assert.equal(duplicate.job.id, job.id)
  process.stdout.write(clip)
' | "${compose[@]}" exec -T api ffprobe -v error \
  -show_entries stream=codec_name,width,height,pix_fmt -of csv=p=0 pipe:0 |
grep -qx h264,1080,1920,yuv420p
for path in work media; do
  test "$(docker cp "$api_id:/var/lib/vidora/$path" - | tar -tf -)" = "$path/"
done

printf 'Compose smoke test passed\n'
