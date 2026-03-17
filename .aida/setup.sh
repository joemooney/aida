#!/bin/bash
# AIDA Project Setup
#
# Run this after cloning to set up the AIDA development environment:
#   .aida/setup.sh
#
# What it does:
# 1. Checks for Rust toolchain (required to build aida)
# 2. Builds the aida CLI and server binaries
# 3. Sets up the aida-store worktree (distributed requirements database)
# 4. Optionally installs aida to ~/.local/bin
# 5. Verifies everything works

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

ok()   { echo -e "${GREEN}✓${NC} $1"; }
warn() { echo -e "${YELLOW}!${NC} $1"; }
fail() { echo -e "${RED}✗${NC} $1"; exit 1; }
info() { echo -e "${CYAN}→${NC} $1"; }

cd "$PROJECT_ROOT"

echo ""
echo -e "${BOLD}AIDA Project Setup${NC}"
echo "─────────────────────────────────────"
echo ""

# ============================================================================
# Step 1: Check prerequisites
# ============================================================================

info "Checking prerequisites..."

# Rust
if command -v cargo &>/dev/null; then
    RUST_VERSION=$(rustc --version | awk '{print $2}')
    ok "Rust $RUST_VERSION"
else
    fail "Rust not installed. Install from https://rustup.rs"
fi

# Git
if command -v git &>/dev/null; then
    GIT_VERSION=$(git --version | awk '{print $3}')
    ok "Git $GIT_VERSION"
else
    fail "Git not installed"
fi

# Node.js (optional, for React dashboard)
if command -v node &>/dev/null; then
    NODE_VERSION=$(node --version)
    ok "Node.js $NODE_VERSION (for React dashboard)"
else
    warn "Node.js not found — React dashboard won't build (optional)"
fi

echo ""

# ============================================================================
# Step 2: Build aida binaries
# ============================================================================

info "Building aida binaries..."

# Check if already built
if [ -f "$PROJECT_ROOT/target/debug/aida" ] && [ -f "$PROJECT_ROOT/target/debug/aida-server" ]; then
    # Check if source is newer than binary
    NEWEST_SRC=$(find aida-core/src aida-cli/src aida-server/src -name "*.rs" -newer target/debug/aida 2>/dev/null | head -1)
    if [ -z "$NEWEST_SRC" ]; then
        ok "Binaries up to date"
    else
        info "Source changed — rebuilding..."
        cargo build -p aida-cli -p aida-server 2>&1 | tail -1
        ok "Built aida and aida-server"
    fi
else
    cargo build -p aida-cli -p aida-server 2>&1 | tail -1
    ok "Built aida and aida-server"
fi

echo ""

# ============================================================================
# Step 3: Set up aida-store worktree
# ============================================================================

info "Setting up requirements store..."

STORE_DIR=".aida-store"
STORE_BRANCH="aida-store"

# Check if the orphan branch exists (from the remote)
BRANCH_EXISTS=$(git branch -a 2>/dev/null | grep -c "$STORE_BRANCH" || true)

if [ -d "$STORE_DIR" ]; then
    # Worktree already exists
    REQ_COUNT=$(./target/debug/aida list 2>/dev/null | tail -1 | grep -oP '\d+' || echo "0")
    ok "Store worktree exists at $STORE_DIR ($REQ_COUNT requirements)"
elif [ "$BRANCH_EXISTS" -gt 0 ]; then
    # Branch exists but worktree not set up
    info "Setting up worktree from $STORE_BRANCH branch..."
    git worktree add "$STORE_DIR" "$STORE_BRANCH" 2>/dev/null
    ok "Worktree created at $STORE_DIR"
else
    # No branch exists — this is a fresh project, initialize
    warn "No $STORE_BRANCH branch found"
    echo "    To initialize distributed mode: aida init --distributed"
    echo "    Or use centralized mode: aida init"
fi

# Verify .aida/config.toml exists
if [ ! -f ".aida/config.toml" ]; then
    if [ -d "$STORE_DIR" ]; then
        mkdir -p .aida
        cat > .aida/config.toml << TOML
# AIDA distributed mode configuration
[deployment]
mode = "distributed"
store_path = "$STORE_DIR"
store_type = "worktree"
branch = "$STORE_BRANCH"
TOML
        ok "Created .aida/config.toml"
    fi
else
    ok ".aida/config.toml exists"
fi

echo ""

# ============================================================================
# Step 4: Install to PATH (optional)
# ============================================================================

INSTALL_DIR="$HOME/.local/bin"
AIDA_BIN="$INSTALL_DIR/aida"
AIDA_SERVER_BIN="$INSTALL_DIR/aida-server"

if [ -f "$AIDA_BIN" ]; then
    INSTALLED_VERSION=$("$AIDA_BIN" --version 2>/dev/null || echo "unknown")
    ok "aida installed at $AIDA_BIN ($INSTALLED_VERSION)"
else
    if echo "$PATH" | grep -q "$INSTALL_DIR"; then
        info "Installing aida to $INSTALL_DIR..."
        mkdir -p "$INSTALL_DIR"
        cp "$PROJECT_ROOT/target/debug/aida" "$AIDA_BIN"
        cp "$PROJECT_ROOT/target/debug/aida-server" "$AIDA_SERVER_BIN"
        ok "Installed aida and aida-server to $INSTALL_DIR"
    else
        warn "aida not in PATH"
        echo "    Option 1: Add to PATH:"
        echo "      mkdir -p $INSTALL_DIR"
        echo "      cp target/debug/aida $INSTALL_DIR/"
        echo "      cp target/debug/aida-server $INSTALL_DIR/"
        echo "      export PATH=\"$INSTALL_DIR:\$PATH\"  # add to ~/.bashrc"
        echo ""
        echo "    Option 2: Use directly:"
        echo "      ./target/debug/aida list"
    fi
fi

echo ""

# ============================================================================
# Step 5: Verify
# ============================================================================

info "Verifying setup..."

AIDA_CMD="./target/debug/aida"
if command -v aida &>/dev/null; then
    AIDA_CMD="aida"
fi

if $AIDA_CMD list &>/dev/null; then
    REQ_COUNT=$($AIDA_CMD list 2>/dev/null | tail -1 | grep -oP '\d+' || echo "?")
    ok "aida list works ($REQ_COUNT requirements)"
else
    if [ -d "$STORE_DIR" ]; then
        warn "aida list failed — store may need initialization"
    else
        warn "No store configured yet"
    fi
fi

echo ""
echo -e "${BOLD}Setup complete${NC}"
echo ""
echo "  Quick start:"
echo -e "    ${CYAN}aida list${NC}                              List requirements"
echo -e "    ${CYAN}aida show FR-3${NC}                         Show by agreed ID"
echo -e "    ${CYAN}aida add --title \"...\" --type functional${NC}  Add requirement"
echo -e "    ${CYAN}aida db status${NC}                         Store status"
echo -e "    ${CYAN}git push origin aida-store${NC}             Sync store to remote"
echo ""
echo "  For more: docs/storage-modes.md"
echo ""
