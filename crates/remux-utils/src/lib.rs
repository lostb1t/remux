#![allow(warnings)]

mod store;
pub use store::Store;

use uuid::Uuid;

const NS: Uuid = uuid::uuid!("6ba7b810-9dad-11d1-80b4-00c04fd430c8");

pub fn get_stable_uuid(v: String) -> Uuid {
    Uuid::new_v5(&NS, v.as_bytes())
}
