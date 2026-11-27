# WebUI frontend ownership

The browser SPA is `crates/product/ironclaw_webui/frontend/`; it uses TypeScript/React and Vite. Its `package.json` and lockfile are the source of truth for frontend scripts and tool versions. Vite output is embedded by the Rust WebUI crate when its relevant build feature is enabled.

The owning Rust crate `crates/product/ironclaw_webui/` owns HTTP routes, authentication, transport, and server-side contracts. Read its `CONTRACT.md` before changing route, streaming, or auth behavior; route descriptors and architecture tests pin these surfaces. The frontend owns presentation and browser interaction. It consumes product APIs/events and must not bypass backend authorization or mutate durable stores directly.

Preserve accessible keyboard behavior and user-visible state while changing components. Use the frontend package scripts and browser test guidance for UI changes; see `mem:suggested_commands`, `mem:backend/core`, and `mem:conventions`.