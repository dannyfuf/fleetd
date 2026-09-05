fn main() -> std::process::ExitCode {
    fleet_git::sequence_editor::maybe_run_from_env().unwrap_or(std::process::ExitCode::SUCCESS)
}
