#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

echo "[Lorelei] Bringing up the Reef (postgres, qdrant, harbor)..."
docker compose up -d postgres qdrant harbor

echo "[Lorelei] Waiting for Harbor readiness..."
for i in {1..60}; do
  if curl -fsS "http://localhost:8080/readyz" >/dev/null; then
    echo "[Lorelei] Harbor is ready."
    break
  fi
  sleep 1
done

echo "[Lorelei] /healthz"
curl -fsS "http://localhost:8080/healthz" >/dev/null

echo "[Lorelei] /v1/providers"
curl -fsS "http://localhost:8080/v1/providers" >/dev/null

echo "[Lorelei] Smoke ok."

echo "[Lorelei] Tearing down..."
docker compose down -v

