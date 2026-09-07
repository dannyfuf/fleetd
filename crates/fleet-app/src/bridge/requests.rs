use super::*;

pub(super) struct Request {
    pub(super) client: Option<Client>,
    pub(super) body: Box<RequestBody>,
    pub(super) reply: Option<Sender<Result<ResponseBody, ProtoError>>>,
}

/// A single owner enqueues event-backed mutations in arrival order. Response waiters remain
/// independent, preserving the existing request API while a slow daemon operation completes.
pub(super) async fn run(requests: Receiver<Request>, events: Sender<BridgeEvent>) {
    while let Ok(Request {
        client,
        body,
        reply,
    }) = requests.recv().await
    {
        match reply {
            Some(reply) => dispatch(client, *body, reply, events.clone()),
            None => {
                if let Some(client) = client {
                    let _ignored = client.request_background(*body).await;
                }
            }
        }
    }
}

/// Sends one request on the runtime without blocking the command loop.
fn dispatch(
    client: Option<Client>,
    body: RequestBody,
    reply: Sender<Result<ResponseBody, ProtoError>>,
    events: Sender<BridgeEvent>,
) {
    let Some(client) = client else {
        let _ignored = reply.try_send(Err(offline("the Fleet daemon is not connected")));
        return;
    };
    tokio::spawn(async move {
        let result = client.request(body).await;
        if let Ok(ResponseBody::Config(config)) = &result {
            let _ = events
                .send(BridgeEvent::TerminalConfig(config.terminal.clone()))
                .await;
            let _ = events
                .send(BridgeEvent::NotificationConfig(
                    config.ui.notifications.clone(),
                ))
                .await;
        }
        let _ignored = reply.send(result).await;
    });
}
