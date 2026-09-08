//! # vivarium-macros
//!
//! Procedural macros for the [`vivarium-rs`] family.
//!
//! Currently provides [`#[derive(Entity)]`][derive@Entity], which derives
//! `vivarium_core::Entity` for single-`i64`-primary-key structs. The derive
//! is re-exported by `vivarium-db` and the `vivarium` facade, so it is usually
//! used as `vivarium_db::Entity` / `vivarium_rs::Entity` without depending on
//! this crate directly.
//!
//! # Attributes
//!
//! - `#[entity(table = "name")]` (struct): override the table name. Defaults
//!   to the snake_case form of the struct name.
//! - `#[entity(id)]` (field): mark the primary-key field. Defaults to the
//!   field named `id`. Must be `i64`.
//! - `#[entity(rename = "col")]` (field): override the column name. Defaults
//!   to the field name.
//! - `#[entity(json)]` (field): serialize the field via `serde_json` into a
//!   JSON column instead of requiring a natively supported type.
//! - `#[entity(skip)]` (field): exclude the field from
//!   `columns_and_values` (it still decodes in `FromRow`).
//!
//! # Example
//!
//! ```
//! use serde_json::Value;
//! use vivarium_core::Entity;
//!
//! #[derive(vivarium_macros::Entity)]
//! #[entity(table = "users")]
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
//! assert_eq!(user.columns_and_values().len(), 3);
//! ```
//!
//! [`vivarium-rs`]: https://docs.rs/vivarium-rs
#![deny(missing_docs)]
#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Error, Fields, LitStr, Meta, Result, Type, parse_macro_input,
};

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

    let (id_field, id_col_lit): (&syn::Field, LitStr) = {
        let marked: Vec<&syn::Field> = fields
            .iter()
            .filter(|f| field_attr(f, "id").is_some())
            .collect();
        let named: Vec<&syn::Field> = fields
            .iter()
            .filter(|f| f.ident.as_ref().is_some_and(|i| i == "id"))
            .collect();
        let field = match (marked.as_slice(), named.as_slice()) {
            ([marked], []) => *marked,
            ([], [named]) => *named,
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
            if !type_is(&field.ty, &["i64"]) {
                return Err(Error::new_spanned(
                    &field.ty,
                    "#[derive(Entity)] requires the id field to be `i64`",
                ));
            }
            let rename = field_attr_lit(field, "rename")?;
            let col_lit = rename.unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
            (field, col_lit)
        }
    };
    let id_ident = id_field.ident.as_ref().expect("named field");

    let mut column_pushes = Vec::new();
    for field in fields {
        let ident = field.ident.as_ref().expect("named field");
        if std::ptr::eq(field, id_field) || field_attr(field, "skip").is_some() {
            continue;
        }
        let rename = field_attr_lit(field, "rename")?;
        let col_lit = rename.unwrap_or_else(|| LitStr::new(&ident.to_string(), ident.span()));
        let value = value_expr(
            field,
            &quote!(self.#ident),
            field_attr(field, "json").is_some(),
        )?;
        column_pushes.push(quote! {
            out.push((#col_lit, #value));
        });
    }

    Ok(quote! {
        impl ::vivarium_core::Entity for #name {
            const TABLE: &'static str = #table;
            const ID_COLUMN: &'static str = #id_col_lit;

            fn id(&self) -> i64 {
                self.#id_ident
            }

            fn columns_and_values(&self) -> Vec<(&'static str, ::vivarium_core::Value)> {
                let mut out = ::std::vec::Vec::new();
                #(#column_pushes)*
                out
            }
        }
    })
}

/// Generates the `Value` conversion expression for a field.
fn value_expr(
    field: &syn::Field,
    access: &proc_macro2::TokenStream,
    json: bool,
) -> Result<proc_macro2::TokenStream> {
    if json {
        return Ok(quote! {
            ::vivarium_core::Value::Json(
                ::serde_json::to_value(#access)
                    .expect("field marked #[entity(json)] must serialize to JSON")
            )
        });
    }
    let ty = &field.ty;
    if type_is(ty, &["i64"]) {
        Ok(quote!(::vivarium_core::Value::I64(#access)))
    } else if type_is(ty, &["i8"])
        || type_is(ty, &["i16"])
        || type_is(ty, &["i32"])
        || type_is(ty, &["u8"])
        || type_is(ty, &["u16"])
        || type_is(ty, &["u32"])
        || type_is(ty, &["u64"])
        || type_is(ty, &["usize"])
        || type_is(ty, &["isize"])
    {
        Ok(quote!(::vivarium_core::Value::I64(#access as i64)))
    } else if type_is(ty, &["f64"]) {
        Ok(quote!(::vivarium_core::Value::F64(#access)))
    } else if type_is(ty, &["f32"]) {
        Ok(quote!(::vivarium_core::Value::F64(#access as f64)))
    } else if type_is(ty, &["bool"]) {
        Ok(quote!(::vivarium_core::Value::Bool(#access)))
    } else if type_is(ty, &["String"]) {
        Ok(quote!(::vivarium_core::Value::Text(#access.clone())))
    } else if type_is(ty, &["Vec", "u8"]) {
        Ok(quote!(::vivarium_core::Value::Bytes(#access.clone())))
    } else if type_is(ty, &["serde_json", "Value"]) || single_ident(ty, "Value") {
        Ok(quote!(::vivarium_core::Value::Json(
            ::serde_json::to_value(&#access)
                .expect("JSON value fields must serialize to JSON")
        )))
    } else if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        let inner_expr = value_expr_inner(inner, &quote!(v))?;
                        return Ok(quote! {
                            match &#access {
                                ::core::option::Option::Some(v) => #inner_expr,
                                ::core::option::Option::None => ::vivarium_core::Value::Null,
                            }
                        });
                    }
                }
            }
        }
        Err(Error::new_spanned(
            ty,
            "#[derive(Entity)] does not support this field type; use one of i8-i64, u8-u64, \
             usize, isize, f32, f64, bool, String, Vec<u8>, serde_json::Value, or Option of \
             those, or mark the field #[entity(json)]",
        ))
    } else {
        Err(Error::new_spanned(
            ty,
            "#[derive(Entity)] does not support this field type; use a path type, or mark the \
             field #[entity(json)]",
        ))
    }
}

/// Like [`value_expr`] but without the field-level `json` attribute (used for
/// `Option` inner types). The `access` token is a `&T` reference here, so all
/// expressions deref it.
fn value_expr_inner(
    ty: &Type,
    access: &proc_macro2::TokenStream,
) -> Result<proc_macro2::TokenStream> {
    if type_is(ty, &["i64"]) {
        Ok(quote!(::vivarium_core::Value::I64(*#access)))
    } else if type_is(ty, &["i8"])
        || type_is(ty, &["i16"])
        || type_is(ty, &["i32"])
        || type_is(ty, &["u8"])
        || type_is(ty, &["u16"])
        || type_is(ty, &["u32"])
        || type_is(ty, &["u64"])
        || type_is(ty, &["usize"])
        || type_is(ty, &["isize"])
    {
        Ok(quote!(::vivarium_core::Value::I64(*#access as i64)))
    } else if type_is(ty, &["f64"]) {
        Ok(quote!(::vivarium_core::Value::F64(*#access)))
    } else if type_is(ty, &["f32"]) {
        Ok(quote!(::vivarium_core::Value::F64(*#access as f64)))
    } else if type_is(ty, &["bool"]) {
        Ok(quote!(::vivarium_core::Value::Bool(*#access)))
    } else if type_is(ty, &["String"]) {
        Ok(quote!(::vivarium_core::Value::Text((#access).clone())))
    } else if type_is(ty, &["Vec", "u8"]) {
        Ok(quote!(::vivarium_core::Value::Bytes((#access).clone())))
    } else if type_is(ty, &["serde_json", "Value"]) || single_ident(ty, "Value") {
        Ok(quote!(::vivarium_core::Value::Json(
            ::serde_json::to_value(#access).expect("JSON value fields must serialize to JSON")
        )))
    } else {
        Err(Error::new_spanned(
            ty,
            "#[derive(Entity)] does not support this field type inside Option; use one of \
             i8-i64, u8-u64, usize, isize, f32, f64, bool, String, Vec<u8>, \
             serde_json::Value",
        ))
    }
}

/// Struct-level `#[entity(...)]` attributes.
struct EntityAttrs {
    table: Option<LitStr>,
}

impl EntityAttrs {
    fn parse(attrs: &[Attribute]) -> Result<Self> {
        let mut table = None;
        for attr in attrs {
            if !attr.path().is_ident("entity") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("table") {
                    let lit: LitStr = meta.value()?.parse()?;
                    table = Some(lit);
                    Ok(())
                } else {
                    Err(meta.error("unknown #[entity] attribute; expected `table = \"...\"`"))
                }
            })?;
        }
        Ok(Self { table })
    }
}

/// Returns the string-literal value of `#[entity(name = "...")]` on a field.
fn field_attr_lit(field: &syn::Field, name: &str) -> Result<Option<LitStr>> {
    let mut found = None;
    for attr in &field.attrs {
        if !attr.path().is_ident("entity") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(name) {
                let lit: LitStr = meta.value()?.parse()?;
                found = Some(lit);
                Ok(())
            } else {
                Ok(())
            }
        })?;
    }
    Ok(found)
}

/// Returns `Some(())` when the field has a bare `#[entity(name)]` marker
/// (either `#[entity(name)]` or `#[entity(name, ...)]`).
fn field_attr(field: &syn::Field, name: &str) -> Option<()> {
    for attr in &field.attrs {
        if !attr.path().is_ident("entity") {
            continue;
        }
        match &attr.meta {
            Meta::List(list) => {
                let mut found = false;
                let _ = list.parse_nested_meta(|meta| {
                    if meta.path.is_ident(name) {
                        found = true;
                    }
                    Ok(())
                });
                if found {
                    return Some(());
                }
            }
            Meta::Path(path) => {
                if path.is_ident(name) {
                    return Some(());
                }
            }
            Meta::NameValue(_) => {}
        }
    }
    None
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
