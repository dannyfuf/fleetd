#!/usr/bin/env python3
"""Generate Fleet's Codex app-server wire types and method tables.

Usage:
    codex app-server generate-json-schema --out /tmp/codex-schema --experimental
    scripts/generate-codex-wire.py /tmp/codex-schema

Writes crates/fleet-daemon/src/agents/codex/methods.rs and .../codex/wire/*.rs.

Hand-transcribing 133 client requests, 70 notifications and 18 item variants rots inside one
Codex release, so these files are generated and checked in. Only the transitive closure of the
methods Fleet actually issues or handles is emitted: the method *tables* are exhaustive, so a
method that appears or disappears upstream is still visible in the diff.
"""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys
from collections import OrderedDict

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "crates/fleet-daemon/src/agents/codex"

# The methods Fleet issues. Everything else in the table is deliberately unused; the reasons are
# in docs/NATIVE-AGENTS.md §4.2 and research/spec-A-harness.md §A.3.4.
USED_REQUESTS = [
    "initialize",
    "thread/start",
    "thread/resume",
    "thread/read",
    "thread/items/list",
    "turn/start",
    "turn/steer",
    "turn/interrupt",
    "thread/settings/update",
    "thread/compact/start",
    "thread/fork",
    "thread/unsubscribe",
    "model/list",
    "skills/list",
    "account/read",
    "account/rateLimits/read",
]

# The notifications Fleet maps. The rest are suppressed at the source or counted as unknown.
USED_NOTIFICATIONS = [
    "thread/started",
    "thread/status/changed",
    "thread/settings/updated",
    "thread/tokenUsage/updated",
    "thread/name/updated",
    "thread/closed",
    "thread/environment/connected",
    "thread/environment/disconnected",
    "thread/compacted",
    "turn/started",
    "turn/completed",
    "turn/diff/updated",
    "turn/plan/updated",
    "item/started",
    "item/completed",
    "item/agentMessage/delta",
    "item/reasoning/summaryTextDelta",
    "item/reasoning/summaryPartAdded",
    "item/reasoning/textDelta",
    "item/plan/delta",
    "item/commandExecution/outputDelta",
    "item/commandExecution/terminalInteraction",
    "item/fileChange/patchUpdated",
    "item/mcpToolCall/progress",
    "item/autoApprovalReview/started",
    "item/autoApprovalReview/completed",
    "serverRequest/resolved",
    "error",
    "model/rerouted",
    "model/safetyBuffering/updated",
    "warning",
    "guardianWarning",
    "deprecationNotice",
    "configWarning",
    "mcpServer/startupStatus/updated",
    "mcpServer/oauthLogin/completed",
    "account/updated",
    "account/rateLimits/updated",
    "skills/changed",
    "hook/started",
    "hook/completed",
]

# The server requests Fleet answers. Everything else answers -32601.
USED_SERVER_REQUESTS = [
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "item/tool/requestUserInput",
    "item/permissions/requestApproval",
    "mcpServer/elicitation/request",
]

# Which generated file a type lands in, by the root group that reaches it first.
BUCKETS = ["handshake", "threads", "turns", "items", "approvals", "accounts", "common"]

# Suppressed at the source. `optOutNotificationMethods` is a server-side event filter, and
# cutting an event at the source beats filtering it in the daemon — it matters most over a remote
# link, where `rawResponseItem/completed` alone roughly doubles the byte volume of a turn and is
# pure debug data. Every entry is checked against the generated notification table below, so a
# method that upstream renames fails generation instead of silently un-suppressing itself.
# `rawResponseItem/completed` and `rawResponse/completed` are named by the harness spec but do
# not exist in codex-cli 0.147.0's notification table, so they are not sent: the capability takes
# exact method names and a name the server does not know is a rejected capability, not a no-op.
OPT_OUT_NOTIFICATIONS = [
    "thread/realtime/started",
    "thread/realtime/itemAdded",
    "thread/realtime/transcript/delta",
    "thread/realtime/transcript/done",
    "thread/realtime/outputAudio/delta",
    "thread/realtime/sdp",
    "thread/realtime/error",
    "thread/realtime/closed",
    "fuzzyFileSearch/sessionUpdated",
    "fuzzyFileSearch/sessionCompleted",
    "app/list/updated",
    "externalAgentConfig/import/progress",
    "externalAgentConfig/import/completed",
    "fs/changed",
    "process/outputDelta",
    "process/exited",
    "command/exec/outputDelta",
    "turn/moderationMetadata",
    "windows/worldWritableWarning",
    "windowsSandbox/setupCompleted",
    "remoteControl/status/changed",
]

