# Beacon package runner on mickey

This installs one repository-scoped, ephemeral GitHub Actions runner for
`Ayyitskevin/beacon`. The runner process is an unprivileged `gh-runner` system
user. It has no login shell and no sudo access. GitHub assigns one job, removes
the ephemeral registration, and the runner exits. A fresh short-lived
registration token is required before the next job.

The self-hosted package job runs in the digest-pinned Ubuntu 22.04 container
declared in `.github/workflows/ci.yml`; package-labeled PRs retain the original
GitHub-hosted Ubuntu 22.04 job. Mickey is Ubuntu 24.04 with glibc 2.39; building
directly on the host would undo the glibc 2.35 compatibility floor accepted in
[ADR 0036](../../docs/adr/0036-appimage-compatibility-floor.md). The runner work
root is therefore `/__w`: GitHub mounts that same path into the job container,
so the first-run harness's nested `docker run --volume ...` paths also exist on
the host.

## Security boundary

The service can write only `/var/lib/gh-runner`, `/__w`, and the Docker socket.
`ProtectHome=yes` hides `/home`, `/root`, and `/run/user` from the runner mount
namespace; `ExecStartPre` refuses to start unless `/home/kevin-lee` is unreadable
there. The account is not placed in `sudo` or given a usable login shell.

The exception is Docker. `scripts/clean-host-first-run` calls the rootful Docker
daemon, so the unit receives the `docker` supplementary group. Access to that
socket is effectively root access and can bypass `ProtectHome` by asking the
daemon to mount host paths. Because a PR label persists across `synchronize` and
is not bound to the reviewed head SHA, even package-labeled PRs run only on
hosted Ubuntu 22.04; the root-equivalent mickey runner is reserved for pushes to
`main` and explicit `workflow_dispatch` refs. The direct runner process cannot
read `/home/kevin-lee`; Docker-capable workflow code is not inside that boundary.

The runner otherwise reads the checked-out repository, ordinary system files,
and the public network resources used by the build. Its registration
credentials live under the mode-0700 runner directory. The one-hour repository
registration token is pasted into a shell variable, consumed by `config.sh`,
unset immediately, and never written to a file, unit, shell history, or log.

## 1. Create the account and directories

Run from a trusted Beacon checkout on mickey:

```sh
sudo apt-get update
sudo apt-get install --no-install-recommends --yes ca-certificates curl git gzip tar docker.io
sudo systemctl enable --now docker

sudo useradd --system --home-dir /var/lib/gh-runner --create-home \
  --shell /usr/sbin/nologin gh-runner
sudo install -d -o gh-runner -g gh-runner -m 0700 \
  /var/lib/gh-runner/actions-runner /__w
```

Do not run `usermod -aG docker gh-runner`; `SupplementaryGroups=docker` grants
that root-equivalent access only while this reviewed unit is running.

## 2. Download and verify the pinned runner

The archive is GitHub's official `actions/runner` v2.337.0 Linux x64 release.
Both its version and GitHub-published SHA-256 are fixed here:

```sh
runner_version=2.337.0
runner_archive="actions-runner-linux-x64-${runner_version}.tar.gz"
runner_sha256=70920811a4f8ad4328818682bca5c6469c1c942fab52448868071d0063816613
runner_url="https://github.com/actions/runner/releases/download/v${runner_version}/${runner_archive}"

cd /var/lib/gh-runner/actions-runner
sudo -u gh-runner curl --fail --location --proto '=https' --tlsv1.2 \
  --output "$runner_archive" "$runner_url"
printf '%s  %s\n' "$runner_sha256" "$runner_archive" | \
  sudo -u gh-runner sha256sum --check -
sudo -u gh-runner tar --extract --gzip --file "$runner_archive"
sudo ./bin/installdependencies.sh
sudo -u gh-runner rm "$runner_archive"
```

Release: <https://github.com/actions/runner/releases/tag/v2.337.0>

## 3. Register one ephemeral job

