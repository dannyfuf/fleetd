//! Newline-delimited JSON frames for the Fleet GUI harness.
//!
//! Command names are additive-only: extending the protocol means adding a command variant,
//! never changing the request or response frame shape.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One correlated request frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Correlation identifier echoed by the response.
    pub id: u64,
    /// Additive command name.
    pub cmd: String,
    /// Command-specific argument object.
    pub args: Value,
}

impl Request {
    /// Encodes a typed command into the stable request envelope.
    pub fn new(id: u64, command: &Command) -> anyhow::Result<Self> {
        let Value::Object(mut object) = serde_json::to_value(command)? else {
            anyhow::bail!("command did not serialize as an object");
        };
        let cmd = object
            .remove("cmd")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| anyhow::anyhow!("serialized command has no string cmd"))?;
        let args = object
            .remove("args")
            .unwrap_or_else(|| serde_json::json!({}));
        Ok(Self { id, cmd, args })
    }

    /// Decodes the typed command while retaining unknown command names in the envelope.
    pub fn command(&self) -> anyhow::Result<Command> {
        serde_json::from_value(serde_json::json!({ "cmd": self.cmd, "args": self.args }))
            .map_err(Into::into)
    }
}

/// One correlated response frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Correlation identifier copied from the request.
    pub id: u64,
    /// Whether the command completed successfully.
    pub ok: bool,
    /// Command-specific JSON data. Errors carry an empty object.
    pub data: Value,
    /// Human-readable failure, or null on success.
    pub error: Option<String>,
}

impl Response {
    /// Constructs a successful response.
    pub fn ok(id: u64, data: impl Into<Value>) -> Self {
        Self {
            id,
            ok: true,
            data: data.into(),
            error: None,
        }
    }

    /// Constructs a failed response.
    pub fn err(id: u64, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            data: Value::Object(Default::default()),
            error: Some(message.into()),
        }
    }
}

/// Complete version-one command vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum Command {
    Meta(EmptyArgs),
    Quit(EmptyArgs),
    Wait(WaitArgs),
    Key(KeyArgs),
    Type(TypeArgs),
    Shot(ShotArgs),
    Dump(DumpArgs),
    Await(AwaitArgs),
    Assert(AssertArgs),
    Move(MoveArgs),
    Click(ClickArgs),
    Press(PressArgs),
    Release(ReleaseArgs),
    Drag(DragArgs),
    Hover(HoverArgs),
    Scroll(ScrollArgs),
    ClipboardSet(ClipboardSetArgs),
    ClipboardGet(EmptyArgs),
    Resize(ResizeArgs),
    Blur(EmptyArgs),
    Focus(EmptyArgs),
    Advance(AdvanceArgs),
}

/// An explicitly empty argument object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyArgs {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitArgs {
    pub millis: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyArgs {
    pub keys: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeArgs {
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShotArgs {
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpArgs {
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AwaitArgs {
    pub predicate: String,
    pub timeout_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertArgs {
    pub predicate: String,
}

/// A target name or raw logical-window point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Location {
    Target { target: String },
    Point { x: f32, y: f32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveArgs {
    pub to: Location,
}

/// Mouse button vocabulary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClickArgs {
    pub at: Location,
    pub button: MouseButton,
    pub count: u8,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PressArgs {
    pub at: Location,
    pub button: MouseButton,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseArgs {
    pub at: Location,
    pub button: MouseButton,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DragArgs {
    pub from: Location,
    pub to: Location,
    pub button: MouseButton,
    pub steps: u16,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HoverArgs {
    pub at: Location,
    pub dwell_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrollArgs {
    pub dx: f32,
    pub dy: f32,
    pub at: Option<Location>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardSetArgs {
    pub text: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResizeArgs {
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdvanceArgs {
    pub millis: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_response_frames_are_pinned() {
        let request = Request::new(
            7,
            &Command::Key(KeyArgs {
                keys: vec!["ctrl-s".into(), "?".into()],
            }),
        )
        .expect("encode request");
        assert_eq!(
            serde_json::to_string(&request).expect("serialize request"),
            r#"{"id":7,"cmd":"key","args":{"keys":["ctrl-s","?"]}}"#
        );
        assert_eq!(
            serde_json::from_str::<Request>(
                r#"{"id":7,"cmd":"key","args":{"keys":["ctrl-s","?"]}}"#
            )
            .expect("deserialize request"),
            request
        );
        assert_eq!(
            request.command().expect("decode command"),
            Command::Key(KeyArgs {
                keys: vec!["ctrl-s".into(), "?".into()]
            })
        );
        assert_eq!(
            serde_json::to_string(&Response::ok(7, serde_json::json!({"handled":true})))
                .expect("serialize response"),
            r#"{"id":7,"ok":true,"data":{"handled":true},"error":null}"#
        );
        assert_eq!(
            serde_json::to_string(&Response::err(7, "bad key")).expect("serialize response"),
            r#"{"id":7,"ok":false,"data":{},"error":"bad key"}"#
        );

        let unknown = serde_json::from_str::<Request>(r#"{"id":9,"cmd":"future","args":{}}"#)
            .expect("unknown command stays in envelope");
        assert_eq!(unknown.cmd, "future");
        assert!(unknown.command().is_err());
    }
}
