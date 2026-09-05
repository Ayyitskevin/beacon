---
renderer_version: beacon.outcome.md.v1
canonicalizer_version: beacon.outcome.manifest.v2
manifest_digest_algorithm: sha256
manifest_digest: 8693fd2b98ef537d3393aa3f837998ad5995eb96378e1866e52470f5865d0751
manifest_bytes: 1741
workspace_id: 00000000-0000-4000-8000-000000000001
mission_id: 00000000-0000-4000-8000-000000000002
completion_id: 00000000-0000-4000-8000-000000000003
boundary_event_seq: 4210
accountable_resident_id: 00000000-0000-4000-8000-000000000005
policy_classification_id: 00000000-0000-4000-8000-000000000004
policy_high_impact: true
appendix_request: policy_default
appendix_mode: included
---

# Mission outcome

Every fact below was verified at boundary event `4210`; it is not re-verified when this document is read.
This document is a record. Nothing derives permission to proceed, Mission completion, or a success verdict from its existence.

## 1. Mission and completion identity

| field | value |
| --- | --- |
| workspace | `00000000-0000-4000-8000-000000000001` |
| Mission | `00000000-0000-4000-8000-000000000002` |
| Mission declaration | Ship the exporter \<script\>alert(1)\</script\> \| pipe \& amp \*star\* \_under\_ \[link\] |
| completion | `00000000-0000-4000-8000-000000000003` |
| completion schema version | `1` |
| completion digest | `sha256:2222222222222222222222222222222222222222222222222222222222222222` (`beacon.completion.v1`) |
| completion recorded at | `2026-09-02T18:12:04.000Z` |
| boundary event sequence | `4210` |
| accountable Resident | `00000000-0000-4000-8000-000000000005` |
| Policy classification | `00000000-0000-4000-8000-000000000004` |
| Policy high impact | `true` |

## 2. Success conditions

### 2.1 `sc-alpha`

The exporter writes canonical Markdown for every completed Mission.

- ordinal: `1`
- state: `verified` (`met`)
- citations:
    - `artifact` `absent (the owner carries no reference identity)` schema `absent` `sha256:4444444444444444444444444444444444444444444444444444444444444444` (`beacon.artifact.v1`)

### 2.2 `sc-beta`

A reviewer who is not the author evaluates the candidate.

- ordinal: `2`
- state: `not_evaluated` (reason: no Evaluation names this condition at the boundary event.)
- citations: `absent` (reason: the completion recorded no supporting object for this condition.)

### 2.3 `sc-gamma`

The check harness runs on a clean host.

- ordinal: `3`
- state: `broken` (reason: the delivered check harness exited before producing a result.)
- citations: `verified`: zero entries were recorded at the boundary event. This is not `absent`.

## 3. Delivered Artifacts

| Artifact | schema | digest | canonicalizer | media type | title |
| --- | --- | --- | --- | --- | --- |
| `absent (the owner carries no reference identity)` | `absent` | `sha256:4444444444444444444444444444444444444444444444444444444444444444` | `beacon.artifact.v1` | `text/markdown; charset=utf-8` | Exporter design note (v3) |

## 4. Evaluations and cited support

`absent` (reason: no Evaluation subject arm exists for an Artifact digest at this boundary.)

## 5. Actual spend and coverage

| field | value |
| --- | --- |
| spend record | `00000000-0000-4000-8000-00000000000a` |
| schema version | `1` |
| digest | `sha256:8888888888888888888888888888888888888888888888888888888888888888` (`beacon.spend.v1`) |
| amount | `1234.56` `USD` |
| coverage | events `1` through `3000` |
| measurement | `measured` |

## 6. Unfinished work

### 6.1 `uw-docs` (ordinal `1`)

The operator runbook was not updated for the new export path.

## 7. Uncertainty

### 7.1 `un-latency` (ordinal `1`)

Render latency was measured on one host only.

## 8. Rulings and Proposal lineage

### 8.1 Rulings

