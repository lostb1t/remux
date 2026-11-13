//! Distinct types for ids that travel side by side.
//!
//! Every row has a `Uuid`, and the version-picking code passes several around
//! at once: whose watch state (`UserId`), which title (`ItemId`), which
//! shared stream group (`StreamGroupId`). As bare `Uuid`s a swapped argument
//! compiles and plays the wrong title; as these types it doesn't compile.

use std::fmt;
use uuid::Uuid;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn uuid(self) -> Uuid {
                self.0
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Uuid {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_type!(
    /// A user.
    UserId
);
id_type!(
    /// A playable title: a movie, an episode, a track.
    ItemId
);
id_type!(
    /// A stream group, the same row for every title it groups.
    StreamGroupId
);
