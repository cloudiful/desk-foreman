#!/usr/bin/env bash
set -euo pipefail

cleanup() {
  if [[ -n "${app_pid:-}" ]]; then
    kill "${app_pid}" 2>/dev/null || true
  fi
  if [[ -n "${nginx_pid:-}" ]]; then
    kill "${nginx_pid}" 2>/dev/null || true
  fi
}

trap cleanup EXIT INT TERM

/usr/local/bin/desk-foreman &
app_pid=$!

nginx -g 'daemon off;' &
nginx_pid=$!

wait -n "${app_pid}" "${nginx_pid}"
