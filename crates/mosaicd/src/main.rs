use std::path::PathBuf;
use std::process::ExitCode;

use mosaicd::{StartupConfig, Supervisor};

const USAGE: &str = "\
Usage:
  mosaicd bootstrap [--store PATH] [--workspace NAME]

Defaults:
  --store .mosaic-alpha/mosaic.sqlite3
  --workspace Personal
";

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if matches!(arguments.as_slice(), [help] if help == "-h" || help == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    match run(arguments.into_iter()) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("mosaicd: {error}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(arguments: impl Iterator<Item = String>) -> Result<String, String> {
    let config = parse_arguments(arguments)?;
    let report = Supervisor::bootstrap(config).map_err(|error| error.to_string())?;
    serde_json::to_string_pretty(&report).map_err(|error| error.to_string())
}

fn parse_arguments(mut arguments: impl Iterator<Item = String>) -> Result<StartupConfig, String> {
    match arguments.next().as_deref() {
        Some("bootstrap") => {}
        Some(command) => return Err(format!("unknown command: {command}")),
        None => return Err("a command is required".to_owned()),
    }

    let mut store_path = PathBuf::from(".mosaic-alpha/mosaic.sqlite3");
    let mut workspace_name = "Personal".to_owned();

    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--store" => {
                store_path = PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--store requires a path".to_owned())?,
                );
            }
            "--workspace" => {
                workspace_name = arguments
                    .next()
                    .ok_or_else(|| "--workspace requires a name".to_owned())?;
            }
            _ => return Err(format!("unknown option: {option}")),
        }
    }

    Ok(StartupConfig {
        store_path,
        initial_workspace_name: workspace_name,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_arguments;

    #[test]
    fn parser_uses_safe_local_defaults() {
        let config = parse_arguments(["bootstrap".to_owned()].into_iter()).unwrap();

        assert_eq!(
            config.store_path.to_string_lossy(),
            ".mosaic-alpha/mosaic.sqlite3"
        );
        assert_eq!(config.initial_workspace_name, "Personal");
    }

    #[test]
    fn parser_rejects_unknown_options() {
        let error = parse_arguments(["bootstrap".to_owned(), "--surprise".to_owned()].into_iter())
            .unwrap_err();

        assert_eq!(error, "unknown option: --surprise");
    }
}
