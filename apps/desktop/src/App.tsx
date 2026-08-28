import {
  Button,
  Checkbox,
  Field,
  FluentProvider,
  Input,
  makeStyles,
  shorthands,
  tokens,
} from "@fluentui/react-components";
import { type FormEvent, useEffect, useRef, useState } from "react";

import {
  type AuthenticationCapabilities,
  type DesktopSessionStatus,
  beginLocalPasswordSetup,
  completeLocalOperatorSetup,
  describeLocalAccountError,
  loadAuthenticationCapabilities,
  loadPlatformSessionMonitorStatus,
  loadSessionPolicy,
  loadSessionStatus,
  lockLocalSession,
  type PlatformSessionMonitorStatus,
  recordLocalOperatorActivity,
  recoverLocalOperatorAccount,
  setLocalSessionTimeout,
  signInWithLocalPassword,
} from "./session";
import { darkTheme, lightTheme } from "./theme";

const applications = ["Briefing", "Projects", "Conversations", "Knowledge", "Software"];
const themeChoices = [
  { value: "light", label: "Light", accessibleLabel: "Use light theme" },
  { value: "dark", label: "Dark", accessibleLabel: "Use dark theme" },
  { value: "system", label: "System", accessibleLabel: "Follow system theme" },
] as const;

type ThemeChoice = (typeof themeChoices)[number]["value"];
type SessionViewState = "loading" | "ready" | "error";

