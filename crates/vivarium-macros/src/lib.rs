//! # vivarium-macros
//!
//! Procedural macros for the [`vivarium-rs`] family.
//!
//! Currently provides [`#[derive(Entity)]`][derive@Entity], which derives
//! `vivarium_core::Entity` for single-primary-key structs. The derive is
//! re-exported by `vivarium-db` and the `vivarium` facade, so it is usually
//! used as `vivarium_db::Entity` / `vivarium_rs::Entity` without depending on
//! this crate directly.
//!
//! # Attributes
//!
//! - `#[entity(table = "name")]` (struct): override the table name. Defaults
//!   to the snake_case form of the struct name.
//! - `#[entity(crate = "path")]` (struct): override the crate path the
//!   generated code anchors on (the `Entity` trait and `Value` type).
//!   Defaults to `::vivarium_rs` (the facade). Set it to
//!   `"vivarium_db"` when using the derive through `vivarium-db` without the
//!   facade, or to `"vivarium_core"` when depending on the lower-level
//!   crates directly.
//! - `#[entity(id)]` (field): mark the primary-key field. Defaults to the
//!   field named `id`. The type must implement `PrimaryKey` (`i64`, `u64`,
//!   `i32`, `u32`, or `String`).
//! - `#[entity(rename = "col")]` (field): override the column name. Defaults
//!   to the field name.
//! - `#[entity(json)]` (field): serialize the field via `serde_json` into a
//!   JSON column instead of requiring a natively supported type.
//!
//! `chrono::DateTime<Utc>`, `chrono::NaiveDate` and `uuid::Uuid` are
//! recognised **by the last path segment of the field type** (`DateTime`,
//! `NaiveDate`, `Uuid`), so a type of that name from another crate is mapped
//! onto the corresponding `Value` variant as well. Mark a colliding type
//! `#[entity(json)]` to store it as JSON instead.
//! - `#[entity(skip)]` (field): exclude the field from
//!   `columns_and_values` (it still decodes in `FromRow`).
//!
//! Unknown attribute keys are compile errors rather than being ignored, and a
//! field whose type is not supported is rejected with the list of supported
//! types. `Option<T>` fields bind as typed `NULL`s
//! (`Value::TypedNull(NullType::…)`) so that drivers which check parameter
//! types accept them; `#[entity(json)]` fields need `T: Serialize` and report
//! a serialization failure as `EncodeError` instead of panicking.
//!
//! # Example
//!
//! ```
//! use serde_json::Value;
//! use vivarium_core::Entity;
//!
//! #[derive(vivarium_macros::Entity)]
//! #[entity(table = "users", crate = "vivarium_core")]
//! struct User {
//!     id: i64,
//!     name: String,
//!     age: Option<i32>,
//!     #[entity(rename = "meta")]
//!     metadata: Value,
//! }
//!
//! let user = User { id: 7, name: "n".into(), age: None, metadata: Value::Null };
//! assert_eq!(User::TABLE, "users");
//! assert_eq!(User::ID_COLUMN, "id");
//! assert_eq!(user.id(), 7);
//!
//! let columns = user.columns_and_values().expect("encodes");
//! assert_eq!(columns.len(), 3);
//! // `None` becomes a typed NULL, not an untyped one.
//! assert_eq!(
//!     columns[1].1,
//!     vivarium_core::Value::TypedNull(vivarium_core::NullType::I64)
//! );
//! ```
//!
//! [`vivarium-rs`]: https://docs.rs/vivarium-rs
#![deny(missing_docs)]
#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, Data, DeriveInput, Error, Fields, LitStr, Result, Type, parse_macro_input};

/// Derives `vivarium_core::Entity` for a named-field struct.
///
/// See the [crate-level documentation](crate#attributes) for the supported
/// `#[entity(...)]` attributes.
#[proc_macro_derive(Entity, attributes(entity))]
pub fn derive_entity(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    match expand(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.into_compile_error().into(),
    }
}

