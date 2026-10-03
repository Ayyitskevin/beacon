# Security

Beacon supervises AI agents doing real work on a real machine. It runs code from repositories it
was pointed at, inside an operating-system sandbox, and it holds the authority boundary between an
operator and the agents working for them. Security reports matter here, and this page says how to
send one and what to expect.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Report privately through GitHub: go to this repository's **Security** tab and choose **Report a
vulnerability**. That opens a private advisory visible only to you and the maintainers.

There is no security email address. If you would rather not use GitHub advisories, say so in any
public issue **without describing the problem**, and a maintainer will arrange another channel.

## What to expect

Beacon is a pre-Alpha project maintained by a single Product Steward (see
[GOVERNANCE.md](GOVERNANCE.md)). **There is no response-time commitment**, no bounty, and no
embargo policy yet. Saying so is more useful than a number nobody has agreed to honour.

What is committed: a report will be read, you will be told whether it is accepted, and if a fix
ships you will be credited unless you ask otherwise.

## Supported versions

There are no released versions. `main` is the only supported target. Successful packaging
workflows can provide short-lived development AppImage artifacts; availability depends on the
workflow and artifact retention. They are not stable releases or a claim of completed Alpha
acceptance. See [CONTRIBUTING.md](CONTRIBUTING.md#downloading-the-ci-appimage) for the build
conditions and measured host limits.

Do not run Beacon against anything you cannot afford to lose.

## What is in scope

- **Sandbox escape.** Beacon confines executed checks with Landlock. Reaching the filesystem or
  network outside the declared boundary is in scope.
- **Authority bypass.** Anything that lets an agent, a plugin, a Run, or a Receipt obtain authority
  the operator did not grant — including a Ruling that survives material Proposal drift.
- **Secret exposure.** Secrets are meant to stay in the operating system's secret store and never
  reach project records, Receipts, or logs.
- **Store integrity.** Forging, replaying, or silently altering Runs, Receipts, or audit events.
- **A guard that does not guard.** A check that reports enforcement it is not performing is a
  security bug here even when nothing is currently exploitable through it.

## Documented limits, which are not vulnerabilities

These are known, recorded, and deliberate. A report that one of them is true is welcome as a
discussion, but it is not a new finding.

- **Beacon's current Landlock ruleset handles TCP bind and connect, but it does not handle UDP.**
  The probe records its TCP check as refused; it does not test UDP. Landlock added UDP rights in
  ABI 10, but Beacon does not request them, so a Run can still emit UDP traffic. Recorded in
  [ADR 0029](docs/adr/0029-landlock-sandbox-proven-before-trusted.md).
- **Hosts that are not Linux have no sandbox.** They report `Unavailable` and Execution refuses to
  run there. Alpha 1 names a Linux build.
- **Alpha relies on operating-system disk encryption** for live Workspace data. Beacon does not
  describe its own store as encrypted.
- **A Run's declared checks are the repository's own code.** Beacon bounds and contains them; it
  does not audit what they do inside those bounds.

If you can defeat one of the bounds rather than observe its documented edge, that is in scope and
we want to hear about it.
