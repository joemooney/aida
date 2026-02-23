# AIDA - AI Design Assistant
# Makefile for building, testing, and running the application

.PHONY: help build build-release build-all cli gui server \
        cli-remote gui-remote run-cli run-gui run-server run-server-force \
        test test-unit test-integration clean install \
        db-info db-migrate-sqlite db-migrate-yaml db-export \
        docs proto fmt lint check \
        web-build web-build-release web-serve web-serve-force web-clean web-deps \
        sync-templates check-templates \
        docker-build docker-up docker-up-d docker-down docker-shell

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
	@echo "$(CYAN)Docker:$(RESET)"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; /^docker/ {printf "  $(GREEN)%-20s$(RESET) %s\n", $$1, $$2}'
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

build-release: ## Build all packages (release mode, optimized)
	cargo build --workspace --release

build-all: build cli-remote gui-remote ## Build everything including remote features

cli: ## Build CLI only (aida binary)
	cargo build -p aida-cli

cli-remote: ## Build CLI with remote server support
	cargo build -p aida-cli --features remote

gui: ## Build GUI only (aida-desktop binary)
	cargo build -p aida-desktop

gui-remote: ## Build GUI with remote server support
	cargo build -p aida-desktop --features remote

server: ## Build gRPC/REST server (aida-server binary)
	cargo build -p aida-server

install: build-release ## Install binaries to ~/.cargo/bin
	cargo install --path aida-cli
	cargo install --path aida-desktop
	cargo install --path aida-server

install-cli: ## Install CLI only
	cargo install --path aida-cli

install-gui: ## Install GUI only
	cargo install --path aida-desktop

install-server: ## Install server only
	cargo install --path aida-server

#==============================================================================
# RUN TARGETS
#==============================================================================

run-cli: cli ## Run CLI (use ARGS="..." for arguments)
	./target/debug/aida $(ARGS)

run-gui: gui ## Run GUI application
	./target/debug/aida-desktop

run-gui-remote: gui-remote ## Run GUI connected to remote server
	./target/debug/aida-desktop --server 127.0.0.1:$(SERVER_PORT)

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

stop-web: ## Stop running trunk/web server
	@pkill -f "^trunk serve" 2>/dev/null && echo "Stopped trunk" || echo "No trunk running"

stop-all: ## Stop all running servers (aida-server, trunk, aida-desktop)
	@echo "Stopping all AIDA processes..."
	@pkill -x "aida-server" 2>/dev/null && echo "  Stopped aida-server" || echo "  No aida-server running"
	@pkill -f "^trunk serve" 2>/dev/null && echo "  Stopped trunk" || echo "  No trunk running"
	@pkill -x "aida-desktop" 2>/dev/null && echo "  Stopped aida-desktop" || echo "  No aida-desktop running"
	@echo "Done."

ps-servers: ## Show running AIDA processes
	@echo "Running AIDA processes:"
	@pgrep -x "aida-server" 2>/dev/null | while read pid; do echo "  aida-server (PID $$pid)"; done || true
	@pgrep -f "^trunk serve" 2>/dev/null | while read pid; do echo "  trunk (PID $$pid)"; done || true
	@pgrep -x "aida-desktop" 2>/dev/null | while read pid; do echo "  aida-desktop (PID $$pid)"; done || true
	@pgrep -x "aida-server" >/dev/null 2>&1 || pgrep -f "^trunk serve" >/dev/null 2>&1 || pgrep -x "aida-desktop" >/dev/null 2>&1 || echo "  (none)"

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

dev: run-gui ## Alias for run-gui (development mode)

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
# WASM WEB CLIENT TARGETS
#==============================================================================
# Primary web client is aida-desktop (full-featured, same codebase as desktop)
# Alternative: aida-web (lightweight, separate crate - use web-*-lite targets)

WEB_PORT ?= 8088

web-build: ## Build WASM web client (requires trunk)
	cd aida-desktop && trunk build

web-build-release: ## Build WASM web client (release/optimized)
	cd aida-desktop && trunk build --release

web-serve: ## Serve WASM web client for development (port 8088)
	cd aida-desktop && trunk serve --port $(WEB_PORT)

web-serve-force: ## Serve web client, killing any existing trunk process on port
	@-pkill -f "trunk serve.*$(WEB_PORT)" 2>/dev/null || true
	@sleep 0.2
	cd aida-desktop && trunk serve --port $(WEB_PORT)

web-clean: ## Clean web build artifacts
	rm -rf aida-desktop/dist aida-web/dist

web-deps: ## Install WASM build dependencies
	rustup target add wasm32-unknown-unknown
	cargo install --locked trunk
	@echo "WASM dependencies installed"

# Lightweight web client (aida-web) - alternative to full aida-desktop
web-build-lite: ## Build lightweight web client (aida-web)
	cd aida-web && trunk build

web-serve-lite: ## Serve lightweight web client (aida-web)
	cd aida-web && trunk serve --port $(WEB_PORT)

#==============================================================================
# TEMPLATE SYNC TARGETS (AIDA Development Only)
#==============================================================================
# These targets manage the dual-copy template system:
# - Master templates in aida-core/templates/ (embedded in binary)
# - Project-local templates in .claude/ (used by Claude Code)
# In the AIDA repo, .claude/ should use symlinks to aida-core/templates/

sync-templates: ## Sync .claude/ templates as symlinks to aida-core/templates/
	@echo "Syncing .claude/ templates to use symlinks..."
	@mkdir -p .claude/skills .claude/commands
	@# Remove existing files and create symlinks for skills
	@for f in aida-core/templates/skills/*.md; do \
		name=$$(basename "$$f"); \
		rm -f ".claude/skills/$$name"; \
		ln -sf "../../aida-core/templates/skills/$$name" ".claude/skills/$$name"; \
		echo "  Linked: .claude/skills/$$name -> aida-core/templates/skills/$$name"; \
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
	if [ $$errors -eq 1 ]; then \
		echo ""; \
		echo "Run 'make sync-templates' to fix issues"; \
		exit 1; \
	fi
	@echo "All templates OK!"

#==============================================================================
# DOCKER TARGETS
#==============================================================================

docker-build: ## Build Docker image (API + React dashboard)
	docker compose build

docker-up: ## Start AIDA in Docker (foreground)
	docker compose up

docker-up-d: ## Start AIDA in Docker (background)
	docker compose up -d

docker-down: ## Stop AIDA Docker containers
	docker compose down

docker-shell: ## Open a shell in the running AIDA container
	docker compose exec aida bash
