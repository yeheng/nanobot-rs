//! Common webhook handler utilities

use axum::{
    body::Body,
    http::{header, Response, StatusCode},
};
use tracing::debug;

/// Create a plain text response
pub fn text_response(status: StatusCode, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Create a JSON response
pub fn json_response<T: serde::Serialize>(status: StatusCode, body: &T) -> Response<Body> {
    let json = match serde_json::to_string(body) {
        Ok(j) => j,
        Err(e) => {
            debug!("Failed to serialize JSON response: {}", e);
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Internal Server Error"))
                .unwrap();
        }
    };

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json))
        .unwrap()
}

/// Create an XML response
#[allow(dead_code)]
pub fn xml_response(status: StatusCode, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Success response helper
pub fn success(body: &str) -> Response<Body> {
    text_response(StatusCode::OK, body)
}

/// Error response helper
pub fn error(status: StatusCode, message: &str) -> Response<Body> {
    text_response(status, message)
}

/// Bad request response helper
pub fn bad_request(message: &str) -> Response<Body> {
    error(StatusCode::BAD_REQUEST, message)
}

/// Unauthorized response helper
#[allow(dead_code)]
pub fn unauthorized(message: &str) -> Response<Body> {
    error(StatusCode::UNAUTHORIZED, message)
}

/// Internal server error response helper
#[allow(dead_code)]
pub fn internal_error(message: &str) -> Response<Body> {
    error(StatusCode::INTERNAL_SERVER_ERROR, message)
}
