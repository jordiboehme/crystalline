//! The OpenAI-compatible remote embedding provider.
//!
//! Posts `{model, input: [texts]}` to `{endpoint}/embeddings` and reads
//! `data[].embedding`, ordered by `data[].index`. The API key comes from the
//! configured environment variable. The dimensionality is not known up front; it
//! is taken from the first response and cached. A transient failure of the POST
//! is retried with bounded backoff (see [`RemoteProvider::embed`]) so one blip
//! does not abort a whole embedding pass; a non-transient failure fails at once.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use crystalline_core::config::EmbeddingsConfig;
use serde::{Deserialize, Serialize};

use super::EmbeddingProvider;
use crate::error::{IndexError, Result};

/// The per-request timeout for the remote endpoint.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The first retry's backoff, doubled on each subsequent retry (500 ms, 1 s,
/// 2 s). See the retry policy on [`RemoteProvider::embed`].
const BACKOFF_BASE: Duration = Duration::from_millis(500);

/// The most retries a transient failure earns, beyond the initial attempt, so a
/// wedged endpoint aborts the pass instead of spinning forever.
const MAX_RETRIES: u32 = 3;

/// The ceiling on the summed backoff across all retries. A `Retry-After` the
/// endpoint asks for is honored only up to what keeps the total under this
/// bound, so a large `Retry-After` cannot stall the pass indefinitely.
const MAX_TOTAL_BACKOFF: Duration = Duration::from_secs(15);

/// An OpenAI-compatible embedding provider.
pub struct RemoteProvider {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: Option<String>,
    dims: Mutex<usize>,
    /// The first retry backoff, doubled on each further retry. Production uses
    /// [`BACKOFF_BASE`]; tests inject a near-zero value so the suite never
    /// waits on real backoff.
    backoff_base: Duration,
}

impl RemoteProvider {
    /// Build a provider from its configuration, reading the API key from the
    /// configured environment variable when one is named.
    pub fn from_config(cfg: &EmbeddingsConfig) -> Result<RemoteProvider> {
        Self::build(cfg, BACKOFF_BASE)
    }

    /// The shared constructor behind [`from_config`](Self::from_config), taking
    /// the retry backoff base so tests drive it near zero. The public API only
    /// ever supplies [`BACKOFF_BASE`].
    fn build(cfg: &EmbeddingsConfig, backoff_base: Duration) -> Result<RemoteProvider> {
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
            backoff_base,
        })
    }

    fn url(&self) -> String {
        format!("{}/embeddings", self.endpoint)
    }

    /// One POST to the endpoint, classified into success, a transient failure
    /// worth retrying or a fatal one. The caller ([`embed`](Self::embed)) owns
    /// the retry loop and the dims cache.
    async fn embed_once(
        &self,
        request: &EmbeddingRequest<'_>,
        inputs: usize,
    ) -> std::result::Result<Vec<Vec<f32>>, AttemptOutcome> {
        let mut builder = self.client.post(self.url()).json(request);
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }

        let response = match builder.send().await {
            Ok(response) => response,
            Err(e) => {
                // A connect failure or a timeout is transient: the endpoint may
                // be restarting or briefly unreachable.
                let outcome = if e.is_timeout() || e.is_connect() {
                    AttemptOutcome::Transient {
                        err: e.into(),
                        retry_after: None,
                    }
                } else {
                    AttemptOutcome::Fatal(e.into())
                };
                return Err(outcome);
            }
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            // 429 and every 5xx are transient; the endpoint is rate-limiting or
            // briefly unhealthy. Only a rate-limit-style response carries a
            // Retry-After the endpoint wants honored.
            let transient = code == 429 || status.is_server_error();
            let retry_after = if code == 429 || code == 503 {
                parse_retry_after(response.headers())
            } else {
                None
            };
            let body = response.text().await.unwrap_or_default();
            let body = body.chars().take(500).collect::<String>();
            let err = IndexError::Remote(format!("endpoint returned {status}: {body}"));
            return Err(if transient {
                AttemptOutcome::Transient { err, retry_after }
            } else {
                AttemptOutcome::Fatal(err)
            });
        }

        let parsed: EmbeddingResponse = match response.json().await {
            Ok(parsed) => parsed,
            // A malformed body is the endpoint's fault, not a blip; replaying it
            // would only fail the same way.
            Err(e) => return Err(AttemptOutcome::Fatal(e.into())),
        };
        let mut data = parsed.data;
        if data.len() != inputs {
            return Err(AttemptOutcome::Fatal(IndexError::Remote(format!(
                "endpoint returned {} embeddings for {} inputs",
                data.len(),
                inputs
            ))));
        }
        data.sort_by_key(|d| d.index);
        Ok(data.into_iter().map(|d| d.embedding).collect())
    }
}

/// One attempt's failure, split by whether replaying the idempotent POST could
/// plausibly succeed.
enum AttemptOutcome {
    /// Worth retrying: a 429, a 5xx, a connect error or a timeout. Carries the
    /// `Retry-After` the endpoint asked for when it sent one.
    Transient {
        err: IndexError,
        retry_after: Option<Duration>,
    },
    /// Not worth retrying: any other 4xx or a malformed response.
    Fatal(IndexError),
}

/// Parse a `Retry-After` header in delta-seconds form. The HTTP-date form is not
/// read; endpoints that rate-limit an API answer in seconds in practice.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?;
    let secs: u64 = value.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(secs))
}

