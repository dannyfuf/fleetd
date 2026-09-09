use clap::Parser;
use fleet_proto::{
    response::{DoctorCheck, DoctorStatus},
    snapshot::{AgentBinaries, HostStatus, LinkState},
};

#[path = "../src/args.rs"]
mod args;

mod envelope {
    use fleet_proto::error::{ErrorKind, ProtoError};
    use serde::Serialize;

    pub fn to_json(value: &impl Serialize) -> Result<String, ProtoError> {
        serde_json::to_string_pretty(value).map_err(|error| ProtoError {
            kind: ErrorKind::Unknown,
            message: error.to_string(),
        })
    }
}

mod human {
    use fleet_proto::response::{DoctorCheck, DoctorStatus};

    pub fn doctor(checks: &[DoctorCheck]) -> String {
        let mut lines = vec!["CHECK STATUS DETAIL".to_owned()];
        lines.extend(checks.iter().map(|check| {
            let status = match check.status {
                DoctorStatus::Ok => "ok",
                DoctorStatus::Warn => "warn",
                DoctorStatus::Fail => "fail",
            };
            format!("{} {status} {}", check.check, check.detail)
        }));
        lines.join("\n")
    }

    pub(crate) fn columns(rows: &[Vec<String>]) -> Vec<String> {
        let mut widths = Vec::new();
        for row in rows {
            for (index, cell) in row.iter().enumerate() {
                if index == widths.len() {
                    widths.push(cell.chars().count());
                } else {
                    widths[index] = widths[index].max(cell.chars().count());
                }
            }
        }
        rows.iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(index, cell)| {
                        if index + 1 == row.len() {
                            cell.clone()
                        } else {
                            format!("{cell:<width$}", width = widths[index])
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("  ")
            })
            .collect()
    }
}

const FAILURE: i32 = 1;

#[derive(Debug, PartialEq, Eq)]
struct CommandOutput {
    text: String,
    exit_code: i32,
}

impl CommandOutput {
    fn success(text: String) -> Self {
        Self { text, exit_code: 0 }
    }

    fn with_exit_code(text: String, exit_code: i32) -> Self {
        Self { text, exit_code }
    }
}

mod jobs {
    use fleet_client::Client;
    use fleet_proto::{error::ProtoError, job::JobRecord};

    pub async fn wait_for_job(
        _client: &Client,
        _job: &JobRecord,
        _operation: &str,
    ) -> Result<(), ProtoError> {
        Ok(())
    }
}

#[path = "../src/commands/hosts.rs"]
mod hosts;

use args::{Cli, Command, HostCommand};
use hosts::{render_bootstrap, render_doctor, render_json, render_statuses};

#[test]
fn parses_list_doctor_and_bootstrap_commands() {
    let _ = args::VERSION_DISPLAY;
    let _ = args::BoardCardFields::clear_flag;
    let _ = hosts::run;
    let list = Cli::try_parse_from(["fleet", "host", "list", "--json"])
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(
        list.command,
        Some(Command::Host(args::HostArgs {
            command: HostCommand::List { json: true }
        }))
    ));

    let doctor = Cli::try_parse_from(["fleet", "host", "doctor", "dev-box"])
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(
        doctor.command,
        Some(Command::Host(args::HostArgs {
            command: HostCommand::Doctor { id }
        })) if id.as_str() == "dev-box"
    ));

    let bootstrap = Cli::try_parse_from([
        "fleet",
        "host",
        "bootstrap",
        "dev-box",
        "--ref",
        "release/next",
    ])
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(
        bootstrap.command,
        Some(Command::Host(args::HostArgs {
            command: HostCommand::Bootstrap { id, git_ref }
        })) if id.as_str() == "dev-box" && git_ref.as_deref() == Some("release/next")
    ));
}

#[test]
fn renders_aligned_host_rows_and_protocol_json_statuses() {
    let status = HostStatus {
        id: "dev-box".parse().unwrap_or_else(|error| panic!("{error}")),
        provider: "tailscale".to_owned(),
        version: Some("fleetd 0.1.0+abc123".to_owned()),
        link: LinkState::Ready,
        address: Some("100.77.28.11".to_owned()),
        agent_binaries: Some(AgentBinaries {
            claude: true,
            opencode: false,
        }),
        reachable: true,
        checked_at: "2026-09-08T12:00:00Z".to_owned(),
        error: None,
    };

    assert_eq!(
        render_statuses(std::slice::from_ref(&status)),
        "ID       PROVIDER   ADDRESS       LINK   VERSION              REACHABLE  AGENTS\n\
         dev-box  tailscale  100.77.28.11  ready  fleetd 0.1.0+abc123  yes        claude"
    );
    let value = serde_json::from_str::<serde_json::Value>(
        &render_json(std::slice::from_ref(&status)).unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(value["protocol"], 1);
    assert_eq!(
        value["hosts"],
        serde_json::to_value(vec![status]).unwrap_or_else(|error| panic!("{error}"))
    );
}

#[test]
fn renders_only_host_doctor_output_and_bootstrap_log_completion() {
    assert_eq!(
        render_doctor(&[DoctorCheck {
            check: "host dev-box protocol".to_owned(),
            status: DoctorStatus::Ok,
            detail: "protocol 7".to_owned(),
        }]),
        "CHECK STATUS DETAIL\nhost dev-box protocol ok protocol 7"
    );
    assert_eq!(
        render_bootstrap(
            "dev-box",
            "job-7",
            &["$ git --version".to_owned(), "git version 2.51".to_owned()]
        ),
        "$ git --version\ngit version 2.51\nBootstrapped dev-box (job-7)"
    );
}
