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
HOOKS_DIR="$(git rev-parse --git-path hooks)"

if git config --get core.hooksPath >/dev/null; then
    echo "ERROR: resolve the configured core.hooksPath before installing Lefthook." >&2
    exit 1
fi

# Git shares hooks between worktrees. The old setup could have run in any of them.
allowed_worktrees=()
while IFS= read -r -d '' entry; do
    if [[ "$entry" == 'worktree '* ]]; then
        allowed_worktrees+=("${entry#worktree }")
    fi
done < <(git worktree list --porcelain -z)

# Lefthook follows hook symlinks. Remove only links created by the old setup.
legacy_hooks=()
hook_paths=(commit-msg pre-commit pre-push commit-msg.old pre-commit.old pre-push.old)
for hook in "${hook_paths[@]}"; do
    hook_path="$HOOKS_DIR/$hook"
    if [ -L "$hook_path" ]; then
        target="$(readlink "$hook_path")"
        for worktree in ${allowed_worktrees[@]+"${allowed_worktrees[@]}"}; do
            case "$hook:$target" in
                "commit-msg:$worktree/scripts/commit-msg-regression.sh" | \
                "pre-commit:$worktree/scripts/pre-commit-safety.sh" | \
                "pre-push:$worktree/.githooks/pre-push")
                    legacy_hooks+=("$hook")
                    continue 2
                    ;;
            esac
        done
        echo "ERROR: resolve the existing hook symlink at $hook_path before installing Lefthook." >&2
        exit 1
    elif [ -e "$hook_path" ] && [ ! -f "$hook_path" ]; then
        echo "ERROR: resolve the existing hook path at $hook_path before installing Lefthook." >&2
        exit 1
    elif [ -f "$hook_path" ] && [[ "$hook" != *.old ]] &&
        ! grep -Fxq "call_lefthook run \"$hook\" \"\$@\"" "$hook_path"; then
        echo "ERROR: migrate the existing custom hook at $hook_path before installing Lefthook." >&2
        exit 1
    fi
done

if [[ -n "${legacy_hooks[0]-}" ]]; then
    for worktree in "${allowed_worktrees[@]}"; do
        if [ ! -f "$worktree/lefthook.yml" ]; then
            echo "ERROR: worktree has no lefthook.yml at $worktree. Update it or use a separate clone before migrating shared hooks." >&2
            exit 1
        fi
    done
fi

(
    mkdir -p -- "$HOOKS_DIR"
    backup_dir="$(mktemp -d "$HOOKS_DIR/.lefthook-migration.XXXXXX")"
    install_started=false
    restore_hooks() {
        local status=$? hook
        for hook in "${hook_paths[@]}"; do
            if [ "$status" -ne 0 ] && $install_started; then
                if ! rm -f -- "$HOOKS_DIR/$hook"; then
                    echo "ERROR: recover $hook from $backup_dir." >&2
                    continue
                fi
                if [ -e "$backup_dir/$hook" ] || [ -L "$backup_dir/$hook" ]; then
                    if ! mv -- "$backup_dir/$hook" "$HOOKS_DIR/"; then
                        echo "ERROR: recover $hook from $backup_dir." >&2
                    fi
                fi
            else
                rm -f -- "$backup_dir/$hook"
            fi
        done
        rmdir -- "$backup_dir"
    }
    trap restore_hooks EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    for hook in "${hook_paths[@]}"; do
        if [ -e "$HOOKS_DIR/$hook" ] || [ -L "$HOOKS_DIR/$hook" ]; then
            cp -a -- "$HOOKS_DIR/$hook" "$backup_dir/"
        fi
    done
    install_started=true
    # Bash 3.2 treats an empty array as unset under set -u.
    for hook in ${legacy_hooks[@]+"${legacy_hooks[@]}"}; do
        rm -- "$HOOKS_DIR/$hook"
    done
    # Keep local Lefthook additions outside this project's rollback transaction.
    lefthook install commit-msg pre-commit pre-push
)

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
