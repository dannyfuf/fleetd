use fleet_client::protocol_transport;
use fleet_proto::{
    codec::FleetCodec,
    request::{Request, RequestBody},
    response::{Response, ResponseBody},
};
use futures_util::{SinkExt, StreamExt};
use tokio_util::codec::Framed;

#[tokio::test]
async fn protocol_transport_accepts_an_in_memory_duplex_stream() {
    let (client_io, server_io) = tokio::io::duplex(4 * 1024);
    let mut client = protocol_transport(client_io);
    let mut server = Framed::new(server_io, FleetCodec::<Response, Request>::new());

    client
        .send(Request {
            id: 41,
            body: RequestBody::DaemonPing,
        })
        .await
        .expect("send request");
    let request = server
        .next()
        .await
        .expect("request frame")
        .expect("decode request");
    assert_eq!(request.id, 41);
    assert!(matches!(request.body, RequestBody::DaemonPing));

    server
        .send(Response {
            id: request.id,
            result: Ok(ResponseBody::Pong),
        })
        .await
        .expect("send response");
    let response: Response = serde_json::from_value(
        client
            .next()
            .await
            .expect("response frame")
            .expect("decode response"),
    )
    .expect("response envelope");
    assert_eq!(response.id, 41);
    assert!(matches!(response.result, Ok(ResponseBody::Pong)));
}
