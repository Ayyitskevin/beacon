# Local-model activation evaluation — 2026-08-28

Scope: generated Ollama-shaped loopback servers, the public activation service, the authenticated
Unix transport, schema-v7 persistence, the Tauri adapter, strict TypeScript parsing, and accessible
desktop interactions. Fixtures exercise the trust boundary without invoking a model.

Commands:

```sh
cargo test -p beacond --test local_model_activation
cargo test -p beacond --test local_transport authenticated_client_reviews_activates_and_deactivates_an_exact_local_model -- --exact
cargo test --workspace
cd apps/desktop && npm test && npm run build
scripts/gate all
```

| Case | Expected behavior | Observed |
|---|---|---|
| Locked Owner inspects a model | Refused before provider access | Pass |
| Exact tags/show/tags sequence | Closed review with canonical inspection digest | Pass |
| Unknown provider feature | Visible in review; absent from approvable enum | Pass |
| Digest changes during inspection | Review withheld as model drift | Pass |
| Activation reinspection | Original review must still match exactly | Pass |
| Selected feature subset | Nonempty, unique, advertised values only | Pass |
| Concurrent activation | Exactly one active decision; one `already_active` | Pass |
| Decision persistence | Decision and exact Workspace Event commit atomically | Pass |
| Ledger update or delete | Rejected by append-only database triggers | Pass |
| Exact-id deactivation | Appends inactive successor | Pass |
| Repeated/stale deactivation | Refused without another decision | Pass |
| Provider offline during deactivation | Deactivation still succeeds | Pass |
| Desktop with provider offline | Stored active decision remains visible and revocable | Pass |
| Real authenticated Unix lifecycle | Inspect, activate, list, deactivate all pass | Pass |
| Strict TypeScript adapter | Widened, remote, unsorted, and invalid-history data refused | Pass |
| Desktop review interaction | Explicit selection and revocation; no Route/run claim | Pass |
| Existing v1–v6 stores | Transactionally migrate to verified schema v7 | Pass |

Final whole-repository gate and independent review are recorded in the task handoff and hosted CI
for the exact reviewed commit.
