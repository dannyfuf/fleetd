//! Directory-scoped OpenCode HTTP transport.

use std::{collections::BTreeMap, path::Path, time::Duration};

use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use fleet_core::agents::ModelSelection;

use super::super::ProviderError;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(super) struct HttpClient {
    client: reqwest::Client,
    base_url: Url,
    directory: String,
    password: Option<String>,
}

#[derive(Debug)]
pub(super) struct HttpFailure {
    pub(super) status: StatusCode,
    pub(super) body: String,
}

impl HttpFailure {
    fn protocol(self, operation: &str) -> ProviderError {
        ProviderError::Protocol {
            message: format!(
                "OpenCode {operation} returned HTTP {}: {}",
                self.status,
                if self.body.is_empty() {
                    "empty response"
                } else {
                    &self.body
                }
            ),
        }
    }
}

impl HttpClient {
    pub(super) fn new(
        base_url: &str,
        directory: &Path,
        password: Option<String>,
    ) -> Result<Self, ProviderError> {
        let base_url = Url::parse(base_url).map_err(|error| ProviderError::Protocol {
            message: format!("invalid OpenCode server URL `{base_url}`: {error}"),
        })?;
        let client = reqwest::Client::builder()
            .connect_timeout(STATUS_TIMEOUT)
            .build()
            .map_err(|error| ProviderError::Protocol {
                message: format!("build OpenCode HTTP client: {error}"),
            })?;
        Ok(Self {
            client,
            base_url,
            directory: directory.to_string_lossy().into_owned(),
            password,
        })
    }

    pub(super) fn base_url(&self) -> &str {
        self.base_url.as_str().trim_end_matches('/')
    }

