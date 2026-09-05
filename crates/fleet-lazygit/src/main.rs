//! `fleet-lazygit [path]` — the native lazygit clone.

fn main() -> std::process::ExitCode {
    // Git re-invokes this same executable as GIT_SEQUENCE_EDITOR during an interactive rebase.
    // When the instruction environment variable is set we *are* that editor: rewrite the todo
    // file and exit without ever touching the window server.
    if let Some(code) = fleet_git::sequence_editor::maybe_run_from_env() {
        return code;
    }

    let path = match std::env::args_os().nth(1) {
        Some(argument) => std::path::PathBuf::from(argument),
        None => match std::env::current_dir() {
            Ok(directory) => directory,
            Err(error) => {
                eprintln!("fleet-lazygit: cannot read the current directory: {error}");
                return std::process::ExitCode::FAILURE;
            }
        },
    };

    match fleet_lazygit::run(path) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fleet-lazygit: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
