# Regenerating the large files deleted from this directory

Deleted 2026-09-02 at Kevin's request (they are all reproducible; logs and captures were kept).

| file | bytes | sha256 | how to get it back |
|---|---|---|---|
| `artifact.zip` | 78,897,256 | (zip of the two files below) | `gh api repos/Ayyitskevin/beacon/actions/artifacts/9832520729/zip > artifact.zip` |
| `Beacon_0.1.0_amd64.AppImage` | 79,522,296 | `d479f4c4d20f905f79f665858e53b8d8b4d09e5d4bce0fb5b0b9fd7d2c7e8c2c` | `unzip artifact.zip` (checksum is still in `Beacon.AppImage.sha256`) |
| `beacond-from-4637716` | 63,744,728 | `706b8e6e7d06bda43fefe30c5031539e5ebb6f728cff655f39ffc8f93b2560e7` | `cargo build -p beacond` in a worktree at `origin/main`, then copy `target/debug/beacond` |

There was no extracted `squashfs-root` on this host: every `--appimage-extract` ran inside a
disposable container, so nothing was left to delete.

Kept: `out/` (all logs, stderr, xwininfo, captures), both `screenshot-repaired-bundle-*.png`,
`Beacon.AppImage.sha256`, and the four probe/driver scripts.

## Added and removed 2026-09-02 (C4 sidecar work)

| file | bytes | sha256 | regenerate |
|---|---|---|---|
| `Beacon-sidecar.AppImage` | 82,835,960 | `7da5bcff5bc369a7ae804310290f49b5de528057e0dd0e9e37f78e3d1bf49f3d` | `cd apps/desktop && npm run tauri build` on `team/opus2-appimage-sidecar`, then copy `target/release/bundle/appimage/Beacon_0.1.0_amd64.AppImage` |
| `Beacon-jammy.AppImage` | 84,376,056 | `7eb21af2006f0b5945dde8085c2afa48d900fdf50cc57e6d9ba4dc2488f1a4ca` | the same build inside `ubuntu:22.04` — driver kept in the C4 report |

Kept: `out-sidecar/` (logs, stderr, xwininfo, process listings, captures), the four
`screenshot-*.png`, `probe3.sh`, `run-sidecar.sh`, `run-any.sh`, `jammy-build.log`.
