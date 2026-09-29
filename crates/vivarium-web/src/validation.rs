//! Structured validation failures, free of any validation-backend type.
//!
//! The extractors in [`crate::varser`] translate whatever their backend
//! produced (`validator` or `garde`) into these two types, so the wire format
//! of `errors` does not depend on the backend that raised it.
//!
//! [`ValidationErrors`] keeps its map private, and every entry holds at least
//! one violation because [`ValidationErrors::insert`] only ever pushes — the
//! bad shapes a public map allowed (`{"email": []}`, a blank field key) cannot
//! be built. An empty report (`{}`) is legal and is what a backend that produced
//! no violations yields. Deserialization enforces the same entry shape and
//! rejects a blank field key or an empty violation list.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::message::Message;

/// One failed rule for one field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct FieldViolation {
    /// The rule that failed (`"length"`, `"email"`, …).
    pub code: String,
    /// The message the DTO declared for this rule, when it declared one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(value_type = Option<String>))]
    pub message: Option<Message>,
    /// The parameters of the failed rule (e.g. `{"min": 3, "max": 20}`).
    ///
    /// The submitted field value is deliberately **not** echoed: `validator`
    /// records it as `params.value`, which would reflect raw input (a
    /// password, a token) back to the client.
    #[cfg_attr(feature = "utoipa", schema(additional_properties))]
    pub params: BTreeMap<String, serde_json::Value>,
}

/// Every violation, grouped by field.
///
/// Keys are flattened field paths: `email` for a top-level field,
/// `profile.email` for a nested struct field, `items[0].name` for a field of a
/// list element. The map is ordered, so the serialized body is stable.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ValidationErrors(BTreeMap<String, Vec<FieldViolation>>);

impl<'de> Deserialize<'de> for ValidationErrors {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let raw = BTreeMap::<String, Vec<FieldViolation>>::deserialize(deserializer)?;
        if let Some((field, _)) = raw
            .iter()
            .find(|(field, violations)| field.trim().is_empty() || violations.is_empty())
        {
            return Err(D::Error::custom(format!(
                "invalid validation errors for field `{field}`"
            )));
        }
        Ok(Self(raw))
    }
}

/// The field key a report-level (non-field) failure is stored under.
///
/// `garde` reports struct-level rules with an empty path; `validator` always
/// names a field. Storing those under `"_"` keeps every key a usable field
/// path.
const ROOT_FIELD: &str = "_";

impl ValidationErrors {
    /// An empty report.
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Whether no rule failed.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The number of fields that failed.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Records `violation` for `field`, keeping any violations already there.
    ///
    /// A blank `field` is stored under `"_"` (a report-level failure); an empty
    /// field entry can never be created, only appended to.
    pub fn insert(&mut self, field: impl Into<String>, violation: FieldViolation) {
        let field = field.into();
        let field = if field.trim().is_empty() {
            ROOT_FIELD.to_string()
        } else {
            field
        };
        self.0.entry(field).or_default().push(violation);
    }

    /// The violations recorded for `field`.
    pub fn get(&self, field: &str) -> Option<&[FieldViolation]> {
        self.0.get(field).map(Vec::as_slice)
    }

    /// Iterates over `(field, violations)` in field order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[FieldViolation])> {
        self.0
            .iter()
            .map(|(field, errors)| (field.as_str(), errors.as_slice()))
    }
}

/// The `code` given to a [`garde`] violation.
///
/// `garde` 0.22 exposes neither the rule name nor the rule parameters on its
/// public error type (only the rendered message), so the backend-neutral code
/// is a constant rather than the rule name the `validator` backend reports.
#[cfg(feature = "validation-garde")]
const GARDE_VIOLATION_CODE: &str = "invalid";

/// Flattening rules for the `validator` backend.
#[cfg(feature = "validation-validator")]
mod validator_backend {
    use std::borrow::Cow;
    use std::collections::{BTreeMap, HashMap};

    use validator::{ValidationError, ValidationErrorsKind};

    use super::{FieldViolation, ValidationErrors};
    use crate::message::Message;

