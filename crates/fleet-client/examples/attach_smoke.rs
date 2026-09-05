use std::{env, error::Error, path::PathBuf, time::Duration};

use fleet_client::{Client, TerminalUpdate};
use fleet_core::sessions::SessionKind;

const WORKTREE_ID: &str = "acme/widgets#feature-one";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let home = PathBuf::from(env::var("FLEET_HOME")?);
    let client = Client::connect(&home).await?;
    let session = client
        .list_sessions()
        .await?
        .into_iter()
        .find(|session| {
            matches!(&session.kind, SessionKind::Worktree(id) if id.as_str() == WORKTREE_ID)
        })
        .ok_or("feature-one session is missing")?;

    let terminal = if let Some(terminal) = session
        .terminals
        .iter()
        .find(|terminal| terminal.name == "e2e-shell")
    {
        terminal.clone()
    } else {
        client
            .new_terminal(session.id, "e2e-shell", ":", session.cwd)
            .await?
    };

    let mut terminal = client.attach(terminal.id, 100, 30).await?;
    terminal
        .send_input(b"printf 'ATTACH_READY\\n'\r".to_vec())
        .await?;
    let initial = next_frame_containing(&mut terminal, "ATTACH_READY").await?;
    println!("initial frame: {initial:?}");

    terminal.send_input(b"echo E2E_OK\r".to_vec()).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("timed out waiting for E2E_OK terminal output".into());
        }
        let update = tokio::time::timeout(remaining, terminal.next_update())
            .await?
            .ok_or("terminal update stream closed")?;
        if let TerminalUpdate::Frame(frame) = update {
            let text = frame
                .rows_changed
                .iter()
                .flat_map(|row| row.cells.iter())
                .map(|cell| cell.text.as_str())
                .collect::<String>();
            if text.contains("E2E_OK") {
                println!("later frame: {text:?}");
                terminal.detach().await?;
                return Ok(());
            }
        }
    }
}

async fn next_frame_containing(
    terminal: &mut fleet_client::TerminalHandle,
    needle: &str,
) -> Result<String, Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!("timed out waiting for {needle} terminal output").into());
        }
        let update = tokio::time::timeout(remaining, terminal.next_update())
            .await?
            .ok_or("terminal update stream closed")?;
        if let TerminalUpdate::Frame(frame) = update {
            let text = frame
                .rows_changed
                .iter()
                .flat_map(|row| row.cells.iter())
                .map(|cell| cell.text.as_str())
                .collect::<String>();
            if text.contains(needle) {
                return Ok(text);
            }
        }
    }
}