# Fields the generator puts behind a `Box`. `ThreadItem` is decoded for every transcript item,
# and a handful of rarely-populated object payloads would otherwise set the size of the whole
# enum: `mcpToolCall` alone carries 522 bytes against a 226-byte second place, which is a moved
# `ThreadItem` copying half a kilobyte per item and a `large_enum_variant` denial under
# `-D warnings`. `Option<Box<T>>` serializes and deserializes exactly like `Option<T>`, so the
# wire shape is unchanged. Keyed by (enum, variant, property) so a name upstream renames shows
# up as a generation diff rather than silently un-boxing itself.
BOXED_FIELDS = {
    ("ThreadItem", "McpToolCall", "result"),
    ("ThreadItem", "McpToolCall", "appContext"),
}


RUST_KEYWORDS = {
    "as", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
    "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
    "where", "while", "async", "await", "box", "final", "macro", "override", "priv", "try",
    "typeof", "unsized", "virtual", "yield",
}


def boxed(rust_type: str) -> str:
    """Put a `Box` around a field type, inside an `Option` so the niche is kept."""
    if rust_type.startswith("Option<") and rust_type.endswith(">"):
        return f"Option<Box<{rust_type[len('Option<') : -1]}>>"
    return f"Box<{rust_type}>"


def snake(name: str) -> str:
    name = re.sub(r"[^0-9a-zA-Z]+", "_", name)
    name = re.sub(r"(.)([A-Z][a-z]+)", r"\1_\2", name)
    name = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name)
    return name.lower().strip("_")


def pascal(name: str) -> str:
    parts = re.split(r"[^0-9a-zA-Z]+", name)
    return "".join(part[:1].upper() + part[1:] for part in parts if part)


def camel(name: str) -> str:
    parts = snake(name).split("_")
    return parts[0] + "".join(part.title() for part in parts[1:])


def field_name(key: str) -> tuple[str, str | None]:
    """Rust field name plus an explicit rename when camelCase would not round-trip."""
    ident = snake(key)
    if ident in RUST_KEYWORDS:
        ident = f"r#{ident}"
    expected = camel(key)
    rename = None if expected == key else key
    return ident, rename


def catch_all(taken: set[str]) -> str:
    """The name of the tolerant catch-all variant, avoiding a schema variant that took it."""
    return "Unknown" if "Unknown" not in taken else "Unrecognized"


def doc_lines(text: str | None, indent: str) -> list[str]:
    if not text:
        return []
    out = []
    for line in text.strip().splitlines():
        line = line.replace("\r", "").rstrip()
        out.append(f"{indent}/// {line}".rstrip())
    return out