    impl From<validator::ValidationErrors> for ValidationErrors {
        fn from(errors: validator::ValidationErrors) -> Self {
            let mut flat = ValidationErrors::new();
            flatten(errors.errors(), "", &mut flat);
            flat
        }
    }

    /// Walks the `validator` error tree, writing flat `path -> violations`
    /// entries into `out`.
    fn flatten(
        errors: &HashMap<Cow<'static, str>, ValidationErrorsKind>,
        prefix: &str,
        out: &mut ValidationErrors,
    ) {
        for (field, kind) in errors {
            match kind {
                ValidationErrorsKind::Field(field_errors) => {
                    let path = join(prefix, field);
                    for error in field_errors {
                        out.insert(path.clone(), violation(error));
                    }
                }
                ValidationErrorsKind::Struct(nested) => {
                    flatten(nested.errors(), &join(prefix, field), out);
                }
                ValidationErrorsKind::List(items) => {
                    for (index, nested) in items {
                        let path = format!("{}[{index}]", join(prefix, field));
                        flatten(nested.errors(), &path, out);
                    }
                }
            }
        }
    }

    /// `prefix` + `field`, separated by `.` unless `prefix` is empty.
    fn join(prefix: &str, field: &str) -> String {
        if prefix.is_empty() {
            field.to_string()
        } else {
            format!("{prefix}.{field}")
        }
    }

    /// The `params` key `validator` uses for the submitted field value.
    const SUBMITTED_VALUE: &str = "value";

    /// Copies one `validator` violation, minus the echoed field value.
    ///
    /// `validator` adds the raw field content as `params.value` for most
    /// rules; reflecting it back would leak whatever the client sent (a
    /// password, a token) in a 422 body, so it is dropped here. Everything
    /// else (`code`, `message`, the rule's own parameters) is copied verbatim,
    /// except that a blank declared message becomes no message at all.
    fn violation(error: &ValidationError) -> FieldViolation {
        FieldViolation {
            code: error.code.to_string(),
            message: error
                .message
                .as_ref()
                .and_then(|text| Message::try_new(text.to_string()).ok()),
            params: error
                .params
                .iter()
                .filter(|(name, _)| name.as_ref() != SUBMITTED_VALUE)
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect::<BTreeMap<_, _>>(),
        }
    }
}

/// Flattening rules for the `garde` backend.
#[cfg(feature = "validation-garde")]
impl From<garde::Report> for ValidationErrors {
    fn from(report: garde::Report) -> Self {
        let mut flat = ValidationErrors::new();
        for (path, error) in report.into_inner() {
            flat.insert(
                path.to_string(),
                FieldViolation {
                    code: GARDE_VIOLATION_CODE.to_string(),
                    message: Message::try_new(error.message().to_string()).ok(),
                    params: BTreeMap::new(),
                },
            );
        }
        flat
    }
}

#[cfg(all(test, feature = "validation-validator"))]
mod validator_tests {
    use super::*;
    use validator::Validate;

    #[derive(Debug, validator::Validate)]
    struct Inner {
        #[validate(length(min = 3, message = "too short"))]
        name: String,
    }

    #[derive(Debug, validator::Validate)]
    struct Outer {
        #[validate(email)]
        email: String,
        #[validate(nested)]
        profile: Inner,
        #[validate(nested)]
        items: Vec<Inner>,
    }

    /// The three flattening shapes: top-level, nested struct, list element.
    #[test]
    fn flattens_top_level_nested_and_list_fields() {
        let value = Outer {
            email: "not-an-email".to_string(),
            profile: Inner {
                name: "x".to_string(),
            },
            items: vec![
                Inner {
                    name: "y".to_string(),
                },
                Inner {
                    name: "ok!".to_string(),
                },
            ],
        };

        let report = ValidationErrors::from(value.validate().expect_err("invalid payload"));
        let mut fields = report.iter().map(|(field, _)| field).collect::<Vec<_>>();
        fields.sort();

        assert_eq!(
            fields,
            vec!["email", "items[0].name", "profile.name"],
            "flattened field paths"
        );
        let nested = report.get("profile.name").expect("nested violation");
        assert_eq!(nested[0].code, "length");
        assert_eq!(nested[0].message.as_deref(), Some("too short"));
        assert!(report.get("items[1].name").is_none());
    }

