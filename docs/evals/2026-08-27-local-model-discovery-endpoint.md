# Local-model discovery endpoint evaluation — 2026-08-27

Scope: real HTTP requests to an Ollama-shaped loopback server, plus the public
`LocalSupervisorClient` crossing the authenticated Unix-domain socket into `beacond`. Generated
fixtures exercise the trust boundary; a separate read-only measurement checks the installed local
Ollama service. Frontend mocks are not counted as endpoint evidence.

Commands:

```sh
cargo test -p beacond --test resource_discovery
cargo test -p beacond --test local_transport authenticated_client_discovers_inactive_local_models_over_both_real_transports -- --exact
curl -fsS http://127.0.0.1:11434/api/tags | jq '{count: (.models | length), all_names_match: ([.models[] | .name == .model] | all), all_sizes_positive: ([.models[] | .size > 0] | all), all_digests_lower_hex_64: ([.models[] | (.digest | test("^[0-9a-f]{64}$"))] | all), all_required_details_present: ([.models[] | ([.details.format, .details.family, .details.parameter_size, .details.quantization_level] | all(. != null and . != ""))] | all)}'
```

| Case | Expected behavior | Observed |
|---|---|---|
| Non-loopback endpoint construction | Refused before HTTP | Pass |
| Locked session with unreachable endpoint | `not_authenticated` before HTTP | Pass |
| Session locks while HTTP is in flight | Account lock remains responsive; result withheld | Pass |
| Valid documented tags response | Available, exact normalized model | Pass |
| Empty model list | Available with no models | Pass |
| Out-of-order model list | Stable name ordering | Pass |
| Two tags sharing one digest | Both retained | Pass |
| Unknown top-level and model fields | Ignored | Pass |
| Non-JSON body | Invalid response, no partial models | Pass |
| Missing `models` | Invalid response, no partial models | Pass |
| Null `models` | Invalid response, no partial models | Pass |
| Duplicate name | Invalid response, no partial models | Pass |
| Different `name` and `model` | Invalid response, no partial models | Pass |
| Blank name | Invalid response, no partial models | Pass |
| Padded name | Invalid response, no partial models | Pass |
| Control character in name | Invalid response, no partial models | Pass |
| Name over 256 characters | Invalid response, no partial models | Pass |
| Zero size | Invalid response, no partial models | Pass |
| Short digest | Invalid response, no partial models | Pass |
| Uppercase digest | Invalid response, no partial models | Pass |
| Missing details | Invalid response, no partial models | Pass |
| Blank format | Invalid response, no partial models | Pass |
| Blank family | Invalid response, no partial models | Pass |
| Blank parameter size | Invalid response, no partial models | Pass |
| Blank quantization | Invalid response, no partial models | Pass |
| Blank modified time | Invalid response, no partial models | Pass |
| 513 models | Invalid response, no partial models | Pass |
| HTTP 302 | Unavailable; redirect not followed | Pass |
| HTTP 401 | Unavailable | Pass |
| HTTP 500 | Unavailable | Pass |
| Body over 1 MiB | Invalid response | Pass |
| Closed loopback port | Unavailable | Pass |
| Real socket before and after manual lock | `not_authenticated` | Pass |
| Real socket while unlocked | Available and every model inactive | Pass |

Result: 34/34 generated endpoint expectations passed. The live installed-service measurement
returned 11 models with matching `name`/`model` identities, positive sizes, lowercase 64-character
digests, and every required detail present. That measurement is evidence about this workstation,
not a product fixture or a supported-model allowlist.
