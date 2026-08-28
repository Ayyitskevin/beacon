# Phase 0 local-model activation

This checkpoint lets an authenticated Owner make one exact installed Ollama model eligible for
later Route binding in one Workspace. It preserves the product boundary between evidence,
eligibility, authority, and execution.

## Domain boundary

Ollama's `capabilities` array is provider evidence. Beacon names each recognized item a
**Provider Feature**; it is not a Beacon Capability and grants no Agent permission. A **Resource
Activation** is the Owner's revocable, Workspace-scoped decision that an exact model and selected
Provider Feature subset may be considered by a later Route.

Activation creates no Route, Agent binding, Invocation, model execution, credential, budget, data
scope, network expansion, or execution authority. Unknown Provider Features stay visible but are
not approvable. Deactivation revokes eligibility without contacting or running the provider.

## Exact review and fail-closed activation

The review sequence is `GET /api/tags` → `POST /api/show` → `GET /api/tags`. The two tag reads
must contain the requested model name and the same expected lowercase digest. Beacon normalizes
recognized and unknown Provider Features, then computes a canonical SHA-256 inspection digest
over the provider, loopback endpoint, model name, model digest, and both feature sets.

Activation repeats the complete inspection and requires byte-equivalent normalized review data
before writing. A changed digest, changed feature set, missing model, unrecognized approval,
duplicate approval, empty approval, stale deactivation id, or already-active resource is refused.
The HTTP client disables proxies and redirects, accepts at most 1 MiB, and uses a two-second bound
for each provider request. The authenticated Unix client allows seven seconds for the complete
three-request inspection.

Ollama documents `model` as the required `POST /api/show` request field and `capabilities` in the
response: [Show model details](https://docs.ollama.com/api-reference/show-model-details). Exact
installed identity continues to come from [List models](https://docs.ollama.com/api/tags).

## Persistence and reconciliation

Schema version 7 adds `local_model_activation_decisions`. Each activation or deactivation appends
one decision and one exact Workspace-scoped Event in the same immediate SQLite transaction.
Database triggers reject updates and deletes from the decision ledger. Each later decision names
the preceding decision for the same provider, endpoint, and model name.

Startup refuses to report ready unless every decision:

- has a valid Workspace, exact known provider, loopback binding data, and normalized feature sets;
- recomputes to its stored inspection digest;
- begins active, alternates active/inactive, and names the exact predecessor;
- has nondecreasing decision time and exactly one matching activation/deactivation Event.

The immediate transaction plus current-state check gives concurrent activation one winner. The
loser receives `already_active`; neither path silently overwrites the first decision.

## Interfaces and presentation

The same closed review and decision shapes cross the public Rust service, authenticated Unix
socket, Tauri commands, and strict TypeScript adapter. The browser adapter revalidates exact keys,
UUIDs, loopback endpoints, digests, feature ordering, approved subsets, state/predecessor rules,
and current-resource ordering. Unexpected authority-shaped fields are refused.

Briefing keeps discovery compact. **Review** opens the advertised features and immutable evidence;
recognized features are individually selected, unknown features are named as unapprovable, and
the no-Route/no-run boundary stays visible. Active resources expose **Deactivate** without hiding
technical details. A changed installed digest is shown as an explicit warning.

## Verification

From the repository root:

```sh
cargo test -p beacond --test local_model_activation
cargo test -p beacond --test local_transport authenticated_client_reviews_activates_and_deactivates_an_exact_local_model -- --exact
cargo test --workspace
cd apps/desktop && npm test && npm run build
scripts/gate all
```

The endpoint, concurrency, persistence, and adapter cases are recorded in
[`docs/evals/2026-08-28-local-model-activation.md`](../evals/2026-08-28-local-model-activation.md).
