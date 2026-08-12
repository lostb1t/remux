#![allow(warnings)]

mod store;
pub use store::Store;

mod retry;

mod types;
pub use types::NonEmptyString;

use uuid::Uuid;

const NS: Uuid = uuid::uuid!("6ba7b810-9dad-11d1-80b4-00c04fd430c8");

pub mod uuid_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use uuid::Uuid;

    pub fn serialize<S: Serializer>(v: &Uuid, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(
            &v.simple()
                .to_string(),
        )
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Uuid, D::Error> {
        let s = String::deserialize(d)?;
        Uuid::parse_str(&s).map_err(serde::de::Error::custom)
    }

    pub mod opt {
        use serde::{Deserialize, Deserializer, Serializer};
        use uuid::Uuid;

        pub fn serialize<S: Serializer>(
            v: &Option<Uuid>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match v {
                Some(u) => s.serialize_some(
                    &u.simple()
                        .to_string(),
                ),
                None => s.serialize_none(),
            }
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Uuid>, D::Error> {
            let s: Option<String> = Option::deserialize(d)?;
            match s {
                Some(s) => Uuid::parse_str(&s)
                    .map(Some)
                    .map_err(serde::de::Error::custom),
                None => Ok(None),
            }
        }
    }

    pub mod vec {
        use serde::{Deserialize, Deserializer, Serializer};
        use uuid::Uuid;

        pub fn serialize<S: Serializer>(v: &[Uuid], s: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeSeq;
            let mut seq = s.serialize_seq(Some(v.len()))?;
            for u in v {
                seq.serialize_element(
                    &u.simple()
                        .to_string(),
                )?;
            }
            seq.end()
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Vec<Uuid>, D::Error> {
            let v: Vec<String> = Vec::deserialize(d)?;
            v.into_iter()
                .map(|s| Uuid::parse_str(&s).map_err(serde::de::Error::custom))
                .collect()
        }
    }

    pub mod opt_vec {
        use serde::{Deserialize, Deserializer, Serializer};
        use uuid::Uuid;

        pub fn serialize<S: Serializer>(
            v: &Option<Vec<Uuid>>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match v {
                Some(uuids) => super::vec::serialize(uuids, s),
                None => s.serialize_none(),
            }
        }
        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Vec<Uuid>>, D::Error> {
            let v: Option<Vec<String>> = Option::deserialize(d)?;
            match v {
                Some(v) => v
                    .into_iter()
                    .map(|s| Uuid::parse_str(&s).map_err(serde::de::Error::custom))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Some),
                None => Ok(None),
            }
        }
    }
}

pub fn get_stable_uuid(v: String) -> Uuid {
    Uuid::new_v5(&NS, v.as_bytes())
}

pub fn merge_option<T: Clone>(dst: &mut Option<T>, src: &Option<T>, replace: bool) {
    if src.is_some() && (replace || dst.is_none()) {
        *dst = src.clone();
    }
}

pub fn merge_vec<T>(dst: &mut Vec<T>, src: Vec<T>, replace: bool) {
    if replace || dst.is_empty() {
        *dst = src;
    }
}

/// Normalizes an ffprobe `format_name` string (which may be comma-separated) to a
/// canonical container extension.
pub fn normalize_container(raw: &str) -> String {
    let base = raw
        .split(',')
        .next()
        .unwrap_or(raw);
    match base {
        "matroska" => "mkv".to_string(),
        "mov" => "mp4".to_string(),
        "mpegts" => "ts".to_string(),
        other => other.to_string(),
    }
}