    pub(super) fn directory(&self) -> &str {
        &self.directory
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url, ProviderError> {
        let mut url = self.base_url.clone();
        let base_url = url.to_string();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|()| ProviderError::Protocol {
                    message: format!("OpenCode base URL cannot contain path segments: {base_url}"),
                })?;
            path.pop_if_empty();
            path.extend(segments);
        }
        url.query_pairs_mut()
            .append_pair("directory", &self.directory);
        Ok(url)
    }

    fn request(
        &self,
        method: Method,
        segments: &[&str],
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let request = self.client.request(method, self.endpoint(segments)?);
        Ok(match &self.password {
            Some(password) => request.basic_auth("opencode", Some(password)),
            None => request,
        })
    }

    async fn send(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<Value>,
        timeout: Duration,
    ) -> Result<reqwest::Response, ProviderError> {
        let mut request = self.request(method, segments)?.timeout(timeout);
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await.map_err(|error| {
            if error.is_timeout() {
                ProviderError::Timeout {
                    what: format!("OpenCode HTTP request `{}`", segments.join("/")),
                }
            } else {
                ProviderError::Protocol {
                    message: format!(
                        "OpenCode HTTP request `{}` failed: {error}",
                        segments.join("/")
                    ),
                }
            }
        })
    }

    async fn decode<T: DeserializeOwned>(
        response: reqwest::Response,
        operation: &str,
    ) -> Result<T, ProviderError> {
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| ProviderError::Protocol {
                message: format!("read OpenCode {operation} response: {error}"),
            })?;
        if !status.is_success() {
            return Err(HttpFailure {
                status,
                body: String::from_utf8_lossy(&bytes).into_owned(),
            }
            .protocol(operation));
        }
        serde_json::from_slice(&bytes).map_err(|error| ProviderError::Protocol {
            message: format!("decode OpenCode {operation} response: {error}"),
        })
    }

    async fn decode_optional<T: DeserializeOwned>(
        response: reqwest::Response,
        operation: &str,
    ) -> Result<Option<T>, ProviderError> {
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::decode(response, operation).await.map(Some)
    }

    pub(super) async fn readiness(&self) -> Result<(), ProviderError> {
        let response = self
            .send(Method::GET, &["session"], None, STATUS_TIMEOUT)
            .await?;
        let _: Vec<Value> = Self::decode(response, "session readiness").await?;
        Ok(())
    }

    pub(super) async fn session(&self, id: &str) -> Result<Option<Value>, ProviderError> {
        let response = self
            .send(Method::GET, &["session", id], None, REQUEST_TIMEOUT)
            .await?;
        Self::decode_optional(response, "get session").await
    }

    pub(super) async fn create_session(&self, body: Value) -> Result<Value, ProviderError> {
        let response = self
            .send(Method::POST, &["session"], Some(body), REQUEST_TIMEOUT)
            .await?;
        Self::decode(response, "create session").await
    }

    pub(super) async fn fork_session(&self, id: &str) -> Result<Value, ProviderError> {
        let response = self
            .send(
                Method::POST,
                &["session", id, "fork"],
                Some(json!({})),
                REQUEST_TIMEOUT,
            )
            .await?;
        Self::decode(response, "fork session").await
    }

    /// The server's slash commands, which `GET /agent` does not answer (§3.2's `commands`).
    pub(super) async fn commands(&self) -> Result<Vec<Value>, ProviderError> {
        let response = self
            .send(Method::GET, &["command"], None, REQUEST_TIMEOUT)
            .await?;
        Self::decode(response, "list commands").await
    }

    pub(super) async fn prompt_async(
        &self,
        session: &str,
        body: Value,
    ) -> Result<(), ProviderError> {
        let response = self
            .send(
                Method::POST,
                &["session", session, "prompt_async"],
                Some(body),
                REQUEST_TIMEOUT,
            )
            .await?;
        let status = response.status();
        if status == StatusCode::NO_CONTENT {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        Err(HttpFailure { status, body }.protocol("submit prompt"))
    }

    pub(super) async fn abort(&self, session: &str) -> Result<bool, ProviderError> {
        let response = self
            .send(
                Method::POST,
                &["session", session, "abort"],
                None,
                REQUEST_TIMEOUT,
            )
            .await?;
        Self::decode(response, "abort session").await
    }

    pub(super) async fn permission_reply(
        &self,
        request: &str,
        reply: &str,
    ) -> Result<bool, ProviderError> {
        let response = self
            .send(
                Method::POST,
                &["permission", request, "reply"],
                Some(permission_reply_body(reply)),
                REQUEST_TIMEOUT,
            )
            .await?;
        Self::decode(response, "reply to permission").await
    }

    pub(super) async fn question_reply(
        &self,
        request: &str,
        answers: &[Vec<String>],
    ) -> Result<bool, ProviderError> {
        let response = self
            .send(
                Method::POST,
                &["question", request, "reply"],
                Some(question_reply_body(answers)),
                REQUEST_TIMEOUT,
            )
            .await?;
        Self::decode(response, "reply to question").await
    }

    pub(super) async fn status(&self) -> Result<BTreeMap<String, Value>, ProviderError> {
        let response = self
            .send(Method::GET, &["session", "status"], None, STATUS_TIMEOUT)
            .await?;
        Self::decode(response, "session status").await
    }

    pub(super) async fn messages(&self, session: &str) -> Result<Vec<Value>, ProviderError> {
        let response = self
            .send(
                Method::GET,
                &["session", session, "message"],
                None,
                STATUS_TIMEOUT,
            )
            .await?;
        Self::decode(response, "session messages").await
    }

    pub(super) async fn permissions(&self) -> Result<Vec<Value>, ProviderError> {
        let response = self
            .send(Method::GET, &["permission"], None, STATUS_TIMEOUT)
            .await?;
        Self::decode(response, "pending permissions").await
    }

    pub(super) async fn questions(&self) -> Result<Vec<Value>, ProviderError> {
        let response = self
            .send(Method::GET, &["question"], None, STATUS_TIMEOUT)
            .await?;
        Self::decode(response, "pending questions").await
    }

    /// Whether the server's own `permission` config allows every protected operation.
    ///
    /// OpenCode decides permission server-side (`opencode.json`), so §2's mode word is a claim
    /// about behaviour the adapter cannot make on its own: a server configured `"permission":
    /// "allow"` never asks, whatever mode the tab was created with. `GET /config` is where the
    /// effective configuration is published. `None` means the server did not say.
    pub(super) async fn permission_allows_everything(&self) -> Result<bool, ProviderError> {
        let response = self
            .send(Method::GET, &["config"], None, REQUEST_TIMEOUT)
            .await?;
        let body: Value = Self::decode(response, "read config").await?;
        Ok(permission_allows_everything(body.get("permission")))
    }

    /// The model the server would use when a prompt names none, as `providerID/modelID`.
    ///
    /// §2 fixes the composer metadata row as `<agent> · <model> · <mode>`, and OpenCode only
    /// reports a model on the first assistant message — so a thread that had not run a turn yet
    /// read `build agent · full access` with the model segment simply missing. `GET /config`
    /// publishes the configured default, which is what that first turn is going to use.
    pub(super) async fn default_model(&self) -> Result<Option<ModelSelection>, ProviderError> {
        let response = self
            .send(Method::GET, &["config"], None, REQUEST_TIMEOUT)
            .await?;
        let body: Value = Self::decode(response, "read config").await?;
        Ok(body
            .get("model")
            .and_then(Value::as_str)
            .and_then(default_model_selection))
    }

    /// The context window of every model the server offers, keyed `providerID/modelID`.
    ///
    /// `GET /config/providers` is where OpenCode publishes `Model.limit.context`, which §2's
    /// `context 34%` is a percentage of. A server that does not answer simply leaves the map
    /// empty; the metadata row then omits the segment instead of inventing one.
    pub(super) async fn model_context_limits(
        &self,
    ) -> Result<BTreeMap<String, u64>, ProviderError> {
        let response = self
            .send(Method::GET, &["config", "providers"], None, REQUEST_TIMEOUT)
            .await?;
        let body: Value = Self::decode(response, "list providers").await?;
        let mut limits = BTreeMap::new();
        let providers = body
            .get("providers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for provider in providers {
            let Some(provider_id) = provider.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(models) = provider.get("models").and_then(Value::as_object) else {
                continue;
            };
            for (model_id, model) in models {
                if let Some(context) = model.pointer("/limit/context").and_then(Value::as_u64) {
                    limits.insert(format!("{provider_id}/{model_id}"), context);
                }
            }
        }
        Ok(limits)
    }

    pub(super) async fn event_stream(&self) -> Result<reqwest::Response, ProviderError> {
        let response = self
            .request(Method::GET, &["event"])?
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .map_err(|error| ProviderError::Protocol {
                message: format!("connect OpenCode event stream: {error}"),
            })?;
        if response.status().is_success() {
            Ok(response)
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            Err(HttpFailure { status, body }.protocol("event stream"))
        }
    }
}