| Ruling | schema | digest | canonicalizer |
| --- | --- | --- | --- |
| `absent (the owner carries no reference identity)` | `absent` | `sha256:6666666666666666666666666666666666666666666666666666666666666666` | `beacon.ruling.v1` |

### 8.2 Proposal lineage

| Proposal | schema | digest | canonicalizer |
| --- | --- | --- | --- |
| `absent (the owner carries no reference identity)` | `absent` | `sha256:5555555555555555555555555555555555555555555555555555555555555555` | `beacon.proposal.v1` |

## 9. Linked Receipts

| run | Receipt version | Outcome checksum |
| --- | --- | --- |
| `00000000-0000-4000-8000-000000000009` | `2` | `sha256:7777777777777777777777777777777777777777777777777777777777777777` |

Each checksum above is an Outcome verification checksum (`beacon.outcome.receipt-checksum.v1`) over the exact stored `receipt_json` bytes. It is not a Receipt content digest: Receipt v1 and v2 define none.

## 10. Verification appendix

Every result below was verified at boundary event `4210`; it is not re-verified at read time.

### 10.1 Cited objects

| kind | identity | schema | digest algorithm | digest | canonicalizer | pinned revision | verification result |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `completion` | `00000000-0000-4000-8000-000000000003` | `1` | `sha256` | `2222222222222222222222222222222222222222222222222222222222222222` | `beacon.completion.v1` | `absent` (reason: this source type pins no content revision.) | `verified at boundary event 4210` |
| `artifact` | `absent (the owner carries no reference identity)` | `absent` | `sha256` | `4444444444444444444444444444444444444444444444444444444444444444` | `beacon.artifact.v1` | `git_commit` `a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2` | `verified at boundary event 4210` |
| `proposal` | `absent (the owner carries no reference identity)` | `absent` | `sha256` | `5555555555555555555555555555555555555555555555555555555555555555` | `beacon.proposal.v1` | `absent` (reason: this source type pins no content revision.) | `verified at boundary event 4210` |
| `ruling` | `absent (the owner carries no reference identity)` | `absent` | `sha256` | `6666666666666666666666666666666666666666666666666666666666666666` | `beacon.ruling.v1` | `absent` (reason: this source type pins no content revision.) | `verified at boundary event 4210` |
| `policy` | `00000000-0000-4000-8000-000000000004` | `1` | `sha256` | `3333333333333333333333333333333333333333333333333333333333333333` | `beacon.policy.v1` | `absent` (reason: this source type pins no content revision.) | `verified at boundary event 4210` |
| `spend` | `00000000-0000-4000-8000-00000000000a` | `1` | `sha256` | `8888888888888888888888888888888888888888888888888888888888888888` | `beacon.spend.v1` | `absent` (reason: a spend measurement pins no content revision) | `verified at boundary event 4210` |

### 10.2 Linked Receipt checksums

| run | Receipt version | checksum algorithm | checksum | label |
| --- | --- | --- | --- | --- |
| `00000000-0000-4000-8000-000000000009` | `2` | `sha256` | `7777777777777777777777777777777777777777777777777777777777777777` | `beacon.outcome.receipt-checksum.v1` |

### 10.3 Candidate and render profile

| field | value |
| --- | --- |
| candidate media type | `text/markdown; charset=utf-8` |
| renderer version | `beacon.outcome.md.v1` |
| PDF render profile | `absent` (reason: this version generates no PDF derivative) |

### 10.4 Policy and appendix mode

| field | value |
| --- | --- |
| Policy classification | `00000000-0000-4000-8000-000000000004` |
| Policy classification digest | `sha256:3333333333333333333333333333333333333333333333333333333333333333` (`beacon.policy.v1`) |
| Policy high impact | `true` |
| appendix request | `policy_default` |
| appendix mode | `included` |
| selecting rule | the verified Policy classification is high-impact, which forces the appendix |

### 10.5 Manifest proof

