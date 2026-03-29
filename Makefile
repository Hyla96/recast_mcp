# Recast MCP — developer shortcuts.
# See also: just --list  (Rust build, test, lint commands)

COMPOSE_BASE := docker compose -f docker-compose.yml
COMPOSE      := docker compose -f docker-compose.yml -f docker/docker-compose.override.yml

POSTGRES_USER     ?= recast
POSTGRES_PASSWORD ?= recast
POSTGRES_DB       ?= recast_mcp
DATABASE_URL      ?= postgres://$(POSTGRES_USER):$(POSTGRES_PASSWORD)@localhost:5432/$(POSTGRES_DB)

.PHONY: dev down logs db-migrate db-reset

## Start all services with hot reload (Rust cargo-watch + Vite HMR).
dev:
	$(COMPOSE) up

## Stop all services and remove containers (volumes preserved).
## To also remove volumes: docker compose down -v
down:
	$(COMPOSE) down

## Tail logs from all running services (Ctrl-C to stop).
logs:
	$(COMPOSE) logs -f

## Run pending sqlx migrations against the running db container.
## Requires: DATABASE_URL pointing to the local PostgreSQL instance.
db-migrate:
	DATABASE_URL="$(DATABASE_URL)" cargo sqlx migrate run

## Drop and recreate the development database, run migrations, and seed.
## WARNING: destroys all data in the local database.
db-reset:
	$(COMPOSE_BASE) exec db psql -U $(POSTGRES_USER) postgres \
	  -c "DROP DATABASE IF EXISTS $(POSTGRES_DB) WITH (FORCE);"
	$(COMPOSE_BASE) exec db psql -U $(POSTGRES_USER) postgres \
	  -c "CREATE DATABASE $(POSTGRES_DB);"
	DATABASE_URL="$(DATABASE_URL)" cargo sqlx migrate run
	DATABASE_URL="$(DATABASE_URL)" psql -f migrations/seed_dev.sql
