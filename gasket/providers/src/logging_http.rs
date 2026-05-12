//! Logging HTTP client wrapper for rig's `HttpClientExt`.
//!
//! Wraps a `reqwest::Client` to emit `tracing::debug!` lines for every
//! request/response that goes through rig's HTTP layer. Pass an instance
//! to any rig `ClientBuilder::http_client()` to get visibility into
//! what URLs rig is hitting.
//!
//! Also supports injecting custom HTTP headers on every outgoing request.

use bytes::Bytes;
use reqwest::header::{HeaderName, HeaderValue};
use rig::http_client::{
    self, HttpClientExt, LazyBody, MultipartForm, Request, Response, StreamingResponse,
};
use rig::wasm_compat::WasmCompatSend;
use std::collections::HashMap;
use std::future::Future;

/// HTTP client that logs every request/response through rig.
///
/// Delegates all calls to the inner `reqwest::Client` after emitting
/// `tracing::debug!` with method, URI, and response status.
///
/// Custom headers can be configured via [`Self::with_extra_headers`] and will
/// be injected into every request before it is sent.
#[derive(Clone, Default, Debug)]
pub struct LoggingHttpClient {
    inner: reqwest::Client,
    extra_headers: HashMap<String, String>,
}

impl LoggingHttpClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            inner: client,
            extra_headers: HashMap::new(),
        }
    }

    /// Attach extra HTTP headers that will be injected into every request.
    pub fn with_extra_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.extra_headers = headers;
        self
    }

    /// Inject configured extra headers into the request.
    fn inject_headers<T>(&self, mut req: Request<T>) -> Request<T> {
        for (k, v) in &self.extra_headers {
            let Ok(name) = HeaderName::from_bytes(k.as_bytes()) else {
                tracing::warn!("Skipping invalid extra header name: {}", k);
                continue;
            };
            let Ok(value) = HeaderValue::from_str(v) else {
                tracing::warn!("Skipping invalid extra header value for {}: {}", k, v);
                continue;
            };
            req.headers_mut().insert(name, value);
        }
        req
    }
}

impl HttpClientExt for LoggingHttpClient {
    fn send<T, U>(
        &self,
        req: Request<T>,
    ) -> impl Future<Output = http_client::Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        T: Into<Bytes> + WasmCompatSend,
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        let req = self.inject_headers(req);
        let method = req.method().clone();
        let uri = req.uri().clone();
        let fut = self.inner.send(req);
        async move {
            tracing::debug!(method = %method, uri = %uri, "rig HTTP →");
            let result = fut.await;
            match &result {
                Ok(resp) => tracing::debug!(status = %resp.status(), uri = %uri, "rig HTTP ←"),
                Err(e) => tracing::debug!(error = %e, uri = %uri, "rig HTTP ✗"),
            }
            result
        }
    }

    fn send_multipart<U>(
        &self,
        req: Request<MultipartForm>,
    ) -> impl Future<Output = http_client::Result<Response<LazyBody<U>>>> + WasmCompatSend + 'static
    where
        U: From<Bytes> + WasmCompatSend + 'static,
    {
        let req = self.inject_headers(req);
        let method = req.method().clone();
        let uri = req.uri().clone();
        let fut = self.inner.send_multipart(req);
        async move {
            tracing::debug!(method = %method, uri = %uri, "rig HTTP (multipart) →");
            let result = fut.await;
            match &result {
                Ok(resp) => tracing::debug!(status = %resp.status(), uri = %uri, "rig HTTP (multipart) ←"),
                Err(e) => tracing::debug!(error = %e, uri = %uri, "rig HTTP (multipart) ✗"),
            }
            result
        }
    }

    fn send_streaming<T>(
        &self,
        req: Request<T>,
    ) -> impl Future<Output = http_client::Result<StreamingResponse>> + WasmCompatSend
    where
        T: Into<Bytes> + WasmCompatSend,
    {
        let req = self.inject_headers(req);
        let method = req.method().clone();
        let uri = req.uri().clone();
        let fut = self.inner.send_streaming(req);
        async move {
            tracing::debug!(method = %method, uri = %uri, "rig HTTP (stream) →");
            let result = fut.await;
            match &result {
                Ok(resp) => tracing::debug!(status = %resp.status(), uri = %uri, "rig HTTP (stream) ←"),
                Err(e) => tracing::debug!(error = %e, uri = %uri, "rig HTTP (stream) ✗"),
            }
            result
        }
    }
}
