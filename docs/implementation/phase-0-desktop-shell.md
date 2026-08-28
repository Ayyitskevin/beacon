# Phase 0 local Owner session

This checkpoint proves the native desktop toolchain, local Owner enrollment and recovery, and the
authoritative inactivity and Linux platform-lock boundary. It does not implement passkeys, Cloud
Link, remote authentication, or a usable Workspace. Those security-sensitive changes retain their
own design and review boundaries.

## Boundary

The bundled main window receives only explicit account and session commands. Before enrollment or
while locked, `session_status` returns this exact closed shape:

```json
{
  "phase": "operator_setup_required",
  "workspaceAccess": false,
  "operatorCommands": false
}
```

The TypeScript adapter validates the exact key set and literal values. Missing fields, added fields,
unexpected phases, enabled access, enabled commands, and non-object responses all fail closed into
the local-session error view. The Tauri crate reaches the independently running supervisor only
through an authenticated, owner-only Unix socket. It cannot open the managed store directly.

The build manifest generates one permission per registered command, and the capability manifest
grants that exact closed set to the `main` window. Generated permission and schema files remain
ignored build products. The content security policy admits bundled content, Tauri IPC, inline styles
required by Fluent UI's runtime styling, and local image data. It does not grant arbitrary network
access. There is no browser development bypass and no JavaScript SQLite access.

## Interface

The React shell uses Fluent UI v9 and a restrained institutional palette. It contains the accepted
primary suite navigation, a central setup-required explanation, and an installation inspector.
Primary apps remain disabled before setup. Light is the initial choice; Dark and Follow System are
available without changing semantic meaning. Appearance persistence belongs to the later approved
preference boundary and is not implied here.

Automated tests cover the locked session shape, malformed-response rejection, failure and retry,
enrollment, recovery, configurable timeout, explicit-activity renewal, unavailable-monitor
warnings, disabled navigation, Light/Dark/Follow System behavior, structural accessibility, and WCAG AA
contrast for the fixed text and brand-control color pairs. The layout includes a keyboard skip link,
semantic landmarks, labeled controls, and reduced-motion handling. This is automated evidence, not
a claim that the later interactive product has completed manual Orca testing.

## Verified dependency baseline

- Tauri 2.11.5 and Tauri Build 2.6.3 use the existing-project and command patterns documented at
  <https://v2.tauri.app/start/create-project/> and <https://v2.tauri.app/develop/calling-rust/>.
- Tauri capabilities and the CSP follow <https://v2.tauri.app/security/capabilities/> and
  <https://v2.tauri.app/security/csp/>.
- React 19.2.8 mounts through the client root documented at
  <https://react.dev/reference/react-dom/client/createRoot>.
- Fluent UI React Components 9.74.7 supplies the accessible controls, semantic tokens, and theme
  creation described at
  <https://github.com/microsoft/fluentui/blob/master/packages/react-components/react-components/README.md>
  and <https://github.com/microsoft/fluentui/blob/master/docs/architecture/design-tokens.md>.
- Vite 8.2.2, TypeScript 7.0.2, and Vitest 4.1.11 are exact lockfile inputs. Vitest runs the UI in
  jsdom as documented at <https://vitest.dev/guide/environment>.
- axe-core 4.13.0 checks the rendered signed-out structure. jsdom cannot render color contrast, so
  deterministic WCAG luminance tests cover the fixed palette separately.
- Node 22.23.2 and npm 10.9.8 are exact development inputs. `.node-version`, `packageManager`, and
  strict package-engine checks fail the clean install when that runtime contract is not met.

## Development and verification

From the repository root:

```sh
scripts/gate desktop
```

For an interactive development launch:

```sh
cd apps/desktop
npm run tauri dev
```

The desktop stage performs a clean lifecycle-script-disabled npm install, frontend tests, the native
pre-authentication contract test, a production web build, and a Tauri no-bundle native build. The
repository-wide gate adds the preserved Python checks and all Rust checks. `bundle.active` remains
false: this checkpoint builds a local executable but creates no public installer, package identity,
or release artifact. Product and executable identifiers use Beacon; explicit legacy migration paths
preserve local state created under the pre-Beacon placeholder. On first launch, Beacon atomically
adopts a private, same-owner legacy application directory and renames its SQLite/WAL family. It
refuses symlink sources, non-private ownership, a running legacy supervisor, or simultaneous old and
new directories instead of choosing or copying credential-bearing state.
