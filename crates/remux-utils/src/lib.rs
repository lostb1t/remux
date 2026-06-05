#![allow(warnings)]

mod store;
pub use store::Store;

mod retry;

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