fn expand(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "#[derive(Entity)] does not support generic structs",
        ));
    }

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            Fields::Unnamed(_) => {
                return Err(Error::new_spanned(
                    &data.fields,
                    "#[derive(Entity)] requires a struct with named fields",
                ));
            }
            Fields::Unit => {
                return Err(Error::new_spanned(
                    &data.fields,
                    "#[derive(Entity)] requires a struct with named fields",
                ));
            }
        },
        Data::Enum(_) | Data::Union(_) => {
            return Err(Error::new_spanned(
                &input.ident,
                "#[derive(Entity)] can only be derived for structs",
            ));
        }
    };

    let struct_attrs = EntityAttrs::parse(&input.attrs)?;
    let table: LitStr = match &struct_attrs.table {
        Some(lit) => lit.clone(),
        None => LitStr::new(&to_snake_case(&name.to_string()), name.span()),
    };
    let anchor_str = struct_attrs
        .crate_path
        .as_ref()
        .map_or_else(|| "::vivarium_rs".to_owned(), |lit| lit.value());
    let anchor: syn::Path = syn::parse_str(&anchor_str).map_err(|_| {
        Error::new_spanned(
            struct_attrs
                .crate_path
                .as_ref()
                .expect("crate attr present"),
            format!("invalid path in `#[entity(crate = ...)]`: {anchor_str}"),
        )
    })?;

    // Parse every field's attributes first, so a typo is reported at its own
    // span instead of surfacing as a confusing "needs an id field" error.
    for field in fields {
        field_attrs(field)?;
    }

    let (id_field, id_col_lit): (&syn::Field, LitStr) = {
        let marked: Vec<&syn::Field> = fields
            .iter()
            .filter(|f| field_attrs(f).is_ok_and(|attrs| attrs.id))
            .collect();
        let named: Vec<&syn::Field> = fields
            .iter()
            .filter(|f| f.ident.as_ref().is_some_and(|i| i == "id"))
            .collect();
        let field = match (marked.as_slice(), named.as_slice()) {
            ([marked], []) => *marked,
            ([], [named]) => *named,
            // `#[entity(id)] id: …`: the marker sits on the field that is also
            // named `id` — one candidate field, not two.
            ([marked], [named]) if std::ptr::eq(*marked, *named) => *marked,
            ([], []) => {
                return Err(Error::new_spanned(
                    name,
                    "#[derive(Entity)] needs an id field: mark one with #[entity(id)], \
                     or name a field `id`",
                ));
            }
            ([_], [_]) => {
                return Err(Error::new_spanned(
                    name,
                    "#[derive(Entity)] found more than one id candidate: a field marked \
                     #[entity(id)] and a field named `id`; keep exactly one",
                ));
            }
            _ => {
                return Err(Error::new_spanned(
                    name,
                    "#[derive(Entity)] found more than one id candidate \
                     (multiple #[entity(id)] fields)",
                ));
            }
        };
        {
            let ident = field.ident.as_ref().expect("named field");
            if !is_primary_key_type(&field.ty) {
                return Err(Error::new_spanned(
                    &field.ty,
                    "#[derive(Entity)] requires the id field to be a primary-key type: \
                     i64, u64, i32, u32, or String",
                ));
            }
            let attrs = field_attrs(field)?;
            let col_lit = attrs
                .rename
                .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
            (field, col_lit)
        }
    };
    let id_ident = id_field.ident.as_ref().expect("named field");

    let mut column_pushes = Vec::new();
    for field in fields {
        let ident = field.ident.as_ref().expect("named field");
        let attrs = field_attrs(field)?;
        if std::ptr::eq(field, id_field) || attrs.skip {
            continue;
        }
        let col_lit = attrs
            .rename
            .unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
        let value = value_expr(field, &quote!(self.#ident), attrs.json, &col_lit, &anchor)?;
        column_pushes.push(quote! {
            out.push((#col_lit, #value));
        });
    }

    let id_ty = &id_field.ty;
    // `PrimaryKey` is `Clone`, not `Copy`, so `String` keys are cloned while
    // the integer keys are copied.
    let id_expr = if type_is(id_ty, &["String"]) {
        quote!(self.#id_ident.clone())
    } else {
        quote!(self.#id_ident)
    };

    Ok(quote! {
        impl #anchor::Entity for #name {
            type Id = #id_ty;

            const TABLE: &'static str = #table;
            const ID_COLUMN: &'static str = #id_col_lit;

            fn id(&self) -> Self::Id {
                #id_expr
            }

            fn columns_and_values(
                &self,
            ) -> ::core::result::Result<
                ::std::vec::Vec<(&'static str, #anchor::Value)>,
                #anchor::EncodeError,
            > {
                let mut out = ::std::vec::Vec::new();
                #(#column_pushes)*
                ::core::result::Result::Ok(out)
            }
        }
    })
}

/// Error for a field type the derive cannot map onto a [`Value`] variant.
const UNSUPPORTED_FIELD: &str = "#[derive(Entity)] does not support this field type; use one of \
     i8-i64, u8-u64, usize, isize, f32, f64, bool, String, Vec<u8>, serde_json::Value, \
     chrono::DateTime<Utc>, chrono::NaiveDate, uuid::Uuid, or Option of those, or mark the field \
     #[entity(json)]";

/// Error for an `Option` inner type the derive cannot map onto a [`Value`]
/// variant.
const UNSUPPORTED_OPTION: &str = "#[derive(Entity)] does not support this field type inside \
     Option; use one of i8-i64, u8-u64, usize, isize, f32, f64, bool, String, Vec<u8>, \
     serde_json::Value, chrono::DateTime<Utc>, chrono::NaiveDate, uuid::Uuid";

/// Generates the `Value` conversion expression for a field.
///
/// `access` is a place expression for the field (`self.name`). `column` is the
/// column the field maps to, used in error messages. `Option<T>` fields bind
/// as typed NULLs so drivers that check parameter types accept them.
fn value_expr(
    field: &syn::Field,
    access: &proc_macro2::TokenStream,
    json: bool,
    column: &LitStr,
    anchor: &syn::Path,
) -> Result<proc_macro2::TokenStream> {
    if json {
        // Handled inside a function returning `Result<_, EncodeError>`, so the
        // `?` converts a serialization failure instead of panicking.
        return Ok(quote! {
            #anchor::Value::Json(::serde_json::to_value(&#access)?)
        });
    }
    let ty = &field.ty;
    if let Some(expr) = scalar_value_expr(ty, access, false, column, anchor) {
        return Ok(expr);
    }
    if let Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
        && segment.ident == "Option"
        && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        let inner_expr = value_expr_inner(inner, &quote!(v), column, anchor)?;
        let null_variant =
            null_variant(inner).ok_or_else(|| Error::new_spanned(inner, UNSUPPORTED_OPTION))?;
        let null_ident = format_ident!("{null_variant}");
        return Ok(quote! {
            match &#access {
                ::core::option::Option::Some(v) => #inner_expr,
                ::core::option::Option::None => {
                    #anchor::Value::TypedNull(#anchor::NullType::#null_ident)
                }
            }
        });
    }
    Err(Error::new_spanned(ty, UNSUPPORTED_FIELD))
}

/// Like [`value_expr`] for the inner type of an `Option` field, whose access
/// token evaluates to a `&T`.
fn value_expr_inner(
    ty: &Type,
    access: &proc_macro2::TokenStream,
    column: &LitStr,
    anchor: &syn::Path,
) -> Result<proc_macro2::TokenStream> {
    scalar_value_expr(ty, access, true, column, anchor)
        .ok_or_else(|| Error::new_spanned(ty, UNSUPPORTED_OPTION))
}

/// The `Value` constructor for a supported scalar field type, if any.
///
/// With `by_ref` the access token evaluates to a `&T` and is dereferenced.
/// `column` is the column the field maps to, used in error messages.
fn scalar_value_expr(
    ty: &Type,
    access: &proc_macro2::TokenStream,
    by_ref: bool,
    column: &LitStr,
    anchor: &syn::Path,
) -> Option<proc_macro2::TokenStream> {
    let deref = |token: &proc_macro2::TokenStream| {
        if by_ref {
            quote!(*#token)
        } else {
            token.clone()
        }
    };
    let value = deref(access);

    if type_is(ty, &["i64"]) {
        Some(quote!(#anchor::Value::I64(#value)))
    } else if is_lossy_integer_type(ty) {
        // `u64`, `usize` and `isize` reach past `i64::MAX`, so `as i64` would
        // silently wrap; ask `TryFrom` instead and report the column.
        let message = LitStr::new(
            &format!("column {} holds an integer above i64::MAX", column.value()),
            column.span(),
        );
        Some(quote! {
            #anchor::Value::I64(
                <i64 as ::std::convert::TryFrom<#ty>>::try_from(#value)
                    .map_err(|_| #anchor::EncodeError::new(#message))?
            )
        })
    } else if is_integer_type(ty) {
        Some(quote!(#anchor::Value::I64(#value as i64)))
    } else if type_is(ty, &["f64"]) {
        Some(quote!(#anchor::Value::F64(#value)))
    } else if type_is(ty, &["f32"]) {
        Some(quote!(#anchor::Value::F64(#value as f64)))
    } else if type_is(ty, &["bool"]) {
        Some(quote!(#anchor::Value::Bool(#value)))
    } else if type_is(ty, &["String"]) {
        let value = if by_ref {
            deref(access)
        } else {
            quote!(#access)
        };
        Some(quote!(#anchor::Value::Text((#value).clone())))
    } else if is_vec_u8(ty) {
        let value = if by_ref {
            deref(access)
        } else {
            quote!(#access)
        };
        Some(quote!(#anchor::Value::Bytes((#value).clone())))
    } else if type_is(ty, &["serde_json", "Value"]) || single_ident(ty, "Value") {
        Some(quote!(#anchor::Value::Json(
            ::serde_json::to_value(&#value)?
        )))
    } else if is_datetime_utc(ty) {
        Some(quote!(#anchor::Value::DateTime(#value)))
    } else if last_segment_is(ty, "NaiveDate") {
        Some(quote!(#anchor::Value::NaiveDate(#value)))
    } else if last_segment_is(ty, "Uuid") {
        Some(quote!(#anchor::Value::Uuid(#value)))
    } else {
        None
    }
}

/// The [`NullType`][anchor] variant an `Option<T>` of this type binds as.
fn null_variant(ty: &Type) -> Option<&'static str> {
    if type_is(ty, &["i64"]) || is_integer_type(ty) {
        Some("I64")
    } else if type_is(ty, &["f64"]) || type_is(ty, &["f32"]) {
        Some("F64")
    } else if type_is(ty, &["bool"]) {
        Some("Bool")
    } else if type_is(ty, &["String"]) {
        Some("Text")
    } else if is_vec_u8(ty) {
        Some("Bytes")
    } else if type_is(ty, &["serde_json", "Value"]) || single_ident(ty, "Value") {
        Some("Json")
    } else if is_datetime_utc(ty) {
        Some("DateTime")
    } else if last_segment_is(ty, "NaiveDate") {
        Some("NaiveDate")
    } else if last_segment_is(ty, "Uuid") {
        Some("Uuid")
    } else {
        None
    }
}

/// True for the primary-key types of `vivarium_core::PrimaryKey`.
fn is_primary_key_type(ty: &Type) -> bool {
    type_is(ty, &["i64"])
        || type_is(ty, &["u64"])
        || type_is(ty, &["i32"])
        || type_is(ty, &["u32"])
        || type_is(ty, &["String"])
}

/// True for the integer types mapped onto `Value::I64` (`i64` excluded; it is
/// handled separately so it can be passed through without a cast).
///
/// `u64`/`usize`/`isize` stay in this list because `null_variant` uses it to
/// pick the typed `NULL` for `Option<T>` fields; `scalar_value_expr` routes
/// those three through [`is_lossy_integer_type`] first, so their values are
/// converted with a checked conversion.
fn is_integer_type(ty: &Type) -> bool {
    [
        "i8", "i16", "i32", "u8", "u16", "u32", "u64", "usize", "isize",
    ]
    .iter()
    .any(|name| type_is(ty, &[*name]))
}

/// True for the integer types whose range reaches past `i64::MAX`, so the
/// conversion onto `Value::I64` must be checked instead of using `as i64`.
fn is_lossy_integer_type(ty: &Type) -> bool {
    type_is(ty, &["u64"]) || type_is(ty, &["usize"]) || type_is(ty, &["isize"])
}

/// True for `Vec<u8>` (however it is qualified), which maps onto
/// `Value::Bytes`.
///
/// `Vec<u8>` is one path segment carrying an argument, so the generic
/// [`type_is`] helper can never match it.
fn is_vec_u8(ty: &Type) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };
    let Some(segment) = type_path.path.segments.last() else {
        return false;
    };
    if segment.ident != "Vec" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return false;
    };
    args.args.len() == 1
        && matches!(
            args.args.first(),
            Some(syn::GenericArgument::Type(inner)) if type_is(inner, &["u8"])
        )
}

/// True for `chrono::DateTime<chrono::Utc>`, however it is qualified.
fn is_datetime_utc(ty: &Type) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };
    let Some(segment) = type_path.path.segments.last() else {
        return false;
    };
    if segment.ident != "DateTime" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return false;
    };
    args.args.iter().any(|arg| match arg {
        syn::GenericArgument::Type(Type::Path(inner)) => {
            inner.path.segments.last().is_some_and(|s| s.ident == "Utc")
        }
        _ => false,
    })
}

/// True when the last path segment is the bare identifier `name`
/// (`NaiveDate`, `chrono::NaiveDate`, …).
fn last_segment_is(ty: &Type, name: &str) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };
    type_path
        .path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == name && segment.arguments.is_none())
}

/// Struct-level `#[entity(...)]` attributes.
struct EntityAttrs {
    table: Option<LitStr>,
    crate_path: Option<LitStr>,
}

impl EntityAttrs {
    fn parse(attrs: &[Attribute]) -> Result<Self> {
        let mut table = None;
        let mut crate_path = None;
        for attr in attrs {
            if !attr.path().is_ident("entity") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("table") {
                    let lit: LitStr = meta.value()?.parse()?;
                    table = Some(lit);
                    Ok(())
                } else if meta.path.is_ident("crate") {
                    let lit: LitStr = meta.value()?.parse()?;
                    crate_path = Some(lit);
                    Ok(())
                } else {
                    Err(meta.error("unknown #[entity] attribute; expected `table = \"...\"` or `crate = \"...\"`"))
                }
            })?;
        }
        Ok(Self { table, crate_path })
    }
}

/// Field-level `#[entity(...)]` attributes.
#[derive(Default)]
struct FieldAttrs {
    id: bool,
    json: bool,
    skip: bool,
    rename: Option<LitStr>,
}

impl FieldAttrs {
    /// Parses a field's attributes, rejecting unknown keys instead of
    /// ignoring them.
    fn parse(attrs: &[Attribute]) -> Result<Self> {
        let mut parsed = Self::default();
        for attr in attrs {
            if !attr.path().is_ident("entity") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("id") {
                    parsed.id = true;
                    Ok(())
                } else if meta.path.is_ident("json") {
                    parsed.json = true;
                    Ok(())
                } else if meta.path.is_ident("skip") {
                    parsed.skip = true;
                    Ok(())
                } else if meta.path.is_ident("rename") {
                    let lit: LitStr = meta.value()?.parse()?;
                    parsed.rename = Some(lit);
                    Ok(())
                } else {
                    Err(meta.error(
                        "unknown #[entity] field attribute; expected `id`, `json`, `skip`, or \
                         `rename = \"...\"`",
                    ))
                }
            })?;
        }
        Ok(parsed)
    }
}

/// Parses a field's `#[entity(...)]` attributes.
fn field_attrs(field: &syn::Field) -> Result<FieldAttrs> {
    FieldAttrs::parse(&field.attrs)
}

/// True when the type path is exactly the given segments (e.g. `i64` or
/// `serde_json::Value`).
fn type_is(ty: &Type, segments: &[&str]) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };
    let path = &type_path.path;
    if path.leading_colon.is_some() {
        return false;
    }
    path.segments.len() == segments.len()
        && path
            .segments
            .iter()
            .zip(segments)
            .all(|(a, b)| a.ident == *b)
        && path.segments.iter().all(|s| s.arguments.is_none())
}

/// True when the type path is a single bare identifier equal to `name`.
fn single_ident(ty: &Type, name: &str) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };
    let path = &type_path.path;
    path.leading_colon.is_none()
        && path.segments.len() == 1
        && path.segments[0].ident == name
        && path.segments[0].arguments.is_none()
}

/// Converts `CamelCase` / `PascalCase` to `snake_case`, leaving lowercase
/// input unchanged.
fn to_snake_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 4);
    let mut prev: Option<char> = None;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_uppercase() {
            let prev_lower_or_digit = prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit());
            let prev_upper = prev.is_some_and(|p| p.is_uppercase());
            let next_lower = chars.peek().is_some_and(|n| n.is_lowercase());
            if !out.is_empty() && (prev_lower_or_digit || (prev_upper && next_lower)) {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
        prev = Some(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::to_snake_case;

    #[test]
    fn snake_case_conversion() {
        assert_eq!(to_snake_case("User"), "user");
        assert_eq!(to_snake_case("UserProfile"), "user_profile");
        assert_eq!(to_snake_case("HTTPServer"), "http_server");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
        assert_eq!(to_snake_case("user2FA"), "user2_fa");
    }
}
