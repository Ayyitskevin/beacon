# Resident lifecycle evaluation — 2026-08-28

Scope: schema-v8 migration, public lifecycle service, authenticated Unix transport, Tauri command
and capability adapters, strict TypeScript parsing, and desktop lifecycle interactions. The real
transport sweep uses an actual private Unix socket and independently running beacond server.

## Research basis

- NIST SP 800-53 AC-2 separates creating, enabling, modifying, disabling, and removing accounts and
  requires those changes to be audited. That supports explicit lifecycle actions rather than one
  ambiguous enabled flag: <https://csrc.nist.gov/CSRC/media/Projects/risk-management/800-53%20Downloads/800-53r5/SP_800-53_v5_1-derived-OSCAL.pdf>.
- Microsoft Entra lifecycle workflows likewise model enable and disable as distinct tasks:
  <https://learn.microsoft.com/en-us/entra/id-governance/lifecycle-workflow-tasks>.
- GitHub's SCIM guidance distinguishes reversible soft deprovisioning from irreversible hard
  deprovisioning while retaining authored resources. Beacon therefore separates suspension from
  terminal retirement and preserves identity/history:
  <https://docs.github.com/en/enterprise-cloud@latest/admin/managing-iam/provisioning-user-accounts-with-scim/provisioning-users-and-groups-with-scim-using-the-rest-api>.

These sources concern account lifecycle, not Agent authority. Beacon's additional separation of
availability from execution authority follows its accepted local authority model.

## Live authenticated-socket measurement

Command:

```sh
cargo test -p beacond --test local_transport authenticated_client_controls_resident_lifecycle_over_the_real_socket -- --exact
```

| # | Real socket case | Expected | Observed |
|---:|---|---|---|
| 1 | List while signed out | `not_authenticated` | Pass |
| 2 | List newly admitted Resident | Inactive, no decision ID | Pass |
| 3 | Suspend inactive Resident | Invalid transition | Pass |
| 4 | Activate from exact initial state | Active successor | Pass |
| 5 | Replay initial activation | Stale decision | Pass |
| 6 | Re-read after activation | Exact active projection | Pass |
| 7 | Suspend from exact active decision | Suspended successor | Pass |
| 8 | Replay prior suspension | Stale decision | Pass |
| 9 | Reactivate from exact suspension | Active successor | Pass |
| 10 | Retire from exact active decision | Retired successor | Pass |
| 11 | Reactivate retired Resident | Invalid transition | Pass |
| 12 | Retire unknown Resident ID | Not found | Pass |
| 13 | Re-read after retirement | Exact terminal projection | Pass |
| 14 | Re-read Starter admission | Original inactive snapshot unchanged | Pass |
| 15 | List after locking session | `not_authenticated` | Pass |

## Additional measured boundaries

| Case | Expected behavior | Observed |
|---|---|---|
| Concurrent initial activation | One winner and one stale command | Pass |
| Decision update or delete | Rejected by append-only triggers | Pass |
| Current Agent projection tampered | Startup refusal | Pass |
| Matching lifecycle Event deleted | Startup refusal | Pass |
| Existing schema-v7 admission | Transactionally preserved at schema v8 | Pass |
| Strict TypeScript adapter | Unknown authority-shaped fields refused | Pass |
| Desktop retirement | No command before explicit confirmation | Pass |
| Retired desktop row | No further action rendered | Pass |

The whole-repository gate and independent review belong to the exact branch handoff; this evidence
does not itself authorize merge or publication.
