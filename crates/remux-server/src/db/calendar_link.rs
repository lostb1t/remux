use anyhow::Result;
use chrono::{DateTime, Utc};
use rand::RngCore;
use sqlx::SqlitePool;
use uuid::Uuid;

/// Bytes of entropy in a feed token. 256 bits: the token is a bearer credential
/// on a URL that calendar clients store in plaintext and poll unauthenticated,
/// so it must be infeasible to guess.
const TOKEN_ENTROPY_BYTES: usize = 32;

/// Marks a string as a Remux calendar token so an obviously malformed feed URL
/// is rejected before it reaches the database.
const TOKEN_PREFIX: &str = "remux_cal_";

/// A user's ICS feed link.
///
/// The token is stored in plaintext: only an administrator creates and
/// distributes these links, so the admin UI must be able to show an existing
/// token rather than rotate it to learn its value. `api_keys` stores its
/// `access_token` the same way. The token is wrapped in [`remux_utils::Secret`]
/// so it is not serialised or logged by accident.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CalendarLink {
    pub token: remux_utils::Secret<String>,
    pub user_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub rotated_at: Option<DateTime<Utc>>,
}

impl CalendarLink {
    /// Returns the user's existing link, creating one only if absent.
    ///
    /// Deliberately non-destructive: an admin asking for someone's link must not
    /// invalidate the URL that person already subscribed with. Use
    /// [`Self::rotate`] to replace a token on purpose.
    pub async fn get_or_create(db: &SqlitePool, user_id: &Uuid) -> Result<Self> {
        if let Some(existing) = Self::get_by_user(db, user_id).await? {
            return Ok(existing);
        }
        // Racing callers: one INSERT wins, the loser reads the winner's row.
        let token = generate_token();
        sqlx::query(
            "INSERT INTO user_calendar_links (token, user_id, created_at) \
             VALUES (?1, ?2, ?3) ON CONFLICT(user_id) DO NOTHING",
        )
        .bind(&token)
        .bind(user_id)
        .bind(Utc::now())
        .execute(db)
        .await?;
        Self::get_by_user(db, user_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("calendar link not found after insert"))
    }

    /// Replaces the user's token, killing the previous URL immediately.
    pub async fn rotate(db: &SqlitePool, user_id: &Uuid) -> Result<Self> {
        let token = generate_token();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO user_calendar_links (token, user_id, created_at) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(user_id) DO UPDATE SET token = ?1, rotated_at = ?3",
        )
        .bind(&token)
        .bind(user_id)
        .bind(now)
        .execute(db)
        .await?;
        Self::get_by_user(db, user_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("calendar link not found after rotate"))
    }

    pub async fn get_by_user(db: &SqlitePool, user_id: &Uuid) -> Result<Option<Self>> {
        Ok(sqlx::query_as::<_, Self>(
            "SELECT * FROM user_calendar_links WHERE user_id = ?1",
        )
        .bind(user_id)
        .fetch_optional(db)
        .await?)
    }

    /// Resolves the token a feed request presented to its owning link.
    ///
    /// Malformed tokens are rejected before any query: the encoding is fixed, so
    /// a mismatch cannot be a token we issued.
    pub async fn get_by_token(db: &SqlitePool, token: &str) -> Result<Option<Self>> {
        if !is_well_formed(token) {
            return Ok(None);
        }
        Ok(sqlx::query_as::<_, Self>(
            "SELECT * FROM user_calendar_links WHERE token = ?1",
        )
        .bind(token)
        .fetch_optional(db)
        .await?)
    }

    /// Every link, newest first. Admin-only listing.
    pub async fn get_all(db: &SqlitePool) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(
            "SELECT * FROM user_calendar_links ORDER BY created_at DESC",
        )
        .fetch_all(db)
        .await?)
    }

    pub async fn delete_by_user(db: &SqlitePool, user_id: &Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM user_calendar_links WHERE user_id = ?1")
            .bind(user_id)
            .execute(db)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

/// Mints a token.
fn generate_token() -> String {
    let mut entropy = [0u8; TOKEN_ENTROPY_BYTES];
    rand::thread_rng().fill_bytes(&mut entropy);
    format!("{TOKEN_PREFIX}{}", base64_url_nopad(&entropy))
}

/// Whether `token` matches the shape we issue: correct prefix, exact encoded
/// length, base64url alphabet only.
fn is_well_formed(token: &str) -> bool {
    let Some(encoded) = token.strip_prefix(TOKEN_PREFIX) else {
        return false;
    };
    encoded.len() == base64_url_nopad_len(TOKEN_ENTROPY_BYTES)
        && encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn base64_url_nopad(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Encoded length of `n` bytes in unpadded base64.
const fn base64_url_nopad_len(n: usize) -> usize {
    n.div_ceil(3) * 4 - (3 - n % 3) % 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_prefixed_and_full_entropy() {
        let token = generate_token();
        assert!(token.starts_with(TOKEN_PREFIX), "{token}");
        assert!(is_well_formed(&token), "own token rejected: {token}");
        assert_eq!(
            token.len(),
            TOKEN_PREFIX.len() + base64_url_nopad_len(TOKEN_ENTROPY_BYTES),
            "token must carry the full 256 bits: {token}"
        );
    }

    #[test]
    fn generated_tokens_are_unique() {
        assert_ne!(generate_token(), generate_token());
    }

    #[test]
    fn rejects_malformed_tokens() {
        let valid = generate_token();
        let body = valid
            .strip_prefix(TOKEN_PREFIX)
            .unwrap();
        for bad in [
            // No prefix: not a token we issued.
            body.to_string(),
            // Wrong prefix.
            format!("remux_api_{body}"),
            // Truncated and over-long bodies.
            format!("{TOKEN_PREFIX}{}", &body[..body.len() - 1]),
            format!("{TOKEN_PREFIX}{body}x"),
            // Non-base64url characters, including SQL metacharacters.
            format!("{TOKEN_PREFIX}{}'", &body[..body.len() - 1]),
            format!("{TOKEN_PREFIX}{}=", &body[..body.len() - 1]),
            // Empty body.
            TOKEN_PREFIX.to_string(),
            String::new(),
        ] {
            assert!(!is_well_formed(&bad), "accepted malformed token {bad:?}");
        }
    }

    #[test]
    fn base64_length_matches_encoder() {
        for n in [1usize, 2, 3, 16, 31, 32, 33] {
            assert_eq!(
                base64_url_nopad_len(n),
                base64_url_nopad(&vec![0u8; n]).len(),
                "length mismatch for {n} bytes"
            );
        }
    }
}
