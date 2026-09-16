//! The account family, mapped from the fixture's own frames.
//!
//! The fixture is hand-built rather than captured for one reason worth naming: a real capture of
//! `account/read` carries the signed-in user's email, which is the one payload §4.5's privacy
//! rules keep out of the repository. Every line in it is the shape `codex app-server
//! generate-json-schema` declares for 0.147.0, so a rename upstream still fails here.

use super::*;
use crate::agents::codex::account;

/// The captured account frames, by the `_case` label each line carries.
const ACCOUNT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agents/codex/codex-account.ndjson"
));

/// The `result` of the fixture line labelled `_case`.
fn result_of(case: &str) -> Value {
    frame(case)
        .get("result")
        .cloned()
        .unwrap_or_else(|| panic!("the {case} fixture line is a response"))
}

/// The fixture line labelled `_case`.
fn frame(case: &str) -> Value {
    ACCOUNT
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value.get("_case").and_then(Value::as_str) == Some(case))
        .unwrap_or_else(|| panic!("the fixture has no {case} line"))
}

/// The `params` of the fixture notification labelled `_case`.
fn params_of(case: &str) -> Value {
    frame(case)
        .get("params")
        .cloned()
        .unwrap_or_else(|| panic!("the {case} fixture line is a notification"))
}

#[test]
fn every_account_read_shape_normalizes() {
    assert_eq!(
        account::status_of(&result_of("chatgpt"))
            .unwrap_or_else(|error| panic!("chatgpt: {error}")),
        Some(fleet_core::agents::AccountStatus::SignedIn(
            fleet_core::agents::AccountInfo {
                kind: fleet_core::agents::AccountKind::ChatGpt {
                    email: Some("dev@example.com".to_owned()),
                    plan: Some("business".to_owned()),
                },
            }
        ))
    );
    assert_eq!(
        account::status_of(&result_of("api-key")).unwrap_or_else(|error| panic!("apiKey: {error}")),
        Some(fleet_core::agents::AccountStatus::SignedIn(
            fleet_core::agents::AccountInfo {
                kind: fleet_core::agents::AccountKind::ApiKey,
            }
        ))
    );
    assert_eq!(
        account::status_of(&result_of("signed-out"))
            .unwrap_or_else(|error| panic!("signed out: {error}")),
        Some(fleet_core::agents::AccountStatus::SignedOut)
    );
    assert_eq!(
        account::status_of(&result_of("other-provider"))
            .unwrap_or_else(|error| panic!("other provider: {error}")),
        None,
        "a thread that needs no OpenAI account is not signed out"
    );
}

/// `account/updated` names no account, so it is a re-read trigger and appends nothing.
#[test]
fn account_updated_asks_for_a_read_and_appends_nothing() {
    let mut session = CodexSession::default();
    let output = map::handle(&mut session, "account/updated", &params_of("updated"));

    assert!(output.events.is_empty(), "{:?}", names(&output.events));
    assert_eq!(
        output.follow_up,
        [map::FollowUp::Account {
            announce_sign_in: false
        }]
    );
}

/// A completed sign-in re-reads, announces, and releases the pending flow.
#[test]
fn a_completed_sign_in_reads_the_account_and_announces_it() {
    let mut session = CodexSession {
        pending_login: Some("9f1a0d1c-6f0a-4a0a-9a0a-1c2d3e4f5a6b".to_owned()),
        ..CodexSession::default()
    };
    let output = map::handle(
        &mut session,
        "account/login/completed",
        &params_of("login-ok"),
    );

    assert!(output.events.is_empty(), "{:?}", names(&output.events));
    assert_eq!(
        output.follow_up,
        [map::FollowUp::Account {
            announce_sign_in: true
        }]
    );
    assert_eq!(
        session.pending_login, None,
        "a settled flow is not something a second /login may cancel"
    );
}

