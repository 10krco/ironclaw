# Common checks

Use the checked-in toolchain and lockfiles. Common workspace checks:
- `cargo fmt --all -- --check`
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
- `cargo test --locked --workspace`
- `pnpm --dir crates/product/ironclaw_webui/frontend test`

The fast local push gate is `./.githooks/pre-push origin --fast`. The deterministic end-to-end suite is `scripts/ci/run-hermetic-deterministic-suite.sh all`; consult its documentation for Docker/Postgres, Emulate, Python E2E, and Chromium prerequisites. The root `integration` feature has no consumers, so `cargo test --features integration` does not select the backend-heavy integration suites.

These commands can change as CI evolves: verify `.development-system.toml`, root `AGENTS.md`, and the relevant crate/test guidance before choosing a gate. See `mem:task_completion` and `mem:frontend/core`.