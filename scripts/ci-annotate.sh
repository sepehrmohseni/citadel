#!/usr/bin/env bash
set -uo pipefail
title="$1"
shift
log="$(mktemp)"
"$@" 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
if [ "$status" -ne 0 ]; then
  body="$(grep -E -i -B2 -A12 'error|panicked|FAIL|failed|assert' "$log" | tail -n 120)"
  [ -n "$body" ] || body="$(tail -n 120 "$log")"
  body="${body//'%'/'%25'}"
  body="${body//$'\r'/'%0D'}"
  body="${body//$'\n'/'%0A'}"
  echo "::error title=${title}::${body:0:60000}"
fi
rm -f "$log"
exit "$status"