/// A failed sign-in is a transcript row on its own, carrying Codex's own sentence.
#[test]
fn a_failed_sign_in_is_a_notice_and_reads_nothing() {
    let mut session = CodexSession {
        pending_login: Some("9f1a0d1c-6f0a-4a0a-9a0a-1c2d3e4f5a6b".to_owned()),
        ..CodexSession::default()
    };
    let output = map::handle(
        &mut session,
        "account/login/completed",
        &params_of("login-failed"),
    );

    assert!(output.follow_up.is_empty(), "{:?}", output.follow_up);
    let [AgentEvent::Notice(text)] = output.events.as_slice() else {
        panic!("one notice: {:?}", names(&output.events));
    };
    assert_eq!(
        text,
        "Codex sign-in failed: the authorization request was denied"
    );
    assert_eq!(session.pending_login, None);
}

/// A malformed payload degrades to `Unknown` and never to silence (§4.5 rule 2).
#[test]
fn an_undecodable_login_completion_degrades_rather_than_disappearing() {
    let mut session = CodexSession::default();
    let output = map::handle(
        &mut session,
        "account/login/completed",
        &json!({"success": "yes"}),
    );

    assert_eq!(names(&output.events), ["unknown"]);
    assert!(output.follow_up.is_empty());
}

/// The started sign-in answers a URL and a flow id, and nothing else is accepted for it.
#[test]
fn a_started_chatgpt_login_carries_its_url_and_flow_id() {
    let decoded: crate::agents::codex::wire::LoginAccountResponse =
        serde_json::from_value(result_of("login-start"))
            .unwrap_or_else(|error| panic!("login start: {error}"));

    assert_eq!(
        decoded,
        crate::agents::codex::wire::LoginAccountResponse::Chatgpt {
            auth_url: "http://localhost:1455/auth/callback?state=9f1a0d1c".to_owned(),
            login_id: "9f1a0d1c-6f0a-4a0a-9a0a-1c2d3e4f5a6b".to_owned(),
        }
    );
}

