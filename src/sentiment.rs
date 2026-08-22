//! Optional market sentiment provider.

use std::{future::Future, pin::Pin, time::Duration};

use serde::Deserialize;
use url::Url;

use crate::{api::ApiError, http::HttpClient};

const DEFAULT_URL: &str = "https://api.alternative.me/fng/?limit=1&format=json";
const MAX_BODY_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct FearGreedIndex {
    pub value: u8,
    pub classification: String,
}

pub trait FearGreedProvider: Send + Sync {
    fn fetch<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<FearGreedIndex, ApiError>> + Send + 'a>>;
}

pub struct AlternativeMeClient {
    client: HttpClient,
    url: Url,
}

impl AlternativeMeClient {
    pub fn new() -> Result<Self, ApiError> {
        let client = HttpClient::new(Duration::from_secs(10), Duration::from_secs(20))?;
        Ok(Self {
            client,
            url: Url::parse(DEFAULT_URL).map_err(|_| ApiError::InvalidBaseUrl)?,
        })
    }

    async fn fetch_index(&self) -> Result<FearGreedIndex, ApiError> {
        let body = self
            .client
            .get(self.url.clone(), &[], None, MAX_BODY_BYTES, true)
            .await?;
        let response: Response =
            serde_json::from_slice(&body).map_err(|_| ApiError::MalformedResponse)?;
        let item = response
            .data
            .into_iter()
            .next()
            .ok_or(ApiError::MalformedResponse)?;
        let value = item
            .value
            .parse::<u8>()
            .map_err(|_| ApiError::MalformedResponse)?;
        if value > 100 || item.classification.is_empty() {
            return Err(ApiError::MalformedResponse);
        }
        Ok(FearGreedIndex {
            value,
            classification: item.classification,
        })
    }
}

impl FearGreedProvider for AlternativeMeClient {
    fn fetch<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<FearGreedIndex, ApiError>> + Send + 'a>> {
        Box::pin(self.fetch_index())
    }
}

#[derive(Deserialize)]
struct Response {
    data: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    value: String,
    #[serde(rename = "value_classification")]
    classification: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternative_me_payload_is_bounded_and_typed() {
        let response: Response =
            serde_json::from_str(r#"{"data":[{"value":"72","value_classification":"Greed"}]}"#)
                .unwrap();
        let item = response.data.into_iter().next().unwrap();
        assert_eq!(item.value.parse::<u8>().unwrap(), 72);
        assert_eq!(item.classification, "Greed");
    }
}
