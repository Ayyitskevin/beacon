use std::path::PathBuf;
use std::process::ExitCode;

#[cfg(unix)]
use beacond::{LocalServerConfig, LocalSupervisorServer};
use beacond::{StartupConfig, Supervisor};

const USAGE: &str = "\
Usage:
  beacond bootstrap [--store PATH] [--workspace NAME]
  beacond serve --store PATH --socket PATH --auth-token PATH --lock PATH [--workspace NAME]

Bootstrap defaults:
  --store .beacon-alpha/beacon.sqlite3
  --workspace Personal
";

enum Command {
    Bootstrap(StartupConfig),
    #[cfg(unix)]
    Serve(LocalServerConfig),
}

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if matches!(arguments.as_slice(), [help] if help == "-h" || help == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match parse_arguments(arguments.into_iter()).and_then(execute) {
        Ok(Some(output)) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("beacond: {error}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn execute(command: Command) -> Result<Option<String>, String> {
    match command {
        Command::Bootstrap(config) => {
            let report = Supervisor::bootstrap(config).map_err(|error| error.to_string())?;
            serde_json::to_string_pretty(&report)
                .map(Some)
                .map_err(|error| error.to_string())
        }
        #[cfg(unix)]
        Command::Serve(config) => LocalSupervisorServer::bind(config)
            .and_then(LocalSupervisorServer::serve)
            .map(|()| None)
            .map_err(|error| error.to_string()),
    }
}

fn parse_arguments(mut arguments: impl Iterator<Item = String>) -> Result<Command, String> {
    match arguments.next().as_deref() {
        Some("bootstrap") => parse_bootstrap(arguments).map(Command::Bootstrap),
        #[cfg(unix)]
        Some("serve") => parse_serve(arguments).map(Command::Serve),
        Some(command) => Err(format!("unknown command: {command}")),
        None => Err("a command is required".to_owned()),
    }
}

fn parse_bootstrap(mut arguments: impl Iterator<Item = String>) -> Result<StartupConfig, String> {
    let mut store_path = PathBuf::from(".beacon-alpha/beacon.sqlite3");
    let mut workspace_name = "Personal".to_owned();
    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--store" => store_path = required_path(&mut arguments, "--store")?,
            "--workspace" => workspace_name = required_value(&mut arguments, "--workspace")?,
            _ => return Err(format!("unknown option: {option}")),
        }
    }
    Ok(StartupConfig {
        store_path,
        initial_workspace_name: workspace_name,
    })
}

#[cfg(unix)]
fn parse_serve(mut arguments: impl Iterator<Item = String>) -> Result<LocalServerConfig, String> {
    let mut store_path = None;
    let mut socket_path = None;
    let mut auth_token_path = None;
    let mut lock_path = None;
    let mut workspace_name = "Personal".to_owned();
    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--store" => store_path = Some(required_path(&mut arguments, "--store")?),
            "--socket" => socket_path = Some(required_path(&mut arguments, "--socket")?),
            "--auth-token" => {
                auth_token_path = Some(required_path(&mut arguments, "--auth-token")?)
            }
            "--lock" => lock_path = Some(required_path(&mut arguments, "--lock")?),
            "--workspace" => workspace_name = required_value(&mut arguments, "--workspace")?,
            _ => return Err(format!("unknown option: {option}")),
        }
    }
    Ok(LocalServerConfig {
        store_path: store_path.ok_or_else(|| "serve requires --store".to_owned())?,
        initial_workspace_name: workspace_name,
        socket_path: socket_path.ok_or_else(|| "serve requires --socket".to_owned())?,
        auth_token_path: auth_token_path.ok_or_else(|| "serve requires --auth-token".to_owned())?,
        lock_path: lock_path.ok_or_else(|| "serve requires --lock".to_owned())?,
    })
}

fn required_path(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<PathBuf, String> {
    required_value(arguments, option).map(PathBuf::from)
}

fn required_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

#[cfg(test)]
mod tests {
    use super::{Command, parse_arguments};

    #[test]
    fn parser_uses_safe_local_bootstrap_defaults() {
        let Command::Bootstrap(config) =
            parse_arguments(["bootstrap".to_owned()].into_iter()).unwrap()
        else {
            panic!("bootstrap command parsed as another command");
        };

        assert_eq!(
            config.store_path.to_string_lossy(),
            ".beacon-alpha/beacon.sqlite3"
        );
        assert_eq!(config.initial_workspace_name, "Personal");
    }

    #[test]
    fn parser_rejects_unknown_options() {
        let error = parse_arguments(["bootstrap".to_owned(), "--surprise".to_owned()].into_iter())
            .err()
            .unwrap();

        assert_eq!(error, "unknown option: --surprise");
    }

    #[cfg(unix)]
    #[test]
    fn serve_requires_every_private_runtime_path() {
        let error = parse_arguments(
            [
                "serve".to_owned(),
                "--store".to_owned(),
                "store.db".to_owned(),
            ]
            .into_iter(),
        )
        .err()
        .unwrap();

        assert_eq!(error, "serve requires --socket");
    }
}
