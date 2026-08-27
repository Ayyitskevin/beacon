---
status: accepted
---

# Use a Rust supervisor behind the Tauri desktop

Beacon uses a Rust workspace, a local supervisor that solely owns SQLite and execution authority,
and a Tauri v2 desktop with a TypeScript/React interface. Desktop, MCP, and recovery CLI adapters
all use the same typed command, query, and event interfaces. The separate supervisor adds process
complexity, but agent Teams and long-running Runs cannot safely depend on a webview window owning
their database or lifetime.

## Consequences

SQLite uses plain SQL and Rust-owned connections. The desktop and Agents receive no raw database
handle. Linux is the first supported platform. The Python prototype is not converted in place; it
is preserved while a new Rust trunk is established.
