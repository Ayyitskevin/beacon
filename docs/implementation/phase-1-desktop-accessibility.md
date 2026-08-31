# Phase 1 desktop keyboard and focus evidence

This checkpoint proves one bounded rendered-desktop path: an authenticated Operator can use the
keyboard to enter Beacon's main content or move through the available application navigation into
Projects. It does not claim that every core workflow is keyboard-complete or that Beacon has passed
its manual accessibility acceptance gate.

## Keyboard path

The skip link is the shell's first sequential focus stop. Activating it with Enter explicitly moves
focus to the `main` landmark. The landmark has `tabIndex={-1}`, so it can receive programmatic focus
without becoming an extra ordinary Tab stop.

After an authenticated Workspace opens, Tab reaches Briefing and Projects in document order.
Conversations, Knowledge, and Software remain disabled and outside sequential keyboard navigation.
Enter and Space both activate the Projects button, update its `aria-current` state, and leave the
next Tab stop on the Projects workflow's Create Project command.

## Automated evidence

`App.test.tsx` exercises these behaviors through the rendered DOM with
`@testing-library/user-event`:

- the focus-transfer test was first observed failing against the previous implementation because
  Enter left focus on the skip link; and
- the navigation test characterizes the existing Fluent button behavior for both Enter and Space,
  including the next focus stop after the three disabled application buttons.

The smallest source repair adds a reference to the `main` landmark, makes that landmark
programmatically focusable, and focuses it when the skip link is activated. It does not add custom
keyboard handling to controls whose native Fluent behavior already satisfies the tested path.

## Evidence limits

These tests establish DOM focus location and keyboard activation under jsdom. jsdom does not lay
out or paint the interface, so this checkpoint does not prove that focus indicators are visibly
rendered. It also does not establish Orca announcements, 200% text scaling, runtime reduced-motion
behavior, rendered contrast, AppImage behavior, or any untested workflow. The Blueprint still
requires manual keyboard and Orca testing on supported Linux in addition to automated checks.

## Verification

From `apps/desktop`:

```sh
npx vitest run src/App.test.tsx -t "moves focus to main content"
npx vitest run src/App.test.tsx -t "enter Projects and continue"
```

From the repository root:

```sh
scripts/gate desktop
scripts/gate all
```
