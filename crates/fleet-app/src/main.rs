//! Entry point that selects Fleet's CLI or native GPUI application mode.

fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).is_some() {
        let exit_code = fleet_cli::run();
        if exit_code != 0 {
            std::process::exit(exit_code);
        }
        return Ok(());
    }
    fleet_app::run()
}
