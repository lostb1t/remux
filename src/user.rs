use axum::response::Html;
use reqwest;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use async_trait::async_trait;
use axum::ServiceExt;
use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::extract::Request;
use axum::http::request::Parts;
use axum::middleware;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::{
    Json, Router,
    http::StatusCode,
    response::Redirect,
    routing::{get, post},
};
use axum_anyhow::ApiError;
use axum_anyhow::on_error;
use axum_anyhow::set_expose_errors;
use axum_anyhow::{ApiResult, OptionExt, ResultExt};
use chrono::prelude::*;
use chrono::{Duration, Utc};
use config;
use config::Config;
use futures::future::BoxFuture;
use futures_util::StreamExt;
use http::Uri;
use reqwest::header::LOCATION;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use std;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use timed;
use tower::Layer;
use tower::util::MapRequestLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tracing;
use tracing::debug;
use tracing::instrument;
use tracing::warn;
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, prelude::*};
use url::Url;
use uuid::Uuid;
use argon2::{
    password_hash::{
        rand_core::OsRng,
        PasswordHash, PasswordHasher, PasswordVerifier, SaltString
    },
    Argon2
};

use crate::sdks;
use crate::utils::get_uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    #[serde(skip_serializing)]
    pub aio_url: String,
}

impl User {
   pub async fn save(&mut self, db: &SqlitePool) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO auth_users (id, username, password_hash, aio_url)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET
                username      = excluded.username,
                password_hash = excluded.password_hash,
                aio_url       = excluded.aio_url
            "#,
            self.id,
            self.username,
            self.password_hash,
            self.aio_url
        )
        .execute(db)
        .await?;

        Ok(())
    }
  
  pub async fn get_by_id(
        db: &SqlitePool,
        id: &String,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as!(
            Self,
            r#"
            SELECT
            *
            FROM auth_users
            WHERE id = ?1
            "#,
            id
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }
     
  pub async fn get_by_username(
        db: &SqlitePool,
        username: &str,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as!(
            Self,
            r#"
            SELECT
            *
            FROM auth_users
            WHERE username = ?1
            "#,
            username
        )
        .fetch_optional(db)
        .await?;

        Ok(row)
    }   
    
    pub fn new_with_password(key: String, username: String, password: &str, aio_url: String) -> Result<Self> {
        let password_hash = Self::hash_password(password)?;
        Ok(Self {
            id: get_uuid(),
            username,
            password_hash,
            aio_url,
        })
    }

    pub fn set_password(&mut self, password: &str) -> Result<()> {
        self.password_hash = Self::hash_password(password)?;
        Ok(())
    }

    pub fn verify_password(&self, password: &str) -> Result<bool> {
        let parsed = PasswordHash::new(&self.password_hash)
            .map_err(|e| anyhow!("invalid stored password hash: {e}"))?;

        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    }

    pub fn hash_password(password: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow!("password hashing failed: {e}"))?;

        Ok(hash.to_string())
    }

   pub async fn authenticate(db: &SqlitePool, username: &str, password: &str) -> Result<Option<Self>> {
        let Some(user) = Self::get_by_username(db, username).await? else {
            return Ok(None);
        };

        if user.verify_password(password)? {
            Ok(Some(user))
        } else {
            Ok(None)
        }
    }

 

    pub fn get_aio(&self) -> Result<sdks::RestClient> {
        let url = self
            .aio_url
            .strip_suffix("manifest.json")
            .unwrap_or(self.aio_url.as_str());

        Ok(sdks::aio::client(url)?)
    }

    pub fn get_aio_search(&self) -> Result<sdks::RestClient<sdks::BasicAuth>> {
        let mut url = Url::parse(&self.aio_url)?;

        let segments: Vec<&str> = url
            .path_segments()
            .ok_or_else(|| anyhow!("url has no path segments"))?
            .collect();

        if segments.len() < 3 {
            return Err(anyhow!(
                "invalid aio_url format: expected /stremio/<username>/<password>/..."
            ));
        }

        let username = segments[1].to_string();
        let password = segments[2].to_string();

        url.set_path("/api/v1");
        url.set_query(None);
        url.set_fragment(None);

        let search_url = url.as_str().to_string();

        Ok(sdks::aio::search_client(&search_url, username, password)?)
    }

    pub async fn get_stream(
        &self,
        media_type: sdks::aio::MediaType,
        id: String,
        stream_id: String,
    ) -> Result<sdks::aio::Stream> {
        let streams = self
            .get_aio_search()?
            .execute(&sdks::aio::Search {
                kind: media_type.into(),
                id,
                ..Default::default()
            })
            .await?;

        let stream = streams
            .data
            .results
            .into_iter()
            .find(|x| x.id() == stream_id)
            .context("no stream")?;

        Ok(stream)
    }
}