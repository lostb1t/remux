pub mod system;
pub mod users;
pub mod items;
pub mod shows;
pub mod images;
pub mod playback;
pub mod devices;
pub mod localization;
pub mod tasks;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use http::StatusCode;
use serde_json::json;

use crate::AppState;
use crate::jellyfin;
use axum_anyhow::ApiResult as Result;

pub async fn stub(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn stub_json(State(state): State<AppState>) -> Result<impl IntoResponse> {
    //let user: UserDtoDummy = Faker.fake();
    //Ok(Json().into_response())
    Ok(Json(json!({
      "ThemeVideosResult": {
        "OwnerId": "f27caa37e5142225cceded48f6553502",
        "Items": [],
        "TotalRecordCount": 0,
        "StartIndex": 0
      },
      "ThemeSongsResult": {
        "OwnerId": "f27caa37e5142225cceded48f6553502",
        "Items": [],
        "TotalRecordCount": 0,
        "StartIndex": 0
      },
      "SoundtrackSongsResult": {
        "OwnerId": "00000000000000000000000000000000",
        "Items": [],
        "TotalRecordCount": 0,
        "StartIndex": 0
      }
    }))
    .into_response())
    // match media::Entity::find_by_id(id).one(&state.conn).await? {
    //     Some(item) => {
    //         Ok(Json(jellyfin_sdk::types::BaseItemDto::from(item)).into_response())
    //    }
    //    None => Ok(StatusCode::NOT_FOUND.into_response()),
    // }
}

pub async fn mock_items(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        ..Default::default()
    }))
}
