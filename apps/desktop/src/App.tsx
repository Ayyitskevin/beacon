import {
  Button,
  FluentProvider,
  makeStyles,
  shorthands,
  tokens,
} from "@fluentui/react-components";
import { useEffect, useState } from "react";

import { loadSessionStatus } from "./session";
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
  navigationButton: {
    justifyContent: "flex-start",
  },
  main: {
    display: "grid",
    alignContent: "center",
    minWidth: 0,
    ...shorthands.padding(tokens.spacingVerticalXXXL, tokens.spacingHorizontalXXXL),
  },
  attention: {
    maxWidth: "640px",
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
  inspector: {
    backgroundColor: tokens.colorNeutralBackground1,
    ...shorthands.borderLeft(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.padding(tokens.spacingVerticalXXL, tokens.spacingHorizontalL),
  },
  inspectorHeading: {
    fontSize: tokens.fontSizeBase400,
    marginTop: 0,
  },
  status: {
    color: tokens.colorNeutralForeground2,
  },
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
  const [sessionState, setSessionState] = useState<SessionViewState>("loading");
  const [sessionAttempt, setSessionAttempt] = useState(0);
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
    setSessionState("loading");
    loadSessionStatus().then(
      () => {
        if (active) setSessionState("ready");
      },
      () => {
        if (active) setSessionState("error");
      },
    );
    return () => {
      active = false;
    };
  }, [sessionAttempt]);

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
              disabled
            >
              {application}
            </Button>
          ))}
        </nav>
        <main id="main-content" className={classes.main}>
          {sessionState === "error" ? (
            <div role="alert">
              <p>Beacon could not verify the local session.</p>
              <Button appearance="primary" onClick={() => setSessionAttempt((value) => value + 1)}>
                Check again
              </Button>
            </div>
          ) : sessionState === "loading" ? (
            <p role="status">Checking local session</p>
          ) : (
            <section className={classes.attention} aria-labelledby="setup-heading">
              <h1 id="setup-heading" className={classes.heading}>
                Set up your operator account
              </h1>
              <p className={classes.body}>
                Account security is the next implementation checkpoint. Beacon keeps Workspace
                data and Operator Commands closed until setup is available.
              </p>
            </section>
          )}
        </main>
        <aside
          className={`${classes.inspector} desktop-inspector`}
          aria-labelledby="installation-heading"
        >
          <h2 id="installation-heading" className={classes.inspectorHeading}>
            Installation
          </h2>
          <p className={classes.status}>Workspace access locked</p>
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
