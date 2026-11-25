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
(
    HOOKS_DIR="$(git rev-parse --git-path hooks)"

    if git config --get core.hooksPath >/dev/null; then
        echo "ERROR: resolve the configured core.hooksPath before installing Lefthook." >&2
        exit 1
    fi

    # Linked worktrees share this lock. Take it before inspecting any hook.
    mkdir -p -- "$HOOKS_DIR"
    hook_lock="$HOOKS_DIR/.ironclaw-lefthook-install.lock"
    if ! mkdir -- "$hook_lock"; then
        if [ -d "$hook_lock" ]; then
            echo "ERROR: hook migration is already in progress at $hook_lock. If setup was interrupted, verify it has stopped before removing the lock." >&2
        fi
        exit 1
    fi
    worktree_inventory="$hook_lock/worktrees"
    trap 'rm -f -- "$worktree_inventory"; rmdir -- "$hook_lock"' EXIT
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM

    # Git shares hooks between worktrees. The old setup could have run in any of them.
    if ! git worktree list --porcelain -z > "$worktree_inventory"; then
        echo "ERROR: could not list Git worktrees before installing shared hooks." >&2
        exit 1
    fi
    allowed_worktrees=()
    worktree=''
    while IFS= read -r -d '' entry; do
        case "$entry" in
            'worktree '*) worktree="${entry#worktree }" ;;
            bare) worktree='' ;;
            '')
                if [[ -n "$worktree" ]]; then
                    allowed_worktrees+=("$worktree")
                    worktree=''
                fi
                ;;
        esac
    done < "$worktree_inventory"
    if [[ -z "${allowed_worktrees[0]-}" ]]; then
        echo "ERROR: Git reported no checkouts before installing shared hooks." >&2
        exit 1
    fi

    launcher_dispatches_hook() {
        grep -Fxq "call_lefthook run \"$2\" \"\$@\"" "$1"
    }

    compatible_launcher() {
        grep -Fxq '[ -f ./.lefthookrc ] && . ./.lefthookrc' "$1" &&
            grep -Fq 'ERROR: Operation is aborted due to lefthook settings.' "$1"
    }

    # Lefthook follows hook symlinks. Remove only links created by the old setup.
    legacy_hooks=()
    hooks_to_install=()
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
                        hooks_to_install+=("$hook")
                        continue 2
                        ;;
                esac
            done
            echo "ERROR: resolve the existing hook symlink at $hook_path before installing Lefthook." >&2
            exit 1
        elif [ -e "$hook_path" ] && [ ! -f "$hook_path" ]; then
            echo "ERROR: resolve the existing hook path at $hook_path before installing Lefthook." >&2
            exit 1
        elif [[ "$hook" != *.old ]]; then
            if [ ! -e "$hook_path" ]; then
                hooks_to_install+=("$hook")
            elif ! launcher_dispatches_hook "$hook_path" "$hook"; then
                echo "ERROR: migrate the existing custom hook at $hook_path before installing Lefthook." >&2
                exit 1
            elif [ ! -x "$hook_path" ]; then
                echo "ERROR: installed hook is not executable at $hook_path. Restore its execute permission before running setup." >&2
                exit 1
            elif ! compatible_launcher "$hook_path"; then
                echo "ERROR: existing Lefthook launcher is incompatible at $hook_path. Back it up, remove it, and rerun setup." >&2
                exit 1
            fi
        fi
    done

    if [[ -n "${hooks_to_install[0]-}" ]]; then
        for worktree in "${allowed_worktrees[@]}"; do
            if [ ! -d "$worktree" ]; then
                echo "ERROR: registered worktree is unavailable at $worktree. Restore its path, run 'git worktree repair' if it moved, or inspect 'git worktree prune --dry-run' if it was removed." >&2
                exit 1
            fi
            if [ ! -f "$worktree/lefthook.yml" ]; then
                echo "ERROR: worktree has no lefthook.yml at $worktree. Update it or use a separate clone before migrating shared hooks." >&2
                exit 1
            fi
            if ! cmp -s lefthook.yml "$worktree/lefthook.yml" ||
                [ ! -f "$worktree/.lefthookrc" ] ||
                ! cmp -s .lefthookrc "$worktree/.lefthookrc"; then
                echo "ERROR: incompatible hook configuration at $worktree. Update it or use a separate clone before migrating shared hooks." >&2
                exit 1
            fi
        done
    fi

    (
        # Preserve installed launchers, including any local edits to them.
        [[ -n "${hooks_to_install[0]-}" ]] || exit 0
        staging_dir="$(mktemp -d "$HOOKS_DIR/.lefthook-migration.XXXXXX")"
        publishing_hooks=0
        cleanup_hook_install() {
            local status="$1" hook restore_failed=0
            if (( status != 0 && publishing_hooks )); then
                for hook in "${hooks_to_install[@]}"; do
                    # A launcher still in staging never replaced the live path.
                    [ ! -e "$staging_dir/$hook" ] || continue
                    if [ -L "$staging_dir/original/$hook" ]; then
                        mv -f -- "$staging_dir/original/$hook" "$HOOKS_DIR/" || restore_failed=1
                    else
                        rm -f -- "$HOOKS_DIR/$hook" || restore_failed=1
                    fi
                done
            fi
            if (( restore_failed )); then
                echo "ERROR: could not restore all hooks. Inspect the backups at $staging_dir/original before retrying setup." >&2
            else
                rm -rf -- "$staging_dir"
            fi
        }
        trap 'cleanup_hook_install "$?"' EXIT
        trap 'exit 129' HUP
        trap 'exit 130' INT
        trap 'exit 143' TERM

        # Override the hook destination for this installer process only.
        # The staging directory shares a filesystem with the live hooks.
        config_count="${GIT_CONFIG_COUNT:-0}"
        if [[ "$config_count" =~ ^[[:space:]]*\+?([0-9]+)$ ||
              "$config_count" =~ ^[[:space:]]*-(0+)$ ]]; then
            config_count="${BASH_REMATCH[1]}"
        else
            echo "ERROR: GIT_CONFIG_COUNT must be a nonnegative integer." >&2
            exit 1
        fi
        config_count="$((10#$config_count))"
        env GIT_CONFIG_COUNT="$((config_count + 1))" \
            "GIT_CONFIG_KEY_${config_count}=core.hooksPath" \
            "GIT_CONFIG_VALUE_${config_count}=$staging_dir" \
            lefthook install --force "${hooks_to_install[@]}"
        for hook in "${hooks_to_install[@]}"; do
            launcher="$staging_dir/$hook"
            if [ -L "$launcher" ] || [ ! -f "$launcher" ] || [ ! -x "$launcher" ]; then
                echo "ERROR: Lefthook did not generate an executable launcher for $hook." >&2
                exit 1
            fi
            if ! compatible_launcher "$launcher" || ! launcher_dispatches_hook "$launcher" "$hook"; then
                echo "ERROR: generated an incompatible Lefthook launcher for $hook. Verify hook dispatch and local overrides for rc and assert_lefthook_installed." >&2
                exit 1
            fi
        done
        # Keep the previous links until every live hook has been published.
        mkdir -- "$staging_dir/original"
        for hook in ${legacy_hooks[@]+"${legacy_hooks[@]}"}; do
            cp -P -- "$HOOKS_DIR/$hook" "$staging_dir/original/"
        done
        publishing_hooks=1
        for hook in "${hooks_to_install[@]}"; do
            mv -f -- "$staging_dir/$hook" "$HOOKS_DIR/"
        done
    )
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
