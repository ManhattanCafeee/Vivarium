//! The library's message catalog.
//!
//! The crate's own default messages — extractor rejections, session and JWT
//! refusals, the fixed text of each [`ErrorKind`](crate::ErrorKind), and
//! [`ApiError::database`](crate::ApiError::database) — come from [`Texts`].
//! [`Texts::default`] is entirely English; an application that needs another
//! language builds a catalog and installs it once, at startup, before serving
//! traffic. Every field is a [`Message`], which rejects empty and
//! whitespace-only text, so a catalog can never render a blank `message`:
//!
//! ```
//! use vivarium_web::texts::{Message, Texts, install_texts};
//!
//! let localized = Texts {
//!     unauthorized: Message::try_new("session cookie required").expect("non-blank"),
//!     echo_details: true,
//!     ..Texts::default()
//! };
//! // The first call wins; a second one returns `TextsAlreadySet`.
//! let _ = install_texts(localized);
//! assert_eq!(texts().unauthorized, "session cookie required");
//! # use vivarium_web::texts::texts;
//! ```
//!
//! Messages a call site supplies itself are never overridden by the catalog: a
//! handler's `ApiError::not_found("no such user")` is exactly what the client
//! sees. The library's own refusals do come from here — the extractor
//! rejections, the session and JWT 401s, the 403 of
//! [`PermissionSet::require`](crate::authz::PermissionSet::require), and the
//! refresh-token 401 ([`invalid_refresh`](Texts::invalid_refresh)).

use std::sync::OnceLock;

/// The field type of every catalog entry, re-exported here because that is
/// where a catalog author looks for it.
pub use crate::message::Message;

/// The process-wide message catalog.
///
/// Every field defaults to English. Fields are public so a catalog can be
/// written with struct-update syntax (`..Texts::default()`); each one is a
/// [`Message`], built with [`Message::try_new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texts {
    /// Message of a [`ErrorKind::DataParse`](crate::ErrorKind::DataParse)
    /// error: the request body could not be deserialized.
    pub data_parse: Message,
    /// Message of a [`ErrorKind::BadRequest`](crate::ErrorKind::BadRequest)
    /// error: the request is malformed.
    pub bad_request: Message,
    /// Message of a [`ErrorKind::Validation`](crate::ErrorKind::Validation)
    /// error; the details travel in the structured `errors` field.
    pub validation: Message,
    /// Message of a [`ErrorKind::Unauthorized`](crate::ErrorKind::Unauthorized)
    /// error.
    pub unauthorized: Message,
    /// Message of a [`ErrorKind::Forbidden`](crate::ErrorKind::Forbidden) error.
    pub forbidden: Message,
    /// Message of a [`ErrorKind::NotFound`](crate::ErrorKind::NotFound) error.
    pub not_found: Message,
    /// Message of a [`ErrorKind::Conflict`](crate::ErrorKind::Conflict) error.
    pub conflict: Message,
    /// Message of a [`ErrorKind::TooManyRequests`](crate::ErrorKind::TooManyRequests) error.
    pub too_many_requests: Message,
    /// Message of the 401 a rejected refresh token produces: unknown, expired,
    /// already consumed, or raced by a concurrent rotation.
    pub invalid_refresh: Message,
    /// Message of a [`ErrorKind::Internal`](crate::ErrorKind::Internal) error
    /// that carries no further detail.
    pub internal: Message,
    /// Message of an [`ErrorKind::Internal`](crate::ErrorKind::Internal) error
    /// caused by the database.
    pub database: Message,
    /// Whether a client-facing `message` may repeat the raw upstream detail
    /// (a `serde` / `axum` rejection text) of a 4xx parse failure.
    ///
    /// `false` keeps the fixed English text above and leaves the detail in the
    /// error source, where it is logged. (A 4xx never carries a `system`
    /// field: only `Internal` errors do, and only in debug mode.)
    /// Applications whose clients parse those messages — for example to
    /// surface a custom `Deserialize` error — turn this on.
    pub echo_details: bool,
}

impl Default for Texts {
    fn default() -> Self {
        // Const-evaluated, so `Message::literal`'s blank check runs at compile
        // time: a blank default is a build error, not a runtime one.
        const ENGLISH: Texts = Texts {
            data_parse: Message::literal("invalid request data"),
            bad_request: Message::literal("invalid request"),
            validation: Message::literal("validation failed"),
            unauthorized: Message::literal("authentication required"),
            forbidden: Message::literal("permission denied"),
            not_found: Message::literal("not found"),
            conflict: Message::literal("conflict"),
            too_many_requests: Message::literal("too many requests"),
            invalid_refresh: Message::literal("invalid or expired refresh token"),
            internal: Message::literal("internal error"),
            database: Message::literal("database error"),
            echo_details: false,
        };
        ENGLISH
    }
}

/// Returned by [`install_texts`] when a catalog was already installed.
///
/// The first installation wins, so a repeated call is harmless and can be
/// ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the message catalog was already installed")]
pub struct TextsAlreadySet;

/// The installed catalog, if any.
static TEXTS: OnceLock<Texts> = OnceLock::new();

/// Installs the process-wide [`Texts`], once.
///
/// Call this at startup, before serving requests. Returns
/// [`TextsAlreadySet`] when a catalog was already installed; the first one
/// stays in place. Every field is a [`Message`], so an installed catalog cannot
/// hold a blank message.
///
/// # Errors
///
/// Returns [`TextsAlreadySet`] if called more than once per process.
pub fn install_texts(texts: Texts) -> Result<(), TextsAlreadySet> {
    TEXTS.set(texts).map_err(|_| TextsAlreadySet)
}

/// The process-wide catalog: the installed one, or the English default.
pub fn texts() -> &'static Texts {
    TEXTS.get_or_init(Texts::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults are the crate's English contract (appendix A.1) and must
    /// stay exactly as documented — no test in this crate installs a catalog,
    /// because installation is process-wide and would leak across tests.
    #[test]
    fn default_catalog_is_the_english_contract() {
        let texts = Texts::default();
        assert_eq!(texts.data_parse, "invalid request data");
        assert_eq!(texts.bad_request, "invalid request");
        assert_eq!(texts.validation, "validation failed");
        assert_eq!(texts.unauthorized, "authentication required");
        assert_eq!(texts.forbidden, "permission denied");
        assert_eq!(texts.not_found, "not found");
        assert_eq!(texts.conflict, "conflict");
        assert_eq!(texts.too_many_requests, "too many requests");
        assert_eq!(texts.invalid_refresh, "invalid or expired refresh token");
        assert_eq!(texts.internal, "internal error");
        assert_eq!(texts.database, "database error");
        assert!(!texts.echo_details);
    }

    #[test]
    fn texts_is_never_empty() {
        let texts = texts();
        for message in [
            &texts.data_parse,
            &texts.bad_request,
            &texts.validation,
            &texts.unauthorized,
            &texts.forbidden,
            &texts.not_found,
            &texts.conflict,
            &texts.too_many_requests,
            &texts.invalid_refresh,
            &texts.internal,
            &texts.database,
        ] {
            assert!(!message.is_empty());
        }
    }
}
