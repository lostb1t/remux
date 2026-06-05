use axum_anyhow::{ApiError, ApiResult};

pub trait ResultExt<T>: Sized {
    fn context_not_found(self, detail: &str) -> ApiResult<T>;
    fn context_bad_request(self, detail: &str) -> ApiResult<T>;
    fn context_unauthorized(self, detail: &str) -> ApiResult<T>;
    fn context_forbidden(self, detail: &str) -> ApiResult<T>;
    fn context_internal(self, detail: &str) -> ApiResult<T>;
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
