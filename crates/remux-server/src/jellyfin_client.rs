use crate::api::{CollectionType, GetItemsQuery, ItemSortBy, SortOrder};

pub trait JellyfinClient: Send {
    fn hide_sources(&self) -> bool {
        false
    }

    fn mixed_collection_type(&self) -> Option<CollectionType> {
        None
    }

    /// Returns true when the query carries the client's built-in default sort,
    /// meaning the user has not expressed a preference and the collection's own
    /// sort order should be applied instead.
    fn is_default_sort(&self, q: &GetItemsQuery) -> bool {
        q.sort_by
            .as_deref()
            .map(|s| {
                s.is_empty()
                    || matches!(
                        s.first(),
                        Some(ItemSortBy::SortName | ItemSortBy::Name)
                    )
            })
            .unwrap_or(true)
    }
}

pub struct Plezy;
pub struct Swiftfin;
pub struct SenPlayer;
pub struct GenericClient;

impl JellyfinClient for Plezy {
    fn hide_sources(&self) -> bool {
        true
    }
}

impl JellyfinClient for Swiftfin {
    fn mixed_collection_type(&self) -> Option<CollectionType> {
        // Swiftfin's SDK has no "mixed" case; homevideos is accepted and shows a home row.
        Some(CollectionType::Homevideos)
    }
}

impl JellyfinClient for SenPlayer {
    fn is_default_sort(&self, q: &GetItemsQuery) -> bool {
        // SenPlayer's built-in default: DateLastContentAdded,DateCreated,SortName / Descending.
        let is_senplayer_default = q
            .sort_by
            .as_deref()
            == Some(&[
                ItemSortBy::DateLastContentAdded,
                ItemSortBy::DateCreated,
                ItemSortBy::SortName,
            ])
            && q.sort_order
                .as_deref()
                == Some(&[SortOrder::Descending]);

        is_senplayer_default
            || q.sort_by
                .as_deref()
                .map(|s| {
                    s.is_empty()
                        || matches!(
                            s.first(),
                            Some(ItemSortBy::SortName | ItemSortBy::Name)
                        )
                })
                .unwrap_or(true)
    }
}

impl JellyfinClient for GenericClient {}

pub fn from_app_name(name: &str) -> Box<dyn JellyfinClient> {
    match name {
        "Plezy" => Box::new(Plezy),
        s if s.contains("Swiftfin") => Box::new(Swiftfin),
        "SenPlayer" => Box::new(SenPlayer),
        _ => Box::new(GenericClient),
    }
}