| field | value |
| --- | --- |
| canonicalizer | `beacon.outcome.manifest.v2` |
| digest algorithm | `sha256` |
| digest | `8693fd2b98ef537d3393aa3f837998ad5995eb96378e1866e52470f5865d0751` |
| byte length | `1741` |
| boundary event sequence | `4210` |

Concatenate the lines below in order, decode as lowercase hexadecimal, and hash the result with SHA-256 to obtain the manifest digest above.

```
626561636f6e2e6f7574636f6d652e6d616e69666573742e7632000000000000
00001a626561636f6e2e6f7574636f6d652e6d616e69666573742e7632000000
0000000014626561636f6e2e6f7574636f6d652e6d642e763100000000000040
0080000000000000010000000000004000800000000000000200000000000040
0080000000000000010202000000000000400080000000000000030000000122
2222222222222222222222222222222222222222222222222222222222222200
000000000000067368613235360000000000000014626561636f6e2e636f6d70
6c6574696f6e2e763102000000000000002a7468697320736f75726365207479
70652070696e73206e6f20636f6e74656e74207265766973696f6e2e00000000
0000400080000000000000010702000000000000400080000000000000040000
0001333333333333333333333333333333333333333333333333333333333333
333300000000000000067368613235360000000000000010626561636f6e2e70
6f6c6963792e763102000000000000002a7468697320736f7572636520747970
652070696e73206e6f20636f6e74656e74207265766973696f6e2e0000000000
001072000001a063522920000000000000400080000000000000050101000000
0300000001000000000000000873632d616c7068610101010000000100000000
0000400080000000000000010301444444444444444444444444444444444444
4444444444444444444444444444000000000000000673686132353600000000
00000012626561636f6e2e61727469666163742e763101010000000000000028
6131623263336434653566366131623263336434653566366131623263336434
653566366131623200000002000000000000000773632d626574610400000000
000000396e6f204576616c756174696f6e206e616d6573207468697320636f6e
646974696f6e2061742074686520626f756e64617279206576656e742e020000
00000000004074686520636f6d706c6574696f6e207265636f72646564206e6f
20737570706f7274696e67206f626a65637420666f72207468697320636f6e64
6974696f6e2e00000003000000000000000873632d67616d6d61030000000000
00003d7468652064656c69766572656420636865636b206861726e6573732065
7869746564206265666f72652070726f647563696e67206120726573756c742e
0100000000010000000100000000000040008000000000000001030144444444
4444444444444444444444444444444444444444444444444444444400000000
000000067368613235360000000000000012626561636f6e2e61727469666163
742e763101010000000000000028613162326333643465356636613162326333
6434653566366131623263336434653566366131623201000000010000000000
0040008000000000000001040155555555555555555555555555555555555555
5555555555555555555555555500000000000000067368613235360000000000
000012626561636f6e2e70726f706f73616c2e763102000000000000002a7468
697320736f7572636520747970652070696e73206e6f20636f6e74656e742072
65766973696f6e2e010000000100000000000040008000000000000001050166
6666666666666666666666666666666666666666666666666666666666666600
000000000000067368613235360000000000000010626561636f6e2e72756c69
6e672e763102000000000000002a7468697320736f7572636520747970652070
696e73206e6f20636f6e74656e74207265766973696f6e2e0200000000000000
496e6f204576616c756174696f6e207375626a6563742061726d206578697374
7320666f7220616e204172746966616374206469676573742061742074686973
20626f756e646172792e01000000010000000000004000800000000000000100
0000000000400080000000000000090000000277777777777777777777777777
7777777777777777777777777777777777777700000000000000067368613235
360000000000000022626561636f6e2e6f7574636f6d652e726563656970742d
636865636b73756d2e7631010000000000004000800000000000000100000000
00004000800000000000000a0000000188888888888888888888888888888888
8888888888888888888888888888888800000000000000067368613235360000
00000000000f626561636f6e2e7370656e642e7631000000000001e240000000
00000000035553440200000000000000010000000000000bb801010000000100
000001000000000000000775772d646f63730100000001000000010000000000
00000a756e2d6c6174656e6379
```
