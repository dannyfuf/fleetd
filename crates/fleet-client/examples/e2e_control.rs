use std::{env, error::Error, path::PathBuf};

use fleet_client::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let action = args.next().ok_or("expected restore or shutdown")?;
    let home = PathBuf::from(env::var("FLEET_HOME")?);
    let client = Client::connect(home).await?;

    match action.as_str() {
        "restore" => {
            let entry = args.next().ok_or("expected a trash entry")?;
            client.restore_trash(&entry).await?;
            println!("restored {entry}");
        }
        "shutdown" => {
            client.daemon_shutdown(false).await?;
            println!("daemon shutdown requested");
        }
        _ => return Err(format!("unknown action: {action}").into()),
    }
    Ok(())
}
