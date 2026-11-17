use crate::{
    clipboard::{self, ClipboardSummary},
    config::Config,
    tools::{Availability, ToolDescriptor},
};
use serde::Serialize;

pub struct Response {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Response {
    fn success(stdout: impl Into<String>) -> Self {
        Self {
            code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn failure(code: i32, message: impl Into<String>, json: bool) -> Self {
        let message = message.into();
        if json {
            let output = serde_json::json!({
                "error": { "code": code, "message": message }
            });
            Self {
                code,
                stdout: serde_json::to_string_pretty(&output).unwrap(),
                stderr: String::new(),
            }
        } else {
            Self {
                code,
                stdout: String::new(),
                stderr: format!("TrayKit: {message}"),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command<'a> {
    Help,
    Version,
    List { json: bool },
    Run { id: &'a str },
    Doctor { json: bool },
}

#[derive(Serialize)]
struct ActionStatus {
    #[serde(flatten)]
    descriptor: ToolDescriptor,
    enabled: bool,
    availability: Availability,
}

#[derive(Serialize)]
struct Check {
    ok: bool,
    detail: String,
}

#[derive(Serialize)]
struct DoctorReport {
    version: &'static str,
    ok: bool,
    config_path: Option<String>,
    config: Check,
    clipboard: Check,
    clipboard_summary: Option<ClipboardSummary>,
    registered_action_count: usize,
}

const HELP: &str = "TrayKit
  no arguments: start the system tray
  actions list [--json]: list registered actions and current availability
  run <action-id>: run one enabled action without starting the tray
  doctor [--json]: check configuration, registry and clipboard visibility
  --convert: legacy alias for `run clipboard-image`
  --version
Config: %APPDATA%\\TrayKit\\config.toml (or TRAYKIT_CONFIG)";

fn parse(args: &[String]) -> Result<Command<'_>, String> {
    match args {
        [arg] if arg == "--help" || arg == "-h" => Ok(Command::Help),
        [arg] if arg == "--version" => Ok(Command::Version),
        [first, second] if first == "actions" && second == "list" => {
            Ok(Command::List { json: false })
        }
        [first, second, format] if first == "actions" && second == "list" && format == "--json" => {
            Ok(Command::List { json: true })
        }
        [command, id] if command == "run" => Ok(Command::Run { id }),
        [arg] if arg == "--convert" => Ok(Command::Run {
            id: "clipboard-image",
        }),
        [command] if command == "doctor" => Ok(Command::Doctor { json: false }),
        [command, format] if command == "doctor" && format == "--json" => {
            Ok(Command::Doctor { json: true })
        }
        _ => Err("Unknown argument. Use --help".into()),
    }
}

fn action_statuses(config: &Config, summary: &ClipboardSummary) -> Vec<ActionStatus> {
    crate::tools::registered()
        .iter()
        .map(|tool| ActionStatus {
            descriptor: tool.descriptor(),
            enabled: config.is_enabled(tool.id()),
            availability: tool.availability(summary),
        })
        .collect()
}

fn list(json: bool) -> Response {
    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => return Response::failure(1, format!("{error:#}"), json),
    };
    let summary = match clipboard::summarize() {
        Ok(summary) => summary,
        Err(error) => return Response::failure(1, format!("{error:#}"), json),
    };
    let statuses = action_statuses(&config, &summary);
    if json {
        return Response::success(
            serde_json::to_string_pretty(&serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "actions": statuses,
            }))
            .unwrap(),
        );
    }
    let lines = statuses
        .into_iter()
        .map(|status| {
            let state = if !status.enabled {
                "disabled"
            } else if status.availability.is_available() {
                "available"
            } else {
                "unavailable"
            };
            format!(
                "{}\t{}\t{}",
                status.descriptor.id, state, status.descriptor.name
            )
        })
        .collect::<Vec<_>>();
    Response::success(lines.join("\n"))
}

fn run(id: &str) -> Response {
    let tools = crate::tools::registered();
    let Some(tool) = tools.iter().find(|tool| tool.id() == id) else {
        return Response::failure(2, format!("Unknown action ID: {id}"), false);
    };
    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => return Response::failure(1, format!("{error:#}"), false),
    };
    if !config.is_enabled(id) {
        return Response::failure(3, format!("Action is disabled in settings: {id}"), false);
    }
    let summary = match clipboard::summarize() {
        Ok(summary) => summary,
        Err(error) => return Response::failure(1, format!("{error:#}"), false),
    };
    let availability = tool.availability(&summary);
    if let Some(reason) = availability.reason() {
        return Response::failure(3, reason, false);
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tool.run()));
    match result {
        Ok(Ok(message)) => Response::success(message),
        Ok(Err(error)) => Response::failure(1, format!("{error:#}"), false),
        Err(_) => Response::failure(1, "Action panicked", false),
    }
}

fn doctor(json: bool) -> Response {
    let config_path = Config::path().map(|path| path.display().to_string());
    let config = Config::load();
    let clipboard = clipboard::summarize();
    let config_check = Check {
        ok: config.is_ok(),
        detail: match &config {
            Ok(_) => "configuration loaded and validated".into(),
            Err(error) => format!("{error:#}"),
        },
    };
    let clipboard_check = Check {
        ok: clipboard.is_ok(),
        detail: match &clipboard {
            Ok(summary) => format!(
                "sequence {} observed without opening the clipboard",
                summary.sequence
            ),
            Err(error) => format!("{error:#}"),
        },
    };
    let ok = config_check.ok && clipboard_check.ok && config_path.is_ok();
    let report = DoctorReport {
        version: env!("CARGO_PKG_VERSION"),
        ok,
        config_path: config_path.ok(),
        config: config_check,
        clipboard: clipboard_check,
        clipboard_summary: clipboard.ok(),
        registered_action_count: crate::tools::registered().len(),
    };
    let output = if json {
        serde_json::to_string_pretty(&report).unwrap()
    } else {
        format!(
            "TrayKit {}\noverall: {}\nconfig: {}\nclipboard: {}\nactions: {}",
            report.version,
            if report.ok { "ok" } else { "failed" },
            report.config.detail,
            report.clipboard.detail,
            report.registered_action_count
        )
    };
    Response {
        code: if report.ok { 0 } else { 1 },
        stdout: output,
        stderr: String::new(),
    }
}

pub fn execute(args: &[String]) -> Response {
    match parse(args) {
        Ok(Command::Help) => Response::success(HELP),
        Ok(Command::Version) => Response::success(format!("TrayKit {}", env!("CARGO_PKG_VERSION"))),
        Ok(Command::List { json }) => list(json),
        Ok(Command::Run { id }) => run(id),
        Ok(Command::Doctor { json }) => doctor(json),
        Err(message) => Response::failure(2, message, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn parser_preserves_legacy_alias_and_rejects_ambiguous_forms() {
        assert_eq!(
            parse(&strings(&["--convert"])),
            Ok(Command::Run {
                id: "clipboard-image"
            })
        );
        assert_eq!(
            parse(&strings(&["actions", "list", "--json"])),
            Ok(Command::List { json: true })
        );
        assert_eq!(
            parse(&strings(&["doctor", "--json"])),
            Ok(Command::Doctor { json: true })
        );
        assert!(parse(&strings(&["run"])).is_err());
        assert!(parse(&strings(&["actions", "list", "extra"])).is_err());
    }

    #[test]
    fn action_json_uses_the_same_registry_and_reflects_enablement() {
        let mut config = Config::default();
        config.disabled_tools.insert("path-rdp".into());
        let summary = ClipboardSummary {
            sequence: 7,
            unicode_text: true,
            image: false,
            file_drop: false,
            virtual_file: false,
        };
        let value = serde_json::to_value(action_statuses(&config, &summary)).unwrap();
        assert_eq!(
            value.as_array().unwrap().len(),
            crate::tools::registered().len()
        );
        assert_eq!(value[0]["id"], "clipboard-image");
        assert_eq!(value[0]["availability"]["state"], "unavailable");
        assert_eq!(value[1]["availability"]["state"], "available");
        assert_eq!(value[3]["enabled"], false);
    }

    #[test]
    fn machine_readable_errors_are_valid_json() {
        let response = Response::failure(3, "blocked", true);
        assert_eq!(response.code, 3);
        let value: serde_json::Value = serde_json::from_str(&response.stdout).unwrap();
        assert_eq!(value["error"]["code"], 3);
        assert!(response.stderr.is_empty());
    }
}
