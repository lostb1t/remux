use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use remux_macros::{get, route};
use http::StatusCode;
use serde_json::json;

use crate::AppState;
use crate::jellyfin;
use crate::utils::server_id;
use axum_anyhow::ApiResult as Result;

use super::{mock_items, stub};

/// TODO: make a real server id
#[get("/system/info/public")]
pub async fn system_info_public(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::PublicSystemInfo {
        local_address: Some("".to_string()),
        server_name: Some("Remux".to_string()),
        product_name: Some("Jellyfin Server".to_string()),
        startup_wizard_completed: Some(true),
        version: Some("10.10.7".to_string()),
        operating_system: Some("".to_string()),
        id: Some(server_id()),
        ..Default::default()
    }))
}

#[get("/system/info")]
pub async fn system_info(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::SystemInfo {
        id: Some(server_id()),
        server_name: Some(server_id()),
        // server_id: Some(server_id()),
        ..Default::default()
    }))
}

#[get("/system/ping")]
pub async fn system_ping(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(json!("Remux Server")))
}

/// Get storage information
#[get("/system/info/storage")]
pub async fn system_info_storage(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    // Create storage information following the Jellyfin SystemStorageInfo structure
    let system_storage_info = jellyfin::SystemStorageInfo {
        program_data_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/data".to_string()),
            free_space: Some(500000000),
            used_space: Some(500000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("data-device".to_string()),
            ..Default::default()
        }),
        web_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/web".to_string()),
            free_space: Some(1000000000),
            used_space: Some(100000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("web-device".to_string()),
            ..Default::default()
        }),
        image_cache_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/cache/images".to_string()),
            free_space: Some(800000000),
            used_space: Some(200000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("cache-device".to_string()),
            ..Default::default()
        }),
        cache_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/tmp".to_string()),
            free_space: Some(900000000),
            used_space: Some(100000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("tmp-device".to_string()),
            ..Default::default()
        }),
        log_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/logs".to_string()),
            free_space: Some(700000000),
            used_space: Some(300000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("log-device".to_string()),
            ..Default::default()
        }),
        internal_metadata_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/metadata".to_string()),
            free_space: Some(600000000),
            used_space: Some(400000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("metadata-device".to_string()),
            ..Default::default()
        }),
        transcoding_temp_folder: Some(jellyfin::FolderStorageInfo {
            path: Some("/transcodes".to_string()),
            free_space: Some(1500000000),
            used_space: Some(500000000),
            storage_type: Some("DefaultFileSystem".to_string()),
            device_id: Some("transcode-device".to_string()),
            ..Default::default()
        }),
        libraries: Some(vec![
            jellyfin::LibraryStorageInfo {
                id: Some("movies-library-id".to_string()),
                name: Some("Movies".to_string()),
                folders: Some(vec![
                    jellyfin::FolderStorageInfo {
                        path: Some("/media/movies".to_string()),
                        free_space: Some(2000000000),
                        used_space: Some(1000000000),
                        storage_type: Some("DefaultFileSystem".to_string()),
                        device_id: Some("media-device".to_string()),
                        ..Default::default()
                    }
                ]),
                ..Default::default()
            },
            jellyfin::LibraryStorageInfo {
                id: Some("series-library-id".to_string()),
                name: Some("TV Shows".to_string()),
                folders: Some(vec![
                    jellyfin::FolderStorageInfo {
                        path: Some("/media/tv".to_string()),
                        free_space: Some(2000000000),
                        used_space: Some(1500000000),
                        storage_type: Some("DefaultFileSystem".to_string()),
                        device_id: Some("media-device".to_string()),
                        ..Default::default()
                    }
                ]),
                ..Default::default()
            }
        ]),
        ..Default::default()
    };

    Ok(Json(system_storage_info))
}

#[cfg(test)]
#[tokio::test]
async fn system_ping_test() {
    let server = crate::integration_test::new_test_server().await.unwrap();

    let response = server.get("/system/ping").await;

    response.assert_status_ok();
    //response.assert_text("Remux Server");
}

#[cfg(test)]
#[tokio::test]
async fn system_info_storage_test() {
    let server = crate::integration_test::new_test_server().await.unwrap();

    let response = server.get("/system/info/storage").await;

    response.assert_status_ok();
    let storage_info: crate::jellyfin::SystemStorageInfo = response.json();
    
    // Check that we have the expected storage folders
    assert!(storage_info.program_data_folder.is_some());
    assert!(storage_info.cache_folder.is_some());
    assert!(storage_info.web_folder.is_some());
    
    // Check that we have libraries
    assert!(storage_info.libraries.is_some());
    let libraries = storage_info.libraries.unwrap();
    assert_eq!(libraries.len(), 2);
    
    // Check library names
    let library_names: Vec<_> = libraries.iter().filter_map(|lib| lib.name.as_ref()).collect();
    assert!(library_names.contains(&"Movies"));
    assert!(library_names.contains(&"TV Shows"));
}

#[get("/system/endpoint")]
pub async fn system_endpoint(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json(json!({
        "IsLocal": false,
        "IsInNetwork": false,

    })))
}

#[get("/syncplay/list")]
pub async fn syncplay_list(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[route("/quickconnect/enabled", method = "GET", method = "POST")]
pub async fn quickconnect_enabled(State(state): State<AppState>) -> Result<impl IntoResponse> {
    stub(State(state)).await
}

#[route("/branding/configuration", method = "GET", method = "POST")]
pub async fn branding_configuration(State(state): State<AppState>) -> Result<impl IntoResponse> {
    stub(State(state)).await
}
