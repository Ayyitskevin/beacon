# Legacy self-hosted Beacon package runner

Beacon now packages on GitHub-hosted Ubuntu 22.04 for main pushes, manual builds and
package-labeled pull requests. See the [current CI instructions](../../CONTRIBUTING.md#downloading-the-ci-appimage).
The following notes document the former ephemeral runner for operators maintaining an existing installation.

## Security boundary

The service can write only `/var/lib/example-runner`, `/example-work`, and the Docker socket.
`ProtectHome=yes` hides `/home`, `/root`, and `/run/user` from the runner mount
namespace; `ExecStartPre` refuses to start unless `/home/example` is unreadable
there. The account is not placed in `sudo` or given a usable login shell.

The exception is Docker. `scripts/clean-host-first-run` calls the rootful Docker
daemon, so the unit receives the `docker` supplementary group. Access to that
socket is effectively root access and can bypass `ProtectHome` by asking the
daemon to mount host paths. Because a PR label persists across `synchronize` and
is not bound to the reviewed head SHA, the former workflow ran package-labeled PRs on hosted Ubuntu 22.04 and reserved the
self-hosted runner for main pushes and explicit dispatches. The direct runner process cannot
read `/home/example`; Docker-capable workflow code is not inside that boundary.

The runner otherwise reads the checked-out repository, ordinary system files,
and the public network resources used by the build. Its registration
credentials live under the mode-0700 runner directory. The one-hour repository
registration token is pasted into a shell variable, consumed by `config.sh`,
unset immediately, and never written to a file, unit, shell history, or log.

## 1. Create the account and directories

Run from a trusted Beacon checkout on node-a:

```sh
sudo apt-get update
sudo apt-get install --no-install-recommends --yes ca-certificates curl git gzip tar docker.io
sudo systemctl enable --now docker

sudo useradd --system --home-dir /var/lib/example-runner --create-home \
  --shell /usr/sbin/nologin example-runner
sudo install -d -o example-runner -g example-runner -m 0700 \
  /var/lib/example-runner/actions-runner /example-work
```

Do not run `usermod -aG docker example-runner`; `SupplementaryGroups=docker` grants
that root-equivalent access only while this reviewed unit is running.

## 2. Download and verify the pinned runner

The archive is GitHub's official `actions/runner` v2.337.0 Linux x64 release.
Both its version and GitHub-published SHA-256 are fixed here:

```sh
runner_version=2.337.0
runner_archive="actions-runner-linux-x64-${runner_version}.tar.gz"
runner_sha256=70920811a4f8ad4328818682bca5c6469c1c942fab52448868071d0063816613
runner_url="https://github.com/actions/runner/releases/download/v${runner_version}/${runner_archive}"

cd /var/lib/example-runner/actions-runner
sudo -u example-runner curl --fail --location --proto '=https' --tlsv1.2 \
  --output "$runner_archive" "$runner_url"
printf '%s  %s\n' "$runner_sha256" "$runner_archive" | \
  sudo -u example-runner sha256sum --check -
sudo -u example-runner tar --extract --gzip --file "$runner_archive"
sudo ./bin/installdependencies.sh
sudo -u example-runner rm "$runner_archive"
```

Release: <https://github.com/actions/runner/releases/tag/v2.337.0>

## 3. Register one ephemeral job

As a Beacon repository administrator, open **Settings → Actions → Runners → New
self-hosted runner** and copy the short-lived token. Do not paste it into the
command line or save it. Run this block; the silent prompt keeps it out of shell
history:

```sh
sudo -u example-runner -H /usr/bin/bash
cd /var/lib/example-runner/actions-runner
read -r -s -p 'Repository runner token: ' RUNNER_TOKEN; printf '\n'
./config.sh \
  --url https://github.com/Ayyitskevin/beacon \
  --token "$RUNNER_TOKEN" \
  --name node-a-beacon-package \
  --work /example-work \
  --labels node-a \
  --unattended \
  --ephemeral \
  --disableupdate \
  --replace
unset RUNNER_TOKEN
exit
```

GitHub automatically adds `self-hosted`, `linux`, and `x64`; `--labels node-a`
makes the complete routing set `self-hosted,linux,x64,node-a`. Automatic runner
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
sudo -u example-runner /usr/bin/test ! -r /home/example
sudo systemd-run --quiet --wait --pipe --collect \
  -p User=example-runner -p Group=example-runner -p SupplementaryGroups=docker \
  -p ProtectHome=yes -p NoNewPrivileges=yes -p PrivateTmp=yes \
  /usr/bin/test ! -r /home/example
```

The second probe reproduces the relevant unit identity, supplemental group, and
mount protection. The unit repeats the same assertion in `ExecStartPre`.

## Ephemeral lifecycle and cache choice

**One job per registration.** A registration is consumed by the first job it
runs. There is no second job without a new token and a new `config.sh`, and this
is the whole shape of the lifecycle rather than a detail of it.

After that job, GitHub deregisters the runner and `actions/runner` un-configures
itself locally: `DeleteLocalRunnerConfig` removes `.credentials` and its RSA key,
then `.runner` (v2.337.0, `Runner.Listener/Runner.cs:877-880` and
`Configuration/ConfigurationManager.cs:496-524`). The unit requires both files,
so a cleanup interrupted between the two deletions still refuses to start.

`Restart=on-failure` is what stops a consumed runner from looping, and the
condition is a second belt. It has to be `on-failure`, because `run.sh` does not
let the listener's exit code reach systemd: an un-configured runner exits 1, and
`run-helper.sh.template` turns that into "stop the service, no retry needed"
followed by `exit 0`. Under `Restart=always` that request to stop was read as a
success worth repeating, and the unit restarted six times into
`start-limit-hit`. A real crash — a signal death, or `run.sh` itself failing —
is still a failure, still restarts, and is still stopped by
`StartLimitBurst=5` within `StartLimitIntervalSec=300`.

`ExecStopPost` clears `/example-work` after all action post-job hooks finish.

### Register again for the next job

```sh
# 1. A fresh registration token from the repository's runner settings.
# 2. As example-runner, from /var/lib/example-runner/actions-runner:
sudo -u example-runner ./config.sh --url https://github.com/Ayyitskevin/beacon \
  --token "$FRESH_REGISTRATION_TOKEN" --ephemeral --unattended
# 3. Clear the previous run's failure state, if the unit reached its start limit:
sudo systemctl reset-failed gh-runner.service
# 4. Start it for the one job:
sudo systemctl start gh-runner.service
```

Step 3 is not optional after a `start-limit-hit`: until the failure state is
cleared, `systemctl start` refuses.

### The persistent alternative — Kevin's call, not made here

Dropping `--ephemeral` at `config.sh` gives a runner that stays registered and
takes job after job, which removes steps 1-4 above entirely.

It also removes the property this setup was built for: with one job per
registration, no state and no credential outlives a single job, so a workflow
that compromises the runner cannot reach the next one. A persistent runner
carries its filesystem, its Docker state and its registration credential across
every job that follows, and on this host that is the same machine as the
fleet's own work.

Keep the existing SHA-pinned `actions/cache` step as the sole cross-job cache.
It stores Cargo registry and `target` data in GitHub's cache service. Retaining a
second local `~/.cargo` or `target` cache would be redundant and would carry
unreviewed build state between ephemeral registrations, so the unit deletes the
job-local workspace instead. The runner's journald output remains available
after its one-shot registration is gone.

## Remove the runner

Stop execution first. Removing a registered-but-unused runner also requires a
fresh removal token from the repository runner settings; a consumed ephemeral
runner is already absent from GitHub.

```sh
sudo systemctl disable --now gh-runner.service
# If it is still registered, run as example-runner from the runner directory:
# ./config.sh remove --token "$FRESH_REMOVAL_TOKEN"
sudo rm /etc/systemd/system/gh-runner.service
sudo systemctl daemon-reload
sudo rm -rf /var/lib/example-runner/actions-runner /example-work
sudo userdel --remove example-runner
```

The final four commands permanently remove local state; inspect the paths and
confirm the GitHub runner entry is absent before running them.

> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
