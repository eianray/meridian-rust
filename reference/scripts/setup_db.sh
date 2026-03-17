#!/bin/bash
# One-time PostgreSQL setup for Meridian
# Requires PostgreSQL to be installed and running

set -e

DB_NAME="meridian"
DB_USER="meridian"
DB_PASS="meridian"

echo "Creating Meridian database..."

psql postgres -c "CREATE USER ${DB_USER} WITH PASSWORD '${DB_PASS}';" 2>/dev/null || echo "User already exists"
psql postgres -c "CREATE DATABASE ${DB_NAME} OWNER ${DB_USER};" 2>/dev/null || echo "Database already exists"
psql postgres -c "GRANT ALL PRIVILEGES ON DATABASE ${DB_NAME} TO ${DB_USER};"

echo "Database ready: postgresql://${DB_USER}:${DB_PASS}@localhost:5432/${DB_NAME}"
echo "Tables will be created automatically on first startup."