/// Whether a `Config.permission` value grants everything without ever asking.
///
/// The field is either a bare word (`"allow"` / `"ask"`) or a map of operation to word; only a
/// configuration in which nothing can ask is a blanket allow. A missing field is the server's
/// default, which does ask, so it is not one.
/// Splits OpenCode's `providerID/modelID` reference into a [`ModelSelection`].
///
/// A reference with no `/` is not one OpenCode can resolve either, so it is dropped rather than
/// shown: §2's row states the model the next turn will use, not a string that failed to parse.
fn default_model_selection(reference: &str) -> Option<ModelSelection> {
    let (provider, model) = reference.split_once('/')?;
    (!provider.is_empty() && !model.is_empty()).then(|| ModelSelection {
        model: model.to_owned(),
        effort: None,
        provider: Some(provider.to_owned()),
    })
}

fn permission_allows_everything(permission: Option<&Value>) -> bool {
    fn is_allow(value: &Value) -> bool {
        match value {
            Value::String(word) => word == "allow",
            // A nested rule map (`{"bash": {"*": "allow"}}`) allows everything only if each of
            // its own entries does.
            Value::Object(entries) => !entries.is_empty() && entries.values().all(is_allow),
            _ => false,
        }
    }

    permission.is_some_and(is_allow)
}

pub(super) fn permission_reply_body(reply: &str) -> Value {
    json!({ "reply": reply })
}

pub(super) fn question_reply_body(answers: &[Vec<String>]) -> Value {
    json!({ "answers": answers })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_configuration_that_can_never_ask_counts_as_a_blanket_allow() {
        assert!(permission_allows_everything(Some(&json!("allow"))));
        assert!(permission_allows_everything(Some(
            &json!({ "edit": "allow", "bash": { "*": "allow" } })
        )));
        assert!(!permission_allows_everything(None));
        assert!(!permission_allows_everything(Some(&json!("ask"))));
        assert!(!permission_allows_everything(Some(
            &json!({ "edit": "allow", "bash": "ask" })
        )));
        assert!(!permission_allows_everything(Some(&json!({}))));
    }

    #[test]
    fn the_configured_default_model_seeds_the_metadata_row() {
        assert_eq!(
            default_model_selection("anthropic/claude-sonnet-5"),
            Some(ModelSelection {
                model: "claude-sonnet-5".to_owned(),
                effort: None,
                provider: Some("anthropic".to_owned()),
            })
        );
        // A reference OpenCode itself could not resolve leaves the segment out (§2 states the
        // model the next turn uses, never a string that failed to parse).
        assert_eq!(default_model_selection("claude-sonnet-5"), None);
        assert_eq!(default_model_selection("/claude-sonnet-5"), None);
        assert_eq!(default_model_selection("anthropic/"), None);
    }
}
