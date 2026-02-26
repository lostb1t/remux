use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use http::StatusCode;
use remux_macros::{get, post};

use crate::AppState;
use crate::db::auth;
use crate::jellyfin;
use axum_anyhow::{ApiResult as Result, IntoApiError};

const NETWORK_CONFIG_KEY: &str = "network_configuration";

fn default_network_configuration() -> jellyfin::NetworkConfiguration {
    jellyfin::NetworkConfiguration {
        require_https: Some(false),
        base_url: Some("".to_string()),
        public_https_port: Some(8920),
        http_server_port_number: Some(8096),
        https_port_number: Some(8920),
        enable_https: Some(false),
        is_port_authorized: Some(true),
        auto_discovery: Some(true),
        enable_u_pn_p: Some(false),
        enable_i_pv4: Some(true),
        enable_i_pv6: Some(false),
        internal_http_port: Some(8096),
        internal_https_port: Some(8920),
        public_http_port: Some(8096),
        local_network_subnets: Some(vec![]),
        local_network_addresses: Some(vec![]),
        known_proxies: Some(vec![]),
        ignore_virtual_interfaces: Some(true),
        virtual_interface_names: Some(vec!["vEthernet*".to_string()]),
        enable_published_server_uri_by_request: Some(false),
        published_server_uri_by_subnet: Some(vec![]),
    }
}

#[get("/system/configuration/network")]
pub async fn get_network_configuration(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    if !session.user.is_admin {
        return Err(anyhow::anyhow!("").context_forbidden("Forbidden", "Admin access required"));
    }
    let config = match crate::db::Settings::get(&state.ctx.db, NETWORK_CONFIG_KEY).await? {
        Some(json) => serde_json::from_str(&json).unwrap_or_else(|_| default_network_configuration()),
        None => default_network_configuration(),
    };
    Ok(Json(config))
}

#[post("/system/configuration/network")]
pub async fn update_network_configuration(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(config): Json<jellyfin::NetworkConfiguration>,
) -> Result<impl IntoResponse> {
    if !session.user.is_admin {
        return Err(anyhow::anyhow!("").context_forbidden("Forbidden", "Admin access required"));
    }
    let json = serde_json::to_string(&config)?;
    crate::db::Settings::set(&state.ctx.db, NETWORK_CONFIG_KEY, &json).await?;
    Ok(StatusCode::NO_CONTENT)
}
