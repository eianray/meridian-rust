#!/bin/bash
# Migrate PostgreSQL data from Mac mini to Hetzner
# Run this ON THE MAC MINI, then transfer the dump to Hetzner
#
# Usage:
#   On Mac mini:  bash deploy/migrate-db.sh dump
#   On Hetzner:   bash deploy/migrate-db.sh restore

set -e

DUMP_FILE="meridian-$(date +%Y%m%d-%H%M).sql"
DB_NAME="meridian"
DB_USER="meridian"

case "$1" in
  dump)
    echo "Dumping database from Mac mini..."
    pg_dump -U "$DB_USER" -d "$DB_NAME" \
        --no-owner --no-acl \
        -f "$DUMP_FILE"
    echo "✅ Dump written to: $DUMP_FILE"
    echo ""
    echo "Transfer to Hetzner:"
    echo "  scp $DUMP_FILE nodeapi@<HETZNER_IP>:/opt/node-api/deploy/"
    echo "Then on Hetzner run:"
    echo "  bash deploy/migrate-db.sh restore $DUMP_FILE"
    ;;

  restore)
    DUMP="${2:-$DUMP_FILE}"
    if [ ! -f "$DUMP" ]; then
        echo "Error: dump file not found: $DUMP"
        exit 1
    fi
    echo "Restoring database to Hetzner PostgreSQL..."
    sudo -u postgres psql -d "$DB_NAME" -f "$DUMP"
    echo "✅ Database restored from: $DUMP"
    ;;

  *)
    echo "Usage: $0 [dump|restore] [dump_file]"
    exit 1
    ;;
esac