As a Beacon repository administrator, open **Settings → Actions → Runners → New
self-hosted runner** and copy the short-lived token. Do not paste it into the
command line or save it. Run this block; the silent prompt keeps it out of shell
history:

```sh
sudo -u gh-runner -H /usr/bin/bash
cd /var/lib/gh-runner/actions-runner
read -r -s -p 'Repository runner token: ' RUNNER_TOKEN; printf '\n'
./config.sh \
  --url https://github.com/Ayyitskevin/beacon \
  --token "$RUNNER_TOKEN" \
  --name mickey-beacon-package \
  --work /__w \
  --labels mickey \
  --unattended \
  --ephemeral \
  --disableupdate \
  --replace
unset RUNNER_TOKEN
exit
```

GitHub automatically adds `self-hosted`, `linux`, and `x64`; `--labels mickey`
makes the complete routing set `self-hosted,linux,x64,mickey`. Automatic runner
updates are disabled so the verified version remains the executable version.
GitHub may stop scheduling an old runner, so update this version and checksum
promptly when a new required runner release is published.

## 4. Install, verify, and start the unit

```sh
sudo install -o root -g root -m 0644 ops/runner/gh-runner.service \
  /etc/systemd/system/gh-runner.service
sudo systemd-analyze verify /etc/systemd/system/gh-runner.service
sudo systemctl daemon-reload
sudo systemctl enable --now gh-runner.service
sudo systemctl status --no-pager gh-runner.service
sudo journalctl -u gh-runner.service -n 50 --no-pager
```

Prove both the account boundary and the unit's `ProtectHome` namespace after
installation. Each command must exit zero:

```sh
sudo -u gh-runner /usr/bin/test ! -r /home/kevin-lee
sudo systemd-run --quiet --wait --pipe --collect \
  -p User=gh-runner -p Group=gh-runner -p SupplementaryGroups=docker \
  -p ProtectHome=yes -p NoNewPrivileges=yes -p PrivateTmp=yes \
  /usr/bin/test ! -r /home/kevin-lee
```

The second probe reproduces the relevant unit identity, supplemental group, and
mount protection. The unit repeats the same assertion in `ExecStartPre`.

## Ephemeral lifecycle and cache choice

After one job, GitHub deregisters the runner and `actions/runner` removes its
local `.runner` file. `Restart=always` restarts unexpected failures while that
file exists; `ConditionPathExists` prevents a consumed ephemeral runner from
looping. `ExecStopPost` clears `/__w` after all action post-job hooks finish.
Register again with a new token and run `sudo systemctl start gh-runner` for the
next package job.

Keep the existing SHA-pinned `actions/cache` step as the sole cross-job cache.
It stores Cargo registry and `target` data in GitHub's cache service. Retaining a
second local `~/.cargo` or `target` cache would be redundant and would carry
unreviewed build state between ephemeral registrations, so the unit deletes the
job-local workspace instead. The runner's journald output remains available
after its one-shot registration is gone.

## Workflow check

An unlabeled pull request skips packaging. Adding the exact `package` label
schedules the GitHub-hosted Ubuntu 22.04 job, including after later synchronize
events. Pushes to `main` and manual dispatches with an explicit `ref` schedule
the mickey job. Fast suites remain on GitHub-hosted runners.

## Remove the runner

Stop execution first. Removing a registered-but-unused runner also requires a
fresh removal token from the repository runner settings; a consumed ephemeral
runner is already absent from GitHub.

```sh
sudo systemctl disable --now gh-runner.service
# If it is still registered, run as gh-runner from the runner directory:
# ./config.sh remove --token "$FRESH_REMOVAL_TOKEN"
sudo rm /etc/systemd/system/gh-runner.service
sudo systemctl daemon-reload
sudo rm -rf /var/lib/gh-runner/actions-runner /__w
sudo userdel --remove gh-runner
```

The final four commands permanently remove local state; inspect the paths and
confirm the GitHub runner entry is absent before running them.
