#!/bin/bash
# Seed the Employee #001 API key into greed-compute
# Uses the same key that's already in OpenClaw config
# Run after greed-compute is running: bash deploy/seed-key.sh
set -e

# The employee's existing API key from OpenClaw config
EMPLOYEE_KEY="emp_0cf84135e0c4568060d6e1fadb8cedac4fbf5a7f6919148b"

echo "Seeding Employee #001 API key into greed-compute..."

# Insert directly into SQLite
sqlite3 /opt/greed-compute/greed-compute.db "INSERT OR IGNORE INTO api_keys (key, name, tier, created_at, is_active) VALUES ('$EMPLOYEE_KEY', 'employee-001', 'pro', datetime('now'), 1);"

echo "Done. Testing..."
curl -s http://localhost:8080/v1/health -H "X-API-Key: $EMPLOYEE_KEY" | python3 -m json.tool

echo ""
echo "Key seeded: $EMPLOYEE_KEY (tier: pro)"
