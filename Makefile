.PHONY: migrate db-reset

# Requires:
# - PostgreSQL reachable via $$DATABASE_URL
# - sqlx-cli installed (e.g. `cargo install sqlx-cli --no-default-features --features postgres`)

migrate:
	sqlx migrate run

db-reset:
	sqlx database drop -y
	sqlx database create
	sqlx migrate run

