use fleet_proto::request::{Request, RequestBody};

#[test]
fn old_and_reset_state_requests_remain_json_compatible() {
    let old: Request = serde_json::from_str(r#"{"id":1,"body":{"type":"doctor"}}"#).unwrap();
    assert_eq!(old.body, RequestBody::Doctor);

    let reset: Request = serde_json::from_str(r#"{"id":2,"body":{"type":"reset_state"}}"#).unwrap();
    assert_eq!(reset.body, RequestBody::ResetState);
    assert_eq!(
        serde_json::to_string(&reset).unwrap(),
        r#"{"id":2,"body":{"type":"reset_state"}}"#
    );
}
