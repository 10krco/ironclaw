# Technology and version sources

The backend is a Rust Cargo workspace. `rust-toolchain.toml` pins the local toolchain; `.github/actions/setup-rust/action.yml` must stay synchronized with its default, enforced by `scripts/ci/ws12_workflow_contracts.py`. The workspace MSRV is declared in root `Cargo.toml`; do not treat it as the installed toolchain pin.

The WebUI includes a Rust server crate and a TypeScript/React SPA built with Vite. The frontend's `package.json` and lockfile own Node/pnpm engine ranges and exact dependency versions; consult them instead of copying version pins into other files. See `mem:frontend/core`.

Composition selects storage backends; persistence and filesystem ownership are defined by the owning contracts, not by frontend or transport code. See `mem:backend/core` and `mem:conventions` for architecture boundaries.