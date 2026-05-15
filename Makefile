.PHONY: migrate db-reset up down logs ship

# Requires:
# - PostgreSQL reachable via $$DATABASE_URL
# - sqlx-cli installed (e.g. `cargo install sqlx-cli --no-default-features --features postgres`)

migrate:
	sqlx migrate run

db-reset:
	sqlx database drop -y
	sqlx database create
	sqlx migrate run

up:
	docker compose up --build

down:
	docker compose down

logs:
	docker compose logs -f --tail=200

ship:
	docker build -f docker/Dockerfile --target runtime -t lorelei-harbor:local .
