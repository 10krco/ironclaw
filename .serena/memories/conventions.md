# Repository conventions and boundaries

Root `AGENTS.md` is the canonical agent contract; `crates/AGENTS.md` routes production work to a family, then the owning crate guide and module spec. Cross-crate contracts live under `docs/internal/reborn/contracts/`. Do not hand-edit generated `openwiki/` content.

Keep ownership boundaries intact: neutral contracts carry shared vocabulary; substrates own privileged mechanisms; domains own typed records/services; kernel crates mediate authority; lanes execute authorized work; loop crates own agent strategy; extensions own installable integration surfaces; product crates orchestrate user-facing behavior; app/composition assembles dependencies without policy. WebUI owns HTTP/transport; transports do not invent durable state.

Use strong types for domain shapes, contextual error propagation, and no `.unwrap()`/`.expect()` in production. I/O is async; shared state uses `Arc`. Prompts belong in owner-crate files loaded with `include_str!`. Do not weaken trust, auth, approval, secret, or redaction boundaries. See `mem:backend/core`, `mem:frontend/core`, and `mem:task_completion`.