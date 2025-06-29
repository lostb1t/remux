#[derive(Debug)]
pub enum ApiError {
    /// HTTP client creation failed
    HttpClientError,

    /// Failed to parse final URL.
    UrlError,

    /// Failed to serialize struct to JSON (in POST).
    SerializeParseError(serde_json::Error),

    /// Failed to deserialize data to struct (in GET or POST response).
    DeserializeParseError(serde_json::Error, String),

    /// Failed to deserialize data to struct from simd_json crate (in GET or POST response).
    #[cfg(feature = "lib-simd-json")]
    DeserializeParseSimdJsonError(simd_json::Error, String),

    /// Failed to make the outgoing request.
    RequestError,

    /// Failed to perform HTTP call using Hyper
    // HyperError(hyper::Error),

    /// Failed to perform IO operation
    IoError(std::io::Error),

    /// Server returned non-success status.
    HttpError(u16, String),

    /// Request has timed out
    TimeoutError,

    /// Invalid parameter value
    InvalidValue,
}
