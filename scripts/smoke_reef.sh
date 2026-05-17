#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

compose="${COMPOSE_FILE:-docker-compose.yml}"

fail() {
  echo "smoke_reef: FAILED" >&2
  docker compose -f "$compose" ps >&2 || true
  docker compose -f "$compose" logs --no-color --tail=300 >&2 || true
  exit 1
}

if [[ ! -f ".env" ]]; then
  cp .env.example .env
fi

if [[ ! -f "lorelei.toml" ]]; then
  cp configs/reef-mock.toml lorelei.toml
fi

echo "smoke_reef: docker compose up --build -d"
docker compose -f "$compose" up --build -d || fail

echo "smoke_reef: waiting for harbor /healthz and /readyz"
for i in $(seq 1 120); do
  if docker compose -f "$compose" exec -T harbor wget -qO- http://localhost:8080/healthz >/dev/null 2>&1; then
    if docker compose -f "$compose" exec -T harbor wget -qO- http://localhost:8080/readyz >/dev/null 2>&1; then
      break
    fi
  fi
  sleep 1
  if [[ "$i" -eq 120 ]]; then
    fail
  fi
done

echo "smoke_reef: lore memo"
docker compose -f "$compose" exec -T harbor lore memo "smoke: hello reef" || fail

echo "smoke_reef: lore echo"
docker compose -f "$compose" exec -T harbor lore echo "hello reef" --top-k 3 || fail

echo "smoke_reef: lore ask (no memory)"
docker compose -f "$compose" exec -T harbor lore ask "Say 'ok'." --no-memory --progress false || fail

echo "smoke_reef: OK"

