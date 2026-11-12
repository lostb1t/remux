//! The id of a stream group picked as a version of one title.
//!
//! A stream group ("1080p · Blu-ray") is a single row shared by every title, so
//! its id alone doesn't say which movie or episode a client means. Clients that
//! pick a version keep the version's id as the item id (Android TV refetches
//! the item by it and plays it, see `item_for_user`), so that id must name the
//! title too. A version id is the title's id XOR a mask derived from the group:
//! unique per title and group, a plain UUID to clients, and decodable without
//! storing anything by trying each group's mask until one yields a title. It
//! survives restarts and can't be repointed by browsing another title.
//!
//! Inside the server a version id is always turned back into its title plus
//! the group as the requested source (`StreamService::version_target`,
//! `MediaResolveService::playable_item_id`); everything past that works with
//! the plain group id.

use sqlx::SqlitePool;
use uuid::Uuid;

/// Bits of a UUID that hold its version and variant. The mask leaves them
/// alone, so a version id keeps the title id's and parses as the same kind of
/// UUID.
const VERSION_VARIANT_BITS: u128 = 0x0000_0000_0000_f000_c000_0000_0000_0000;

const NAMESPACE: Uuid = Uuid::from_u128(0x8f1f_3c6e_52a4_4d0b_9e57_6a2d_0c41_b7e3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VersionId {
    /// The movie or episode the version belongs to.
    pub owner: Uuid,
    /// The stream group picked as the version.
    pub group: Uuid,
}

impl VersionId {
    pub(crate) fn new(owner: Uuid, group: Uuid) -> Self {
        Self { owner, group }
    }

    fn mask(group: Uuid) -> u128 {
        Uuid::new_v5(&NAMESPACE, group.as_bytes()).as_u128() & !VERSION_VARIANT_BITS
    }

    /// The id clients see for this version.
    pub(crate) fn id(&self) -> Uuid {
        Uuid::from_u128(
            self.owner
                .as_u128()
                ^ Self::mask(self.group),
        )
    }

    /// The version `id` names, if it is one. Only call this for an id that is
    /// not a media row itself: it costs a query per call.
    pub(crate) async fn decode(db: &SqlitePool, id: Uuid) -> Option<Self> {
        let groups: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM media WHERE kind = 'stream_group'")
                .fetch_all(db)
                .await
                .ok()?;
        let candidates: Vec<Self> = groups
            .into_iter()
            .map(|group| {
                Self::new(Uuid::from_u128(id.as_u128() ^ Self::mask(group)), group)
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT id FROM media WHERE kind IN ('movie', 'episode') AND id IN (",
        );
        let mut sep = qb.separated(", ");
        for candidate in &candidates {
            sep.push_bind(candidate.owner);
        }
        qb.push(")");
        let owner: Uuid = qb
            .build_query_scalar()
            .fetch_optional(db)
            .await
            .ok()??;
        candidates
            .into_iter()
            .find(|c| c.owner == owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_id_is_unique_per_title_and_group() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (bluray, web) = (Uuid::new_v4(), Uuid::new_v4());
        let ids = [
            VersionId::new(a, bluray).id(),
            VersionId::new(a, web).id(),
            VersionId::new(b, bluray).id(),
            VersionId::new(b, web).id(),
        ];
        for (i, x) in ids
            .iter()
            .enumerate()
        {
            for y in &ids[i + 1..] {
                assert_ne!(x, y);
            }
            assert_ne!(*x, a);
            assert_ne!(*x, b);
        }
    }

    #[test]
    fn a_version_id_keeps_the_title_ids_uuid_version_and_variant() {
        for owner in [
            Uuid::new_v4(),
            Uuid::new_v5(&Uuid::NAMESPACE_OID, b"tt0903747"),
        ] {
            let id = VersionId::new(owner, Uuid::new_v4()).id();
            assert_eq!(id.get_version_num(), owner.get_version_num());
            assert_eq!(id.get_variant(), owner.get_variant());
        }
    }
}
