use crate::ResultExt;
/// Axum extractor for Jellyfin-style image uploads.
///
/// Jellyfin clients POST images as a **base64-encoded** body (optionally
/// prefixed with a `data:<mime>;base64,` data-URI header).  This extractor
/// transparently decodes the body and sniffs the content-type from the magic
/// bytes, giving handlers ready-to-use raw image bytes.
///
/// # Usage
/// ```rust,ignore
/// #[post("/some/image/endpoint")]
/// async fn handler(image: JellyfinImage) -> impl IntoResponse {
///     let (bytes, content_type) = image.into_parts();
///     // bytes is decoded raw image data, content_type is e.g. "image/jpeg"
/// }
/// ```
use axum::body::Bytes;
use axum::extract::FromRequest;
use axum::extract::Request;
use axum_anyhow::ApiError;
use base64::Engine;

pub struct JellyfinImage {
    pub bytes: Bytes,
    pub content_type: &'static str,
}

impl JellyfinImage {
    pub fn into_parts(self) -> (Bytes, &'static str) {
        (self.bytes, self.content_type)
    }
}

/// Decode a Jellyfin image body: strip optional data-URI prefix, then
/// base64-decode. Falls back to raw bytes if decoding fails (e.g. a future
/// client sending raw binary directly).
fn decode_body(body: &[u8]) -> Vec<u8> {
    // Strip "data:<mime>;base64," prefix if present
    let src = if let Some(pos) = body
        .iter()
        .position(|&b| b == b',')
    {
        &body[pos + 1..]
    } else {
        body
    };

    base64::engine::general_purpose::STANDARD
        .decode(src)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(src))
        .unwrap_or_else(|_| body.to_vec())
}

pub fn detect_content_type(bytes: &[u8]) -> &'static str {
    match bytes {
        [0xff, 0xd8, 0xff, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', ..] => "image/gif",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        _ => "image/jpeg",
    }
}

impl<S> FromRequest<S> for JellyfinImage
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let body = axum::body::to_bytes(req.into_body(), 50 * 1024 * 1024)
            .await
            .context_internal("body read error")?;

        let decoded = decode_body(&body);
        let content_type = detect_content_type(&decoded);

        Ok(JellyfinImage {
            bytes: Bytes::from(decoded),
            content_type,
        })
    }
}
