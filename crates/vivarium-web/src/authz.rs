//! RBAC permission codes with wildcard matching.
//!
//! Anchors permission checks in plain strings so applications can model codes
//! however they like (`"user:read"`, `"role:*"`, `"*"`); [`perms_match`] gives
//! the wildcard semantics and [`PermissionSet`] wraps a set of granted codes.

use crate::error::ApiError;

/// Returns true when the `held` permission code grants access to a request
/// requiring the `target` code.
///
/// Semantics (ported from the sibling project's reference implementation):
/// - `"*"` matches everything.
/// - Exact equality matches.
/// - `"prefix:*"` matches any `target` starting with the same `prefix` (the
///   literal `prefix` itself included).
pub fn perms_match(held: &str, target: &str) -> bool {
    if held == "*" {
        return true;
    }
    if held == target {
        return true;
    }
    if let (Some(prefix), Some(target_prefix)) = (held.strip_suffix(":*"), target.split(':').next())
    {
        return prefix == target_prefix;
    }
    false
}

/// The set of permission codes granted to a principal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionSet {
    codes: Vec<String>,
}

impl PermissionSet {
    /// Builds a set from any iterator of code strings.
    pub fn new<I, S>(codes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            codes: codes.into_iter().map(Into::into).collect(),
        }
    }

    /// Returns true when any granted code [`perms_match`]es `target`.
    pub fn has(&self, target: &str) -> bool {
        self.codes.iter().any(|held| perms_match(held, target))
    }

    /// Returns `Ok(())` when `target` is granted, otherwise
    /// `Err(ApiError::forbidden)` with the catalog's
    /// [`forbidden`](crate::texts::Texts::forbidden) message.
    ///
    /// The refused code is logged at debug level and deliberately kept out of
    /// the response: naming the missing permission tells an attacker which
    /// privileges exist, while an operator gets it from the log.
    pub fn require(&self, target: &str) -> Result<(), ApiError> {
        if self.has(target) {
            Ok(())
        } else {
            tracing::debug!(target, held = ?self.codes, "permission denied");
            Err(ApiError::forbidden(crate::texts::texts().forbidden.clone()))
        }
    }

    /// The granted codes, in insertion order.
    pub fn codes(&self) -> &[String] {
        &self.codes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perms_match_wildcard_matrix() {
        assert!(perms_match("*", "anything"));
        assert!(perms_match("*", "*"));
        assert!(perms_match("user:read", "user:read"));
        assert!(perms_match("user:*", "user:delete"));
        // Parity edge: a prefix wildcard grants the bare prefix itself, since
        // `target.split(':').next()` of a colon-less target is the target.
        assert!(perms_match("user:*", "user"));

        assert!(!perms_match("user:*", "role:read"));
        assert!(!perms_match("user:read", "user:write"));
        assert!(!perms_match("", "user:read"));
    }

    #[test]
    fn permission_set_has_and_require() {
        let perms = PermissionSet::new(["user:*", "role:read"]);

        assert!(perms.has("user:write"));
        assert!(perms.has("role:read"));
        assert!(!perms.has("role:delete"));
        assert!(!perms.has("admin:x"));
        assert_eq!(
            perms.codes(),
            &["user:*".to_string(), "role:read".to_string()]
        );

        assert!(perms.require("user:write").is_ok());
        let err = perms.require("admin:x").expect_err("must be denied");
        assert_eq!(err.kind(), crate::error::ErrorKind::Forbidden);
        assert_eq!(err.message(), crate::texts::texts().forbidden.as_ref());
        assert!(
            !err.message().contains("admin:x"),
            "the refused code belongs in the logs, not in the response: {}",
            err.message()
        );
    }
}
