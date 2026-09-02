//! The sandbox — ADR 0029.
//!
//! These tests assert enforcement, not configuration. A test that can pass
//! while the sandbox fails to contain anything is the wrong test.

use beacond::{ProbeObservation, SandboxCapability, SandboxUnavailableReason, detect_sandbox};

#[test]
fn this_host_enforces_and_names_the_abi_it_proved() {
    match detect_sandbox() {
        SandboxCapability::Enforcing {
            landlock_abi,
            observed,
        } => {
            // Network restriction arrived in ABI 4; anything reporting
            // Enforcing must be at or above it.
            assert!(
                landlock_abi >= 4,
                "Enforcing was reported under ABI {landlock_abi}, which cannot restrict TCP"
            );
            // The claim must be backed by what the child actually saw.
            assert!(
                observed.wrote_inside_target,
                "a usable sandbox permits its own target"
            );
            assert!(
                observed.refused_write_outside_target,
                "containment did not hold: a write outside every declared target succeeded"
            );
            assert!(
                observed.read_system_path,
                "a sandbox that blocks system reads cannot exec a binary"
            );
            assert!(observed.refused_tcp_connect, "TCP was not denied");
        }
        // A kernel without Landlock, or with a pre-network Landlock, is a
        // legitimate host; it simply may not run checks.
        other => {
            assert!(
                !other.admits_execution(),
                "a non-Enforcing capability must never admit execution: {other:?}"
            );
        }
    }
}

#[test]
fn public_detection_preserves_the_real_probe_child_observations() {
    match detect_sandbox() {
        SandboxCapability::Enforcing { observed, .. } => assert_eq!(
            observed,
            ProbeObservation {
                wrote_inside_target: true,
                refused_write_outside_target: true,
                read_system_path: true,
                refused_tcp_connect: true,
            },
            "the public capability must decode the real child's exact exit bits"
        ),
        SandboxCapability::FilesystemOnly { observed, .. } => assert_eq!(
            observed,
            ProbeObservation {
                wrote_inside_target: true,
                refused_write_outside_target: true,
                read_system_path: true,
                refused_tcp_connect: false,
            },
            "a pre-network ruleset must preserve the real child's filesystem bits"
        ),
        SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::ProbeRefused { observed },
        } => panic!("the real probe child reported a broken observation protocol: {observed:?}"),
        // These outcomes mean setup could not reach a reporting child. They are
        // legitimate capability answers and carry no exit-bit claim to check.
        SandboxCapability::Unavailable { .. } => {}
    }
}

#[test]
fn filesystem_only_never_admits_execution() {
    // The Blueprint denies network by default, so a sandbox that cannot
    // restrict the network is not sufficient, however well it bounds writes.
    let capability = SandboxCapability::FilesystemOnly {
        landlock_abi: 3,
        observed: ProbeObservation {
            wrote_inside_target: true,
            refused_write_outside_target: true,
            read_system_path: true,
            refused_tcp_connect: false,
        },
    };
    assert!(!capability.admits_execution());
    assert_eq!(
        capability.receipt_version(),
        "landlock-abi-3/filesystem-only"
    );
}

#[test]
fn a_probe_that_never_reported_is_not_a_probe_that_reported_failure() {
    let inconclusive = SandboxCapability::Unavailable {
        reason: SandboxUnavailableReason::ProbeInconclusive {
            detail: "the probe child was terminated by signal 9".to_owned(),
        },
    };
    let refused = SandboxCapability::Unavailable {
        reason: SandboxUnavailableReason::ProbeRefused {
            observed: ProbeObservation {
                wrote_inside_target: true,
                refused_write_outside_target: false,
                read_system_path: true,
                refused_tcp_connect: true,
            },
        },
    };
    assert_ne!(inconclusive, refused);
    assert_eq!(
        inconclusive.receipt_version(),
        "unavailable/probe-inconclusive"
    );
    assert_eq!(refused.receipt_version(), "unavailable/probe-refused");
    assert!(!inconclusive.admits_execution());
    assert!(!refused.admits_execution());
}

#[test]
fn enforcement_is_observed_rather_than_assumed() {
    // The observation that matters: if containment did not hold, the capability
    // must be Unavailable even though every syscall returned success.
    let capability = detect_sandbox();
    if let SandboxCapability::Enforcing { .. } = capability {
        // Detection built a ruleset, applied it in a child, and that child
        // reported a refused write outside its target. Re-running must agree —
        // a capability that flickers is not a capability.
        assert_eq!(detect_sandbox(), capability);
    }
}

#[test]
fn the_supervisor_is_never_restricted_by_detecting() {
    // Landlock is irreversible for the thread that arms it. If detection armed
    // the supervisor, this write would fail and every later test would too.
    let capability = detect_sandbox();
    let scratch =
        std::env::temp_dir().join(format!("beacon-unrestricted-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&scratch).expect("the supervisor must still create directories");
    std::fs::write(scratch.join("proof"), b"unrestricted")
        .expect("the supervisor must still write outside any probe target");
    std::fs::remove_dir_all(&scratch).unwrap();
    // And detection must still work afterwards, proving nothing leaked.
    assert_eq!(detect_sandbox(), capability);
}

#[test]
fn a_receipt_version_is_recorded_for_every_capability() {
    for capability in [
        SandboxCapability::Enforcing {
            landlock_abi: 8,
            observed: ProbeObservation {
                wrote_inside_target: true,
                refused_write_outside_target: true,
                read_system_path: true,
                refused_tcp_connect: true,
            },
        },
        SandboxCapability::FilesystemOnly {
            landlock_abi: 1,
            observed: ProbeObservation {
                wrote_inside_target: true,
                refused_write_outside_target: true,
                read_system_path: true,
                refused_tcp_connect: false,
            },
        },
        SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::LandlockAbsent {},
        },
        SandboxCapability::Unavailable {
            reason: SandboxUnavailableReason::UnsupportedPlatform {},
        },
    ] {
        let version = capability.receipt_version();
        assert!(!version.is_empty());
        // A Receipt must never record the bare word that preceded this slice.
        assert_ne!(version, "none");
    }
}

#[test]
fn an_unknown_field_is_rejected_even_on_a_variant_that_carries_none() {
    // `deny_unknown_fields` is INERT on a unit variant of an internally tagged
    // enum: serde reads the tag and matches the rest leniently. Every variant
    // here therefore carries an empty struct body, and this test is what keeps
    // it that way — a future `LandlockAbsent,` without braces silently reopens
    // the hole and nothing else would notice.
    let honest = r#"{"kind":"landlockAbsent"}"#;
    let decoded: SandboxUnavailableReason = serde_json::from_str(honest).unwrap();
    assert_eq!(decoded, SandboxUnavailableReason::LandlockAbsent {});

    for smuggled in [
        r#"{"kind":"landlockAbsent","surprise":true}"#,
        r#"{"kind":"unsupportedPlatform","errno":13}"#,
    ] {
        assert!(
            serde_json::from_str::<SandboxUnavailableReason>(smuggled).is_err(),
            "an unknown field survived the guard: {smuggled}"
        );
    }

    // The wire form of a braced variant is unchanged, so this is not a
    // breaking change to anything already recorded.
    assert_eq!(
        serde_json::to_string(&SandboxUnavailableReason::LandlockAbsent {}).unwrap(),
        honest
    );
}
