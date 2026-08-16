//! Values that must never reach logs. Redaction lives on the value, so
//! anything holding one can still `#[derive(Debug)]`.
//!
//! This is about logs only. The sqlx impls pass the value straight through, so
//! it is stored unencrypted exactly as before. Encryption at rest is a separate
//! problem: it needs a key, a migration, and an answer for what happens when
//! the key is lost.

use serde::{Deserialize, Serialize};
use std::{fmt, ops::Deref};

/// Prints as `<redacted>` however it is formatted.
#[derive(Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Named so every read of a secret is greppable.
    pub fn expose(&self) -> &T {
        &self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl Secret<serde_json::Value> {
    /// A trimmed, non-empty string field from a secret JSON blob.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Also redacted: `{}` reaches logs as readily as `{:?}`.
impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<T> Deref for Secret<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> From<T> for Secret<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

// sqlx passthrough: a wrapped column behaves like the bare type.
#[cfg(feature = "sqlx")]
mod sqlx_impls {
    use super::Secret;

    impl<T, DB> sqlx::Type<DB> for Secret<T>
    where
        DB: sqlx::Database,
        T: sqlx::Type<DB>,
    {
        fn type_info() -> DB::TypeInfo {
            T::type_info()
        }
        fn compatible(ty: &DB::TypeInfo) -> bool {
            T::compatible(ty)
        }
    }

    impl<'r, DB, T> sqlx::Decode<'r, DB> for Secret<T>
    where
        DB: sqlx::Database,
        T: sqlx::Decode<'r, DB>,
    {
        fn decode(
            value: <DB as sqlx::Database>::ValueRef<'r>,
        ) -> Result<Self, sqlx::error::BoxDynError> {
            T::decode(value).map(Secret)
        }
    }

    impl<'q, DB, T> sqlx::Encode<'q, DB> for Secret<T>
    where
        DB: sqlx::Database,
        T: sqlx::Encode<'q, DB>,
    {
        fn encode_by_ref(
            &self,
            buf: &mut <DB as sqlx::Database>::ArgumentBuffer<'q>,
        ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
            self.0
                .encode_by_ref(buf)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Redaction must not depend on the `sqlx` feature, so this holds in every
    /// build configuration.
    #[test]
    fn both_debug_and_display_are_redacted() {
        let s = Secret::new("hunter2".to_string());
        assert_eq!(format!("{s:?}"), "<redacted>");
        assert_eq!(format!("{s}"), "<redacted>");
        assert_eq!(s.expose(), "hunter2");
    }

    /// The point of the wrapper: holders can still derive Debug.
    #[test]
    fn a_deriving_struct_inherits_the_redaction() {
        #[derive(Debug)]
        struct Holder {
            name: String,
            token: Secret<String>,
        }
        let shown = format!(
            "{:?}",
            Holder {
                name: "alice".into(),
                token: Secret::new("super-secret".into()),
            }
        );
        assert!(!shown.contains("super-secret"), "leaked: {shown}");
        assert!(shown.contains("alice"), "non-secrets should still print");
    }

    #[test]
    fn get_str_reads_a_field_without_exposing_the_blob() {
        let creds = Secret::new(serde_json::json!({
            "token": "  abc  ",
            "blank": "",
        }));
        assert_eq!(creds.get_str("token"), Some("abc"));
        assert_eq!(creds.get_str("blank"), None);
        assert_eq!(creds.get_str("missing"), None);
        assert!(!format!("{creds:?}").contains("abc"));
    }

    #[test]
    fn serde_is_transparent_so_storage_is_unchanged() {
        let s: Secret<String> = Secret::new("v".into());
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"v\"");
        let back: Secret<String> = serde_json::from_str("\"v\"").unwrap();
        assert_eq!(back.expose(), "v");
    }
}
