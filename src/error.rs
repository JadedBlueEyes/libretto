use std::ops::Deref;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use color_eyre::eyre;

/// Application error wrapper for unified error handling.
#[derive(Debug)]
pub struct AppError(pub eyre::Report);

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {:?}", self.0),
        )
            .into_response()
    }
}

impl Deref for AppError {
    type Target = eyre::Report;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<eyre::Report> for AppError {
    fn from(err: eyre::Report) -> Self {
        Self(err)
    }
}

// impl<E> From<E> for AppError
// where
//     E: Into<eyre::Report>,
// {
//     fn from(err: E) -> Self {
//         Self(err.into())
//     }
// }