#[async_trait]
impl EmbeddingProvider for RemoteProvider {
    /// Retry policy: a transient failure (HTTP 429, any 5xx, a connect error or
    /// a timeout) is retried up to [`MAX_RETRIES`] times with backoff doubling
    /// from [`BACKOFF_BASE`]; a 429 or 503 that carries a `Retry-After` in
    /// seconds waits the larger of that and the backoff. The summed wait is
    /// capped at [`MAX_TOTAL_BACKOFF`]. Any other 4xx and a malformed response
    /// are non-transient and fail on the first attempt. The POST is idempotent,
    /// so replaying it is safe.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let request = EmbeddingRequest {
            model: &self.model,
            input: texts,
        };

        let mut waited = Duration::ZERO;
        let mut retries = 0u32;
        loop {
            let (err, retry_after) = match self.embed_once(&request, texts.len()).await {
                Ok(vectors) => {
                    if let Some(first) = vectors.first() {
                        *self.dims.lock().unwrap() = first.len();
                    }
                    return Ok(vectors);
                }
                Err(AttemptOutcome::Fatal(err)) => return Err(err),
                Err(AttemptOutcome::Transient { err, retry_after }) => (err, retry_after),
            };
            if retries >= MAX_RETRIES {
                return Err(err);
            }
            let backoff = self.backoff_base * (1u32 << retries);
            let wanted = match retry_after {
                Some(after) => after.max(backoff),
                None => backoff,
            };
            // Never let the summed wait cross the ceiling: a wedged endpoint
            // asking for a long Retry-After cannot stall the pass indefinitely.
            let wait = wanted.min(MAX_TOTAL_BACKOFF.saturating_sub(waited));
            tokio::time::sleep(wait).await;
            waited += wait;
            retries += 1;
        }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use tokio::net::TcpListener;

    use super::*;

    /// Serve `router` on an ephemeral localhost port and return its base URL.
    async fn spawn(router: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}")
    }

    /// A provider pointed at `base` with a near-zero backoff so the retry tests
    /// never wait on real time; only the endpoint's Retry-After (kept at 0 in
    /// the stubs) and this base contribute to the wait, both effectively zero.
    fn provider(base: &str) -> RemoteProvider {
        let cfg = EmbeddingsConfig {
            provider: "openai".to_string(),
            model: "test-model".to_string(),
            endpoint: Some(base.to_string()),
            api_key_env: None,
        };
        RemoteProvider::build(&cfg, Duration::from_millis(0)).unwrap()
    }

    /// One embedding for one input, the success shape the provider expects.
    fn ok_body() -> Response {
        axum::Json(serde_json::json!({
            "data": [{ "embedding": [0.1f32, 0.2, 0.3], "index": 0 }]
        }))
        .into_response()
    }

    // A transient 429 that clears: the first two attempts are rate-limited with
    // `Retry-After: 0`, the third succeeds. The pass must recover after exactly
    // three requests, proving the two retries happened and then stopped.
    #[tokio::test]
    async fn retries_transient_429_then_succeeds() {
        let count = Arc::new(AtomicUsize::new(0));
        async fn handler(State(count): State<Arc<AtomicUsize>>) -> Response {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "0")],
                    "slow down",
                )
                    .into_response()
            } else {
                ok_body()
            }
        }
        let app = Router::new()
            .route("/embeddings", post(handler))
            .with_state(count.clone());
        let base = spawn(app).await;

        let vectors = provider(&base)
            .embed(&["hello".to_string()])
            .await
            .expect("a transient 429 that clears must yield success");
        assert_eq!(vectors.len(), 1);
        assert_eq!(
            count.load(Ordering::SeqCst),
            3,
            "two retries after two 429s, then the success"
        );
    }

    // A 400 is a client fault a retry cannot fix: fail on the first request,
    // never retry.
    #[tokio::test]
    async fn does_not_retry_client_error_400() {
        let count = Arc::new(AtomicUsize::new(0));
        async fn handler(State(count): State<Arc<AtomicUsize>>) -> Response {
            count.fetch_add(1, Ordering::SeqCst);
            (StatusCode::BAD_REQUEST, "bad input").into_response()
        }
        let app = Router::new()
            .route("/embeddings", post(handler))
            .with_state(count.clone());
        let base = spawn(app).await;

        let err = provider(&base)
            .embed(&["hello".to_string()])
            .await
            .expect_err("a 400 must fail immediately");
        assert!(matches!(err, IndexError::Remote(_)));
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "a non-transient status is never retried"
        );
    }

    // A persistent 503 exhausts the retry budget: four requests total (the
    // first plus three retries), then the last transient error surfaces.
    #[tokio::test]
    async fn exhausts_retries_on_persistent_503() {
        let count = Arc::new(AtomicUsize::new(0));
        async fn handler(State(count): State<Arc<AtomicUsize>>) -> Response {
            count.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [("retry-after", "0")],
                "unavailable",
            )
                .into_response()
        }
        let app = Router::new()
            .route("/embeddings", post(handler))
            .with_state(count.clone());
        let base = spawn(app).await;

        let err = provider(&base)
            .embed(&["hello".to_string()])
            .await
            .expect_err("a persistent 503 must fail after the retry budget");
        assert!(matches!(err, IndexError::Remote(_)));
        assert_eq!(
            count.load(Ordering::SeqCst),
            4,
            "the first request plus three retries"
        );
    }
}
