use axum::{Json, extract::State, response::IntoResponse};
use tracing::{debug, info};
use uuid::Uuid;

use crate::{
    AppState,
    api::{self, db_media_to_item},
    db::{self, MediaKind, UserMediaState, auth},
    intro,
};
use axum_anyhow::ApiResult as Result;

/// Core intro selection logic, called from the route stubs in items.rs and users.rs.
pub async fn get_intros_inner(
    state: AppState,
    session: auth::AuthSession,
    id: Uuid,
) -> Result<impl IntoResponse> {
    let opts = db::Settings::get_intro_config(
        &state
            .ctx
            .db,
    )
    .await?;
    if opts
        .intro_dir
        .is_none()
    {
        return Ok(Json(api::BaseItemDtoQueryResult::empty()));
    }

    let media = match db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    {
        Some(m) => m,
        None => return Ok(Json(api::BaseItemDtoQueryResult::empty())),
    };

    let triggered = match media.kind {
        MediaKind::Movie => {
            opts.triggers
                .movies
        }
        MediaKind::Episode => {
            (opts
                .triggers
                .season_premieres
                && media.idx == Some(1))
                || opts
                    .triggers
                    .all_episodes
        }
        _ => false,
    };
    if !triggered {
        debug!(item_id = %id, kind = ?media.kind, "intro: no trigger match");
        return Ok(Json(api::BaseItemDtoQueryResult::empty()));
    }

    if opts.skip_resume {
        if let Ok(Some(state_row)) = UserMediaState::get_by_user_and_media(
            &state
                .ctx
                .db,
            &session.user,
            &media,
        )
        .await
        {
            if state_row.playback_position > 0 {
                debug!(item_id = %id, pos = state_row.playback_position, "intro: skipping — user resuming");
                return Ok(Json(api::BaseItemDtoQueryResult::empty()));
            }
        }
    }

    let intros = intro::all_intros(
        &state
            .ctx
            .db,
    )
    .await?;
    let chosen = match intro::pick_intro(
        &intros,
        opts.order,
        &state
            .ctx
            .store,
    ) {
        Some(m) => m.clone(),
        None => return Ok(Json(api::BaseItemDtoQueryResult::empty())),
    };

    info!(intro_id = %chosen.id, item_id = %id, kind = ?media.kind, "intro selected");

    let dto = db_media_to_item(chosen, false);
    Ok(Json(api::BaseItemDtoQueryResult::single(dto)))
}
