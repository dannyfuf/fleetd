//! Codex's account surface: the handshake read, `/login`, `/logout`, and what an answer means.
//!
//! The one read this module does *not* own is the notification-triggered re-read, which lives in
//! [`super::transport`] because it runs on the stdout reader's own budget; it decides what to
//! publish with [`status_of`] and [`signed_in_notice`] from here, so there is one interpretation
//! and not two. Three rules shape the rest.
//!
//! **Absent is not negative.** `{account: null, requiresOpenaiAuth: false}` is Codex saying it
//! needs no OpenAI account at all — a thread pointed at another model provider — and reporting
//! that as "signed out" would be a lie the metadata row repeats on every frame. Only
//! `requiresOpenaiAuth: true` with no account is [`AccountStatus::SignedOut`].
//!
//! **A read that fails is a warning, never a failed start.** Codex answers turns perfectly well
//! while Fleet has no idea who it is signed in as, so every read here answers an `Option` and
//! none of them can end a session.
//!
//! **The account is a signal, not a return value.** Nothing here hands an account back to the
//! caller: it is published as [`AgentEvent::AccountChanged`], so a mirrored thread on another
//! host learns the same fact from the same event as the client that asked for it
//! (`docs/NATIVE-AGENTS.md` §4.4). [`login`] returns only the URL the user has to open, because
//! that is the one thing an event cannot do for them — the account it eventually produces
//! arrives on `account/login/completed`, a re-read later.

use fleet_core::agents::{AccountInfo, AccountKind, AccountStatus, AgentEvent};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::{
    LOGIN_DEADLINE,
    session::CodexSession,
    transport::{self, Transport},
    wire::{Account, GetAccountResponse, LoginAccountResponse, PlanType},
};
use crate::agents::harness::{
    AccountOutcome, HarnessError, HarnessResult, HarnessSink, ProtocolOp,
    fingerprint::{IssueKind, SchemaFingerprint},
};

/// Reads the account during the handshake and publishes what came back.
///
/// Silent when Codex has nothing to report or the read failed: a chip Fleet could not read must
/// not become a transcript row claiming the session is signed out.
pub(super) async fn announce_on_open(transport: &Transport, events: &HarnessSink) {
    let Some(status) = transport.read_account().await else {
        return;
    };
    let signed_out = matches!(status, AccountStatus::SignedOut);
    transport::emit(
        events,
        AgentEvent::AccountChanged { account: status },
        Some("account/read"),
    );
    if signed_out {
        transport::emit(
            events,
            AgentEvent::Notice(SIGNED_OUT_NOTICE.to_owned()),
            Some("account/read"),
        );
    }
}

/// Starts a ChatGPT sign-in and answers the URL the user has to open.
///
/// A second `/login` while one is pending cancels the first: two live callbacks mean two browser
/// tabs that both claim to be the sign-in, and only one of them can win. The cancel is
/// best-effort — a flow Codex has already forgotten is the state this asked for — and the pending
/// id is dropped either way.
pub(super) async fn login(
    transport: &Transport,
    session: &Mutex<CodexSession>,
    events: &HarnessSink,
) -> HarnessResult<AccountOutcome> {
    if let Some(login) = session.lock().await.pending_login.take()
        && let Err(error) = transport
            .request(
                "account/login/cancel",
                Some(json!({ "loginId": login })),
                LOGIN_DEADLINE,
            )
            .await
    {
        tracing::warn!(
            target: "fleet::agents::codex",
            %error,
            "Codex did not cancel the sign-in Fleet had already started"
        );
    }
    let result = transport
        .request(
            "account/login/start",
            Some(json!({"type": "chatgpt"})),
            LOGIN_DEADLINE,
        )
        .await?;
    let (auth_url, login_id) = chatgpt_login(&result)?;
    session.lock().await.pending_login = Some(login_id);
    // The URL goes in the transcript as well as back to the caller, so the sign-in is still
    // reachable when the app cannot open a browser — and so the row is there to scroll back to
    // while the user is looking for the tab they lost.
    transport::emit(
        events,
        AgentEvent::Notice(format!(
            "Finish signing in to Codex in your browser: {auth_url}"
        )),
        Some("account/login/start"),
    );
    Ok(AccountOutcome::Browser { auth_url })
}

/// Signs out, then re-reads rather than assuming: Codex publishes no notification for a logout it
/// was asked for, and the event is what a mirror learns this from.
pub(super) async fn logout(
    transport: &Transport,
    session: &Mutex<CodexSession>,
    events: &HarnessSink,
) -> HarnessResult<AccountOutcome> {
    transport
        .request("account/logout", None, LOGIN_DEADLINE)
        .await?;
    // Whatever sign-in was in flight is meaningless now.
    session.lock().await.pending_login = None;
    if let Some(status) = transport.read_account().await {
        transport::emit(
            events,
            AgentEvent::AccountChanged { account: status },
            Some("account/logout"),
        );
    }
    transport::emit(
        events,
        AgentEvent::Notice("Signed out of Codex.".to_owned()),
        Some("account/logout"),
    );
    Ok(AccountOutcome::Settled)
}

/// The `authUrl` and `loginId` of a started ChatGPT sign-in.
///
/// Every other variant of `LoginAccountResponse` answers a login mode Fleet did not ask for, so
/// it is a protocol failure rather than a silently ignored answer.
fn chatgpt_login(result: &Value) -> HarnessResult<(String, String)> {
    let response: LoginAccountResponse = serde_json::from_value(result.clone())
        .map_err(|error| HarnessError::decode(Some("account/login/start"), &error, result))?;
    match response {
        LoginAccountResponse::Chatgpt { auth_url, login_id } => Ok((auth_url, login_id)),
        _ => Err(HarnessError::Protocol {
            op: ProtocolOp::Decode,
            method: Some("account/login/start".to_owned()),
            fingerprint: SchemaFingerprint::of_value(IssueKind::Shape, result),
        }),
    }
}

