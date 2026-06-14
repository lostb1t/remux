use nutype::nutype;
use serde::{Deserialize, Serialize};

#[nutype(
    validate(not_empty),
    derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
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
