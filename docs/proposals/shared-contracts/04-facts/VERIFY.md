# Static package verification

From repository root:

```sh
(cd docs/proposals/shared-contracts/04-facts && sha256sum --check --strict SHA256SUMS)
git diff --check 84fc76a662c57ccad9aa06c8b59bbdf8b41e1ec4...HEAD -- docs/proposals/shared-contracts/04-facts/README.md docs/proposals/shared-contracts/04-facts/PROVENANCE.json docs/proposals/shared-contracts/04-facts/VERIFY.md docs/proposals/shared-contracts/04-facts/SHA256SUMS
git diff --name-only 84fc76a662c57ccad9aa06c8b59bbdf8b41e1ec4...HEAD
```

Only this packet directory may appear in the diff. The whitespace command checks
new review files separately: historical sources are preserved byte-for-byte,
including original whitespace. The unrestricted diff check can report their
pre-existing trailing whitespace/blank EOF lines; exact outcomes are in the PR.
PROVENANCE.json records original source hashes and paths. Copied bytes were compared
with those sources by the helm. A GitHub-only reader can verify included bytes,
not external custody, unavailable historical dependencies or authority. README
links resolve inside this packet; links inside preserved sources may require the
original workspace. No historical command was executed. Product/runtime tests and
the full product gate were not run for documentary packaging. Independent review
is recorded in the PR after it returns. No acceptance or implementation follows.
