//! What the API answers when it refuses a request or fails: the same JSON
//! body whatever went wrong, and the status that goes with it.

use axum::Json;
use axum::http::header::{CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::api::{ErrorBody, ErrorCode, ErrorResponse};
use crate::auth::Refused;

#[derive(Debug)]
pub(super) struct ApiError {
    pub(super) status: StatusCode,
    pub(super) body: ErrorBody,
}

impl ApiError {
    pub(super) fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorBody {
                code,
                message: message.into(),
                rejections: None,
            },
        }
    }

    pub(super) fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ErrorCode::Invalid, message)
    }

    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, message)
    }

    pub(super) fn internal(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            message,
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(ErrorResponse { error: self.body })).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

impl From<spool::Error> for ApiError {
    fn from(error: spool::Error) -> Self {
        use spool::Error;
        let message = error.to_string();
        match error {
            Error::Rejected(rejections) => Self {
                status: StatusCode::CONFLICT,
                body: ErrorBody {
                    code: ErrorCode::Rejected,
                    message,
                    rejections: Some(rejections),
                },
            },
            Error::UnknownRun(_)
            | Error::UnknownDraft(_)
            | Error::UnknownIdentity(_)
            | Error::IdentityNotYetCreated { .. }
            | Error::UnknownDocument(_)
            | Error::UnknownRecord(_)
            | Error::UnknownChangeset(_)
            | Error::UnknownReviewItem(_) => Self::not_found(message),
            Error::Run { .. }
            | Error::Draft { .. }
            | Error::Review { .. }
            | Error::Split { .. }
            | Error::StalePlan { .. }
            | Error::Retracted(_)
            | Error::NothingToUndo(_) => {
                Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, message)
            }
            Error::Load(_)
            | Error::Retract(_)
            | Error::Documents(_)
            | Error::UnknownExtractor { .. }
            | Error::Judgements(_)
            | Error::Rows(_)
            | Error::Arrow(_)
            | Error::InvalidEntityType(_)
            | Error::FutureIdentitySnapshot { .. }
            | Error::UndeclaredType(_)
            | Error::SameRecord(_)
            | Error::Recompute(_)
            | Error::UnknownStrategy { .. } => Self::invalid(message),
            Error::Strategy { .. } => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::StrategyFailed,
                message,
            ),
            Error::Extractor { .. } | Error::Mentions(_) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::ExtractorFailed,
                message,
            ),
            _ => Self::internal(message),
        }
    }
}

pub(super) type ApiResult<T = Response> = Result<T, ApiError>;

/// Gives a failure the framework answered itself, such as a body over the
/// limit or a method a path does not take, the same JSON body as the rest.
pub(super) async fn describe_failure(response: Response) -> Response {
    let status = response.status();
    let described = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    if described || !(status.is_client_error() || status.is_server_error()) {
        return response;
    }
    let (code, message) = match status {
        StatusCode::PAYLOAD_TOO_LARGE => (
            ErrorCode::TooLarge,
            "the body is larger than this server reads",
        ),
        StatusCode::METHOD_NOT_ALLOWED => {
            (ErrorCode::Invalid, "this path does not take this method")
        }
        StatusCode::NOT_FOUND => (ErrorCode::NotFound, "there is nothing at this path"),
        _ if status.is_server_error() => (ErrorCode::Internal, "the server failed"),
        _ => (ErrorCode::Invalid, "the request could not be read"),
    };
    let mut described = ApiError::new(status, code, message).into_response();
    if let Some(allow) = response.headers().get(axum::http::header::ALLOW) {
        described
            .headers_mut()
            .insert(axum::http::header::ALLOW, allow.clone());
    }
    described
}

pub(super) async fn not_found() -> ApiError {
    ApiError::not_found("there is nothing at this path")
}

impl From<Refused> for ApiError {
    fn from(refused: Refused) -> Self {
        let message = refused.to_string();
        match refused {
            Refused::Invalid(_) | Refused::WrongPassword => Self::invalid(message),
            Refused::Unknown | Refused::UnknownToken | Refused::UnknownGrant => {
                Self::not_found(message)
            }
            Refused::TooMany(_) => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                ErrorCode::TooManyAttempts,
                message,
            ),
            Refused::Taken(_)
            | Refused::EmailTaken(_)
            | Refused::TokenTaken(_)
            | Refused::LastAdmin
            | Refused::SetUp => Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, message),
            Refused::Failed(_) => Self::internal(message),
        }
    }
}
