//! Shared bounded HTTP transport for provider adapters.

use std::time::Duration;

use reqwest::{header, redirect::Policy, StatusCode, Url};

use crate::api::ApiError;

const USER_AGENT: &str = "coin-tui/0.1";

pub(crate) struct HttpClient {
    client: reqwest::Client,
    total_timeout: Duration,
}

impl HttpClient {
    pub(crate) fn new(
        connect_timeout: Duration,
        total_timeout: Duration,
    ) -> Result<Self, ApiError> {
        if tokio::time::Instant::now()
            .checked_add(total_timeout)
            .is_none()
        {
            return Err(ApiError::InvalidTimeoutConfiguration);
        }
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .redirect(Policy::none())
            .connect_timeout(connect_timeout)
            .build()
            .map_err(|_| ApiError::Transport)?;
        Ok(Self {
            client,
            total_timeout,
        })
    }

    pub(crate) async fn get(
        &self,
        url: Url,
        query: &[(&str, &str)],
        api_key: Option<&str>,
        max_bytes: usize,
        require_json: bool,
    ) -> Result<Vec<u8>, ApiError> {
        tokio::time::timeout(self.total_timeout, async {
            let mut request = self.client.get(url).query(query);
            if let Some(key) = api_key {
                request = request.header("x-cg-demo-api-key", key);
            }
            let response = request.send().await.map_err(classify_request_error)?;
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                return Err(ApiError::RateLimited {
                    retry_after: parse_retry_after(response.headers()),
                });
            }
            if !response.status().is_success() {
                return Err(ApiError::HttpStatus {
                    status: response.status().as_u16(),
                });
            }
            if require_json && !is_json_content_type(response.headers()) {
                return Err(ApiError::MalformedResponse);
            }
            read_bounded(response, max_bytes).await
        })
        .await
        .map_err(|_| ApiError::Timeout)?
    }
}

pub(crate) fn validate_url(raw: &str) -> Result<Url, ApiError> {
    let url = Url::parse(raw).map_err(|_| ApiError::InvalidBaseUrl)?;
    let allowed_http = url.scheme() == "http" && url.host().map(is_loopback_host).unwrap_or(false);
    if url.scheme() != "https" && !allowed_http
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ApiError::InvalidBaseUrl);
    }
    Ok(url)
}

async fn read_bounded(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, ApiError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(classify_request_error)? {
        if chunk.len() > max_bytes.saturating_sub(body.len()) {
            return Err(ApiError::MalformedResponse);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn classify_request_error(error: reqwest::Error) -> ApiError {
    if error.is_timeout() {
        ApiError::Timeout
    } else {
        ApiError::Transport
    }
}

fn parse_retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    let value = headers.get(header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(
        date.duration_since(std::time::SystemTime::now())
            .unwrap_or_default(),
    )
}

fn is_json_content_type(headers: &header::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn is_loopback_host(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address == std::net::Ipv4Addr::LOCALHOST,
        url::Host::Ipv6(address) => address.is_loopback(),
    }
}
