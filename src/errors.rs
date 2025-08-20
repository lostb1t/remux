use axum;
use axum::{
    extract::FromRequest,
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_valid::Valid as AxumValid;
pub use eyre;
pub use eyre::OptionExt;
use serde::Serialize;
use serde_json::json;
use serde_with::skip_serializing_none;

#[derive(Debug, Serialize, Default)]
#[skip_serializing_none]
pub struct ErrorDetail {
    pub error: Option<String>,
    pub description: Option<String>,
    pub errors: Option<serde_json::Value>,
}

/// Error type which implements `IntoResponse`
#[derive(Debug)]
pub struct ApiError(eyre::Report);

/// This enables using `?`
impl<E: Into<eyre::Report>> From<E> for ApiError {
    fn from(error: E) -> Self {
        // tracing::trace!("API Error: {:?}", error);

        //println!("{}", std::any::type_name::<E>());
        Self(error.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::error!("API Error: {:?}", self.0);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(ErrorDetail {
                error: Some(format!("{:?}", self.0)),
                ..Default::default()
            }),
        )
            .into_response()
    }
}

// Result type
pub type Result<T, E = ApiError> = eyre::Result<T, E>;

//#[derive(FromRequest)]
//#[from_request(via(AxumValid), rejection(ApiError))]
//pub struct Valid<T>(pub T);

// We implement `IntoResponse` for our extractor so it can be used as a response
//impl<T: Serialize> IntoResponse for Valid<T> {
//    fn into_response(self) -> axum::response::Response {
//self.0
//      axum::Json(self.0).into_response()
//    }
//}
