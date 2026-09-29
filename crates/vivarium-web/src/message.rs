//! A client-facing message that cannot be blank.
//!
//! [`Message`] is the value type of every message the library renders —
//! [`Texts`](crate::texts::Texts) fields,
//! [`ApiResponse::message`](crate::ApiResponse::message), and
//! [`FieldViolation::message`](crate::validation::FieldViolation::message) — so
//! an empty or whitespace-only string cannot reach a response body. There is
//! deliberately no infallible `From` conversion: build one with
//! [`Message::try_new`] (or the `TryFrom` impls) and handle [`MessageError`]
//! where the message enters the program.
//!
//! ```
//! use vivarium_web::Message;
//!
//! assert!(Message::try_new("no such user").is_ok());
//! assert!(Message::try_new("   ").is_err());
//! ```

use std::borrow::Cow;
use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

/// A non-blank, client-facing message.
///
/// Built through [`Message::try_new`], which rejects empty and whitespace-only
/// values, or through the `TryFrom<&str>` / `TryFrom<String>` /
/// `TryFrom<Cow>` impls. It serializes as the bare string, so the wire shape of
/// [`ApiResponse`](crate::ApiResponse) is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "utoipa", schema(value_type = String))]
pub struct Message(Cow<'static, str>);

impl Message {
    /// Builds a message, rejecting empty and whitespace-only values.
    ///
    /// Any other text is kept verbatim (it is not trimmed).
    ///
    /// # Errors
    ///
    /// Returns [`MessageError`] when `message` is empty or whitespace-only.
    pub fn try_new(message: impl Into<Cow<'static, str>>) -> Result<Self, MessageError> {
        let message = message.into();
        if message.trim().is_empty() {
            return Err(MessageError);
        }
        Ok(Self(message))
    }

    /// The message text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A `'static` literal the crate supplies itself: the English catalog
    /// defaults and the `"ok"` success text.
    ///
    /// Crate-private on purpose — a caller outside this crate must go through
    /// [`Message::try_new`]. Both call sites are const items, so the blank check
    /// (ASCII whitespace, since `str::trim` is not `const`) runs at compile
    /// time: a blank literal is a build error rather than a runtime one.
    pub(crate) const fn literal(message: &'static str) -> Self {
        let bytes = message.as_bytes();
        let mut index = 0;
        while index < bytes.len()
            && matches!(bytes[index], b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
        {
            index += 1;
        }
        assert!(index < bytes.len(), "a message must not be blank");
        Self(Cow::Borrowed(message))
    }
}

impl TryFrom<&str> for Message {
    type Error = MessageError;

    fn try_from(message: &str) -> Result<Self, Self::Error> {
        Self::try_new(message.to_string())
    }
}

impl TryFrom<String> for Message {
    type Error = MessageError;

    fn try_from(message: String) -> Result<Self, Self::Error> {
        Self::try_new(message)
    }
}

impl TryFrom<Cow<'static, str>> for Message {
    type Error = MessageError;

    fn try_from(message: Cow<'static, str>) -> Result<Self, Self::Error> {
        Self::try_new(message)
    }
}

impl From<Message> for String {
    fn from(message: Message) -> Self {
        message.0.into_owned()
    }
}

impl From<Message> for Cow<'static, str> {
    fn from(message: Message) -> Self {
        message.0
    }
}

impl Deref for Message {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for Message {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<str> for Message {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Message {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// Returned by [`Message::try_new`] for an empty or whitespace-only message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a message must not be blank")]
pub struct MessageError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_messages_are_rejected() {
        assert!(Message::try_new("").is_err());
        assert!(Message::try_new("   \t").is_err());
        assert!(Message::try_new("\n").is_err());
        assert!(Message::try_from("").is_err());

        let message = Message::try_new("ok").expect("non-blank");
        assert_eq!(message.as_str(), "ok");
        assert_eq!(message, "ok");
        assert_eq!(&*message, "ok");
    }

    #[test]
    fn message_serializes_as_a_bare_string() {
        let message = Message::try_new("not found").expect("non-blank");
        assert_eq!(
            serde_json::to_value(&message).expect("serializes"),
            serde_json::json!("not found")
        );
        assert_eq!(
            serde_json::from_str::<Message>(r#""not found""#).expect("deserializes"),
            message
        );
        assert!(serde_json::from_str::<Message>(r#""""#).is_err());
    }

    #[test]
    fn message_text_is_kept_verbatim() {
        let message = Message::try_new("  spaced  ").expect("non-blank");
        assert_eq!(message.as_str(), "  spaced  ");
    }
}