/// The normalized account a `account/read` result describes.
///
/// `None` means **there is nothing to report**: Codex answered that it needs no OpenAI account at
/// all, which is what a thread pointed at another model provider looks like. That is a different
/// answer from [`AccountStatus::SignedOut`], and drawing "signed out" for it would be a lie.
///
/// # Errors
///
/// Returns the decoder's own error when the result is not a `GetAccountResponse`.
pub(super) fn status_of(result: &Value) -> Result<Option<AccountStatus>, serde_json::Error> {
    let response: GetAccountResponse = serde_json::from_value(result.clone())?;
    Ok(match response.account {
        Some(account) => Some(AccountStatus::SignedIn(AccountInfo {
            kind: kind_of(account),
        })),
        None if response.requires_openai_auth => Some(AccountStatus::SignedOut),
        None => None,
    })
}

/// Normalizes one Codex account into the provider-neutral kind.
fn kind_of(account: Account) -> AccountKind {
    match account {
        Account::ApiKey => AccountKind::ApiKey,
        Account::Chatgpt { email, plan_type } => AccountKind::ChatGpt {
            email: email.filter(|email| !email.is_empty()),
            plan: plan_label(plan_type),
        },
        Account::AmazonBedrock { .. } => AccountKind::Other("amazon bedrock".to_owned()),
        // A mode this build does not model still names itself rather than disappearing (§4.5).
        Account::Unknown => AccountKind::Other("another account".to_owned()),
    }
}

/// The plan as a label, or `None` when Codex itself does not know which plan this is.
///
/// Read back through serde rather than matched arm by arm: the ladder has fourteen members today
/// and Codex adds to it without a version bump, so a match here would silently answer `None` for
/// every plan added after this build.
fn plan_label(plan: PlanType) -> Option<String> {
    let value = serde_json::to_value(plan).ok()?;
    let name = value.as_str()?;
    if name.eq_ignore_ascii_case("unknown") || name.eq_ignore_ascii_case("unrecognized") {
        return None;
    }
    Some(name.replace('_', " "))
}

/// The sentence a successful sign-in appends to the transcript.
#[must_use]
pub(super) fn signed_in_notice(status: &AccountStatus) -> String {
    match status {
        AccountStatus::SignedIn(info) => match info.label() {
            Some(label) => format!("Signed in to Codex as {label}."),
            None => "Signed in to Codex.".to_owned(),
        },
        // Codex said the login completed and then reported no account. Saying so is more useful
        // than claiming a sign-in the next turn will contradict.
        AccountStatus::SignedOut => {
            "Codex reported a completed sign-in but is still signed out.".to_owned()
        }
    }
}

/// The transcript line a signed-out session opens with.
pub(super) const SIGNED_OUT_NOTICE: &str = "Codex is signed out. Type /login to sign in.";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_chatgpt_account_keeps_its_email_and_plan() {
        let status = status_of(&json!({
            "account": {"type": "chatgpt", "email": "dev@example.com", "planType": "business"},
            "requiresOpenaiAuth": true,
        }))
        .unwrap_or_else(|error| panic!("decode: {error}"));

        assert_eq!(
            status,
            Some(AccountStatus::SignedIn(AccountInfo {
                kind: AccountKind::ChatGpt {
                    email: Some("dev@example.com".to_owned()),
                    plan: Some("business".to_owned()),
                },
            }))
        );
    }

    #[test]
    fn a_plan_codex_cannot_name_contributes_no_label() {
        let status = status_of(&json!({
            "account": {"type": "chatgpt", "email": null, "planType": "unknown"},
            "requiresOpenaiAuth": true,
        }))
        .unwrap_or_else(|error| panic!("decode: {error}"));

        let Some(AccountStatus::SignedIn(info)) = status else {
            panic!("a chatgpt account is signed in: {status:?}");
        };
        assert_eq!(info.label(), None, "nothing is invented for an empty plan");
    }

    #[test]
    fn an_api_key_account_is_signed_in() {
        let status = status_of(&json!({
            "account": {"type": "apiKey"},
            "requiresOpenaiAuth": true,
        }))
        .unwrap_or_else(|error| panic!("decode: {error}"));

        assert_eq!(
            status,
            Some(AccountStatus::SignedIn(AccountInfo {
                kind: AccountKind::ApiKey,
            }))
        );
    }

    #[test]
    fn no_account_with_auth_required_is_signed_out() {
        let status = status_of(&json!({"requiresOpenaiAuth": true}))
            .unwrap_or_else(|error| panic!("decode: {error}"));

        assert_eq!(status, Some(AccountStatus::SignedOut));
    }

    #[test]
    fn no_account_and_no_auth_required_reports_nothing_at_all() {
        let status = status_of(&json!({"account": null, "requiresOpenaiAuth": false}))
            .unwrap_or_else(|error| panic!("decode: {error}"));

        assert_eq!(
            status, None,
            "a thread pointed at another provider is not signed out"
        );
    }

    #[test]
    fn an_auth_mode_this_build_does_not_model_still_names_itself() {
        let status = status_of(&json!({
            "account": {"type": "somethingNew"},
            "requiresOpenaiAuth": true,
        }))
        .unwrap_or_else(|error| {
            panic!("an unknown account kind must not fail the decode: {error}")
        });

        assert_eq!(
            status,
            Some(AccountStatus::SignedIn(AccountInfo {
                kind: AccountKind::Other("another account".to_owned()),
            }))
        );
    }
}
