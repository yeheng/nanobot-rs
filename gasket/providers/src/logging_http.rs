//! Logging HTTP client wrapper for rig's `HttpClientExt`.
//!
//! Wraps a `reqwest::Client` to emit `tracing::debug!` lines for every
//! request/response that goes through rig's HTTP layer. Pass an instance
//! to any rig `ClientBuilder::http_client()` to get visibility into
//! what URLs rig is hitting.

use bytes::Bytes;
use rig::http_client::{
    self, HttpClientExt, LazyBody, MultipartForm, Request, Response, StreamingResponse,
};
use rig::wasm_compat::WasmCompatSend;
use std::future::Future;

/// HTTP client that logs every request/response through rig.
///
/// Delegates all calls to the inner `reqwest::Client` after emitting
/// `tracing::debug!` with method, URI, and response status.
#[derive(Clone, Default, Debug)]
pub struct LoggingHttpClient {
    inner: reqwest::Client,
}

impl LoggingHttpClient {
    pub fn new(client: reqwest::Client) -> Self {
        Self { inner: client }
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
