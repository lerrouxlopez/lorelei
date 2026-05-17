#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

compose="${COMPOSE_FILE:-docker-compose.yml}"

fail() {
  echo "v1_acceptance: FAILED: $*" >&2
  docker compose -f "$compose" ps >&2 || true
  docker compose -f "$compose" logs --no-color --tail=400 >&2 || true
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

need docker
need bash
need date
need grep

if [[ ! -f ".env" ]]; then
  cp .env.example .env
fi
cp configs/reef-mock.toml lorelei.toml

echo "v1_acceptance: docker compose config"
docker compose -f "$compose" config -q || fail "docker compose config failed"

echo "v1_acceptance: docker compose up --build -d"
docker compose -f "$compose" up --build -d || fail "docker compose up failed"

echo "v1_acceptance: wait for harbor readiness"
for i in $(seq 1 120); do
  if docker compose -f "$compose" exec -T harbor wget -qO- http://localhost:8080/healthz >/dev/null 2>&1; then
    if docker compose -f "$compose" exec -T harbor wget -qO- http://localhost:8080/readyz >/dev/null 2>&1; then
      break
    fi
  fi
  sleep 1
  [[ "$i" -eq 120 ]] && fail "harbor did not become ready"
done

echo "v1_acceptance: lore doctor"
docker compose -f "$compose" exec -T harbor lore doctor || fail "lore doctor failed"

pearl_content="v1: my name is Eddie $(date -u +%s)"
echo "v1_acceptance: memo (manual pearl save)"
docker compose -f "$compose" exec -T harbor lore memo "$pearl_content" || fail "memo failed"

echo "v1_acceptance: echo (retrieves pearl)"
echo_out="$(docker compose -f "$compose" exec -T harbor lore echo "Eddie" --top-k 10)" || fail "echo failed"
echo "$echo_out" | grep -F "Eddie" >/dev/null || fail "echo did not include expected pearl"

echo "v1_acceptance: ask uses memory (should include Using memory:)"
ask_out="$(docker compose -f "$compose" exec -T harbor lore ask "What is my name?" --progress false)" || fail "ask failed"
echo "$ask_out" | grep -F "Using memory:" >/dev/null || fail "memory was not used in final answer"

echo "v1_acceptance: reflection stores durable preference"
docker compose -f "$compose" exec -T harbor lore ask "remember that I prefer tea" --progress false || fail "ask remember preference failed"
pref_out="$(docker compose -f "$compose" exec -T harbor lore pearls --pearl-type preference)" || fail "list pearls failed"
echo "$pref_out" | grep -i "prefer tea" >/dev/null || fail "preference was not stored"

echo "v1_acceptance: reflection rejects temporary fact"
before_count="$(docker compose -f "$compose" exec -T harbor lore pearls | wc -l | tr -d ' ')" || true
docker compose -f "$compose" exec -T harbor lore ask "remember todo: call mom tomorrow" --progress false || fail "ask remember temporary failed"
after_out="$(docker compose -f "$compose" exec -T harbor lore pearls)" || fail "list pearls failed"
echo "$after_out" | grep -i "call mom" >/dev/null && fail "temporary fact was stored (should be rejected)"

echo "v1_acceptance: low-risk shell executes automatically"
docker compose -f "$compose" exec -T harbor lore ask "LORELEI_TEST_CALL_SHELL=noop" --progress false || fail "low-risk shell did not execute"

echo "v1_acceptance: high-risk shell requires approval"
# Create a pearl to delete, then request forget_pearl via the agent tool.
del_content="v1: delete-me $(date -u +%s)"
del_line="$(docker compose -f "$compose" exec -T harbor lore memo "$del_content")" || fail "memo for deletion failed"
del_id="$(echo "$del_line" | awk '{print $3}' | tr -d '\r')" || true
[[ -z "$del_id" ]] && fail "could not parse pearl_id from memo output: $del_line"

deny_out="$(docker compose -f "$compose" exec -T harbor lore ask \"LORELEI_TEST_CALL_SHELL=forget_pearl pearl_id=$del_id\" --progress false)" || fail "high-risk shell ask failed"
echo "$deny_out" | grep -F "Approval required" >/dev/null || fail "expected approval requirement for high-risk shell"

echo "v1_acceptance: deleted pearl exclusion"
docker compose -f "$compose" exec -T harbor lore forget "$del_id" || fail "forget command failed"
echo2="$(docker compose -f "$compose" exec -T harbor lore echo \"$del_content\" --top-k 10)" || fail "echo after delete failed"
echo "$echo2" | grep -F "$del_content" >/dev/null && fail "deleted pearl was returned by echo"

echo "v1_acceptance: tenant isolation (wrong tenant sees no pearls)"
docker compose -f "$compose" exec -T harbor sh -lc "cat > /tmp/other_tenant.toml <<'EOF'
[agent]
tenant_id = \"00000000-0000-0000-0000-000000000099\"
agent_id = \"00000000-0000-0000-0000-000000000002\"
default_provider = \"mock\"
default_embedding_provider = \"mock\"

[harbor]
host = \"0.0.0.0\"
port = 8080

[lore]
postgres_url_env = \"DATABASE_URL\"
qdrant_url_env = \"QDRANT_URL\"
collection = \"lorelei_mock\"

[echo]
top_k = 20
rerank_top_k = 10
min_confidence = 0.25

[siren]
require_approval_for_high_risk = true
allow_shell_execution = true
allow_network_tools = false

[docs]
allowed_dirs = [\"./docs\", \"./prompts\"]

[providers.mock]
kind = \"mock\"
base_url = \"http://127.0.0.1:0\"
api_key_env = \"LORELEI_MOCK_API_KEY\"
chat_model = \"mock-chat\"
embedding_model = \"mock-embed\"
EOF

lore pearls --config /tmp/other_tenant.toml | grep -q . && exit 42 || exit 0" || fail "tenant isolation failed (other tenant saw data)"

echo "v1_acceptance: scheduled task runs"
at="$(date -u -d '+1 minute' +%H:%M)"
task_line="$(docker compose -f "$compose" exec -T harbor lore task add \"LORELEI_TEST_CALL_SHELL=noop\" --daily --at \"$at\")" || fail "task add failed"
task_id="$(echo "$task_line" | awk '{print $2}' | tr -d '\r')" || true
[[ -z "$task_id" ]] && fail "could not parse task_id from: $task_line"

# Wait until due, then run worker once.
sleep 70
docker compose -f "$compose" exec -T worker lorelei-harbor worker --once || fail "worker once failed"

echo "v1_acceptance: docs ingest/search"
docker compose -f "$compose" exec -T harbor lore docs ingest docs/ADR-0001-architecture.md || fail "docs ingest failed"
docker compose -f "$compose" exec -T harbor lore docs search architecture --top-k 3 || fail "docs search failed"

echo "v1_acceptance: logs contain run_id and avoid secrets"
docker compose -f "$compose" logs --no-color --tail=400 harbor | grep -E "run_id=" >/dev/null || fail "expected run_id in logs"
docker compose -f "$compose" logs --no-color --tail=400 harbor | grep -E "OPENAI_API_KEY|ANTHROPIC_API_KEY|LORELEI_LOCAL_API_KEY|LORELEI_MOCK_API_KEY" >/dev/null && fail "secret-like env var names found in logs"

echo "v1_acceptance: docker builds + smoke"
docker build -f docker/Dockerfile -t lorelei-reef:v1-acceptance . || fail "reef docker build failed"
docker build -f docker/Dockerfile.lore -t lorelei-cli:v1-acceptance . || fail "cli docker build failed"
bash scripts/smoke_reef.sh || fail "smoke_reef failed"

echo "v1_acceptance: OK"
