//! Entry point that selects Fleet's CLI or native GPUI application mode.

fn main() -> anyhow::Result<()> {
    if std::env::args_os().nth(1).is_some() {
        return fleet_cli::run();
    }
    fleet_app::run()
}
