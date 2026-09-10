//! The re-export surface, asserted at compile time.
//!
//! The facade's contract is "one dependency, the whole family": every name the
//! README, `AGENTS.md` and the crate docs tell a user to write must be reachable
//! at that path. The imports below *are* the assertion — this file exists to be
//! compiled, and the one test keeps it from being an empty target that cargo
//! might stop building.
#![cfg(all(feature = "web", feature = "db", feature = "config"))]
#![allow(unused_imports)]

// Modules the docs promise.
use vivarium_rs::{
    authz, cache, error, jwt, password, response, secrets, serve, session, texts, token,
    validation, varser,
};

// The JWT shorthands the README's prose writes out in full.
use vivarium_rs::jwt::{decode_token, jwt_auth, sign_token};

// Core vocabulary and the db layer, including the doc-hidden driver glue that a
// bound written against `vivarium-db` needs.
use vivarium_rs::{
    Column, DbError, DriverOps, EncodeError, Entity, Expr, NullType, Order, Page, Pagination,
    Predicate, PrimaryKey, PrimaryKeyError, Query, RawFragment, RawFragmentError, Sorter, Step,
    Update, Value, count, create, delete, exists, find_by_id, is_unique_violation, sqlx,
    update_by_id, with_transaction,
};

// The web layer: the envelope, the extractors, the auth surface, and the
// serving helpers.
use vivarium_rs::{
    AccessClaims, ApiError, ApiResponse, Argon2Params, CacheControl, Claims, CookieOptions,
    ErrorKind, FieldViolation, FormVarser, Initializer, JwtConfig, JwtVerifier, KeyRing,
    OptionalSessionCtx, PathVarser, PermissionSet, QueryVarser, RefreshTokenManager,
    RefreshTokenRecord, RefreshTokenStore, SameSite, SessionAuth, SessionCtx, SessionId,
    SessionRecord, SessionStore, Texts, TokenPair, ValidationErrors, Varser, VerifyOutcome,
    debug_mode, get_authorization, hash, hash_token, hash_with, install_debug_mode, install_texts,
    needs_rehash, perms_match, serve_with_shutdown, session_layer, should_extend, shutdown_signal,
    verify, verify_and_upgrade, verify_login,
};

// The config layer.
use vivarium_rs::{Config, ConfigError, ConfigOptions, ConfigWatcher, HandlerId};

#[cfg(feature = "validation-garde")]
use vivarium_rs::{GardeFormVarser, GardePathVarser, GardeQueryVarser, GardeVarser};

#[cfg(feature = "utoipa")]
use vivarium_rs::openapi;

#[test]
fn the_promised_paths_resolve() {
    // Two of the names above are easy to rename by accident and cheap to pin by
    // value; the rest are pinned by the fact that this file compiles.
    assert_eq!(ErrorKind::NotFound.as_str(), "not_found");
    assert_eq!(error::ErrorKind::Forbidden.status().as_u16(), 403);
    assert_eq!(ApiError::not_found("x").message(), "x");
}
