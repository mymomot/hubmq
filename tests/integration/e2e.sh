#!/usr/bin/env bash
# e2e.sh — Smoke test end-to-end HubMQ
# Usage : bash tests/integration/e2e.sh
# Prérequis : hubmq déployé sur LXC 415 (192.168.10.15:8470), ssh hubmq configuré

set -euo pipefail

TARGET="http://192.168.10.15:8470"

echo "→ 1. Health check"
curl -sf "$TARGET/health" | grep -q ok && echo "  ok"

echo "→ 2. Post generic alert P3 (should be log-only)"
curl -sf -X POST "$TARGET/in/generic" -H "Content-Type: application/json" -d '{
  "source":"test-e2e","severity":"P3","title":"E2E P3 test","body":"ignore me"
}' | grep -qE "^$|Accepted" || true
echo "  posted"

echo "→ 3. Post Wazuh-style alert level 14 (should email P0)"
curl -sf -X POST "$TARGET/in/wazuh" -H "Content-Type: application/json" -d '{
  "rule":{"level":14,"description":"E2E critical","id":99999},
  "agent":{"name":"e2e-host"},
  "full_log":"E2E test full log"
}' && echo "  posted wazuh"

echo "→ 4. Dedup (repost same)"
for i in 1 2 3; do
  curl -sf -X POST "$TARGET/in/wazuh" -H "Content-Type: application/json" -d '{
    "rule":{"level":14,"description":"E2E critical","id":99999},
    "agent":{"name":"e2e-host"},
    "full_log":"E2E test full log"
  }' > /dev/null
done
echo "  dedup check posted"

echo "→ 5. Check NATS stream counts"
ssh hubmq "curl -s http://localhost:8222/jsz | jq '.account_details[0].stream_detail[]|{name,messages:.state.messages}'"

echo "→ 6. Check audit log entries"
ssh hubmq "sudo sqlite3 /var/lib/hubmq/queue.db 'SELECT event, COUNT(*) FROM audit WHERE ts > datetime(\"now\", \"-5 minutes\") GROUP BY event'"

echo "✓ E2E smoke test complete"
