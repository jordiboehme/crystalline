//! The OpenAI-compatible remote embedding provider.
//!
//! Posts `{model, input: [texts]}` to `{endpoint}/embeddings` and reads
//! `data[].embedding`, ordered by `data[].index`. The API key comes from the
//! configured environment variable. The dimensionality is not known up front; it
//! is taken from the first response and cached. There is a single request per
//! batch with a request timeout and no retry beyond that one attempt: the M5
//! daemon queue owns backoff.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use crystalline_core::config::EmbeddingsConfig;
use serde::{Deserialize, Serialize};

use super::EmbeddingProvider;
use crate::error::{IndexError, Result};

/// The per-request timeout for the remote endpoint.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// An OpenAI-compatible embedding provider.
pub struct RemoteProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: Option<String>,
    dims: Mutex<usize>,
}

impl RemoteProvider {
    /// Build a provider from its configuration, reading the API key from the
    /// configured environment variable when one is named.
    pub fn from_config(cfg: &EmbeddingsConfig) -> Result<RemoteProvider> {
        let endpoint = cfg
            .endpoint
            .as_deref()
            .filter(|e| !e.trim().is_empty())
            .ok_or_else(|| {
                IndexError::Invalid(
                    "openai-compatible embeddings require an 'endpoint' in the config".into(),
                )
            })?
            .trim_end_matches('/')
            .to_string();

        if cfg.model.trim().is_empty() {
            return Err(IndexError::Invalid(
                "openai-compatible embeddings require a 'model' in the config".into(),
            ));
        }

        let api_key = match &cfg.api_key_env {
            Some(var) => Some(std::env::var(var).map_err(|_| {
                IndexError::Invalid(format!(
                    "embeddings api key environment variable '{var}' is not set"
                ))
            })?),
            None => None,
        };

        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;

        Ok(RemoteProvider {
            client,
            endpoint,
            model: cfg.model.clone(),
            api_key,
            dims: Mutex::new(0),
        })
    }

    fn url(&self) -> String {
        format!("{}/embeddings", self.endpoint)
    }
}

#[async_trait]
impl EmbeddingProvider for RemoteProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let request = EmbeddingRequest {
            model: &self.model,
            input: texts,
        };
        let mut builder = self.client.post(self.url()).json(&request);
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }

        let response = builder.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let body = body.chars().take(500).collect::<String>();
            return Err(IndexError::Remote(format!(
                "endpoint returned {status}: {body}"
            )));
        }

        let parsed: EmbeddingResponse = response.json().await?;
        let mut data = parsed.data;
        if data.len() != texts.len() {
            return Err(IndexError::Remote(format!(
                "endpoint returned {} embeddings for {} inputs",
                data.len(),
                texts.len()
            )));
        }
        data.sort_by_key(|d| d.index);
        let vectors: Vec<Vec<f32>> = data.into_iter().map(|d| d.embedding).collect();

        if let Some(first) = vectors.first() {
            *self.dims.lock().unwrap() = first.len();
        }
        Ok(vectors)
    }

    fn model_id(&self) -> &str {
        &self.model
    }

    fn dims(&self) -> usize {
        *self.dims.lock().unwrap()
    }

    fn max_input_tokens(&self) -> usize {
        // A conservative budget shared by the common OpenAI-compatible models;
        // chunk packing stays well under any endpoint's real limit.
        8192
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    #[serde(default)]
    index: usize,
}
