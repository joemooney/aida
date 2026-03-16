#!/bin/bash
# Initialize the production PostgreSQL with AIDA data.
#
# Usage: ./docker/init-pg.sh
#
# This script:
# 1. Ensures the production PostgreSQL container is running
# 2. Creates the aida_default database if it doesn't exist
# 3. Migrates data from SQLite (requirements.db) to PostgreSQL
#
# Prerequisites:
# - docker/docker-compose.yml services running (at least postgres)
# - requirements.db exists in the project root

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

PG_USER=$(cat "$SCRIPT_DIR/secrets/pg_user.txt")
PG_PASS=$(cat "$SCRIPT_DIR/secrets/pg_password.txt")
PG_HOST="localhost"
PG_PORT="5432"
PG_DB="aida_default"

echo "Initializing AIDA PostgreSQL database..."

# Check if the dev PG is running (port 5432)
if docker exec aida_db_1 pg_isready -U aida 2>/dev/null; then
    echo "Using dev PostgreSQL (aida_db_1)"
    PG_CONTAINER="aida_db_1"
    PG_USER="aida"
    PG_PASS="aida"
elif docker exec postgres pg_isready -U "$PG_USER" 2>/dev/null; then
    echo "Using production PostgreSQL (postgres)"
    PG_CONTAINER="postgres"
else
    echo "Error: No PostgreSQL container is running."
    echo "Start with: docker compose -f .aida/docker-compose.dev.yml up -d"
    echo "Or:         cd docker && docker compose up -d postgres"
    exit 1
fi

# Check if database exists
DB_EXISTS=$(docker exec "$PG_CONTAINER" psql -U "$PG_USER" -lqt 2>/dev/null | grep -c "$PG_DB" || true)

if [ "$DB_EXISTS" -gt 0 ]; then
    REQ_COUNT=$(docker exec "$PG_CONTAINER" psql -U "$PG_USER" -d "$PG_DB" -t -c "SELECT COUNT(*) FROM requirements;" 2>/dev/null | tr -d ' ')
    echo "Database '$PG_DB' already exists with $REQ_COUNT requirements."

    if [ "$REQ_COUNT" -gt 0 ]; then
        echo "Skipping migration — data already present."
        exit 0
    fi
fi

# Create database if needed
if [ "$DB_EXISTS" -eq 0 ]; then
    echo "Creating database '$PG_DB'..."
    docker exec "$PG_CONTAINER" psql -U "$PG_USER" -c "CREATE DATABASE $PG_DB;" 2>/dev/null
fi

# Migrate from SQLite
if [ -f "$PROJECT_ROOT/requirements.db" ]; then
    echo "Migrating from requirements.db..."
    cd "$PROJECT_ROOT"
    CONN="postgres://$PG_USER:$PG_PASS@$PG_HOST:$PG_PORT/$PG_DB"
    ./target/debug/aida db migrate --from sqlite --to postgres --output "$CONN"
    echo "Migration complete."
else
    echo "No requirements.db found — empty database created."
fi

echo ""
echo "PostgreSQL is ready for multi-user access."
echo ""
echo "Connect from CLI:"
echo "  aida --file 'postgres://$PG_USER:$PG_PASS@$PG_HOST:$PG_PORT/$PG_DB' list"
echo ""
echo "Start the server:"
echo "  aida-server --database 'postgres://$PG_USER:$PG_PASS@$PG_HOST:$PG_PORT/$PG_DB' --rest-port 8080"
echo ""
echo "For another machine on your network:"
echo "  aida --file 'postgres://$PG_USER:$PG_PASS@YOUR_IP:$PG_PORT/$PG_DB' list"
