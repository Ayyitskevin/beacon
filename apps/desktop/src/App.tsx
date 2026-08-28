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
import { type FormEvent, useEffect, useState } from "react";

import {
  type AuthenticationCapabilities,
  type DesktopSessionStatus,
  beginLocalPasswordSetup,
  completeLocalOperatorSetup,
  describeLocalAccountError,
  loadAuthenticationCapabilities,
  loadSessionStatus,
  lockLocalSession,
  recoverLocalOperatorAccount,
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
});

export function App() {
  const classes = useStyles();
  const [viewState, setViewState] = useState<SessionViewState>("loading");
  const [sessionAttempt, setSessionAttempt] = useState(0);
  const [session, setSession] = useState<DesktopSessionStatus | null>(null);
  const [capabilities, setCapabilities] = useState<AuthenticationCapabilities | null>(null);
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [recoveryKey, setRecoveryKey] = useState("");
  const [recoverySaved, setRecoverySaved] = useState(false);
  const [recoveryMode, setRecoveryMode] = useState(false);
  const [replacementKey, setReplacementKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState("");
  const [themeChoice, setThemeChoice] = useState<ThemeChoice>("light");
  const [systemPrefersDark, setSystemPrefersDark] = useState(
    () =>
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches,
  );
  const resolvedTheme =
    themeChoice === "system" ? (systemPrefersDark ? "dark" : "light") : themeChoice;

  useEffect(() => {
    let active = true;
    setViewState("loading");
    Promise.all([loadSessionStatus(), loadAuthenticationCapabilities()]).then(
      ([nextSession, nextCapabilities]) => {
        if (!active) return;
        setSession(nextSession);
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
    if (session?.phase !== "unlocked") return;
    let active = true;
    let checking = false;
    const timer = window.setInterval(() => {
      if (checking) return;
      checking = true;
      void loadSessionStatus().then(
        (nextSession) => {
          if (active) setSession(nextSession);
        },
        () => {
          if (!active) return;
          setSession(null);
          setViewState("error");
        },
      ).finally(() => {
        checking = false;
      });
    }, 15_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [session?.phase]);

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
      const nextSession = await completeLocalOperatorSetup(recoverySaved);
      setRecoveryKey("");
      setRecoverySaved(false);
      setSession(nextSession);
    });
  }

  async function submitSignIn(event: FormEvent) {
    event.preventDefault();
    const submittedPassword = password;
    setPassword("");
    await runAction(async () => setSession(await signInWithLocalPassword(submittedPassword)));
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
      const output = await recoverLocalOperatorAccount(
        submittedKey,
        submittedPassword,
        submittedConfirmation,
      );
      setReplacementKey(output.recoveryKey);
      setSession(await loadSessionStatus());
    });
  }

  async function lockSession() {
    await runAction(async () => {
      setSession(await lockLocalSession());
      setRecoveryMode(false);
      setReplacementKey("");
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

  const unlocked = session?.phase === "unlocked";

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