const useStyles = makeStyles({
  provider: {
    minHeight: "100dvh",
    backgroundColor: tokens.colorNeutralBackground2,
    color: tokens.colorNeutralForeground1,
  },
  shell: {
    minHeight: "100dvh",
    display: "grid",
    gridTemplateColumns: "220px minmax(0, 1fr) 280px",
  },
  rail: {
    display: "flex",
    flexDirection: "column",
    rowGap: tokens.spacingVerticalXS,
    backgroundColor: tokens.colorNeutralBackground1,
    ...shorthands.borderRight(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.padding(tokens.spacingVerticalXXL, tokens.spacingHorizontalL),
  },
  brand: {
    fontSize: tokens.fontSizeBase500,
    fontWeight: tokens.fontWeightSemibold,
    marginBottom: tokens.spacingVerticalXXL,
  },
  navigationButton: { justifyContent: "flex-start" },
  main: {
    display: "grid",
    alignContent: "center",
    minWidth: 0,
    ...shorthands.padding(tokens.spacingVerticalXXXL, tokens.spacingHorizontalXXXL),
  },
  attention: {
    width: "min(100%, 640px)",
    ...shorthands.borderLeft("4px", "solid", tokens.colorBrandBackground),
    paddingLeft: tokens.spacingHorizontalXXL,
  },
  heading: {
    fontSize: tokens.fontSizeHero800,
    lineHeight: tokens.lineHeightHero800,
    letterSpacing: "-0.03em",
    marginTop: 0,
    marginBottom: tokens.spacingVerticalL,
  },
  body: {
    maxWidth: "58ch",
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase400,
    lineHeight: tokens.lineHeightBase400,
  },
  methodNotice: {
    marginTop: tokens.spacingVerticalL,
    marginBottom: tokens.spacingVerticalL,
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
    backgroundColor: tokens.colorNeutralBackground3,
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
  form: {
    display: "grid",
    rowGap: tokens.spacingVerticalM,
    maxWidth: "420px",
    marginTop: tokens.spacingVerticalL,
  },
  actions: {
    display: "flex",
    columnGap: tokens.spacingHorizontalS,
    alignItems: "center",
    marginTop: tokens.spacingVerticalS,
  },
  recoveryKey: {
    display: "block",
    fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace",
    fontSize: tokens.fontSizeBase400,
    letterSpacing: "0.04em",
    overflowWrap: "anywhere",
    userSelect: "all",
    marginTop: tokens.spacingVerticalL,
    marginBottom: tokens.spacingVerticalL,
    ...shorthands.padding(tokens.spacingVerticalL, tokens.spacingHorizontalL),
    backgroundColor: tokens.colorNeutralBackground3,
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
  actionError: { color: tokens.colorPaletteRedForeground1 },
  inspector: {
    backgroundColor: tokens.colorNeutralBackground1,
    ...shorthands.borderLeft(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.padding(tokens.spacingVerticalXXL, tokens.spacingHorizontalL),
  },
  inspectorHeading: { fontSize: tokens.fontSizeBase400, marginTop: 0 },
  status: { color: tokens.colorNeutralForeground2 },
  themeHeading: {
    fontSize: tokens.fontSizeBase300,
    marginTop: tokens.spacingVerticalXXL,
    marginBottom: tokens.spacingVerticalS,
  },
  themeControls: {
    display: "grid",
    gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
    columnGap: tokens.spacingHorizontalXS,
  },
  settingsForm: {
    display: "grid",
    rowGap: tokens.spacingVerticalS,
    marginTop: tokens.spacingVerticalS,
  },
  monitorWarning: { color: tokens.colorPaletteDarkOrangeForeground1 },
});

export function App() {
  const classes = useStyles();
  const [viewState, setViewState] = useState<SessionViewState>("loading");
  const [sessionAttempt, setSessionAttempt] = useState(0);
  const [session, setSession] = useState<DesktopSessionStatus | null>(null);
  const [foregroundVerified, setForegroundVerified] = useState(false);
  const foregroundEpoch = useRef(0);
  const foregroundEligible = useRef(true);
  const [capabilities, setCapabilities] = useState<AuthenticationCapabilities | null>(null);
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [recoveryKey, setRecoveryKey] = useState("");
  const [recoverySaved, setRecoverySaved] = useState(false);
  const [recoveryMode, setRecoveryMode] = useState(false);
  const [replacementKey, setReplacementKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState("");
  const [timeoutMinutes, setTimeoutMinutes] = useState("30");
  const [platformMonitor, setPlatformMonitor] =
    useState<PlatformSessionMonitorStatus | null>(null);
  const [themeChoice, setThemeChoice] = useState<ThemeChoice>("light");
  const [systemPrefersDark, setSystemPrefersDark] = useState(
    () =>
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches,
  );
  const resolvedTheme =
    themeChoice === "system" ? (systemPrefersDark ? "dark" : "light") : themeChoice;
  const unlocked = session?.phase === "unlocked" && foregroundVerified;

  function applyVerifiedSession(nextSession: DesktopSessionStatus, expectedEpoch: number) {
    setSession(nextSession);
    setForegroundVerified(
      nextSession.phase === "unlocked" &&
        foregroundEligible.current &&
        foregroundEpoch.current === expectedEpoch &&
        document.visibilityState !== "hidden",
    );
  }

  useEffect(() => {
    let active = true;
    const verificationEpoch = foregroundEpoch.current;
    setViewState("loading");
    Promise.all([loadSessionStatus(), loadAuthenticationCapabilities()]).then(
      ([nextSession, nextCapabilities]) => {
        if (!active) return;
        applyVerifiedSession(nextSession, verificationEpoch);
        setCapabilities(nextCapabilities);
        setViewState("ready");
      },
      () => {
        if (active) setViewState("error");
      },
    );
    return () => {
      active = false;
    };
  }, [sessionAttempt]);

  useEffect(() => {
    if (!unlocked) return;
    let active = true;
    Promise.all([loadSessionPolicy(), loadPlatformSessionMonitorStatus()]).then(
      ([policy, monitor]) => {
        if (!active) return;
        setTimeoutMinutes(String(policy.inactivityTimeoutMinutes));
        setPlatformMonitor(monitor);
      },
      () => {
        if (!active) return;
        setSession(null);
        setForegroundVerified(false);
        setViewState("error");
      },
    );
    return () => {
      active = false;
    };
  }, [unlocked]);

  useEffect(() => {
    if (!unlocked) return;
    let active = true;
    let checking = false;
    const timer = window.setInterval(() => {
      if (checking) return;
      checking = true;
      void Promise.all([loadSessionStatus(), loadPlatformSessionMonitorStatus()]).then(
        ([nextSession, monitor]) => {
          if (!active) return;
          setSession(nextSession);
          if (nextSession.phase !== "unlocked") setForegroundVerified(false);
          setPlatformMonitor(monitor);
        },
        () => {
          if (!active) return;
          setSession(null);
          setForegroundVerified(false);
          setViewState("error");
        },
      ).finally(() => {
        checking = false;
      });
    }, 1_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [unlocked]);

  useEffect(() => {
    if (!unlocked) return;
    let active = true;
    let renewing = false;
    let lastRenewal = 0;
    const renew = () => {
      const now = Date.now();
      if (renewing || now - lastRenewal < 30_000) return;
      renewing = true;
      void recordLocalOperatorActivity().then(
        () => {
          lastRenewal = Date.now();
        },
        () => {
          if (!active) return;
          setSession(null);
          setForegroundVerified(false);
          setViewState("error");
        },
      ).finally(() => {
        renewing = false;
      });
    };
    document.addEventListener("pointerdown", renew);
    document.addEventListener("keydown", renew);
    return () => {
      active = false;
      document.removeEventListener("pointerdown", renew);
      document.removeEventListener("keydown", renew);
    };
  }, [unlocked]);

  useEffect(() => {
    let active = true;
    const conceal = () => {
      foregroundEligible.current = false;
      foregroundEpoch.current += 1;
      setForegroundVerified(false);
    };
    const verifyBeforeReveal = () => {
      if (document.visibilityState === "hidden") return;
      foregroundEligible.current = true;
      const verificationEpoch = ++foregroundEpoch.current;
      setForegroundVerified(false);
      void loadSessionStatus().then(
        (nextSession) => {
          if (!active) return;
          applyVerifiedSession(nextSession, verificationEpoch);
        },
        () => {
          if (!active) return;
          setSession(null);
          setForegroundVerified(false);
          setViewState("error");
        },
      );
    };
    const handleVisibilityChange = () => {
      if (document.visibilityState === "hidden") conceal();
      else verifyBeforeReveal();
    };
    window.addEventListener("blur", conceal);
    window.addEventListener("focus", verifyBeforeReveal);
    document.addEventListener("visibilitychange", handleVisibilityChange);
    return () => {
      active = false;
      window.removeEventListener("blur", conceal);
      window.removeEventListener("focus", verifyBeforeReveal);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, []);

  useEffect(() => {
    document.documentElement.dataset.theme = resolvedTheme;
    document.documentElement.style.colorScheme = resolvedTheme;
  }, [resolvedTheme]);

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const updateSystemTheme = (event: MediaQueryListEvent) => setSystemPrefersDark(event.matches);
    setSystemPrefersDark(mediaQuery.matches);
    mediaQuery.addEventListener("change", updateSystemTheme);
    return () => mediaQuery.removeEventListener("change", updateSystemTheme);
  }, []);

  async function submitSetup(event: FormEvent) {
    event.preventDefault();
    const submittedPassword = password;
    const submittedConfirmation = confirmation;
    setPassword("");
    setConfirmation("");
    await runAction(async () => {
      const output = await beginLocalPasswordSetup(submittedPassword, submittedConfirmation);
      setRecoveryKey(output.recoveryKey);
    });
  }

  async function finishSetup() {
    await runAction(async () => {
      const verificationEpoch = foregroundEpoch.current;
      const nextSession = await completeLocalOperatorSetup(recoverySaved);
      setRecoveryKey("");
      setRecoverySaved(false);
      applyVerifiedSession(nextSession, verificationEpoch);
    });
  }

  async function submitSignIn(event: FormEvent) {
    event.preventDefault();
    const submittedPassword = password;
    setPassword("");
    await runAction(async () => {
      const verificationEpoch = foregroundEpoch.current;
      const nextSession = await signInWithLocalPassword(submittedPassword);
      applyVerifiedSession(nextSession, verificationEpoch);
    });
  }

  async function submitRecovery(event: FormEvent) {
    event.preventDefault();
    const submittedKey = recoveryKey;
    const submittedPassword = password;
    const submittedConfirmation = confirmation;
    setRecoveryKey("");
    setPassword("");
    setConfirmation("");
    await runAction(async () => {
      const verificationEpoch = foregroundEpoch.current;
      const output = await recoverLocalOperatorAccount(
        submittedKey,
        submittedPassword,
        submittedConfirmation,
      );
      setReplacementKey(output.recoveryKey);
      const nextSession = await loadSessionStatus();
      applyVerifiedSession(nextSession, verificationEpoch);
    });
  }

  async function lockSession() {
    await runAction(async () => {
      setSession(await lockLocalSession());
      setForegroundVerified(false);
      setRecoveryMode(false);
      setReplacementKey("");
    });
  }

  async function saveSessionPolicy(event: FormEvent) {
    event.preventDefault();
    const value = Number(timeoutMinutes);
    await runAction(async () => {
      const policy = await setLocalSessionTimeout(value);
      setTimeoutMinutes(String(policy.inactivityTimeoutMinutes));
    });
  }

  async function runAction(action: () => Promise<void>) {
    setBusy(true);
    setActionError("");
    try {
      await action();
    } catch (error) {
      setActionError(describeLocalAccountError(error));
    } finally {
      setBusy(false);
    }
  }

  return (
    <FluentProvider
      className={classes.provider}
      theme={resolvedTheme === "dark" ? darkTheme : lightTheme}
    >
      <a href="#main-content">Skip to main content</a>
      <div className={`${classes.shell} desktop-shell`}>
        <nav className={classes.rail} aria-label="Beacon applications">
          <div className={classes.brand}>Beacon</div>
          {applications.map((application) => (
            <Button
              key={application}
              className={classes.navigationButton}
              appearance="subtle"
              disabled={!unlocked}
            >
              {application}
            </Button>
          ))}
        </nav>
        <main id="main-content" className={classes.main}>
          {viewState === "error" ? (
            <div role="alert">
              <p>Beacon could not verify the local session.</p>
              <Button appearance="primary" onClick={() => setSessionAttempt((value) => value + 1)}>
                Check again
              </Button>
            </div>
          ) : viewState === "loading" || !session || !capabilities ? (
            <p role="status">Checking local session</p>
          ) : session.phase === "unlocked" && !foregroundVerified ? (
            <section className={classes.attention} aria-labelledby="concealed-heading">
              <h1 id="concealed-heading" className={classes.heading}>
                Beacon is concealed
              </h1>
              <p className={classes.body}>Rechecking the local session before showing Workspace data.</p>
            </section>
          ) : session.phase === "operator_setup_required" ? (
            <section className={classes.attention} aria-labelledby="setup-heading">
              <h1 id="setup-heading" className={classes.heading}>
                Set up your operator account
              </h1>
              {recoveryKey ? (
                <RecoveryKeyStep
                  classes={classes}
                  recoveryKey={recoveryKey}
                  saved={recoverySaved}
                  busy={busy}
                  error={actionError}
                  onSavedChange={setRecoverySaved}
                  onFinish={finishSetup}
                />
              ) : (
                <>
                  <p className={classes.body}>
                    The Owner account stays on this computer. Your password and Recovery Key are
                    never sent to a cloud service.
                  </p>
                  <PasskeyNotice capabilities={capabilities} className={classes.methodNotice} />
                  <PasswordForm
                    classes={classes}
                    password={password}
                    confirmation={confirmation}
                    busy={busy}
                    error={actionError}
                    submitLabel="Create recovery key"
                    onPasswordChange={setPassword}
                    onConfirmationChange={setConfirmation}
                    onSubmit={submitSetup}
                  />
                </>
              )}
            </section>
          ) : session.phase === "locked" ? (
            <section className={classes.attention} aria-labelledby="locked-heading">
              <h1 id="locked-heading" className={classes.heading}>
                Welcome back
              </h1>
              <PasskeyNotice capabilities={capabilities} className={classes.methodNotice} />
              {recoveryMode ? (
                <form className={classes.form} onSubmit={submitRecovery}>
                  <Field label="Recovery Key" required>
                    <Input
                      aria-label="Recovery Key"
                      value={recoveryKey}
                      autoComplete="off"
                      onChange={(_, data) => setRecoveryKey(data.value)}
                      required
                    />
                  </Field>
                  <PasswordFields
                    password={password}
                    confirmation={confirmation}
                    onPasswordChange={setPassword}
                    onConfirmationChange={setConfirmation}
                  />
                  {actionError && <p className={classes.actionError}>{actionError}</p>}
                  <div className={classes.actions}>
                    <Button appearance="primary" type="submit" disabled={busy}>
                      Recover account
                    </Button>
                    <Button type="button" disabled={busy} onClick={() => setRecoveryMode(false)}>
                      Back
                    </Button>
                  </div>
                </form>
              ) : (
                <form className={classes.form} onSubmit={submitSignIn}>
                  <Field label="Password" required>
                    <Input
                      aria-label="Password"
                      type="password"
                      value={password}
                      autoComplete="current-password"
                      onChange={(_, data) => setPassword(data.value)}
                      required
                    />
                  </Field>
                  {actionError && <p className={classes.actionError}>{actionError}</p>}
                  <div className={classes.actions}>
                    <Button appearance="primary" type="submit" disabled={busy}>
                      Unlock Beacon
                    </Button>
                    <Button type="button" appearance="subtle" onClick={() => setRecoveryMode(true)}>
                      Use Recovery Key
                    </Button>
                  </div>
                </form>
              )}
            </section>
          ) : (
            <section className={classes.attention} aria-labelledby="workspace-heading">
              <h1 id="workspace-heading" className={classes.heading}>
                Owner workspace
              </h1>
              <p className={classes.body}>
                Your local Operator session is active. Authentication opens Beacon; Policy and
                Gates still control every agent effect.
              </p>
              {replacementKey && (
                <>
                  <p>A new one-time Recovery Key replaced the old key. Save it offline now.</p>
                  <code className={classes.recoveryKey}>{replacementKey}</code>
                </>
              )}
              {actionError && <p className={classes.actionError}>{actionError}</p>}
              <div className={classes.actions}>
                <Button onClick={lockSession} disabled={busy}>
                  Lock Beacon
                </Button>
              </div>
            </section>
          )}
        </main>
        <aside className={`${classes.inspector} desktop-inspector`} aria-labelledby="installation-heading">
          <h2 id="installation-heading" className={classes.inspectorHeading}>
            Installation
          </h2>
          <p className={classes.status}>
            Workspace access {unlocked ? "open" : "locked"}
          </p>
          <p className={classes.status}>Network activity off</p>
          {unlocked && (
            <>
              <h3 className={classes.themeHeading}>Session</h3>
              <form className={classes.settingsForm} onSubmit={saveSessionPolicy}>
                <Field label="Lock after inactivity" hint="Minutes, from 1 to 1,440">
                  <Input
                    aria-label="Lock after inactivity in minutes"
                    type="number"
                    min={1}
                    max={1440}
                    value={timeoutMinutes}
                    onChange={(_, data) => setTimeoutMinutes(data.value)}
                    required
                  />
                </Field>
                <Button type="submit" size="small" disabled={busy}>
                  Save timeout
                </Button>
              </form>
              {platformMonitor &&
                (platformMonitor.screenLock === "unavailable" ||
                  platformMonitor.suspend === "unavailable") && (
                  <p className={classes.monitorWarning} role="status">
                    Automatic OS lock monitoring is unavailable. Manual and inactivity locking
                    remain active.
                  </p>
                )}
              {platformMonitor &&
                platformMonitor.screenLock !== "unavailable" &&
                platformMonitor.suspend !== "unavailable" && (
                  <p className={classes.status}>
                    OS lock monitoring {platformMonitor.screenLock === "active" &&
                    platformMonitor.suspend === "active"
                      ? "active"
                      : "starting"}
                  </p>
                )}
            </>
          )}
          <h3 className={classes.themeHeading}>Appearance</h3>
          <div className={classes.themeControls} role="group" aria-label="Appearance">
            {themeChoices.map((choice) => (
              <Button
                key={choice.value}
                aria-label={choice.accessibleLabel}
                aria-pressed={themeChoice === choice.value}
                appearance={themeChoice === choice.value ? "primary" : "subtle"}
                size="small"
                onClick={() => setThemeChoice(choice.value)}
              >
                {choice.label}
              </Button>
            ))}
          </div>
        </aside>
      </div>
    </FluentProvider>
  );
}

type StyleClasses = ReturnType<typeof useStyles>;

function PasskeyNotice({
  capabilities,
  className,
}: {
  capabilities: AuthenticationCapabilities;
  className: string;
}) {
  return (
    <div className={className}>
      <strong>Passkey preferred</strong>
      <div>
        {capabilities.passkey.reason === "unsupported_linux_webkitgtk"
          ? "Passkeys are not available on this Linux build. Use the same-machine password fallback."
          : "A native passkey adapter is not available in this build. Use the same-machine password fallback."}
      </div>
    </div>
  );
}

function PasswordFields({
  password,
  confirmation,
  onPasswordChange,
  onConfirmationChange,
}: {
  password: string;
  confirmation: string;
  onPasswordChange: (value: string) => void;
  onConfirmationChange: (value: string) => void;
}) {
  return (
    <>
      <Field label="Password" hint="Use at least 15 characters. Spaces and Unicode are welcome." required>
        <Input
          aria-label="Password"
          type="password"
          value={password}
          minLength={15}
          maxLength={1024}
          autoComplete="new-password"
          onChange={(_, data) => onPasswordChange(data.value)}
          required
        />
      </Field>
      <Field label="Confirm password" required>
        <Input
          aria-label="Confirm password"
          type="password"
          value={confirmation}
          minLength={15}
          maxLength={1024}
          autoComplete="new-password"
          onChange={(_, data) => onConfirmationChange(data.value)}
          required
        />
      </Field>
    </>
  );
}

function PasswordForm({
  classes,
  password,
  confirmation,
  busy,
  error,
  submitLabel,
  onPasswordChange,
  onConfirmationChange,
  onSubmit,
}: {
  classes: StyleClasses;
  password: string;
  confirmation: string;
  busy: boolean;
  error: string;
  submitLabel: string;
  onPasswordChange: (value: string) => void;
  onConfirmationChange: (value: string) => void;
  onSubmit: (event: FormEvent) => void;
}) {
  return (
    <form className={classes.form} onSubmit={onSubmit}>
      <PasswordFields
        password={password}
        confirmation={confirmation}
        onPasswordChange={onPasswordChange}
        onConfirmationChange={onConfirmationChange}
      />
      {error && <p className={classes.actionError}>{error}</p>}
      <div className={classes.actions}>
        <Button appearance="primary" type="submit" disabled={busy}>
          {submitLabel}
        </Button>
      </div>
    </form>
  );
}

function RecoveryKeyStep({
  classes,
  recoveryKey,
  saved,
  busy,
  error,
  onSavedChange,
  onFinish,
}: {
  classes: StyleClasses;
  recoveryKey: string;
  saved: boolean;
  busy: boolean;
  error: string;
  onSavedChange: (value: boolean) => void;
  onFinish: () => void;
}) {
  return (
    <>
      <p className={classes.body}>
        Save this one-time Recovery Key somewhere offline. Beacon stores only a verifier and cannot
        show this key again.
      </p>
      <code className={classes.recoveryKey}>{recoveryKey}</code>
      <Checkbox
        checked={saved}
        label="I saved this Recovery Key offline"
        onChange={(_, data) => onSavedChange(data.checked === true)}
      />
      {error && <p className={classes.actionError}>{error}</p>}
      <div className={classes.actions}>
        <Button appearance="primary" onClick={onFinish} disabled={!saved || busy}>
          Finish setup
        </Button>
      </div>
    </>
  );
}
