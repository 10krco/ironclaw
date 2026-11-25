#!/usr/bin/env bash
# Developer setup script for IronClaw.
#
# Gets a fresh checkout ready for development without requiring
# Docker, PostgreSQL, or any external services.
#
# Usage:
#   ./scripts/dev-setup.sh
#
# After running, you can:
#   cargo check           # default build
#   cargo test            # default test suite
#   cargo test --all-features         # full test suite (requires Node.js 22 + Corepack/pnpm for WebUI bundle)

set -euo pipefail

cd "$(dirname "$0")/.."

echo "=== IronClaw Developer Setup ==="
echo ""

# 1. Check rustup and Lefthook
if ! command -v lefthook &>/dev/null; then
    echo "ERROR: lefthook not found. Install it from https://lefthook.dev/installation/" >&2
    exit 1
fi
if ! command -v rustup &>/dev/null; then
    echo "ERROR: rustup not found. Install from https://rustup.rs"
    exit 1
fi
echo "[1/6] rustup found: $(rustup --version 2>/dev/null | head -1)"

# 2. Add WASM target (required by build.rs for channel compilation)
echo "[2/6] Adding wasm32-wasip2 target..."
rustup target add wasm32-wasip2

# 3. Install wasm-tools (required by build.rs for WASM component model)
echo "[3/6] Installing wasm-tools..."
if command -v wasm-tools &>/dev/null; then
    echo "  wasm-tools already installed: $(wasm-tools --version)"
else
    cargo install wasm-tools --locked
fi

# 4. Verify the project compiles
echo "[4/6] Running cargo check..."
cargo check

# 5. Run tests using libsql temp DB (no Docker/external DB needed)
echo "[5/6] Running tests (no external DB required)..."
cargo test

# 6. Install git hooks
echo "[6/6] Installing git hooks..."
REPO_ROOT="$(git rev-parse --show-toplevel)"
HOOKS_DIR="$(git rev-parse --git-path hooks)"

if git config --get core.hooksPath >/dev/null; then
    echo "ERROR: resolve the configured core.hooksPath before installing Lefthook." >&2
    exit 1
fi

# Lefthook follows symlinks while writing hooks. Temporarily move only the
# links created by the previous setup script and restore them on failure.
legacy_hooks=()
for hook in commit-msg pre-commit pre-push; do
    hook_path="$HOOKS_DIR/$hook"
    if [ -L "$hook_path" ]; then
        case "$hook:$(readlink "$hook_path")" in
            "commit-msg:$REPO_ROOT/scripts/commit-msg-regression.sh" | \
            "pre-commit:$REPO_ROOT/scripts/pre-commit-safety.sh" | \
            "pre-push:$REPO_ROOT/.githooks/pre-push")
                legacy_hooks+=("$hook")
                ;;
            *)
                echo "ERROR: resolve the existing hook symlink at $hook_path before installing Lefthook." >&2
                exit 1
                ;;
        esac
    fi
done

if [ "${#legacy_hooks[@]}" -eq 0 ]; then
    lefthook install
else
    (
        backup_dir="$(mktemp -d "$HOOKS_DIR/.lefthook-migration.XXXXXX")"
        restore_hooks() {
            local status=$? hook
            if [ "$status" -ne 0 ]; then
                for hook in "${legacy_hooks[@]}"; do
                    if [ -L "$backup_dir/$hook" ]; then
                        if ! { rm -f -- "$HOOKS_DIR/$hook" && mv -- "$backup_dir/$hook" "$HOOKS_DIR/"; }; then
                            echo "ERROR: recover $hook from $backup_dir." >&2
                        fi
                    fi
                done
            else
                for hook in "${legacy_hooks[@]}"; do
                    rm -- "$backup_dir/$hook"
                done
            fi
            rmdir -- "$backup_dir"
        }
        trap restore_hooks EXIT
        trap 'exit 130' INT
        trap 'exit 143' TERM
        for hook in "${legacy_hooks[@]}"; do
            mv -- "$HOOKS_DIR/$hook" "$backup_dir/$hook"
        done
        lefthook install
    )
fi

echo ""
# Codebase knowledge graph (codebase-memory MCP) — powers agent code discovery.
# Single static binary, no deps, no API keys, 100% local. See CLAUDE.md -> "Code Discovery".
if command -v codebase-memory-mcp &>/dev/null; then
    echo "[graph] codebase-memory-mcp found: $(command -v codebase-memory-mcp)"
else
    echo "[graph] Installing codebase-memory-mcp (agent code-discovery graph)..."
    if curl -fsSL https://raw.githubusercontent.com/DeusData/codebase-memory-mcp/main/install.sh | bash; then
        echo "  installed. The repo's .mcp.json wires it into Claude Code automatically."
    else
        echo "  WARN: install failed — agents will fall back to grep."
        echo "        Install manually: https://github.com/DeusData/codebase-memory-mcp"
    fi
fi

echo ""
echo "=== Setup complete ==="
echo ""
echo "Quick start:"
echo "  cargo run                            # Run the default build"
echo "  cargo test                           # Test suite"
echo "  cargo test --all-features            # Full test suite (requires Node.js 22 + Corepack/pnpm for WebUI bundle)"
echo "  cargo clippy --all-features          # Lint all code (requires Node.js 22 + Corepack/pnpm for WebUI bundle)"
