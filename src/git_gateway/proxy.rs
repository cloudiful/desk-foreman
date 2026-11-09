//! Bounded streaming proxy for the public native Git Smart HTTP routes.

use std::{io, sync::OnceLock};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::stream::{self, StreamExt};
use reqwest::{Client, Method, Url, redirect::Policy};
use tokio::time::Instant;
use tokio::time::sleep_until;

use crate::{AppState, config::GitGatewayConfig};

use super::limits::{declared_content_length, exceeds_limit};

static CLIENT: OnceLock<Client> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GatewayRoute {
    InfoRefs(&'static str),
    UploadPack,
    ReceivePack,
}

impl GatewayRoute {
    fn suffix(self) -> String {
        match self {
            Self::InfoRefs(service) => format!("/info/refs?service={service}"),
            Self::UploadPack => "/git-upload-pack".to_string(),
            Self::ReceivePack => "/git-receive-pack".to_string(),
        }
    }

    fn method(self) -> Method {
        match self {
            Self::InfoRefs(_) => Method::GET,
            Self::UploadPack | Self::ReceivePack => Method::POST,
        }
    }
}

pub(crate) struct UpstreamTarget {
    pub url: Url,
    pub username: Option<String>,
    pub ciphertext: Option<Vec<u8>>,
    pub workspace_binding_id: i64,
    pub upstream_id: i64,
}

pub(crate) fn parse_target(target_url: &str, route: GatewayRoute) -> anyhow::Result<Url> {
    let base = Url::parse(target_url)?;
    if !matches!(base.scheme(), "http" | "https") {
        anyhow::bail!("stored Git target must use http or https");
    }
    if base.host_str().is_none() {
        anyhow::bail!("stored Git target must include a host");
    }
    if !base.username().is_empty() || base.password().is_some() {
        anyhow::bail!("stored Git target must not include user information");
    }
    if base.query().is_some() || base.fragment().is_some() {
        anyhow::bail!("stored Git target must not include a query or fragment");
    }
    let joined = format!("{}{}", target_url.trim_end_matches('/'), route.suffix());
    Url::parse(&joined).map_err(Into::into)
}

fn client(config: &GitGatewayConfig) -> anyhow::Result<&'static Client> {
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let built = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(config.connect_timeout)
        .build()?;
    let _ = CLIENT.set(built);
    CLIENT
        .get()
        .ok_or_else(|| anyhow::anyhow!("failed to initialize Git gateway client"))
}

pub(crate) async fn forward(
    state: &AppState,
    route: GatewayRoute,
    headers: &HeaderMap,
    body: Option<Body>,
    target: UpstreamTarget,
) -> Response {
    let config = state.config.git_gateway.clone();
    if exceeds_limit(declared_content_length(headers), config.max_body_bytes) {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, "request body too large");
    }

    let client = match client(&config) {
        Ok(client) => client,
        Err(_) => {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "git gateway unavailable");
        }
    };
    let mut request = client.request(route.method(), target.url);
    for (name, value) in forwarded_headers(headers) {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.body(counted_request_body(body, config.max_body_bytes));
    }
    if let Some(username) = target.username.as_deref() {
        let Some(ciphertext) = target.ciphertext.as_deref() else {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "git gateway unavailable");
        };
        let Some(root_key) = state.config.upstream_credential_key.as_ref() else {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "git gateway unavailable");
        };
        let Ok(secret) =
            root_key.decrypt(target.workspace_binding_id, target.upstream_id, ciphertext)
        else {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "git gateway unavailable");
        };
        let Ok(secret) = String::from_utf8(secret) else {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "git gateway unavailable");
        };
        request = request.basic_auth(username.to_string(), Some(secret));
    }

    let response = match tokio::time::timeout(config.head_timeout, request.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => return error_response(StatusCode::BAD_GATEWAY, "upstream request failed"),
        Err(_) => return error_response(StatusCode::GATEWAY_TIMEOUT, "upstream request timed out"),
    };
    let status = response.status();
    if exceeds_limit(
        declared_content_length(response.headers()),
        config.max_body_bytes,
    ) {
        return error_response(StatusCode::BAD_GATEWAY, "upstream response too large");
    }
    if status.is_redirection() || status.is_server_error() {
        return error_response(StatusCode::BAD_GATEWAY, "upstream request failed");
    }
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return error_response(StatusCode::FORBIDDEN, "upstream access denied");
    }
    if status == StatusCode::NOT_FOUND {
        return error_response(StatusCode::NOT_FOUND, "upstream not found");
    }
    if !status.is_success() {
        return error_response(StatusCode::BAD_GATEWAY, "upstream request failed");
    }

    let mut builder = Response::builder().status(status);
    for (name, value) in forwarded_response_headers(response.headers()) {
        builder = builder.header(name, value);
    }
    let deadline = Instant::now() + config.stream_timeout;
    let upstream = response
        .bytes_stream()
        .map(|item| {
            item.map_err(|_| io::Error::new(io::ErrorKind::Other, "upstream stream failed"))
        })
        .take_until(sleep_until(deadline));
    builder
        .body(Body::from_stream(counted_response_stream(
            upstream,
            config.max_body_bytes,
        )))
        .unwrap_or_else(|_| error_response(StatusCode::BAD_GATEWAY, "upstream response failed"))
}

fn forwarded_headers(headers: &HeaderMap) -> Vec<(HeaderName, HeaderValue)> {
    headers
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "content-type" | "accept" | "git-protocol" | "content-encoding"
            )
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn forwarded_response_headers(headers: &HeaderMap) -> Vec<(HeaderName, HeaderValue)> {
    headers
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "content-type" | "content-encoding" | "cache-control" | "expires" | "pragma"
            )
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn counted_request_body(body: Body, max: usize) -> reqwest::Body {
    let stream = body
        .into_data_stream()
        .map(|item| item.map_err(|_| io::Error::new(io::ErrorKind::Other, "request body failed")));
    let counted = stream::unfold(
        (Box::pin(stream), 0usize),
        move |(mut inner, seen)| async move {
            match inner.next().await {
                None => None,
                Some(Ok(bytes)) => {
                    let next = seen.saturating_add(bytes.len());
                    if next > max {
                        Some((
                            Err(io::Error::new(
                                io::ErrorKind::Other,
                                "request body too large",
                            )),
                            (inner, next),
                        ))
                    } else {
                        Some((Ok(bytes), (inner, next)))
                    }
                }
                Some(Err(error)) => Some((Err(error), (inner, seen))),
            }
        },
    );
    reqwest::Body::wrap_stream(counted)
}

fn counted_response_stream<S>(
    stream: S,
    max: usize,
) -> impl futures_util::Stream<Item = io::Result<Bytes>>
where
    S: futures_util::Stream<Item = io::Result<Bytes>> + Send + 'static,
{
    stream::unfold(
        (Box::pin(stream), 0usize),
        move |(mut inner, seen)| async move {
            match inner.next().await {
                None => None,
                Some(Ok(bytes)) => {
                    let next = seen.saturating_add(bytes.len());
                    if next > max {
                        Some((
                            Err(io::Error::new(
                                io::ErrorKind::Other,
                                "response body too large",
                            )),
                            (inner, next),
                        ))
                    } else {
                        Some((Ok(bytes), (inner, next)))
                    }
                }
                Some(Err(error)) => Some((Err(error), (inner, seen))),
            }
        },
    )
}

pub(crate) fn error_response(status: StatusCode, message: &'static str) -> Response {
    let mut response = (status, message).into_response();
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"DF Git\""),
        );
    }
    response
}