class Generator:
    def __init__(self, schema_dir: pathlib.Path):
        self.definitions: dict[str, dict] = {}
        self.roots: "OrderedDict[str, str]" = OrderedDict()
        self.emitted: dict[str, list[str]] = {bucket: [] for bucket in BUCKETS}
        self.bucket_of: dict[str, str] = {}
        self.load(schema_dir)

    def load(self, schema_dir: pathlib.Path) -> None:
        files = sorted(schema_dir.glob("*.json")) + sorted(schema_dir.glob("v2/*.json"))
        for path in files:
            data = json.loads(path.read_text())
            for name, schema in data.get("definitions", {}).items():
                self.definitions.setdefault(name, schema)
            title = data.get("title")
            if title and "definitions" in data and title not in self.definitions:
                body = {k: v for k, v in data.items() if k not in ("definitions", "$schema")}
                if body:
                    self.definitions.setdefault(title, body)
        self.client_requests = self.variants(schema_dir / "ClientRequest.json")
        self.server_notifications = self.variants(schema_dir / "ServerNotification.json")
        self.server_requests = self.variants(schema_dir / "ServerRequest.json")

    @staticmethod
    def variants(path: pathlib.Path) -> "OrderedDict[str, str | None]":
        data = json.loads(path.read_text())
        out: "OrderedDict[str, str | None]" = OrderedDict()
        for variant in data["oneOf"]:
            method = variant["properties"]["method"]["enum"][0]
            params = variant["properties"].get("params", {}).get("$ref")
            out[method] = params.split("/")[-1] if params else None
        return out

    # ---- type resolution ---------------------------------------------------

    def rust_type(self, schema, owner: str, hint: str, bucket: str) -> str:
        if schema is True or schema == {}:
            return "serde_json::Value"
        if not isinstance(schema, dict):
            return "serde_json::Value"
        if "$ref" in schema:
            name = schema["$ref"].split("/")[-1]
            self.want(name, bucket)
            return pascal(name)
        for key in ("anyOf", "oneOf"):
            if key in schema:
                options = [option for option in schema[key] if option != {"type": "null"}]
                nullable = len(options) != len(schema[key])
                if len(options) == 1:
                    inner = self.rust_type(options[0], owner, hint, bucket)
                    return f"Option<{inner}>" if nullable else inner
                # A real union: emit a named type for it.
                name = schema.get("title") or f"{pascal(owner)}{pascal(hint)}"
                self.emit_union(name, schema, options, bucket)
                return f"Option<{pascal(name)}>" if nullable else pascal(name)
        if "allOf" in schema and len(schema["allOf"]) == 1:
            return self.rust_type(schema["allOf"][0], owner, hint, bucket)
        kinds = schema.get("type")
        if isinstance(kinds, list):
            nullable = "null" in kinds
            rest = [kind for kind in kinds if kind != "null"]
            inner = self.rust_type({**schema, "type": rest[0]} if rest else True, owner, hint, bucket)
            return f"Option<{inner}>" if nullable else inner
        if "enum" in schema and kinds == "string":
            name = schema.get("title") or f"{pascal(owner)}{pascal(hint)}"
            self.emit_string_enum(name, schema, bucket)
            return pascal(name)
        if kinds == "string":
            return "String"
        if kinds == "integer":
            return "u64" if str(schema.get("format", "")).startswith("uint") else "i64"
        if kinds == "number":
            return "f64"
        if kinds == "boolean":
            return "bool"
        if kinds == "array":
            inner = self.rust_type(schema.get("items", True), owner, f"{hint}Item", bucket)
            return f"Vec<{inner}>"
        if kinds == "object":
            if "properties" in schema:
                name = schema.get("title") or f"{pascal(owner)}{pascal(hint)}"
                self.emit_struct(name, schema, bucket)
                return pascal(name)
            return "serde_json::Value"
        return "serde_json::Value"

    def want(self, name: str, bucket: str) -> None:
        if name in self.bucket_of:
            return
        schema = self.definitions.get(name)
        if schema is None:
            self.bucket_of[name] = bucket
            self.emitted[bucket].append(
                f"/// `{name}` is not in the generated schema; kept opaque rather than guessed.\n"
                f"pub type {pascal(name)} = serde_json::Value;\n"
            )
            return
        self.bucket_of[name] = bucket
        self.emit_definition(name, schema, bucket)

    # ---- emission ----------------------------------------------------------

    def emit_definition(self, name: str, schema: dict, bucket: str) -> None:
        if "$ref" in schema:
            target = schema["$ref"].split("/")[-1]
            self.want(target, bucket)
            self.emitted[bucket].append(f"pub type {pascal(name)} = {pascal(target)};\n")
            return
        if "enum" in schema and schema.get("type") == "string":
            self.emit_string_enum(name, schema, bucket, declared=True)
            return
        for key in ("oneOf", "anyOf"):
            if key in schema:
                options = [option for option in schema[key] if option != {"type": "null"}]
                if len(options) == 1:
                    inner = self.rust_type(options[0], name, "", bucket)
                    self.emitted[bucket].append(f"pub type {pascal(name)} = {inner};\n")
                    return
                if schema.get("properties"):
                    # Both a union and sibling properties: the union is flattened into the
                    # struct, exactly as the schema means it.
                    variant = f"{name}Variant"
                    self.emit_union(variant, {"oneOf": options}, options, bucket)
                    self.emit_struct(
                        name,
                        {
                            **{k: v for k, v in schema.items() if k not in ("oneOf", "anyOf")},
                            "type": "object",
                        },
                        bucket,
                        declared=True,
                        flattened=(pascal(variant), "variant"),
                    )
                    return
                self.emit_union(name, schema, options, bucket, declared=True)
                return
        if schema.get("type") == "object" and "properties" in schema:
            self.emit_struct(name, schema, bucket, declared=True)
            return
        inner = self.rust_type(schema, name, "", bucket)
        if inner != pascal(name):
            self.emitted[bucket].append(f"pub type {pascal(name)} = {inner};\n")

    def emit_struct(
        self,
        name: str,
        schema: dict,
        bucket: str,
        declared: bool = False,
        flattened: tuple[str, str] | None = None,
    ) -> None:
        key = pascal(name)
        if not declared:
            if key in self.bucket_of:
                return
            self.bucket_of[key] = bucket
        lines: list[str] = []
        lines += doc_lines(schema.get("description") or f"`{name}`.", "")
        lines.append("#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]")
        lines.append('#[serde(rename_all = "camelCase")]')
        lines.append(f"pub struct {key} {{")
        required = set(schema.get("required", []))
        properties = schema.get("properties", {})
        for prop, prop_schema in properties.items():
            ident, rename = field_name(prop)
            inner = self.rust_type(prop_schema, name, prop, bucket)
            optional = prop not in required
            if optional and not inner.startswith("Option<"):
                inner = f"Option<{inner}>"
            lines += doc_lines(
                (prop_schema.get("description") if isinstance(prop_schema, dict) else None)
                or f"`{prop}`.",
                "    ",
            )
            attrs = []
            if rename:
                attrs.append(f'rename = "{rename}"')
            if inner.startswith("Option<"):
                attrs.append("default")
                attrs.append('skip_serializing_if = "Option::is_none"')
            elif optional:
                attrs.append("default")
            if attrs:
                lines.append(f"    #[serde({', '.join(attrs)})]")
            lines.append(f"    pub {ident}: {inner},")
        if flattened:
            kind, field = flattened
            lines.append("    /// The variant the schema flattens into this object.")
            lines.append("    #[serde(flatten)]")
            lines.append(f"    pub {field}: {kind},")
        lines.append("}\n")
        self.emitted[bucket].append("\n".join(lines))

    def emit_string_enum(self, name: str, schema: dict, bucket: str, declared: bool = False) -> None:
        key = pascal(name)
        if not declared:
            if key in self.bucket_of:
                return
            self.bucket_of[key] = bucket
        lines = doc_lines(schema.get("description") or f"`{name}`.", "")
        lines.append("#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]")
        lines.append(f"pub enum {key} {{")
        taken = set()
        for value in schema["enum"]:
            variant = pascal(str(value))
            taken.add(variant)
            lines.append(f'    #[serde(rename = "{value}")]')
            lines.append(f"    {variant},")
        lines.append("    /// A value this build does not know. Never a decode failure (§4.5).")
        lines.append("    #[serde(other)]")
        lines.append(f"    {catch_all(taken)},")
        lines.append("}\n")
        self.emitted[bucket].append("\n".join(lines))

    def emit_union(
        self, name: str, schema: dict, options: list[dict], bucket: str, declared: bool = False
    ) -> None:
        key = pascal(name)
        if not declared:
            if key in self.bucket_of:
                return
            self.bucket_of[key] = bucket
        internal_tag = self.internal_tag(options)
        if internal_tag:
            self.emit_internally_tagged(key, name, schema, options, internal_tag, bucket)
        else:
            self.emit_externally_tagged(key, name, schema, options, bucket)

    @staticmethod
    def internal_tag(options: list[dict]) -> str | None:
        tags = set()
        for option in options:
            properties = option.get("properties") or {}
            required = set(option.get("required") or [])
            candidates = [
                prop
                for prop, prop_schema in properties.items()
                if prop in required
                and isinstance(prop_schema, dict)
                and isinstance(prop_schema.get("enum"), list)
                and len(prop_schema["enum"]) == 1
            ]
            if not candidates:
                return None
            tags.add(candidates[0])
        return tags.pop() if len(tags) == 1 else None

    def emit_internally_tagged(
        self,
        key: str,
        name: str,
        schema: dict,
        options: list[dict],
        tag: str,
        bucket: str,
    ) -> None:
        lines = doc_lines(schema.get("description") or f"`{name}`.", "")
        lines.append("#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]")
        # `rename_all` renames *variants*; the fields inside a variant need
        # `rename_all_fields`, and without it every camelCase key in a tagged union fails to
        # decode. That is one of the two bugs the captured-trace test caught.
        lines.append(
            f'#[serde(tag = "{tag}", rename_all = "camelCase", rename_all_fields = "camelCase")]'
        )
        lines.append(f"pub enum {key} {{")
        taken = {
            pascal(str(dict(option.get("properties") or {})[tag]["enum"][0]))
            for option in options
        }
        for option in options:
            properties = dict(option.get("properties") or {})
            value = properties.pop(tag)["enum"][0]
            variant = pascal(str(value))
            required = set(option.get("required") or [])
            lines += doc_lines(option.get("description"), "    ")
            if not properties:
                lines.append(f'    #[serde(rename = "{value}")]')
                lines.append(f"    {variant},")
                continue
            lines.append(f'    #[serde(rename = "{value}")]')
            lines.append(f"    {variant} {{")
            for prop, prop_schema in properties.items():
                ident, rename = field_name(prop)
                inner = self.rust_type(prop_schema, f"{name}{variant}", prop, bucket)
                optional = prop not in required
                if optional and not inner.startswith("Option<"):
                    inner = f"Option<{inner}>"
                if (key, variant, prop) in BOXED_FIELDS:
                    inner = boxed(inner)
                attrs = []
                if rename:
                    attrs.append(f'rename = "{rename}"')
                if inner.startswith("Option<"):
                    attrs.append("default")
                    attrs.append('skip_serializing_if = "Option::is_none"')
                elif optional:
                    attrs.append("default")
                if attrs:
                    lines.append(f"        #[serde({', '.join(attrs)})]")
                lines.append(f"        {ident}: {inner},")
            lines.append("    },")
        lines.append("    /// A variant this build does not know, kept so one new value cannot")
        lines.append("    /// make a whole class of frames invisible (§4.5 rule 1).")
        lines.append("    #[serde(other)]")
        lines.append(f"    {catch_all(taken)},")
        lines.append("}\n")
        self.emitted[bucket].append("\n".join(lines))

    def emit_externally_tagged(
        self, key: str, name: str, schema: dict, options: list[dict], bucket: str
    ) -> None:
        """A union of bare strings and single-key objects: Rust's default enum representation."""
        nameable = [
            option
            for option in options
            if isinstance(option.get("enum"), list) or len(option.get("properties") or {}) == 1
        ]
        if not nameable:
            # A union of bare primitives (`string | integer`): there is nothing to name, and the
            # protocol treats it as an opaque token anyway.
            lines = doc_lines(schema.get("description") or f"`{name}`.", "")
            lines.append(f"pub type {key} = serde_json::Value;\n")
            self.emitted[bucket].append("\n".join(lines))
            return
        known = f"{key}Known"
        lines = doc_lines(schema.get("description") or f"`{name}`.", "")
        lines.append(f"pub type {key} = crate::agents::codex::tolerant::Tolerant<{known}>;\n")
        lines += doc_lines(f"The variants of `{name}` this build knows.", "")
        lines.append("#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]")
        lines.append("#[serde(rename_all = \"camelCase\")]")
        lines.append(f"pub enum {known} {{")
        for option in options:
            if isinstance(option.get("enum"), list):
                # One option can enumerate many bare strings; each is its own unit variant.
                for value in option["enum"]:
                    lines += doc_lines(option.get("description"), "    ")
                    lines.append(f'    #[serde(rename = "{value}")]')
                    lines.append(f"    {pascal(str(value))},")
                continue
            properties = option.get("properties") or {}
            if len(properties) == 1:
                prop, prop_schema = next(iter(properties.items()))
                inner = self.rust_type(prop_schema, f"{name}{pascal(prop)}", prop, bucket)
                lines += doc_lines(option.get("description"), "    ")
                lines.append(f'    #[serde(rename = "{prop}")]')
                lines.append(f"    {pascal(prop)}({inner}),")
                continue
            # Anything else is left to the tolerant wrapper rather than guessed at: an
            # externally tagged enum cannot carry a serde `other` arm.
            lines.append(f"    // A shape the generator could not name is decoded by")
            lines.append(f"    // `Tolerant<{known}>` as raw JSON.")
        lines.append("}\n")
        self.emitted[bucket].append("\n".join(lines))

    # ---- drivers -----------------------------------------------------------

    def response_name(self, params: str | None, method: str) -> str | None:
        if params and params.endswith("Params"):
            candidate = params[: -len("Params")] + "Response"
            if candidate in self.definitions:
                return candidate
        candidate = pascal(method.replace("/", " ")) + "Response"
        if candidate in self.definitions:
            return candidate
        return None

    def run(self) -> None:
        groups = [
            ("handshake", ["initialize"]),
            ("threads", [m for m in USED_REQUESTS if m.startswith("thread/")]
             + [m for m in USED_NOTIFICATIONS if m.startswith("thread/") or m.startswith("skills/")]),
            ("turns", [m for m in USED_REQUESTS if m.startswith("turn/")]
             + [m for m in USED_NOTIFICATIONS if m.startswith("turn/") or m == "error"]),
            ("items", [m for m in USED_NOTIFICATIONS if m.startswith("item/")]),
            ("approvals", USED_SERVER_REQUESTS + ["serverRequest/resolved"]),
            ("accounts", [m for m in USED_REQUESTS if m.startswith("account/") or m in ("model/list", "skills/list")]
             + [m for m in USED_NOTIFICATIONS if m.startswith("account/") or m.startswith("model/")]),
            ("common", USED_REQUESTS + USED_NOTIFICATIONS + USED_SERVER_REQUESTS),
        ]
        for bucket, methods in groups:
            for method in methods:
                for params, response in self.method_types(method):
                    if params:
                        self.want(params, bucket)
                    if response:
                        self.want(response, bucket)

    def method_types(self, method: str):
        if method in self.client_requests:
            params = self.client_requests[method]
            yield params, self.response_name(params, method)
        if method in self.server_notifications:
            yield self.server_notifications[method], None
        if method in self.server_requests:
            params = self.server_requests[method]
            yield params, self.response_name(params, method)

    def header(self, source: str) -> str:
        return (
            "// @generated by scripts/generate-codex-wire.py — DO NOT EDIT BY HAND.\n"
            f"//\n// Source: `codex app-server generate-json-schema --out <dir> --experimental`\n"
            f"// Generated from: {source}\n"
            "//\n// Regenerate after every Codex upgrade and read the diff: both harnesses add\n"
            "// fields and enum values without a version bump, and this file is where that shows.\n"
        )


