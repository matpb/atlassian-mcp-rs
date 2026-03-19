.PHONY: help build run docker-build up down logs health

help:
	@echo "Targets:"
	@echo "  build        cargo build --release"
	@echo "  run          cargo run --release (sources .env if present — MCP_PORT etc.; not Atlassian)"
	@echo "  docker-build docker compose build"
	@echo "  up           docker compose up -d"
	@echo "  down         docker compose down"
	@echo "  logs         docker compose logs -f"
	@echo "  health       GET /health (uses MCP_PORT from env, default 8432)"

build:
	cargo build --release

run:
	@if [ -f .env ]; then set -a && . ./.env && set +a; fi; cargo run --release

docker-build:
	docker compose build

up:
	docker compose up -d

down:
	docker compose down

logs:
	docker compose logs -f

health:
	curl -sf "http://127.0.0.1:$${MCP_PORT:-8432}/health" && echo
