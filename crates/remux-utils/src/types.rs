use nutype::nutype;

#[nutype(
    validate(not_empty),
    derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        Display,
        Serialize,
        Deserialize,
        AsRef,
        Deref,
        Into
    )
)]
pub struct NonEmptyString(String);

// A column read back as text must still be non-empty, so an empty string
// stored before this type existed fails loudly instead of slipping through.
#[cfg(feature = "sqlx")]
mod sqlx_impls {
    use super::NonEmptyString;

    impl<DB> sqlx::Type<DB> for NonEmptyString
    where
        DB: sqlx::Database,
        String: sqlx::Type<DB>,
    {
        fn type_info() -> DB::TypeInfo {
            String::type_info()
        }
        fn compatible(ty: &DB::TypeInfo) -> bool {
            String::compatible(ty)
        }
    }

    impl<'r, DB> sqlx::Decode<'r, DB> for NonEmptyString
    where
        DB: sqlx::Database,
        String: sqlx::Decode<'r, DB>,
    {
        fn decode(
            value: <DB as sqlx::Database>::ValueRef<'r>,
        ) -> Result<Self, sqlx::error::BoxDynError> {
            Ok(NonEmptyString::try_new(String::decode(value)?)?)
        }
    }

    impl<'q, DB> sqlx::Encode<'q, DB> for NonEmptyString
    where
        DB: sqlx::Database,
        String: sqlx::Encode<'q, DB>,
    {
        fn encode_by_ref(
            &self,
            buf: &mut <DB as sqlx::Database>::ArgumentBuffer<'q>,
        ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
            <String as sqlx::Encode<'q, DB>>::encode_by_ref(&**self, buf)
        }
    }
}