def chunked(blocks: list[str], budget: int) -> list[list[str]]:
    """Groups emitted blocks into files of at most `budget` lines, never splitting a block."""
    chunks: list[list[str]] = []
    current: list[str] = []
    lines = 0
    for block in blocks:
        block_lines = block.count("\n") + 1
        if current and lines + block_lines > budget:
            chunks.append(current)
            current = []
            lines = 0
        current.append(block)
        lines += block_lines
    if current:
        chunks.append(current)
    return chunks or [[]]


DECLARATION = re.compile(r"^pub (?:struct|enum|type) ([A-Za-z0-9_]+)", re.MULTILINE)


def declared_in(text: str) -> set[str]:
    """The wire type names one generated part declares."""
    return set(DECLARATION.findall(text))


def sibling_glob(text: str, own: set[str], every: set[str]) -> str:
    """`use super::*;` only for a part that actually names a type another part declares.

    The glob is how one part reaches a sibling's type, but a part whose closure happens to be
    self-contained does not need it, and an unconditional glob there is an unused import the
    generator would have to silence with an `#[allow]`.
    """
    words = set(re.findall(r"[A-Za-z0-9_]+", text))
    if words & (every - own):
        return (
            "// Every part re-exports through `wire::mod`, so the glob below is what lets\n"
            "// this part name a type another part declares.\n"
            "use super::*;\n\n"
        )
    return ""


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    schema_dir = pathlib.Path(sys.argv[1])
    version = subprocess.run(
        ["codex", "--version"], capture_output=True, text=True, check=False
    ).stdout.strip() or "unknown"
    generator = Generator(schema_dir)
    generator.run()

    wire_dir = OUT_DIR / "wire"
    wire_dir.mkdir(parents=True, exist_ok=True)
    for stale in wire_dir.glob("*.rs"):
        stale.unlink()
    header = generator.header(version)
    modules: list[str] = []
    # No generated file passes ~700 lines: the repo's split-by-concern rule applies to
    # generated code too, and one 2 000-line file is unreviewable after a Codex upgrade.
    parts: list[tuple[str, str, int, str]] = []
    for bucket in BUCKETS:
        for index, chunk in enumerate(chunked(generator.emitted[bucket], 700), start=1):
            name = bucket if index == 1 else f"{bucket}_{index}"
            modules.append(name)
            parts.append((name, bucket, index, "\n".join(chunk)))
    declared = {name: declared_in(body) for name, _, _, body in parts}
    every = set().union(*declared.values()) if declared else set()
    for name, bucket, index, body in parts:
        text = (
            header
            + f"//! Generated Codex wire types reached from the {bucket} methods"
            + (f", part {index}" if index > 1 else "")
            + ".\n\n"
            + "use serde::{Deserialize, Serialize};\n\n"
            + sibling_glob(body, declared[name], every)
            + body
        )
        (wire_dir / f"{name}.rs").write_text(text)
    mod_text = (
        header
        + "//! Generated Codex wire types.\n//!\n"
        + "//! Split into parts so no generated file passes the repo's file-size rule; every type\n"
        + "//! is emitted exactly once, so the re-exports below cannot collide.\n\n"
        + "".join(f"mod {name};\n" for name in modules)
        + "\n"
        + "".join(f"pub use {name}::*;\n" for name in modules)
    )
    (wire_dir / "mod.rs").write_text(mod_text)

    methods = [header, "//! Generated Codex method tables.\n\n"]
    for const, table in (
        ("CLIENT_REQUEST_METHODS", generator.client_requests),
        ("SERVER_NOTIFICATION_METHODS", generator.server_notifications),
        ("SERVER_REQUEST_METHODS", generator.server_requests),
    ):
        names = list(table)
        methods.append(
            f"/// Every `{const.lower().replace('_methods', '')}` method this Codex build declares.\n"
            f"pub const {const}: [&str; {len(names)}] = [\n"
            + "".join(f'    "{name}",\n' for name in names)
            + "];\n\n"
        )
    unknown = [
        method
        for method in OPT_OUT_NOTIFICATIONS
        if method not in generator.server_notifications
    ]
    if unknown:
        print(f"error: opted-out methods no longer exist upstream: {unknown}", file=sys.stderr)
        return 1
    methods.append(
        "/// Notifications Fleet asks the server not to send.\n"
        "///\n"
        "/// Every entry is a real server notification in the schema this file was generated\n"
        "/// from — the generator refuses to write this list otherwise — so a method upstream\n"
        "/// renames fails generation instead of silently un-suppressing itself.\n"
        f"pub const OPT_OUT_NOTIFICATION_METHODS: [&str; {len(OPT_OUT_NOTIFICATIONS)}] = [\n"
        + "".join(f'    "{name}",\n' for name in OPT_OUT_NOTIFICATIONS)
        + "];\n\n"
    )
    methods.append(
        "/// Notifications Fleet maps to normalized events.\n"
        f"pub const MAPPED_NOTIFICATION_METHODS: [&str; {len(USED_NOTIFICATIONS)}] = [\n"
        + "".join(f'    "{name}",\n' for name in USED_NOTIFICATIONS)
        + "];\n\n"
    )
    methods.append(
        "/// Server requests Fleet answers. Every other server request answers `-32601`.\n"
        f"pub const IMPLEMENTED_SERVER_REQUESTS: [&str; {len(USED_SERVER_REQUESTS)}] = [\n"
        + "".join(f'    "{name}",\n' for name in USED_SERVER_REQUESTS)
        + "];\n\n"
    )
    methods.append(
        "/// The legacy v1 approval pair. Seeing either means Fleet took the legacy path, which\n"
        "/// is a version-gate failure by definition.\n"
        'pub const LEGACY_APPROVAL_REQUESTS: [&str; 2] = ["applyPatchApproval", "execCommandApproval"];\n'
    )
    (OUT_DIR / "methods.rs").write_text("".join(methods))
    print(f"wrote {len(BUCKETS)} wire files and methods.rs from {version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
