use remux_sdks::RestClient;
use remux_sdks::deezer as dz;
use tracing::warn;

use crate::AppContext;
use crate::db;

pub(crate) async fn resolve_music_deezer(
    media: &mut db::Media,
    _ctx: &AppContext,
) -> bool {
    match media.kind {
        db::MediaKind::Track => {
            if media.external_ids.deezer_track.is_some() {
                return true;
            }
            let Ok(client) = RestClient::new("https://api.deezer.com/") else {
                return false;
            };
            let hit = match client
                .execute(dz::SearchTracksEndpoint {
                    q: media.title.clone(),
                    limit: 1,
                })
                .await
            {
                Ok(dz::DeezerResult::Ok(list)) => list.data.into_iter().next(),
                Ok(dz::DeezerResult::Err { error }) => {
                    warn!(title = %media.title, %error, "Deezer track search returned error");
                    return false;
                }
                Err(e) => {
                    warn!(title = %media.title, error = %e, "Deezer track search HTTP error");
                    return false;
                }
            };
            let Some(track) = hit else {
                return false;
            };
            media.external_ids.deezer_track = Some(track.id as i64);
            media.external_ids.deezer_album = Some(track.album.id as i64);
            media.external_ids.deezer_artist = Some(track.artist.id as i64);
            true
        }
        db::MediaKind::Album => {
            if media.external_ids.deezer_album.is_some() {
                return true;
            }
            let Ok(client) = RestClient::new("https://api.deezer.com/") else {
                return false;
            };
            let hit = match client
                .execute(dz::SearchAlbumsEndpoint {
                    q: media.title.clone(),
                    limit: 1,
                })
                .await
            {
                Ok(dz::DeezerResult::Ok(list)) => list.data.into_iter().next(),
                Ok(dz::DeezerResult::Err { error }) => {
                    warn!(title = %media.title, %error, "Deezer album search returned error");
                    return false;
                }
                Err(e) => {
                    warn!(title = %media.title, error = %e, "Deezer album search HTTP error");
                    return false;
                }
            };
            let Some(album) = hit else {
                return false;
            };
            media.external_ids.deezer_album = Some(album.id as i64);
            media.external_ids.deezer_artist = Some(album.artist.id as i64);
            true
        }
        _ => false,
    }
}
