//! A union wrapper that survives a variant this build has never seen.
//!
//! Codex adds enum values without a version bump — `rateLimitExceeded` and
//! `misalignmentPolicyViolation` were added to `CodexErrorInfo` after the schema this crate's
//! wire types were generated from, and neither is in `codex-cli 0.147.0`'s own list either.
//! t3code's client silently swallows any notification whose params fail to decode, so one new
//! value can make a whole class of events invisible with no error anywhere (§4.5).
//!
//! Rust's externally tagged enums cannot carry a `#[serde(other)]` arm, so the generated unions
//! that need one are wrapped in this type instead: a known variant decodes as `Known`, and
//! anything else is kept verbatim as `Unknown` so the adapter can still say *something* about it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A union value that is either a variant this build knows, or the raw JSON it could not name.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Tolerant<T> {
    /// A variant this build knows.
    Known(T),
    /// A variant this build does not know, kept as raw JSON.
    ///
    /// Never logged and never rendered as text: the adapter reads its *shape*, and callers that
    /// want to show something say "1 value Fleet doesn't understand".
    Unknown(Value),
}

impl<'de, T> Deserialize<'de> for Tolerant<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Decoded through `Value` rather than with `#[serde(untagged)]` so the fallback is
        // guaranteed: an untagged derive would return an error mentioning every variant it
        // tried, and that error text embeds the payload.
        let raw = Value::deserialize(deserializer)?;
        match T::deserialize(raw.clone()) {
            Ok(known) => Ok(Self::Known(known)),
            Err(_) => Ok(Self::Unknown(raw)),
        }
    }
}

impl<T> Tolerant<T> {
    /// The known variant, if this value is one.
    pub const fn known(&self) -> Option<&T> {
        match self {
            Self::Known(value) => Some(value),
            Self::Unknown(_) => None,
        }
    }

    /// The tag of an unknown variant: a bare string, or an object's single key.
    ///
    /// A *name*, never a value — it is what a warning line and an `AgentEvent::Unknown` may
    /// carry, and it is the only thing read out of an unknown payload.
    #[must_use]
    pub fn unknown_tag(&self) -> Option<&str> {
        match self {
            Self::Known(_) => None,
            Self::Unknown(Value::String(tag)) => Some(tag.as_str()),
            Self::Unknown(Value::Object(fields)) if fields.len() == 1 => {
                fields.keys().next().map(String::as_str)
            }
            Self::Unknown(_) => Some("unnamed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    enum Known {
        #[serde(rename = "usageLimitExceeded")]
        UsageLimitExceeded,
        #[serde(rename = "activeTurnNotSteerable")]
        ActiveTurnNotSteerable(Inner),
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Inner {
        #[serde(rename = "turnKind")]
        turn_kind: String,
    }

    #[test]
    fn a_known_bare_string_and_a_known_object_both_decode() {
        let bare: Tolerant<Known> = serde_json::from_str("\"usageLimitExceeded\"")
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(bare.known(), Some(&Known::UsageLimitExceeded));
        let object: Tolerant<Known> =
            serde_json::from_str(r#"{"activeTurnNotSteerable":{"turnKind":"review"}}"#)
                .unwrap_or_else(|error| panic!("{error}"));
        assert!(matches!(
            object.known(),
            Some(Known::ActiveTurnNotSteerable(inner)) if inner.turn_kind == "review"
        ));
    }

    /// The regression this type exists for: a value added upstream without a version bump.
    #[test]
    fn a_variant_added_upstream_decodes_as_unknown_and_names_only_its_tag() {
        let added: Tolerant<Known> = serde_json::from_str("\"rateLimitExceeded\"")
            .unwrap_or_else(|error| panic!("a new value must never fail the decode: {error}"));
        assert_eq!(added.known(), None);
        assert_eq!(added.unknown_tag(), Some("rateLimitExceeded"));

        let wrapped: Tolerant<Known> =
            serde_json::from_str(r#"{"misalignmentPolicyViolation":{"detail":"secret"}}"#)
                .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(wrapped.unknown_tag(), Some("misalignmentPolicyViolation"));
    }

    #[test]
    fn a_round_trip_keeps_the_wire_shape() {
        let value: Tolerant<Known> = serde_json::from_str("\"usageLimitExceeded\"")
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            serde_json::to_string(&value).unwrap_or_else(|error| panic!("{error}")),
            "\"usageLimitExceeded\""
        );
    }
}
