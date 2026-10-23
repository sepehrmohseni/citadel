#!/usr/bin/env bash
set -uo pipefail
title="$1"
shift
log="$(mktemp)"
"$@" 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
if [ "$status" -ne 0 ]; then
  body="$(sed -e 's/\x1b\[[0-9;]*[A-Za-z]//g' "$log" |
    grep -v -E '^[[:space:]]*(Compiling|Downloaded|Downloading|Updating|Fresh|Checking|Blocking|Locking|Adding|Running unittests|Doc-tests) ' |
    grep -v -E '^test .* \.\.\. ok$' |
    tail -n 150)"
  body="${body//'%'/'%25'}"
  body="${body//$'\r'/'%0D'}"
  body="${body//$'\n'/'%0A'}"
  if [ "${#body}" -gt 30000 ]; then body="${body:${#body}-30000}"; fi
  echo "::error title=${title}::${body}"
fi
rm -f "$log"
exit "$status"
