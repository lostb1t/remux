use crate::sdks::aio::Meta;
use serde_json;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_season_posters_deserialization() {
        // Example JSON response (simplified version)
        let json_data = r#"{
            "id": "tt4574334",
            "type": "series",
            "name": "Stranger Things",
            "poster": "https://artworks.thetvdb.com/banners/posters/305288-4.jpg",
            "videos": [
                {
                    "id": "tt4574334:1:1",
                    "title": "Chapter One: The Vanishing of Will Byers",
                    "released": "2016-07-15T12:00:00.000Z",
                    "thumbnail": "https://artworks.thetvdb.com/banners/episodes/305288/62c1bedbc2744.jpg",
                    "available": true,
                    "episode": 1,
                    "season": 1,
                    "overview": "On his way home from a friend's house, young Will sees something terrifying.",
                    "runtime": "49min"
                }
            ],
            "app_extras": {
                "cast": [
                    {
                        "name": "Millie Bobby Brown",
                        "character": "Eleven / Jane",
                        "photo": "https://artworks.thetvdb.com/banners/v4/actor/330194/photo/627969936f877.jpg"
                    }
                ],
                "directors": [],
                "writers": [],
                "seasonPosters": [
                    "https://artworks.thetvdb.com/banners/v4/season/650459/posters/62974916c5a81.jpg",
                    "https://artworks.thetvdb.com/banners/seasons/305288-1.jpg",
                    "https://artworks.thetvdb.com/banners/v4/season/679296/posters/63dcca3e61c0f.jpg"
                ],
                "certification": "TV-14"
            }
        }"#;

        // Deserialize into Meta struct
        let meta: Meta =
            serde_json::from_str(json_data).expect("Failed to deserialize Meta");

        // Verify basic fields
        assert_eq!(meta.name.as_deref(), Some("Stranger Things"));
        assert_eq!(meta.id, "tt4574334");

        // Verify app_extras is populated
        assert!(meta.app_extras.is_some());
        let app_extras = meta.app_extras.as_ref().unwrap();

        // Verify season posters are extracted
        assert!(app_extras.season_posters.is_some());
        let season_posters = app_extras.season_posters.as_ref().unwrap();
        assert_eq!(season_posters.len(), 3);

        // Verify the poster URLs
        assert_eq!(
            season_posters[0].as_deref(),
            Some(
                "https://artworks.thetvdb.com/banners/v4/season/650459/posters/62974916c5a81.jpg"
            )
        );
        assert_eq!(
            season_posters[1].as_deref(),
            Some("https://artworks.thetvdb.com/banners/seasons/305288-1.jpg")
        );
        assert_eq!(
            season_posters[2].as_deref(),
            Some(
                "https://artworks.thetvdb.com/banners/v4/season/679296/posters/63dcca3e61c0f.jpg"
            )
        );

        // Test get_season_poster method
        assert_eq!(
            meta.get_season_poster(0),
            Some("https://artworks.thetvdb.com/banners/v4/season/650459/posters/62974916c5a81.jpg".to_string())
        );
        assert_eq!(
            meta.get_season_poster(1),
            Some(
                "https://artworks.thetvdb.com/banners/seasons/305288-1.jpg".to_string()
            )
        );
        assert_eq!(
            meta.get_season_poster(2),
            Some("https://artworks.thetvdb.com/banners/v4/season/679296/posters/63dcca3e61c0f.jpg".to_string())
        );

        // Test out-of-bounds season
        assert_eq!(meta.get_season_poster(3), None);
        assert_eq!(meta.get_season_poster(-1), None);
    }
}

#[test]
fn test_season_posters_with_null_elements() {
    // Test JSON with null elements in seasonPosters array
    let json_data = r#"{
        "id": "tt1234567",
        "type": "series",
        "name": "Test Series",
        "app_extras": {
            "cast": [],
            "directors": [],
            "writers": [],
            "seasonPosters": [null, "https://example.com/poster1.jpg", null, "https://example.com/poster3.jpg"],
            "certification": "TV-PG"
        }
    }"#;

    // Deserialize into Meta struct
    let meta: Meta =
        serde_json::from_str(json_data).expect("Failed to deserialize Meta");

    // Verify app_extras is populated
    assert!(meta.app_extras.is_some());
    let app_extras = meta.app_extras.as_ref().unwrap();

    // Verify season posters are extracted
    assert!(app_extras.season_posters.is_some());
    let season_posters = app_extras.season_posters.as_ref().unwrap();
    assert_eq!(season_posters.len(), 4);

    // Verify null and non-null elements
    assert_eq!(season_posters[0], None);
    assert_eq!(
        season_posters[1].as_deref(),
        Some("https://example.com/poster1.jpg")
    );
    assert_eq!(season_posters[2], None);
    assert_eq!(
        season_posters[3].as_deref(),
        Some("https://example.com/poster3.jpg")
    );

    // Test get_season_poster method with null elements
    assert_eq!(meta.get_season_poster(0), None); // null element
    assert_eq!(
        meta.get_season_poster(1),
        Some("https://example.com/poster1.jpg".to_string())
    );
    assert_eq!(meta.get_season_poster(2), None); // null element
    assert_eq!(
        meta.get_season_poster(3),
        Some("https://example.com/poster3.jpg".to_string())
    );
}
