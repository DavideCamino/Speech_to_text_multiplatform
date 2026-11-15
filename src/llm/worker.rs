use super::{Connection, client::Client};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
};

pub enum Event {
    Text(String),
    Models(Vec<super::Model>),
    Status(String),
    Finished(Result<String, String>),
}
pub struct Job {
    pub events: Receiver<Event>,
    pub cancel: Arc<AtomicBool>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
async fn send(
    tx: &SyncSender<Event>,
    mut event: Event,
    cancel: &AtomicBool,
    terminal: bool,
) -> anyhow::Result<()> {
    loop {
        if !terminal && cancel.load(Ordering::Relaxed) {
            anyhow::bail!("Cancelled");
        }
        match tx.try_send(event) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => anyhow::bail!("Window closed"),
            Err(TrySendError::Full(value)) => event = value,
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
/// None tests the connection; Some executes one rewrite with a settings snapshot.
pub fn start(connection: Connection, token: String, rewrite: Option<(String, String)>) -> Job {
    spawn(connection, token, rewrite, None, false)
}

/// Discover available models, or load and rewrite using explicit selections.
pub fn start_configured(
    connection: Connection,
    token: String,
    rewrite: Option<(String, String)>,
    options: super::Options,
) -> Job {
    spawn(connection, token, rewrite, Some(options), false)
}

/// Load the selected model without sending transcript text or generating output.
pub fn start_loading(connection: Connection, token: String, options: super::Options) -> Job {
    spawn(connection, token, None, Some(options), true)
}

fn spawn(
    connection: Connection,
    token: String,
    rewrite: Option<(String, String)>,
    options: Option<super::Options>,
    load_only: bool,
) -> Job {
    let (tx, events) = mpsc::sync_channel(64);
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        match runtime {
            Err(_) => {
                let _ = tx.send(Event::Finished(
                    Err("Could not start network worker".into()),
                ));
            }
            Ok(runtime) => runtime.block_on(async {
                let result = async {
                    let client = Client::new(connection, token, flag.clone())?;
                    let mut reasoning = super::Reasoning::Default;
                    let model = if let Some(options) = options {
                        reasoning = options.reasoning;
                        if load_only {
                            let id = options.model.as_deref().ok_or_else(|| anyhow::anyhow!("Select an LLM model first"))?;
                            client.prepare_model(id).await?;
                            return Ok(format!("Model ready: {id}"));
                        }
                        let models = client.models().await?;
                        if rewrite.is_none() {
                            let count = models.len();
                            send(&tx, Event::Models(models), &flag, false).await?;
                            return Ok(format!("Connected. {count} available model(s)."));
                        }
                        let id = options.model.as_deref().ok_or_else(|| {
                            anyhow::anyhow!("Select an LLM model in Settings first")
                        })?;
                        let model = models.iter().find(|m| m.id == id).ok_or_else(|| {
                            anyhow::anyhow!("Selected model is unavailable; refresh the model list")
                        })?;
                        if model.needs_load {
                            anyhow::bail!("Model is not ready. Open Settings → LLM and click Apply to load it first");
                        }
                        send(&tx, Event::Status("Generating…".into()), &flag, false).await?;
                        id.to_owned()
                    } else {
                        client.model().await?
                    };
                    if let Some((prompt, text)) = rewrite {
                        client
                            .generate(&model, reasoning, prompt, text, async |text| {
                                send(&tx, Event::Text(text), &flag, false).await
                            })
                            .await?;
                        Ok("Rewrite complete.".to_owned())
                    } else {
                        Ok(format!("Connected. Loaded model: {model}"))
                    }
                }
                .await
                .map_err(|error: anyhow::Error| format!("{error:#}"));
                let _ = send(&tx, Event::Finished(result), &flag, true).await;
            }),
        }
    });
    Job { events, cancel }
}
