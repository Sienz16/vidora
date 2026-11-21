#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

has_node24() {
  "$1" -e 'process.exit(process.versions.node.startsWith("24.") ? 0 : 1)' >/dev/null 2>&1
}

if ! has_node24 node; then
  for directory in /opt/homebrew/opt/node@24/bin /usr/local/opt/node@24/bin; do
    if has_node24 "$directory/node"; then
      export PATH="$directory:$PATH"
      break
    fi
  done
fi
if ! has_node24 node; then
  echo 'Vidora requires Node.js 24 on PATH.' >&2
  exit 1
fi
for command in npm cargo ffmpeg ffprobe; do
  if ! command -v "$command" >/dev/null; then
    echo "Vidora requires $command on PATH." >&2
    exit 1
  fi
done

exec node scripts/start-local.mjs
