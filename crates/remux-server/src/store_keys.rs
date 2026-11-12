//! Keys of the shared in-memory store (`AppContext::store`), one function per
//! kind of entry, so a key is spelled the same where it's written and where
//! it's read, and two kinds of entry can't end up sharing a key.

use uuid::Uuid;

use crate::ids::{StreamGroupId, UserId};

/// A Quick Connect request, by its secret.
pub fn quick_connect(secret: &str) -> String {
    format!("qc:{secret}")
}

/// The secret a Quick Connect code belongs to.
pub fn quick_connect_code(code: &str) -> String {
    format!("qc:code:{code}")
}

/// The stream PlaybackInfo picked for an item on a device.
pub fn device_stream(item_id: Uuid, device_id: &str) -> String {
    format!("pstream:{item_id}:{device_id}")
}

/// The audio and subtitle tracks a play session was advertised.
pub fn play_session_tracks(play_session_id: &str) -> String {
    format!("ptracks:{play_session_id}")
}

/// The item a user last saw a stream group under.
pub fn group_item(user_id: UserId, group_id: StreamGroupId) -> String {
    format!("gitem:{user_id}:{group_id}")
}

/// A subtitle found through remote search, by its id.
pub fn remote_subtitle(id: &str) -> String {
    format!("subtitle:{id}")
}

/// The stream a device was given for one of an item's sources, when the
/// client sends no PlaySessionId.
pub fn device_selected_stream(
    device_id: &str,
    item_id: Uuid,
    source_id: Uuid,
) -> String {
    format!("pstream:device:{device_id}:{item_id}:{source_id}")
}

/// The stream a play session was given for one of its sources.
pub fn session_selected_stream(play_session_id: &str, source_id: Uuid) -> String {
    format!("pstream:psid:{play_session_id}:{source_id}")
}

/// The release a request without PlaybackInfo was given for an item, keyed by
/// the release first in line.
pub fn walked_stream(item_id: Uuid, head_id: Uuid) -> String {
    format!("pstream:walk:{item_id}:{head_id}")
}
