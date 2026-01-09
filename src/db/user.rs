use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use rand_core::OsRng;
use anyhow::{Context, Result, anyhow};
use crate::utils::get_uuid;
use super::DbConn;

#[derive(Debug, Default, Clone, Serialize, Deserialize, Queryable, Insertable, Identifiable)]
#[diesel(table_name = auth_users)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    #[serde(skip_serializing)]
    pub aio_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Insertable, Identifiable, Associations)]
#[diesel(table_name = user_media_info)]
#[diesel(belongs_to(User, foreign_key = user_id))]
pub struct UserMediaInfo {
    pub user_id: String,
    pub media_id: String,
    pub is_fav: bool,
    pub playback_position: i64,
}

impl User {
    pub fn save(&mut self, conn: &DbConn) -> Result<()> {
        diesel::insert_into(auth_users::table)
            .values(self)
            .on_conflict(auth_users::id)
            .do_update()
            .set(self)
            .execute(conn)
            .map(|_| ())
            .map_err(|e| anyhow!("Failed to save user: {e}"))
    }

    pub fn get_by_id(conn: &DbConn, id: &String) -> Result<Option<Self>> {
        auth_users::table
            .find(id)
            .first(conn)
            .optional()
            .map_err(|e| anyhow!("Failed to fetch user: {e}"))
    }

    pub fn get_by_username(conn: &DbConn, username: &str) -> Result<Option<Self>> {
        auth_users::table
            .filter(auth_users::username.eq(username))
            .first(conn)
            .optional()
            .map_err(|e| anyhow!("Failed to fetch user: {e}"))
    }

    pub fn new_with_password(
        username: String,
        password: &str,
        aio_url: Option<String>,
    ) -> Result<Self> {
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
            .map_err(|e| anyhow!("Invalid stored password hash: {e}"))?;

        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    }

    pub fn hash_password(password: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow!("Password hashing failed: {e}"))?;

        Ok(hash.to_string())
    }

    pub fn authenticate(
        conn: &DbConn,
        username: &str,
        password: &str,
    ) -> Result<Option<Self>> {
        let user = Self::get_by_username(conn, username)?;

        match user {
            Some(user) if user.verify_password(password)? => Ok(Some(user)),
            _ => Ok(None),
        }
    }
}

impl UserMediaInfo {
    pub fn save(&self, conn: &DbConn) -> Result<()> {
        diesel::insert_into(user_media_info::table)
            .values(self)
            .on_conflict((user_media_info::user_id, user_media_info::media_id))
            .do_update()
            .set(self)
            .execute(conn)
            .map(|_| ())
            .map_err(|e| anyhow!("Failed to save user media info: {e}"))
    }

    pub fn get_by_user_and_media(
        conn: &DbConn,
        user_id: &str,
        media_id: &str,
    ) -> Result<Option<Self>> {
        user_media_info::table
            .filter(user_media_info::user_id.eq(user_id))
            .filter(user_media_info::media_id.eq(media_id))
            .first(conn)
            .optional()
            .map_err(|e| anyhow!("Failed to fetch user media info: {e}"))
    }
}