use anyhow::Result;
use chrono::{DateTime, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use uuid::Uuid;

/// Bytes of entropy in a feed token. 256 bits: the token is a bearer credential
/// on a URL that calendar clients store in plaintext and poll unauthenticated,
/// so it must be infeasible to guess.
const TOKEN_ENTROPY_BYTES: usize = 32;

/// Marks a string as a Remux calendar token so an obviously malformed feed URL
/// is rejected before it reaches the database.
const TOKEN_PREFIX: &str = "remux_cal_";

/// A user's ICS feed link. The token itself is never stored — only its digest.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CalendarLink {
    pub token_hash: Vec<u8>,
    pub user_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub rotated_at: Option<DateTime<Utc>>,
}

/// A freshly minted link plus its plaintext token. The token is returned exactly
/// once, at creation or rotation; afterwards only the digest exists.
#[derive(Debug, Clone)]
pub struct CalendarCredential {
    pub link: CalendarLink,
    pub token: remux_utils::Secret<String>,
}

impl CalendarLink {
    /// Creates the user's link, or rotates it if one already exists.
    ///
    /// Rotation reuses this single upsert so a user can never end up with two
    /// live feed URLs: replacing the row revokes the previous token.
    pub async fn upsert(db: &SqlitePool, user_id: &Uuid) -> Result<CalendarCredential> {
        let (token, token_hash) = generate_token();
        let now = Utc::now();
        // token_hash is the primary key, so a rotation inserts a new key for an
        // existing user_id; the conflict target is therefore user_id.
        sqlx::query(
            "INSERT INTO user_calendar_links (token_hash, user_id, created_at) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(user_id) DO UPDATE SET token_hash = ?1, rotated_at = ?3",
        )
        .bind(&token_hash)
        .bind(user_id)
        .bind(now)
        .execute(db)
        .await?;

        let link = Self::get_by_user(db, user_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("calendar link not found after upsert"))?;
        Ok(CalendarCredential {
            link,
            token: remux_utils::Secret::new(token),
        })
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
    /// Malformed tokens are rejected without a query. Lookup is by digest, so a
    /// stolen database yields no usable feed URLs.
    pub async fn get_by_token(db: &SqlitePool, token: &str) -> Result<Option<Self>> {
        let Some(token_hash) = token_digest(token) else {
            return Ok(None);
        };
        Ok(sqlx::query_as::<_, Self>(
            "SELECT * FROM user_calendar_links WHERE token_hash = ?1",
        )
        .bind(token_hash)
        .fetch_optional(db)
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

/// Mints a token and its digest.
fn generate_token() -> (String, Vec<u8>) {
    let mut entropy = [0u8; TOKEN_ENTROPY_BYTES];
    rand::thread_rng().fill_bytes(&mut entropy);
    let token = format!("{TOKEN_PREFIX}{}", base64_url_nopad(&entropy));
    let digest = Sha256::digest(token.as_bytes()).to_vec();
    (token, digest)
}

/// Digest of a well-formed token, or `None` if the token cannot be one we issued.
fn token_digest(token: &str) -> Option<Vec<u8>> {
    let encoded = token.strip_prefix(TOKEN_PREFIX)?;
    // Reject anything that is not exactly our own encoding: length and alphabet
    // are fixed, so a mismatch cannot be a token we minted.
    if encoded.len() != base64_url_nopad_len(TOKEN_ENTROPY_BYTES)
        || !encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    Some(Sha256::digest(token.as_bytes()).to_vec())
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
        let (token, digest) = generate_token();
        assert!(token.starts_with(TOKEN_PREFIX), "{token}");
        assert_eq!(digest.len(), 32, "digest must be a full SHA-256");
        // The plaintext token must not be recoverable from what we store.
        assert!(
            !digest
                .windows(TOKEN_PREFIX.len())
                .any(|w| w == TOKEN_PREFIX.as_bytes()),
            "digest leaks token material"
        );
    }

    #[test]
    fn generated_tokens_are_unique() {
        let (first, first_digest) = generate_token();
        let (second, second_digest) = generate_token();
        assert_ne!(first, second);
        assert_ne!(first_digest, second_digest);
    }

    #[test]
    fn token_digest_accepts_own_tokens_and_is_stable() {
        let (token, digest) = generate_token();
        assert_eq!(
            token_digest(&token),
            Some(digest),
            "digest must match the one stored at creation"
        );
    }

    #[test]
    fn token_digest_rejects_malformed_tokens() {
        let (valid, _) = generate_token();
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
            assert_eq!(token_digest(&bad), None, "accepted malformed token {bad:?}");
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
