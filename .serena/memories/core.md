# Project core

IronClaw Reborn is the production stack in the single Cargo workspace. The shipping binary is package `ironclaw` in `crates/app/ironclaw_cli`; the workspace root hosts integration tests. Product code belongs under `crates/`.

Route every change through root `AGENTS.md`, then `crates/AGENTS.md`, which selects one of ten crate families. Read the owning family/crate guide and module `CONTRACT.md` before changing a complex area; contracts settle cross-crate behavior. See `mem:backend/core` and `mem:frontend/core` for subsystem ownership, `mem:tech_stack` for toolchain sources, `mem:conventions` for boundaries, `mem:suggested_commands` for checks, and `mem:task_completion` for delivery discipline.

Current fork direction: a private whitelabel server deployed on Linux/Fly.io, initially supporting direct OpenAI API-key calls. Recheck with the user or current task before broadening providers or release platforms.