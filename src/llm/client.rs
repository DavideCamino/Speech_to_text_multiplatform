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
    pub async fn models(&self) -> Result<Vec<super::Model>> {
        let response = interruptible(&self.cancel, async {
            self.request(reqwest::Method::GET, "models")
                .send()
                .await
                .context("Could not list server models")
        })
        .await?;
        let mut response = if response.status() == reqwest::StatusCode::NOT_FOUND {
            self.response(self.request(reqwest::Method::GET, "v1/models"))
                .await?
        } else {
            ensure!(
                response.status().is_success(),
                "Model discovery returned HTTP {}",
                response.status().as_u16()
            );
            response
        };
        let mut bytes = Vec::new();
        while let Some(chunk) = interruptible(&self.cancel, async {
            response.chunk().await.context("Could not read model list")
        })
        .await?
        {
            ensure!(
                bytes.len() + chunk.len() <= 1024 * 1024,
                "Model list too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("Invalid model list")?;
        let data = value["data"].as_array().context("Invalid model list")?;
        let mut models = Vec::new();
        for item in data {
            let id = item["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .context("Invalid model ID")?;
            if !models.iter().any(|m: &super::Model| m.id == id) {
                models.push(super::Model {
                    id: id.to_owned(),
                    loading: item.pointer("/status/value").and_then(|v| v.as_str())
                        == Some("loading"),
                    needs_load: item
                        .pointer("/status/value")
                        .and_then(|v| v.as_str())
                        .is_some_and(|status| status != "loaded" && status != "sleeping"),
                });
            }
        }
        Ok(models)
    }

    pub async fn prepare_model(&self, id: &str) -> Result<()> {
        let models = self.models().await?;
        let model = models
            .iter()
            .find(|m| m.id == id)
            .context("Selected model is unavailable; refresh the model list")?;
        if !model.needs_load {
            return Ok(());
        }
        if !model.loading
            && let Err(error) = self.load(id).await
        {
            // Another client may have started loading after discovery.
            let models = self.models().await?;
            if !models
                .iter()
                .any(|m| m.id == id && (m.loading || !m.needs_load))
            {
                return Err(error);
            }
        }
        interruptible(&self.cancel, async {
            loop {
                let models = self.models().await?;
                let model = models
                    .iter()
                    .find(|m| m.id == id)
                    .context("Selected model disappeared during loading")?;
                if !model.needs_load {
                    return Ok(());
                }
                ensure!(
                    model.loading,
                    "Selected model failed to load; check the llama server terminal"
                );
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
        .await
    }

    pub async fn load(&self, model: &str) -> Result<()> {
        let mut response = self
            .response(
                self.request(reqwest::Method::POST, "models/load")
                    .json(&serde_json::json!({"model": model})),
            )
            .await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = interruptible(&self.cancel, async {
            response
                .chunk()
                .await
                .context("Could not read model loading response")
        })
        .await?
        {
            ensure!(
                bytes.len() + chunk.len() <= 64 * 1024,
                "Model loading response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("Invalid model loading response")?;
        ensure!(
            value["success"] == true,
            "Server could not load selected model"
        );
        Ok(())
    }

    /// Compatibility lookup for the deprecated implicit-model worker.
    pub async fn legacy_first_model(&self) -> Result<String> {
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
        reasoning: super::Reasoning,
        prompt: String,
        text: String,
        mut emit: impl AsyncFnMut(String) -> Result<()>,
    ) -> Result<()> {
        let mut body = serde_json::json!({"model": model, "stream": true, "messages": [
            {"role": "system", "content": prompt},
            {"role": "user", "content": text}
        ]});
        if let Some(effort) = reasoning.parameter() {
            body["reasoning_effort"] = effort.into();
        }
        let request = self
            .request(reqwest::Method::POST, "v1/chat/completions")
            .json(&body);
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
                ensure!(output_bytes <= 512 * 1024, "Response limit");
                emit(text).await?;
            }
            if decoder.done {
                return Ok(());
            }
        }
    }
}
