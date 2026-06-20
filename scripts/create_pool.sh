#!/usr/bin/env bash
# Creates a new Apex pool via the admin API.
#
# Runs every 4 hours (6 pools/day). Cron entry:
#   0 */4 * * * /home/user/apex-backend/scripts/create_pool.sh >> /var/log/apex-pool-creator.log 2>&1
#
# Required env vars (loaded from .env if present):
#   ADMIN_SECRET   - bearer token set in the backend
#   PORT           - backend port (default: 8080)
#
# Optional env vars:
#   POOL_BASE_URL       - override the base URL (default: http://localhost:$PORT)
#   POOL_LEG_COUNT      - number of price legs (default: 1)
#   POOL_ENTRY_FEE      - entry fee in base units (default: 500000000)
#   POOL_COMMIT_WINDOW  - ms until commit deadline (default: 9000000 = 2.5 h)
#   POOL_REVEAL_WINDOW  - ms until reveal deadline (default: 12600000 = 3.5 h)
#   POOL_ORACLE_IDS_JSON - JSON array of oracle IDs, e.g. '["0xabc","0xdef"]'
#                          Falls back to POOL_ORACLE_IDS in .env (comma-separated)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$SCRIPT_DIR/../.env"

if [[ -f "$ENV_FILE" ]]; then
    # shellcheck disable=SC1090
    set -a; source "$ENV_FILE"; set +a
fi

PORT="${PORT:-8080}"
BASE_URL="${POOL_BASE_URL:-http://localhost:$PORT}"

if [[ -z "${ADMIN_SECRET:-}" ]]; then
    echo "[$(date -u +%FT%TZ)] ERROR: ADMIN_SECRET is not set" >&2
    exit 1
fi

# Build the oracle_ids JSON array from POOL_ORACLE_IDS_JSON or POOL_ORACLE_IDS
if [[ -n "${POOL_ORACLE_IDS_JSON:-}" ]]; then
    ORACLE_IDS_JSON="$POOL_ORACLE_IDS_JSON"
elif [[ -n "${POOL_ORACLE_IDS:-}" ]]; then
    # Convert comma-separated "0xaaa,0xbbb" -> ["0xaaa","0xbbb"]
    ORACLE_IDS_JSON=$(printf '%s' "$POOL_ORACLE_IDS" | \
        python3 -c "import sys,json; ids=[s.strip() for s in sys.stdin.read().split(',') if s.strip()]; print(json.dumps(ids))")
else
    ORACLE_IDS_JSON="[]"
fi

BODY=$(printf '{
  "leg_count": %s,
  "entry_fee_amount": %s,
  "commit_window_ms": %s,
  "reveal_window_ms": %s,
  "oracle_ids": %s
}' \
    "${POOL_LEG_COUNT:-1}" \
    "${POOL_ENTRY_FEE:-500000000}" \
    "${POOL_COMMIT_WINDOW:-9000000}" \
    "${POOL_REVEAL_WINDOW:-12600000}" \
    "$ORACLE_IDS_JSON")

echo "[$(date -u +%FT%TZ)] Creating pool with body: $BODY"

RESPONSE=$(curl -sf \
    -X POST "$BASE_URL/admin/pools" \
    -H "Authorization: Bearer $ADMIN_SECRET" \
    -H "Content-Type: application/json" \
    -d "$BODY")

echo "[$(date -u +%FT%TZ)] Response: $RESPONSE"
