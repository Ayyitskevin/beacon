# Desktop accessibility and AppImage exit baseline

Recorded 2026-08-30 and reconciled 2026-08-31 against
`Ayyitskevin/beacon@89dad5082d0d5bd7cc2cb5d6a72b96a5fe812aa8`. This is an evidence record,
not remediation. No application source changed to produce it.

Evidence is labelled throughout:

- **AUTOMATED** means a command ran at the pinned commit and its exit status was observed.
- **SOURCE-INFERRED** means source was inspected at the pinned commit; it is not runtime proof.
- **NOT DONE** means the check did not happen and cannot be cited as a pass.

## Automated build and test evidence

| Command (from `apps/desktop`) | Observed result |
|---|---|
| `npm ci --ignore-scripts` | **AUTOMATED:** exit 0 |
| `npm test` | **AUTOMATED:** exit 0; 4 test files and 81 tests passed |
| `npm run build` | **AUTOMATED:** exit 0; Vite emitted the desktop bundle and warned that one chunk exceeds 500 kB |
| `npm run tauri build -- --no-bundle` | **AUTOMATED:** exit 0; built `target/release/beacon-desktop` |

The build host had `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`,
and `libxdo-dev`; Cargo 1.97.1 matched `rust-toolchain.toml`. `~/.cache/tauri` did not contain
`linuxdeploy` or `appimagetool`.

The `--no-bundle` result proves that the application compiled and linked on the measurement host.
It exercised no bundle target and produced no distributable AppImage.

## Source-inferred desktop surface

The UI uses `@fluentui/react-components` 9.74.7 and React 19.2.8
(`apps/desktop/package.json`). Interactive controls are Fluent components, so searching for raw
HTML `<button>` elements does not inventory this interface.

The ten non-test screen components are `App`, `CheckPreflight`, `LocalModels`, `Projects`,
`ProjectMissions`, `ProjectPolicy`, `ProjectRepository`, `ProjectTeams`,
`ResidentConfigurations`, and `SampleProject`. Their JSX contains 58 `Button`, 25 `Field`,
13 `Input`, 9 `Textarea`, 7 `Checkbox`, and 1 `Select` elements. It contains no Fluent `Link`,
`Tab`, `Menu`, or `Dialog` element.

`App.tsx` declares five rail applications: Briefing, Projects, Conversations, Knowledge, and
Software. Only Briefing and Projects are in `availableApplications`. The mapping renders the
other three as disabled buttons once the other enabling conditions are satisfied, but attaches no
programmatic explanation of that application-level unavailability at the rail item.

The source includes:

- a `Skip to main content` link targeting `<main id="main-content">`;
- `<nav aria-label="Beacon applications">` and `aria-current="page"` on the active rail item;
- 17 `role="status"` regions and 5 `role="alert"` regions; and
- no `dialog`, `tablist`, `tab`, or `region` role in the non-test TSX files.

These are **SOURCE-INFERRED** structure facts. They do not establish the announcements or
interaction observed by assistive technology.

## Source-inferred focus, keyboard, motion, and scaling

`styles.css` has one `:focus` rule, which reveals the skip link. It has no `:focus-visible` rule,
no `outline` declaration, and no `prefers-contrast` query. It does include a
`prefers-reduced-motion: reduce` block. The file has eight `px` values and no `rem` value.

Application TSX contains no explicit `autoFocus`, `.focus()`, `tabIndex`, or `onKeyDown` usage.
Tests contain no `.tab()`, `.keyboard()`, `toHaveFocus`, or `activeElement` assertion.

Visible focus, traversal order, keyboard interaction, runtime motion behavior, and text scaling
therefore remain unverified. The source delegates most control behavior and focus presentation to
Fluent UI, but this record does not treat that dependency as proof.

## Automated accessibility coverage and its limits

The Vitest suite uses axe-core 4.13.0 under jsdom. It has 12 `axe.run` call sites: 11 in
`App.test.tsx` and one in `SampleProject.test.tsx`. Every call disables only
`color-contrast`; no other axe rule is disabled.

`theme.test.ts` separately runs six normal-text token checks: primary text, secondary text, and
brand-control text in both light and dark themes must each meet a 4.5:1 contrast ratio. Those
checks provide **partial automated WCAG 1.4.3 evidence for the named token pairs**.

They do not render the interface, cover every foreground/background pairing, or measure non-text
UI components. jsdom supplies no layout or paint for axe contrast analysis. Rendered contrast and
WCAG 1.4.11 therefore remain **NOT DONE**; the repository must not describe either as passing from
these tests.

## AppImage exit state

The following are **SOURCE-INFERRED** from `apps/desktop/src-tauri/tauri.conf.json` and its icon
directory:

- `bundle.active` is `false`; no bundle target is configured.
- There is no `bundle.icon` or `bundle.linux` section, and `src-tauri/icons` contains only
  `icon.png`.
- The identifier is the Phase 0 placeholder `dev.beacon.phase0`; the version is 0.1.0.

The host toolchain and successful `--no-bundle` build make a later packaging attempt plausible,
but they do not prove AppImage configuration, production, launch, or first-run behavior. Enabling
bundling, selecting the release identifier and icons, producing an artifact, and running the
clean-host exit remain separate work.

## Not done — may not be cited as a pass

- Manual keyboard traversal of any core workflow.
- Visible-focus observation in a browser or Tauri window.
- Orca screen-reader output on a supported Linux configuration.
- Text scaling to 200%.
- Reduced-motion behavior observed at runtime.
- Rendered text or non-text contrast measurement.
- AppImage production, installation-free launch, or clean-host first run.

Source inspection is not a manual accessibility pass. `docs/BLUEPRINT.md` requires automated
audits plus manual keyboard and Orca testing and states that neither substitutes for the other.

## Next bounded evidence slices

1. Add real keyboard traversal and visible-focus assertions for one complete operator workflow;
   any contrast work must include `theme.ts` and `theme.test.ts` in its file fence.
2. Run the manual keyboard, Orca, 200% text, reduced-motion, and rendered-contrast matrix on a
   supported Linux desktop, recording each result or skip.
3. Configure and produce the AppImage, then run the Blueprint clean-host first-run gate against the
   exact artifact.
