# Phase 0 local-model discovery

This checkpoint gives an authenticated Owner a truthful view of installed local Ollama models
without turning discovery into activation or authority. It is the prerequisite for the later
resource-activation decision that must precede Team formation and Mission admission.

## Boundary

`discover_local_models()` is an ephemeral read. It persists nothing, invokes no model, changes no
Resident, assigns no capability, selects no Route, starts no work, and grants no execution
authority. Every returned model is explicitly `inactive`.

The production endpoint is the literal loopback socket `127.0.0.1:11434`. Construction rejects
non-loopback addresses. The HTTP client disables proxies and redirects, applies a two-second global
timeout, accepts at most 1 MiB, and accepts at most 512 models. A network or non-200 result becomes
`unavailable`; an unparseable or structurally invalid 200 response becomes `invalid_response`.
Neither state is silently replaced with another provider.

The account session is checked before and after the request. The account-store mutex is not held
while waiting on HTTP, so a local provider cannot stall session management. If the session closes
during discovery, the result is withheld.

## Upstream contract

Ollama documents installed-model discovery as `GET /api/tags`, including model identity, modified
time, size, digest, and format/family/parameter/quantization details:
[List models](https://docs.ollama.com/api/tags). Unknown upstream fields are ignored so additive
Ollama changes do not break the read, while every field Beacon displays is bounded and validated.

Capabilities are deliberately absent. Ollama documents them on the separate `POST /api/show`
response, not the tags contract: [Show model details](https://docs.ollama.com/api-reference/show-model-details).
Beacon will inspect capabilities only in the later activation flow, where the Operator can review
the exact proposed resource binding.

The HTTP client is pinned to `ureq` 3.4.0 without default features. Its documented configuration
surface supplies explicit proxy, redirect, and global-timeout controls:
[ureq ConfigBuilder](https://docs.rs/ureq/3.4.0/ureq/config/struct.ConfigBuilder.html).

## Interfaces and presentation

The same closed report crosses the public Rust service, authenticated Unix socket, Tauri command,
and strict TypeScript adapter. The browser-side adapter again requires loopback, safe integer
measurements, sorted distinct names, lowercase 64-character digests, bounded text, known status,
Ollama provider, and inactive activation. Unexpected fields or states are refused.

Briefing exposes one explicit **Check local models** action. Results stay compact: availability,
model name, parameter size, quantization, size, and inactive state. Endpoint, digest, family,
format, and modified time remain available through deliberate technical drill-down. Unavailable
Ollama never blocks the rest of Beacon.

## Verification

From the repository root:

```sh
cargo test -p beacond --test resource_discovery
cargo test -p beacond --test local_transport authenticated_client_discovers_inactive_local_models_over_both_real_transports -- --exact
cargo test --workspace
cd apps/desktop && npm test && npm run build
scripts/gate all
```

The response and transport cases are recorded in
[`docs/evals/2026-08-27-local-model-discovery-endpoint.md`](../evals/2026-08-27-local-model-discovery-endpoint.md).
