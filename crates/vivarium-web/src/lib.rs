//! # vivarium-web
//!
//! HTTP ergonomics on top of [`axum`]: a unified [`ApiError`] response
//! contract, extractors that deserialize + initialize + validate in one
//! pipeline, JWT authentication middleware, cookie sessions with sliding
//! renewal, single-use refresh-token rotation, RBAC permission wildcards,
//! password hashing, and a `Cache-Control` layer.
//!
//! [`vivarium-rs`]: https://docs.rs/vivarium-rs
#![deny(missing_docs)]

pub mod authz;
pub mod cache;
pub mod error;
pub mod jwt;
pub mod password;
pub mod serve;
pub mod session;
pub mod token;
pub mod varser;

pub use authz::{PermissionSet, perms_match};
pub use error::ApiError;
pub use password::{hash, verify, verify_login};
pub use session::{
    OptionalSessionCtx, SessionAuth, SessionCtx, SessionRecord, SessionStore, session_layer,
};
pub use token::{RefreshTokenManager, RefreshTokenRecord, RefreshTokenStore, TokenPair};
pub use varser::{FormVarser, Initializer, PathVarser, QueryVarser, Varser, get_authorization};
