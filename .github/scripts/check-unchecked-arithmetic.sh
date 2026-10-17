#!/usr/bin/env bash
# Custom linter: enforce checked math on timestamp/period arithmetic in the
# will contract. Flags raw `+`, `-`, `*` operators applied to identifiers that
# look like timestamps or periods (e.g. `now + period * SECONDS_PER_DAY`).
#
# Usage: check-unchecked-arithmetic.sh [path ...]
# Exits non-zero when unchecked arithmetic is detected.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [ "$#" -gt 0 ]; then
  TARGETS=("$@")
else
  TARGETS=("$ROOT_DIR/contracts/will/src")
fi

# Identifiers that represent timestamps or periods.
NAME_RE='(now|timestamp|period|duration|deadline|expiry|expires|start|end|SECONDS_PER_DAY|SECONDS_PER_HOUR|SECONDS_PER_MINUTE|LEDGERS_PER_DAY)'

# Raw arithmetic operators applied to a timestamp/period identifier.
# Matches e.g. `now + period`, `period * SECONDS_PER_DAY`, `timestamp - x`.
PATTERN="${NAME_RE}[[:space:]]*[+*-][[:space:]]*[A-Za-z0-9_(]"

status=0

for target in "${TARGETS[@]}"; do
  if [ ! -e "$target" ]; then
    continue
  fi

  # Collect offending lines, skipping comments and checked_* method calls.
  matches="$(grep -rnE "$PATTERN" "$target" --include='*.rs' 2>/dev/null \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' \
    | grep -vE 'checked_(add|sub|mul|div)' || true)"

  if [ -n "$matches" ]; then
    echo "error: unchecked arithmetic on timestamp/period detected in $target:" >&2
    echo "$matches" >&2
    echo "" >&2
    echo "Use checked_add/checked_sub/checked_mul with explicit error handling instead." >&2
    status=1
  fi
done

if [ "$status" -eq 0 ]; then
  echo "ok: no unchecked timestamp/period arithmetic found"
fi

exit "$status"
