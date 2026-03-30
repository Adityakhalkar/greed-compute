#!/bin/bash
# Migrate API keys from old VPS to new VPS
#
# Run on the OLD VPS first to export:
#   bash migrate-db.sh export
#
# Copy the output file to new VPS, then run:
#   bash migrate-db.sh import /path/to/keys-export.sql
#
set -e

DB="/opt/greed-compute/greed-compute.db"
EXPORT_FILE="keys-export-$(date +%Y%m%d-%H%M%S).sql"

case "$1" in
  export)
    echo "Exporting API keys from $DB..."
    if [ ! -f "$DB" ]; then
      echo "ERROR: Database not found at $DB"
      exit 1
    fi
    sqlite3 "$DB" ".dump api_keys" > "$EXPORT_FILE"
    echo "Exported to: $EXPORT_FILE"
    echo ""
    echo "Keys exported:"
    sqlite3 "$DB" "SELECT key, name, tier, is_active FROM api_keys;"
    echo ""
    echo "Now copy this file to the new VPS and run:"
    echo "  bash migrate-db.sh import $EXPORT_FILE"
    ;;

  import)
    if [ -z "$2" ]; then
      echo "Usage: bash migrate-db.sh import /path/to/keys-export.sql"
      exit 1
    fi
    if [ ! -f "$2" ]; then
      echo "ERROR: File not found: $2"
      exit 1
    fi
    echo "Importing API keys into $DB..."
    # Wait for greed-compute to have created the DB
    if [ ! -f "$DB" ]; then
      echo "ERROR: Database not found at $DB. Is greed-compute running?"
      exit 1
    fi
    sqlite3 "$DB" < "$2"
    echo "Import complete. Current keys:"
    sqlite3 "$DB" "SELECT key, name, tier, is_active FROM api_keys;"
    echo ""
    echo "Verifying service accepts keys..."
    FIRST_KEY=$(sqlite3 "$DB" "SELECT key FROM api_keys WHERE is_active=1 LIMIT 1;")
    curl -s http://localhost:8080/v1/health -H "X-API-Key: $FIRST_KEY" | python3 -m json.tool
    ;;

  *)
    echo "Usage:"
    echo "  On old VPS:  bash migrate-db.sh export"
    echo "  On new VPS:  bash migrate-db.sh import /path/to/keys-export.sql"
    exit 1
    ;;
esac
