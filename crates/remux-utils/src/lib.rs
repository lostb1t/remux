#![allow(warnings)]

mod store;
pub use store::Store;

pub mod retry;

mod types;
pub use types::NonEmptyString;

use uuid::Uuid;

const NS: Uuid = uuid::uuid!("6ba7b810-9dad-11d1-80b4-00c04fd430c8");

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
