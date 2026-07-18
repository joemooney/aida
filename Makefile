# AIDA - AI Design Assistant
# Makefile for building, testing, and running the application

.PHONY: help build build-release build-fast build-all restart-mcp-servers cli gui server \
        cli-remote gui-remote run-cli run-gui run-server run-server-force \
        test test-unit test-integration clean install \
        db-info db-migrate-sqlite db-migrate-yaml db-export \
        docs docs-build book book-glossary book-serve proto fmt lint check \
        web-build web-build-release web-serve web-serve-force web-clean web-deps \
        sync-templates check-templates \
        docker-build docker-up docker-up-d docker-down docker-shell \
        dev dev-server dev-web dev-pg dev-stop \
        release-patch release-minor release-major release-version

# Default database path
DB ?= requirements.db
SERVER_PORT ?= 50051
REST_PORT ?= 8080
# Set FORCE=1 to kill existing server processes before starting
FORCE ?= 0
FORCE_FLAG := $(if $(filter 1,$(FORCE)),--force,)

# Colors for help output
CYAN := \033[36m
GREEN := \033[32m
YELLOW := \033[33m
RESET := \033[0m

#==============================================================================
# HELP
#==============================================================================

help: ## Show this help message
	@echo ""
	@echo "$(GREEN)AIDA - AI Design Assistant$(RESET)"
	@echo "$(YELLOW)=============================$(RESET)"
	@echo ""
	@echo "$(CYAN)Build Targets:$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; /^(build|cli|gui|server|install)/ {printf "  $(GREEN)%-20s$(RESET) %s\n", $$1, $$2}'
	@echo ""
	@echo "$(CYAN)Run Targets:$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; /^run/ {printf "  $(GREEN)%-20s$(RESET) %s\n", $$1, $$2}'
	@echo ""
	@echo "$(CYAN)Database Targets:$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; /^db/ {printf "  $(GREEN)%-20s$(RESET) %s\n", $$1, $$2}'
	@echo ""
	@echo "$(CYAN)Test & Quality:$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; /^(test|fmt|lint|check|clean)/ {printf "  $(GREEN)%-20s$(RESET) %s\n", $$1, $$2}'
	@echo ""
	@echo "$(CYAN)Documentation:$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; /^(docs|proto)/ {printf "  $(GREEN)%-20s$(RESET) %s\n", $$1, $$2}'
	@echo ""
	@echo "$(CYAN)Web/WASM:$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; /^web/ {printf "  $(GREEN)%-20s$(RESET) %s\n", $$1, $$2}'
	@echo ""
	@echo "$(CYAN)Development:$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; /^dev/ {printf "  $(GREEN)%-20s$(RESET) %s\n", $$1, $$2}'
	@echo ""
	@echo "$(CYAN)Docker:$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; /^docker/ {printf "  $(GREEN)%-20s$(RESET) %s\n", $$1, $$2}'
	@echo ""
	@echo "$(CYAN)Release:$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; /^release/ {printf "  $(GREEN)%-20s$(RESET) %s\n", $$1, $$2}'
	@echo ""
	@echo "$(YELLOW)Variables:$(RESET)"
	@echo "  DB=<path>           Database file (default: requirements.db)"
	@echo "  SERVER_PORT=<port>  gRPC server port (default: 50051)"
	@echo "  REST_PORT=<port>    REST API port (default: 8080)"
	@echo "  WEB_PORT=<port>     Web client port (default: 8088)"
	@echo ""
	@echo "$(YELLOW)Environment Variables:$(RESET)"
	@echo "  AIDA_DEV_MODE=1     Enable dev mode (admin rebuild/restart from browser)"
	@echo "  ANTHROPIC_API_KEY   API key for AI chat features"
	@echo ""
	@echo "$(YELLOW)Examples:$(RESET)"
	@echo "  make build                              # Build all packages (debug)"
	@echo "  make build-release                      # Build all packages (release)"
	@echo "  make run-gui                            # Run GUI application"
	@echo "  make run-server DB=mydb.db              # Run server with custom database"
	@echo "  AIDA_DEV_MODE=1 make run-server         # Run server with dev mode"
	@echo "  make test                               # Run all tests"
	@echo ""

#==============================================================================
# BUILD TARGETS
#==============================================================================

build: ## Build all packages (debug mode)
	cargo build --workspace
	@$(MAKE) --no-print-directory restart-mcp-servers

build-release: ## Build all packages (release mode, optimized)
	cargo build --workspace --release
	@$(MAKE) --no-print-directory restart-mcp-servers

# trace:TASK-1154 | ai:claude
# Iteration build: release profile + incremental compilation. Measured ~43%
# faster on an edit-rebuild (55.7s vs ~98s) because the build is codegen-bound
# and incremental skips re-codegen for untouched units. Kept SEPARATE from
# build-release: CARGO_INCREMENTAL slightly reduces cross-codegen-unit
# optimization, so this is for LOCAL iteration, not shipping. Shipped releases
# build fresh (non-incremental) via CI (.github/workflows/release.yml).
build-fast: ## Build all packages (release + incremental — for iteration, NOT shipping)
	CARGO_INCREMENTAL=1 cargo build --workspace --release
	@$(MAKE) --no-print-directory restart-mcp-servers

build-all: build cli-remote ## Build everything including remote features

# trace:TASK-505 | ai:claude
# Kills only stale MCP servers (binary older than the freshly-built one) so
# the current session's MCP — already running the just-built binary — survives.
# Composes with TASK-493 (MCP self-respawn): post-TASK-493 servers would respawn
# stochastically on the next request anyway; this just makes it deterministic.
# Pre-TASK-493 zombies (no self-respawn code) only ever drop via this nudge.
restart-mcp-servers: ## Kill stale aida mcp-serve processes so MCP clients respawn with the fresh binary
	@./scripts/restart-mcp-servers.sh

cli: ## Build CLI only (aida binary)
	cargo build -p aida-cli

cli-remote: ## Build CLI with remote server support
	cargo build -p aida-cli --features remote

server: ## Build gRPC/REST server (aida-server binary)
	cargo build -p aida-server

# AIDA developer install — overlays your in-repo dev build to ~/.cargo/bin/.
# Whether `aida` from your shell hits this or a released binary depends on
# PATH ordering (run `aida status` to see which install is active).
install: build-release ## Install in-repo dev build to ~/.cargo/bin (developer)
	cargo install --path aida-cli --force
	cargo install --path aida-server --force

install-cli: ## Install CLI only (developer, in-repo build)
	cargo install --path aida-cli --force

install-server: ## Install server only (developer, in-repo build)
	cargo install --path aida-server --force

# End-user install — fetches the latest released binary, no Rust needed.
install-released: ## Install latest released binary to ~/.local/bin (end user)
	./scripts/install.sh

install-released-version: ## Install a specific released version (use VERSION=v0.4.0)
	./scripts/install.sh --version $(VERSION)

#==============================================================================
# RELEASE TARGETS (wrap scripts/release.sh for discoverability via make help)
#==============================================================================

# trace:TASK-79 | ai:claude — RELEASE_YES=1 (or `make release-patch YES=1`)
# threads --yes through to scripts/release.sh so non-interactive invocations
# (`make release-patch | tee log`) don't EOF on the confirm prompt.
RELEASE_YES_FLAG := $(if $(filter 1 yes y true,$(RELEASE_YES) $(YES)),--yes,)

release-patch: ## Cut a patch release (0.4.5 → 0.4.6) — set YES=1 to skip prompt
	./scripts/release.sh patch $(RELEASE_YES_FLAG)

release-minor: ## Cut a minor release (0.4.5 → 0.5.0) — set YES=1 to skip prompt
	./scripts/release.sh minor $(RELEASE_YES_FLAG)

release-major: ## Cut a major release (0.4.5 → 1.0.0) — set YES=1 to skip prompt
	./scripts/release.sh major $(RELEASE_YES_FLAG)

release-version: ## Cut a release with explicit version (use VERSION=0.5.0; YES=1 to skip prompt)
	./scripts/release.sh $(VERSION) $(RELEASE_YES_FLAG)

#==============================================================================
# RUN TARGETS
#==============================================================================

run-cli: cli ## Run CLI (use ARGS="..." for arguments)
	./target/debug/aida $(ARGS)

run-server: server ## Run gRPC/REST server (use DB=path, FORCE=1 to kill existing)
	./target/debug/aida-server --port $(SERVER_PORT) --rest-port $(REST_PORT) --database $(DB) $(FORCE_FLAG)

run-server-force: server ## Run server, killing any existing process on ports
	./target/debug/aida-server --port $(SERVER_PORT) --rest-port $(REST_PORT) --database $(DB) --force

run-server-bg: server ## Run server in background (use FORCE=1 to kill existing)
	@./target/debug/aida-server --port $(SERVER_PORT) --rest-port $(REST_PORT) --database $(DB) $(FORCE_FLAG) &
	@sleep 2
	@echo "Server started on gRPC:$(SERVER_PORT) REST:$(REST_PORT)"

stop-server: ## Stop running AIDA server
	@pkill -x "aida-server" 2>/dev/null && echo "Stopped aida-server" || echo "No aida-server running"

stop-all: ## Stop all running AIDA processes (aida-server, vite dev server)
	@echo "Stopping all AIDA processes..."
	@pkill -x "aida-server" 2>/dev/null && echo "  Stopped aida-server" || echo "  No aida-server running"
	@pkill -f "vite.*aida-web-react" 2>/dev/null && echo "  Stopped vite dev server" || echo "  No vite running"
	@echo "Done."

ps-servers: ## Show running AIDA processes
	@echo "Running AIDA processes:"
	@pgrep -x "aida-server" 2>/dev/null | while read pid; do echo "  aida-server (PID $$pid)"; done || true
	@pgrep -f "vite.*aida-web-react" 2>/dev/null | while read pid; do echo "  vite dev server (PID $$pid)"; done || true
	@pgrep -x "aida-server" >/dev/null 2>&1 || pgrep -f "vite.*aida-web-react" >/dev/null 2>&1 || echo "  (none)"

#==============================================================================
# DATABASE TARGETS
#==============================================================================

db-info: cli ## Show database info and statistics
	./target/debug/aida db info

db-migrate-sqlite: cli ## Migrate YAML database to SQLite
	./target/debug/aida db migrate --from yaml --to sqlite

db-migrate-yaml: cli ## Migrate SQLite database to YAML
	./target/debug/aida db migrate --from sqlite --to yaml

db-export: cli ## Export database to YAML for version control
	./target/debug/aida db export

db-list: cli ## List all requirements
	./target/debug/aida list

db-list-status: cli ## List requirements grouped by status
	@echo "=== Draft ===" && ./target/debug/aida list --status draft 2>/dev/null | tail -n +3
	@echo "\n=== Approved ===" && ./target/debug/aida list --status approved 2>/dev/null | tail -n +3
	@echo "\n=== In Progress ===" && ./target/debug/aida list --status in-progress 2>/dev/null | tail -n +3
	@echo "\n=== Completed ===" && ./target/debug/aida list --status completed 2>/dev/null | tail -n +3

#==============================================================================
# TEST & QUALITY TARGETS
#==============================================================================

test: ## Run all tests
	cargo test --workspace

test-unit: ## Run unit tests only
	cargo test --workspace --lib

test-integration: ## Run integration tests only
	cargo test --workspace --test '*'

test-cli: ## Run CLI tests
	cargo test -p aida-cli

test-core: ## Run core library tests
	cargo test -p aida-core

fmt: ## Format code with rustfmt
	cargo fmt --all

fmt-check: ## Check code formatting
	cargo fmt --all -- --check

lint: ## Run clippy linter
	cargo clippy --workspace --all-targets -- -D warnings

check: ## Run cargo check (fast compile check)
	cargo check --workspace

clean: ## Clean build artifacts
	cargo clean

clean-db: ## Remove database lock files
	rm -f *.lock requirements.yaml.lock requirements.db-wal requirements.db-shm

#==============================================================================
# DOCUMENTATION TARGETS
#==============================================================================

docs: ## Generate and open documentation
	cargo doc --workspace --open

docs-build: ## Generate documentation without opening
	cargo doc --workspace --no-deps

# trace:STORY-608 — the AIDA Book's Glossary chapter is GENERATED from
# `aida docs glossary`, never hand-maintained (ADR-5). Regenerate it, then
# build the mdBook.
book-glossary: ## Regenerate the AIDA Book's Glossary chapter from `aida docs glossary`
	bash docs/cli/generate-glossary.sh

book: book-glossary ## Build the AIDA Book (mdBook) — regenerates the Glossary chapter first
	mdbook build docs/cli

book-serve: book-glossary ## Serve the AIDA Book locally with live reload (regenerates the Glossary first)
	mdbook serve docs/cli --open

user-guide: cli ## Open user guide in browser
	./target/debug/aida user-guide

user-guide-dark: cli ## Open user guide in browser (dark mode)
	./target/debug/aida user-guide --dark

#==============================================================================
# PROTOBUF TARGETS
#==============================================================================

proto: ## Regenerate protobuf code
	@echo "Regenerating protobuf code..."
	@cd aida-server && cargo build
	@cd aida-cli && cargo build --features remote
	@echo "Protobuf code regenerated"

proto-check: ## Check if proto files are up to date
	@echo "Checking proto files..."
	@diff -q proto/aida.proto aida-server/src/generated/aida.rs > /dev/null 2>&1 || \
		echo "Warning: Proto files may be out of sync. Run 'make proto'"

#==============================================================================
# DEVELOPMENT HELPERS
#==============================================================================

watch: ## Watch for changes and rebuild (requires cargo-watch)
	cargo watch -x build

watch-test: ## Watch for changes and run tests (requires cargo-watch)
	cargo watch -x test

# Quick REST API tests
test-rest-ping: ## Test REST API ping endpoint
	@curl -s http://127.0.0.1:$(REST_PORT)/api/ping | jq .

test-rest-status: ## Test REST API status endpoint
	@curl -s http://127.0.0.1:$(REST_PORT)/api/status | jq .

test-rest-list: ## Test REST API list requirements
	@curl -s "http://127.0.0.1:$(REST_PORT)/api/requirements?limit=5" | jq '.requirements | length'
	@echo "requirements returned"

test-grpc-ping: cli-remote ## Test gRPC ping
	./target/debug/aida --server 127.0.0.1:$(SERVER_PORT) server ping

#==============================================================================
# REACT WEB DASHBOARD (aida-web-react/)
#==============================================================================

REACT_PORT ?= 5173

web-dev: ## Run React dashboard dev server (port 5173) — needs aida-server too
	cd aida-web-react && npm run dev -- --port $(REACT_PORT)

web-build: ## Build React dashboard for production (output: aida-web-react/dist)
	cd aida-web-react && npm run build

web-install: ## Install React dashboard npm dependencies
	cd aida-web-react && npm install

#==============================================================================
# TEMPLATE SYNC TARGETS (AIDA Development Only)
#==============================================================================
# These targets manage the dual-copy template system:
# - Master templates in aida-core/templates/ (embedded in binary)
# - Project-local templates in .claude/ (used by Claude Code)
# In the AIDA repo, .claude/ should use symlinks to aida-core/templates/

sync-templates: ## Sync .claude/ templates as symlinks to aida-core/templates/
	@echo "Syncing .claude/ templates to use symlinks..."
	@mkdir -p .claude/skills .claude/commands .claude/skills/local
	@# STORY-305: per-project skill extensions are deny-by-default for sync.
	@# We never enter .claude/skills/local/ and never touch *.local.md files.
	@# The master glob below only walks aida-core/templates/skills/*.md, and
	@# any *.local.md is explicitly skipped as a belt-and-braces guard so a
	@# stray master never overwrites a project's local extension.
	@for f in aida-core/templates/skills/*.md; do \
		name=$$(basename "$$f"); \
		case "$$name" in \
			*.local.md) echo "  Skip: $$name (STORY-305: .local.md never synced)"; continue ;; \
		esac; \
		rm -f ".claude/skills/$$name"; \
		ln -sf "../../aida-core/templates/skills/$$name" ".claude/skills/$$name"; \
		echo "  Linked: .claude/skills/$$name -> aida-core/templates/skills/$$name"; \
	done
	@# TASK-574: folder-form skills (<name>/SKILL.md + templates/ + examples/)
	@# link as a single directory symlink (the *.md loop above skips dirs).
	@for d in aida-core/templates/skills/*/; do \
		name=$$(basename "$$d"); \
		case "$$name" in \
			local) continue ;; \
		esac; \
		rm -rf ".claude/skills/$$name"; \
		ln -sf "../../aida-core/templates/skills/$$name" ".claude/skills/$$name"; \
		echo "  Linked: .claude/skills/$$name/ -> aida-core/templates/skills/$$name/ (folder-form)"; \
	done
	@# Remove existing files and create symlinks for commands
	@for f in aida-core/templates/commands/*.md; do \
		name=$$(basename "$$f"); \
		rm -f ".claude/commands/$$name"; \
		ln -sf "../../aida-core/templates/commands/$$name" ".claude/commands/$$name"; \
		echo "  Linked: .claude/commands/$$name -> aida-core/templates/commands/$$name"; \
	done
	@echo "Template sync complete!"

check-templates: ## Check if .claude/ templates are properly linked
	@echo "Checking template symlinks..."
	@errors=0; \
	for f in aida-core/templates/skills/*.md; do \
		name=$$(basename "$$f"); \
		case "$$name" in \
			*.local.md) continue ;; \
		esac; \
		target=".claude/skills/$$name"; \
		if [ -L "$$target" ]; then \
			echo "  OK: $$target (symlink)"; \
		elif [ -f "$$target" ]; then \
			echo "  WARNING: $$target is a regular file, not a symlink!"; \
			if diff -q "$$f" "$$target" > /dev/null 2>&1; then \
				echo "    Content matches master (but should be symlink)"; \
			else \
				echo "    CONTENT DIFFERS from master!"; \
				errors=1; \
			fi; \
		else \
			echo "  MISSING: $$target"; \
			errors=1; \
		fi; \
	done; \
	for d in aida-core/templates/skills/*/; do \
		name=$$(basename "$$d"); \
		case "$$name" in \
			local) continue ;; \
		esac; \
		target=".claude/skills/$$name"; \
		if [ -L "$$target" ]; then \
			echo "  OK: $$target (folder-form symlink)"; \
		else \
			echo "  MISSING: $$target (folder-form, TASK-574)"; \
			errors=1; \
		fi; \
	done; \
	for f in aida-core/templates/commands/*.md; do \
		name=$$(basename "$$f"); \
		target=".claude/commands/$$name"; \
		if [ -L "$$target" ]; then \
			echo "  OK: $$target (symlink)"; \
		elif [ -f "$$target" ]; then \
			echo "  WARNING: $$target is a regular file, not a symlink!"; \
			if diff -q "$$f" "$$target" > /dev/null 2>&1; then \
				echo "    Content matches master (but should be symlink)"; \
			else \
				echo "    CONTENT DIFFERS from master!"; \
				errors=1; \
			fi; \
		else \
			echo "  MISSING: $$target"; \
			errors=1; \
		fi; \
	done; \
	master="aida-core/templates/plan-template.md"; \
	target="docs/plans/_TEMPLATE.md"; \
	if [ -L "$$target" ]; then \
		echo "  OK: $$target (symlink)"; \
	elif [ -f "$$target" ]; then \
		echo "  WARNING: $$target is a regular file, not a symlink!"; \
		if diff -q "$$master" "$$target" > /dev/null 2>&1; then \
			echo "    Content matches master (but should be symlink)"; \
		else \
			echo "    CONTENT DIFFERS from master!"; errors=1; \
		fi; \
	else \
		echo "  MISSING: $$target (run: ln -sf ../../aida-core/templates/plan-template.md docs/plans/_TEMPLATE.md)"; \
		errors=1; \
	fi; \
	if [ $$errors -eq 1 ]; then \
		echo ""; \
		echo "Run 'make sync-templates' to fix issues"; \
		exit 1; \
	fi
	@echo "All templates OK!"

#==============================================================================
# DOCKER TARGETS
#==============================================================================

COMPOSE_FILE := .aida/docker-compose.yml

docker-build: ## Build Docker image (API + React dashboard)
	docker compose -f $(COMPOSE_FILE) build

docker-up: ## Start AIDA in Docker (foreground)
	docker compose -f $(COMPOSE_FILE) up

docker-up-d: ## Start AIDA in Docker (background)
	docker compose -f $(COMPOSE_FILE) up -d

docker-down: ## Stop AIDA Docker containers
	docker compose -f $(COMPOSE_FILE) down

docker-shell: ## Open a shell in the running AIDA container
	docker compose -f $(COMPOSE_FILE) exec aida bash

#==============================================================================
# DEVELOPMENT TARGETS (hot-reload)
#==============================================================================

DEV_COMPOSE := .aida/docker-compose.dev.yml
DEV_PG_URL ?= postgres://aida:aida@localhost:5432/aida_default
DEV_PID_DIR := .aida/.dev-pids

dev: dev-pg dev-server dev-web ## Start full dev environment (PostgreSQL + cargo-watch + Vite)
	@echo ""
	@echo "$(GREEN)AIDA dev environment running:$(RESET)"
	@echo "  $(CYAN)Web dashboard$(RESET):  http://localhost:5173"
	@echo "  $(CYAN)REST API$(RESET):       http://localhost:$(REST_PORT)"
	@echo "  $(CYAN)PostgreSQL$(RESET):     $(DEV_PG_URL)"
	@echo ""
	@echo "  Stop with: $(YELLOW)make dev-stop$(RESET)"

dev-pg: ## Start PostgreSQL in Docker (dev)
	@if docker compose -f $(DEV_COMPOSE) ps --status running 2>/dev/null | grep -q db; then \
		echo "$(GREEN)PostgreSQL already running$(RESET)"; \
	else \
		echo "Starting PostgreSQL..."; \
		docker compose -f $(DEV_COMPOSE) up -d; \
		echo "Waiting for PostgreSQL..."; \
		for i in 1 2 3 4 5 6 7 8 9 10; do \
			docker compose -f $(DEV_COMPOSE) exec -T db pg_isready -U aida >/dev/null 2>&1 && break; \
			sleep 1; \
		done; \
		echo "$(GREEN)PostgreSQL ready$(RESET)"; \
	fi

dev-server: ## Start Rust server with cargo-watch (hot-reload)
	@if ! command -v cargo-watch >/dev/null 2>&1; then \
		echo "$(YELLOW)cargo-watch not installed. Install with:$(RESET)"; \
		echo "  cargo install cargo-watch"; \
		echo ""; \
		echo "Starting server without hot-reload..."; \
	fi
	@mkdir -p $(DEV_PID_DIR)
	@if [ -f $(DEV_PID_DIR)/server.pid ] && kill -0 $$(cat $(DEV_PID_DIR)/server.pid) 2>/dev/null; then \
		echo "$(GREEN)Server already running (PID $$(cat $(DEV_PID_DIR)/server.pid))$(RESET)"; \
	else \
		if command -v cargo-watch >/dev/null 2>&1; then \
			echo "Starting cargo-watch (server)..."; \
			cargo watch -w aida-core -w aida-server -x \
				'run -p aida-server --features postgres -- --rest-port $(REST_PORT) --database $(DEV_PG_URL)' \
				> .aida/.dev-server.log 2>&1 & \
			echo $$! > $(DEV_PID_DIR)/server.pid; \
		else \
			cargo run -p aida-server --features postgres -- --rest-port $(REST_PORT) --database $(DEV_PG_URL) \
				> .aida/.dev-server.log 2>&1 & \
			echo $$! > $(DEV_PID_DIR)/server.pid; \
		fi; \
		echo "$(GREEN)Server starting (PID $$(cat $(DEV_PID_DIR)/server.pid)) — log: .aida/.dev-server.log$(RESET)"; \
	fi

dev-web: ## Start Vite dev server (hot-reload)
	@mkdir -p $(DEV_PID_DIR)
	@if [ -f $(DEV_PID_DIR)/web.pid ] && kill -0 $$(cat $(DEV_PID_DIR)/web.pid) 2>/dev/null; then \
		echo "$(GREEN)Vite already running (PID $$(cat $(DEV_PID_DIR)/web.pid))$(RESET)"; \
	else \
		echo "Starting Vite dev server..."; \
		cd aida-web-react && npm run dev > ../.aida/.dev-web.log 2>&1 & \
		echo $$! > $(DEV_PID_DIR)/web.pid; \
		echo "$(GREEN)Vite starting (PID $$(cat $(DEV_PID_DIR)/web.pid)) — log: .aida/.dev-web.log$(RESET)"; \
	fi

dev-stop: ## Stop all dev services
	@echo "Stopping dev services..."
	@if [ -f $(DEV_PID_DIR)/server.pid ]; then \
		kill $$(cat $(DEV_PID_DIR)/server.pid) 2>/dev/null || true; \
		rm -f $(DEV_PID_DIR)/server.pid; \
		echo "  Server stopped"; \
	fi
	@if [ -f $(DEV_PID_DIR)/web.pid ]; then \
		kill $$(cat $(DEV_PID_DIR)/web.pid) 2>/dev/null || true; \
		rm -f $(DEV_PID_DIR)/web.pid; \
		echo "  Vite stopped"; \
	fi
	@docker compose -f $(DEV_COMPOSE) down 2>/dev/null || true
	@echo "$(GREEN)Dev environment stopped$(RESET)"

dev-logs: ## Tail dev server logs
	@tail -f .aida/.dev-server.log

serve: ## Start aida-server for multi-user access (PostgreSQL backend)
	@echo "Starting AIDA server with PostgreSQL backend..."
	@echo "  Database: $(DEV_PG_URL)"
	@echo "  REST API: http://0.0.0.0:$(REST_PORT)"
	@echo ""
	@echo "Other machines can connect with:"
	@echo "  aida --file '$(DEV_PG_URL)' list"
	@echo ""
	./target/debug/aida-server --host 0.0.0.0 --port 50051 --rest-port $(REST_PORT) --database "$(DEV_PG_URL)"
