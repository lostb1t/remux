use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use isolang::Language;
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

#[get("/localization/countries")]
pub async fn get_countries(
    State(_state): State<AppState>,
) -> Result<impl IntoResponse> {
    let countries = rust_iso3166::ALL
        .iter()
        .map(|c| jellyfin::CountryInfo {
            name: c.name.to_string(),
            display_name: c.name.to_string(),
            two_letter_iso_region_name: c.alpha2.to_string(),
            three_letter_iso_region_name: c.alpha3.to_string(),
        })
        .collect::<Vec<_>>();
    Ok(Json(countries))
}

#[get("/localization/cultures")]
pub async fn get_cultures(State(_state): State<AppState>) -> Result<impl IntoResponse> {
    let cultures = isolang::languages()
        .filter_map(|lang| {
            let two = lang.to_639_1()?;
            Some(jellyfin::CultureDto {
                name: two.to_string(),
                display_name: lang.to_name().to_string(),
                two_letter_iso_language_name: two.to_string(),
                three_letter_iso_language_name: vec![lang.to_639_3().to_string()],
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(cultures))
}

#[get("/localization/parentalratings")]
pub async fn get_parental_ratings(
    State(_state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json(Vec::<serde_json::Value>::new()))
}