/// The mock peer for the account controls: the handshake plus the three account methods.
fn account_peer() -> crate::agents::harness::mockpeer::BuiltPeer {
    let thread = "01a089f2-5337-7470-adb1-219e71d62a35";
    MockPeer::new()
        .on(
            "initialize",
            Some(r#"{"id":__ID__,"result":{"userAgent":"fleet/0.147.0 (Linux)"}}"#),
            &[],
        )
        .on("account/read", Some(ACCOUNT_READ), &[])
        .on(
            "thread/start",
            Some(&format!(
                r#"{{"id":__ID__,"result":{{"thread":{{"id":"{thread}"}}}}}}"#
            )),
            &[],
        )
        .on(
            "model/list",
            Some(r#"{"id":__ID__,"result":{"data":[],"nextCursor":null}}"#),
            &[],
        )
        .on(
            "skills/list",
            Some(r#"{"id":__ID__,"result":{"data":[]}}"#),
            &[],
        )
        .on(
            "account/login/start",
            Some(
                r#"{"id":__ID__,"result":{"type":"chatgpt","loginId":"login-1","authUrl":"http://localhost:1455/auth/callback?state=1"}}"#,
            ),
            &[],
        )
        .on(
            "account/login/cancel",
            Some(r#"{"id":__ID__,"result":{"status":"canceled"}}"#),
            &[],
        )
        .on("account/logout", Some(r#"{"id":__ID__,"result":{}}"#), &[])
        .lingering()
        .build()
}

/// The methods Fleet wrote, in order, from the peer's own record.
fn written_methods(frames: &[String]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|line| {
            serde_json::from_str::<Value>(line).ok().and_then(|value| {
                value
                    .get("method")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
        })
        .collect()
}

/// `/login` answers the URL, and a second one cancels the flow the first left open.
#[tokio::test]
async fn a_second_login_cancels_the_flow_the_first_one_started() {
    let peer = account_peer();
    let mut harness = harness(peer.command());
    harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("open: {error}"));

    let first = harness
        .account(AccountOp::Login)
        .await
        .unwrap_or_else(|error| panic!("login: {error}"));
    assert_eq!(
        first,
        AccountOutcome::Browser {
            auth_url: "http://localhost:1455/auth/callback?state=1".to_owned()
        }
    );
    assert_eq!(
        harness.session.lock().await.pending_login.as_deref(),
        Some("login-1"),
        "the flow is remembered so a second /login can cancel it"
    );

    harness
        .account(AccountOp::Login)
        .await
        .unwrap_or_else(|error| panic!("second login: {error}"));

    let written = peer.wait_for_frames(9, Duration::from_secs(5)).await;
    assert_eq!(
        written_methods(&written),
        [
            "initialize",
            "initialized",
            "account/read",
            "thread/start",
            "model/list",
            "skills/list",
            "account/login/start",
            "account/login/cancel",
            "account/login/start",
        ],
        "{written:?}"
    );
    let cancel = written
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value.get("method").and_then(Value::as_str) == Some("account/login/cancel"))
        .unwrap_or_else(|| panic!("the cancel was recorded: {written:?}"));
    assert_eq!(
        cancel.pointer("/params/loginId").and_then(Value::as_str),
        Some("login-1"),
        "the cancel names the flow the first login opened"
    );

    harness
        .shutdown(crate::agents::harness::ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
}

/// `/logout` re-reads the account rather than assuming one, and says so in the transcript.
#[tokio::test]
async fn logout_reads_the_account_back_and_announces_it() {
    let peer = account_peer();
    let mut harness = harness(peer.command());
    let mut events = harness.events();
    harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("open: {error}"));

    let outcome = harness
        .account(AccountOp::Logout)
        .await
        .unwrap_or_else(|error| panic!("logout: {error}"));
    assert_eq!(outcome, AccountOutcome::Settled);

    let written = peer.wait_for_frames(8, Duration::from_secs(5)).await;
    assert_eq!(
        written_methods(&written)
            .into_iter()
            .skip(6)
            .collect::<Vec<_>>(),
        ["account/logout", "account/read"],
        "the logout is followed by a real read: {written:?}"
    );

    let mut drained = Vec::new();
    while let Ok(event) = events.try_recv() {
        drained.push(event.event);
    }
    assert!(
        drained.iter().any(|event| matches!(
            event,
            AgentEvent::Notice(text) if text == "Signed out of Codex."
        )),
        "{:?}",
        names(&drained)
    );
    assert_eq!(
        drained
            .iter()
            .filter(|event| matches!(event, AgentEvent::AccountChanged { .. }))
            .count(),
        2,
        "one from the handshake read and one from the logout read: {:?}",
        names(&drained)
    );

    harness
        .shutdown(crate::agents::harness::ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
}

/// A signed-out handshake says so in the transcript, once, and never fails the open.
#[tokio::test]
async fn a_signed_out_handshake_opens_and_says_so() {
    let thread = "01a089f2-5337-7470-adb1-219e71d62a35";
    let peer = MockPeer::new()
        .on(
            "initialize",
            Some(r#"{"id":__ID__,"result":{"userAgent":"fleet/0.147.0 (Linux)"}}"#),
            &[],
        )
        .on(
            "account/read",
            Some(r#"{"id":__ID__,"result":{"account":null,"requiresOpenaiAuth":true}}"#),
            &[],
        )
        .on(
            "thread/start",
            Some(&format!(
                r#"{{"id":__ID__,"result":{{"thread":{{"id":"{thread}"}}}}}}"#
            )),
            &[],
        )
        .on(
            "model/list",
            Some(r#"{"id":__ID__,"result":{"data":[],"nextCursor":null}}"#),
            &[],
        )
        .on(
            "skills/list",
            Some(r#"{"id":__ID__,"result":{"data":[]}}"#),
            &[],
        )
        .lingering()
        .build();
    let mut harness = harness(peer.command());
    let mut events = harness.events();
    harness
        .open(OpenSession {
            start: start_request(),
        })
        .await
        .unwrap_or_else(|error| panic!("a signed-out Codex still opens: {error}"));

    let mut drained = Vec::new();
    while let Ok(event) = events.try_recv() {
        drained.push(event.event);
    }
    assert!(
        drained.iter().any(|event| matches!(
            event,
            AgentEvent::AccountChanged {
                account: fleet_core::agents::AccountStatus::SignedOut
            }
        )),
        "{:?}",
        names(&drained)
    );
    assert_eq!(
        drained
            .iter()
            .filter(|event| matches!(
                event,
                AgentEvent::Notice(text) if text == account::SIGNED_OUT_NOTICE
            ))
            .count(),
        1,
        "one notice, not one per read: {:?}",
        names(&drained)
    );

    harness
        .shutdown(crate::agents::harness::ShutdownReason::User)
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
}
