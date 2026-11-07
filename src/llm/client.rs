use super::{Connection, stream::Decoder};
use anyhow::{Context, Result, ensure};
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(super) async fn interruptible<T>(
    cancel: &AtomicBool,
    operation: impl Future<Output = Result<T>>,
) -> Result<T> {
    let operation = tokio::time::timeout(Duration::from_secs(120), operation);
    tokio::pin!(operation);
    loop {
        ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
        if let Ok(result) = tokio::time::timeout(Duration::from_millis(50), &mut operation).await {
            return result.context("Server timed out")?;
        }
    }
}

pub(super) struct Client {
    http: reqwest::Client,
    base: reqwest::Url,
    token: String,
    cancel: Arc<AtomicBool>,
}
impl Client {
    pub fn new(connection: Connection, token: String, cancel: Arc<AtomicBool>) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .connect_timeout(Duration::from_secs(10))
                .build()?,
            base: connection.base_url()?,
            token,
            cancel,
        })
    }
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let request = self
            .http
            .request(method, self.base.join(path).expect("static API path"));
        if self.token.is_empty() {
            request
        } else {
            request.bearer_auth(&self.token)
        }
    }
    async fn response(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let response = interruptible(&self.cancel, async {
            request.send().await.map_err(|_| {
                anyhow::anyhow!(
                    "Could not connect to the LLM server; check address, TLS and network"
                )
            })
        })
        .await?;
        ensure!(
            response.status().is_success(),
            "LLM server returned HTTP {}{}",
            response.status().as_u16(),
            match response.status().as_u16() {
                401 | 403 => " (check the auth token)",
                404 => " (API endpoint not found)",
                _ => "",
            }
        );
        Ok(response)
    }
    pub async fn model(&self) -> Result<String> {
        let mut response = self
            .response(self.request(reqwest::Method::GET, "v1/models"))
            .await?;
        let mut bytes = Vec::new();
        loop {
            let chunk = interruptible(&self.cancel, async {
                response.chunk().await.context("Could not read model list")
            })
            .await?;
            let Some(chunk) = chunk else {
                break;
            };
            ensure!(
                bytes.len() + chunk.len() <= 1024 * 1024,
                "Model list too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("Invalid model list")?;
        value
            .pointer("/data/0/id")
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .context("The server has no loaded model")
    }
    pub async fn generate(
        &self,
        model: &str,
        prompt: String,
        text: String,
        mut emit: impl AsyncFnMut(String) -> Result<()>,
    ) -> Result<()> {
        let request = self
            .request(reqwest::Method::POST, "v1/chat/completions")
            .json(
                &serde_json::json!({"model": model, "stream": true, "messages": [
                    {"role": "system", "content": prompt},
                    {"role": "user", "content": text}
                ]}),
            );
        let mut response = self.response(request).await?;
        let mut decoder = Decoder::default();
        let mut output_bytes = 0usize;
        loop {
            let chunk = interruptible(&self.cancel, async {
                response
                    .chunk()
                    .await
                    .context("Could not read generation stream")
            })
            .await?;
            let deltas = match chunk {
                Some(chunk) => decoder.push(&chunk)?,
                None => decoder.finish()?,
            };
            for text in deltas {
                output_bytes += text.len();
                ensure!(
                    output_bytes <= 512 * 1024,
                    "Response limit"
                );
                emit(text).await?;
            }
            if decoder.done {
                return Ok(());
            }
        }
    }
}
