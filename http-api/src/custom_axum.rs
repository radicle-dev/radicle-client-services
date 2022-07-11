use axum::{
    async_trait,
    extract::{path::ErrorKind, rejection::PathRejection, FromRequest, RequestParts},
    http::StatusCode,
    Json,
};
use serde::{de::DeserializeOwned, Serialize};

pub struct Path<T>(pub T);

#[async_trait]
impl<B, T> FromRequest<B> for Path<T>
where
    T: DeserializeOwned + Send,
    B: Send,
{
    type Rejection = (StatusCode, axum::Json<Error>);

    async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request(req).await {
            Ok(value) => Ok(Self(value.0)),
            Err(rejection) => {
                let status = StatusCode::BAD_REQUEST;
                let body = match rejection {
                    PathRejection::FailedToDeserializePathParams(inner) => {
                        let kind = inner.into_kind();
                        let body = match &kind {
                            ErrorKind::Message(msg) => Json(Error {
                                success: false,
                                error: msg.to_string(),
                            }),
                            _ => Json(Error {
                                success: false,
                                error: kind.to_string(),
                            }),
                        };

                        body
                    }
                    _ => Json(Error {
                        success: false,
                        error: format!("{}", rejection),
                    }),
                };

                Err((status, body))
            }
        }
    }
}

#[derive(Serialize)]
pub struct Error {
    success: bool,
    error: String,
}