    #[derive(Debug, validator::Validate)]
    struct Params {
        #[validate(length(min = 3, max = 20))]
        username: String,
    }

    /// Rule parameters survive the translation.
    #[test]
    fn keeps_rule_params() {
        let value = Params {
            username: "ab".to_string(),
        };
        let report = ValidationErrors::from(value.validate().expect_err("too short"));

        let violation = &report.get("username").expect("violation")[0];
        assert_eq!(violation.code, "length");
        assert_eq!(violation.params["min"], serde_json::json!(3));
        assert_eq!(violation.params["max"], serde_json::json!(20));
        assert_eq!(violation.message, None, "no message was declared");
        assert!(
            !violation.params.contains_key("value"),
            "the submitted value must not be echoed back: {:?}",
            violation.params
        );
    }

    /// An empty report flattens to an empty payload.
    #[cfg(feature = "validation-validator")]
    #[test]
    fn empty_validator_payload_flattens_to_empty() {
        // Exercises the conversion itself: `ValidationErrors::new()` alone
        // would pass even if `From<validator::ValidationErrors>` broke.
        assert!(ValidationErrors::from(validator::ValidationErrors::default()).is_empty());
    }
}

#[cfg(all(test, feature = "validation-garde"))]
mod garde_tests {
    use super::*;
    use garde::Validate as _;

    #[derive(Debug, garde::Validate)]
    struct Nested {
        #[garde(length(min = 3))]
        name: String,
    }

    #[derive(Debug, garde::Validate)]
    struct Form {
        #[garde(length(min = 3))]
        username: String,
        #[garde(dive)]
        items: Vec<Nested>,
    }

    /// `garde` paths flatten the same way, with the message kept and the code
    /// rendered as the backend-neutral placeholder.
    #[test]
    fn flattens_paths_and_keeps_messages() {
        let value = Form {
            username: "ab".to_string(),
            items: vec![Nested {
                name: "x".to_string(),
            }],
        };
        let report = ValidationErrors::from(value.validate().expect_err("invalid payload"));

        let top = report.get("username").expect("top-level violation");
        assert_eq!(top[0].code, "invalid");
        assert!(top[0].message.as_deref().is_some_and(|m| m.contains("3")));

        let nested = report.get("items[0].name").expect("nested violation");
        assert!(!nested.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn violation() -> FieldViolation {
        FieldViolation {
            code: "invalid".to_string(),
            message: None,
            params: BTreeMap::new(),
        }
    }

    /// A report-level failure (an empty `garde` path) is stored under `_`, so
    /// every key stays a usable field path.
    #[test]
    fn blank_field_keys_are_stored_under_the_root_marker() {
        let mut errors = ValidationErrors::new();
        errors.insert("", violation());

        assert!(errors.get(ROOT_FIELD).is_some());
        assert!(errors.get("").is_none());
        assert_eq!(errors.iter().next().map(|(field, _)| field), Some("_"));
    }

    /// The wire shapes a public map used to allow are rejected on the way in.
    #[test]
    fn malformed_payloads_are_rejected_on_deserialize() {
        assert!(serde_json::from_str::<ValidationErrors>(r#"{"email": []}"#).is_err());
        assert!(
            serde_json::from_str::<ValidationErrors>(r#"{"": [{ "code": "x", "params": {} }]}"#)
                .is_err()
        );

        // An empty report is a legal shape, and a well-formed entry round-trips.
        assert!(serde_json::from_str::<ValidationErrors>("{}").is_ok());
        let mut errors = ValidationErrors::new();
        errors.insert(
            "email",
            FieldViolation {
                code: "email".to_string(),
                message: Message::try_new("not an email").ok(),
                params: BTreeMap::new(),
            },
        );
        let rendered = serde_json::to_string(&errors).expect("serializes");
        assert_eq!(
            rendered,
            r#"{"email":[{"code":"email","message":"not an email","params":{}}]}"#
        );
        assert_eq!(
            serde_json::from_str::<ValidationErrors>(&rendered).expect("round-trips"),
            errors
        );
    }
}
