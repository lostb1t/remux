use axum_anyhow::{ApiError, ApiResult};

pub trait ResultExt<T>: Sized {
    fn context_not_found(self, detail: &str) -> ApiResult<T>;
    fn context_bad_request(self, detail: &str) -> ApiResult<T>;
    fn context_unauthorized(self, detail: &str) -> ApiResult<T>;
    fn context_forbidden(self, detail: &str) -> ApiResult<T>;
    fn context_internal(self, detail: &str) -> ApiResult<T>;
    fn context_bad_gateway(self, detail: &str) -> ApiResult<T>;
    /// Classifies a reqwest network error and returns an appropriate API error:
    /// connection/timeout → 502, 404 → 400, other → 502.
    fn context_not_reachable(self) -> ApiResult<T>;
}

impl<T, E: Into<anyhow::Error>> ResultExt<T> for std::result::Result<T, E> {
    fn context_not_found(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::ResultExt::context_not_found(self, "Not Found", detail)
    }
    fn context_bad_request(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::ResultExt::context_bad_request(self, "Bad Request", detail)
    }
    fn context_unauthorized(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::ResultExt::context_unauthorized(self, "Unauthorized", detail)
    }
    fn context_forbidden(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::ResultExt::context_forbidden(self, "Forbidden", detail)
    }
    fn context_internal(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::ResultExt::context_internal(self, "Internal Server Error", detail)
    }
    fn context_bad_gateway(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::ResultExt::context_bad_gateway(self, "Bad Gateway", detail)
    }
    fn context_not_reachable(self) -> ApiResult<T> {
        self.map_err(IntoApiError::context_not_reachable)
    }
}

pub trait OptionExt<T>: Sized {
    fn context_not_found(self, detail: &str) -> ApiResult<T>;
    fn context_bad_request(self, detail: &str) -> ApiResult<T>;
    fn context_unauthorized(self, detail: &str) -> ApiResult<T>;
    fn context_forbidden(self, detail: &str) -> ApiResult<T>;
    fn context_internal(self, detail: &str) -> ApiResult<T>;
}

pub trait IntoApiError: Sized {
    fn context_not_found(self, detail: &str) -> ApiError;
    fn context_bad_request(self, detail: &str) -> ApiError;
    fn context_unauthorized(self, detail: &str) -> ApiError;
    fn context_forbidden(self, detail: &str) -> ApiError;
    fn context_internal(self, detail: &str) -> ApiError;
    fn context_bad_gateway(self, detail: &str) -> ApiError;
    fn context_not_reachable(self) -> ApiError;
}

impl<E: Into<anyhow::Error>> IntoApiError for E {
    fn context_not_found(self, detail: &str) -> ApiError {
        axum_anyhow::IntoApiError::context_not_found(self, "Not Found", detail)
    }
    fn context_bad_request(self, detail: &str) -> ApiError {
        axum_anyhow::IntoApiError::context_bad_request(self, "Bad Request", detail)
    }
    fn context_unauthorized(self, detail: &str) -> ApiError {
        axum_anyhow::IntoApiError::context_unauthorized(self, "Unauthorized", detail)
    }
    fn context_forbidden(self, detail: &str) -> ApiError {
        axum_anyhow::IntoApiError::context_forbidden(self, "Forbidden", detail)
    }
    fn context_internal(self, detail: &str) -> ApiError {
        axum_anyhow::IntoApiError::context_internal(
            self,
            "Internal Server Error",
            detail,
        )
    }
    fn context_bad_gateway(self, detail: &str) -> ApiError {
        axum_anyhow::IntoApiError::context_bad_gateway(self, "Bad Gateway", detail)
    }
    fn context_not_reachable(self) -> ApiError {
        let e: anyhow::Error = self.into();
        let (is_unreachable, is_not_found) = e
            .chain()
            .find_map(|cause| cause.downcast_ref::<reqwest::Error>())
            .map(|re| {
                (
                    re.is_connect() || re.is_timeout(),
                    re.status()
                        .map(|s| s.as_u16() == 404)
                        .unwrap_or(false),
                )
            })
            .unwrap_or((false, false));
        if is_not_found {
            axum_anyhow::IntoApiError::context_bad_request(
                e,
                "Bad Request",
                "Remote returned 404 — double-check the URL.",
            )
        } else if is_unreachable {
            axum_anyhow::IntoApiError::context_bad_gateway(
                e,
                "Bad Gateway",
                "Could not reach the remote endpoint.",
            )
        } else {
            axum_anyhow::IntoApiError::context_bad_gateway(
                e,
                "Bad Gateway",
                "Remote request failed.",
            )
        }
    }
}

impl<T> OptionExt<T> for Option<T> {
    fn context_not_found(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::OptionExt::context_not_found(self, "Not Found", detail)
    }
    fn context_bad_request(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::OptionExt::context_bad_request(self, "Bad Request", detail)
    }
    fn context_unauthorized(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::OptionExt::context_unauthorized(self, "Unauthorized", detail)
    }
    fn context_forbidden(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::OptionExt::context_forbidden(self, "Forbidden", detail)
    }
    fn context_internal(self, detail: &str) -> ApiResult<T> {
        axum_anyhow::OptionExt::context_internal(self, "Internal Server Error", detail)
    }
}
