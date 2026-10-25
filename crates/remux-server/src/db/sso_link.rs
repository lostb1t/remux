use anyhow::Result;
use sqlx::SqlitePool;
use uuid::Uuid;

use super::User;

/// Ties an OpenID Connect identity — provider slug plus its stable `sub`
/// claim — to a local user. Usernames can change at the provider; `sub` can't.
pub struct SsoLink;

impl SsoLink {
    pub async fn find_user(
        db: &SqlitePool,
        provider: &str,
        subject: &str,
    ) -> Result<Option<User>> {
        Ok(sqlx::query_as::<_, User>(
            r#"
            SELECT u.*
            FROM user_sso_links l
            JOIN users u ON u.id = l.user_id
            WHERE l.provider = ?1 AND l.subject = ?2
            "#,
        )
        .bind(provider)
        .bind(subject)
        .fetch_optional(db)
        .await?)
    }

    pub async fn link(
        db: &SqlitePool,
        provider: &str,
        subject: &str,
        user_id: Uuid,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO user_sso_links (provider, subject, user_id)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(provider, subject) DO UPDATE SET user_id = excluded.user_id
            "#,
        )
        .bind(provider)
        .bind(subject)
        .bind(user_id)
        .execute(db)
        .await?;
        Ok(())
    }
}
