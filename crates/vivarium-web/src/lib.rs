//! # vivarium-web
//!
//! HTTP ergonomics on top of [`axum`]: a unified [`ApiError`] response
//! contract, extractors that deserialize + initialize + validate in one
//! pipeline, JWT authentication middleware, and a `Cache-Control` layer.
//!
//! [`vivarium`]: https://docs.rs/vivarium
#![deny(missing_docs)]

pub mod cache;
pub mod error;
pub mod jwt;
pub mod serve;
pub mod varser;

pub use error::ApiError;
pub use varser::{FormVarser, Initializer, PathVarser, QueryVarser, Varser, get_authorization};
