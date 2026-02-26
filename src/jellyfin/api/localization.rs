use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use remux_macros::get;

use crate::AppState;
use crate::jellyfin;
use axum_anyhow::ApiResult as Result;

/// Get localization options
#[get("/localization/options")]
pub async fn get_localization_options(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    // Return common localization options
    // In a real implementation, this would come from a localization service
    let options = vec![
        jellyfin::LocalizationOption {
            name: "English".to_string(),
            value: "en".to_string(),
        },
        jellyfin::LocalizationOption {
            name: "Spanish".to_string(),
            value: "es".to_string(),
        },
        jellyfin::LocalizationOption {
            name: "French".to_string(),
            value: "fr".to_string(),
        },
        jellyfin::LocalizationOption {
            name: "German".to_string(),
            value: "de".to_string(),
        },
        jellyfin::LocalizationOption {
            name: "Chinese".to_string(),
            value: "zh".to_string(),
        },
        jellyfin::LocalizationOption {
            name: "Japanese".to_string(),
            value: "ja".to_string(),
        },
        jellyfin::LocalizationOption {
            name: "Russian".to_string(),
            value: "ru".to_string(),
        },
        jellyfin::LocalizationOption {
            name: "Portuguese".to_string(),
            value: "pt".to_string(),
        },
        jellyfin::LocalizationOption {
            name: "Arabic".to_string(),
            value: "ar".to_string(),
        },
        jellyfin::LocalizationOption {
            name: "Italian".to_string(),
            value: "it".to_string(),
        },
    ];

    Ok(Json(options))
}

#[get("/localization/parentalratings")]
pub async fn get_parental_ratings(State(_state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(Vec::<serde_json::Value>::new()))
}