use super::{Connection, Options, Reasoning, client::Client};
use anyhow::{Context, Result, ensure};
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
enum Operation {
    Discover,
    LoadSelected(Option<String>),
    Generate {
        prompt: String,
        text: String,
        options: Options,
    },
    // Compatibility only: use the first model from /v1/models without a readiness check.
    LegacyFirstModel(Option<(String, String)>),
}

/// Test the connection or rewrite using the server's first model.
#[deprecated(note = "Use start_configured to discover models or rewrite with an explicit model")]
pub fn start(connection: Connection, token: String, rewrite: Option<(String, String)>) -> Job {
    spawn(connection, token, Operation::LegacyFirstModel(rewrite))
}

/// Discover available models, or rewrite with an explicitly selected ready model.
pub fn start_configured(
    connection: Connection,
    token: String,
    rewrite: Option<(String, String)>,
    options: Options,
) -> Job {
    let operation = match rewrite {
        None => Operation::Discover,
        Some((prompt, text)) => Operation::Generate {
            prompt,
            text,
            options,
        },
    };
    spawn(connection, token, operation)
}

/// Load the selected model without sending transcript text or generating output.
pub fn start_loading(connection: Connection, token: String, options: Options) -> Job {
    spawn(connection, token, Operation::LoadSelected(options.model))
}

fn spawn(connection: Connection, token: String, operation: Operation) -> Job {
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
                    run(operation, &client, &tx, &flag).await
                }
                .await
                .map_err(|error: anyhow::Error| format!("{error:#}"));
                let _ = send(&tx, Event::Finished(result), &flag, true).await;
            }),
        }
    });
    Job { events, cancel }
}

async fn run(
    operation: Operation,
    client: &Client,
    tx: &SyncSender<Event>,
    cancel: &AtomicBool,
) -> Result<String> {
    let (model, reasoning, prompt, text) = match operation {
        Operation::Discover => {
            let models = client.models().await?;
            let count = models.len();
            send(tx, Event::Models(models), cancel, false).await?;
            return Ok(format!("Connected. {count} available model(s)."));
        }
        Operation::LoadSelected(model) => {
            let id = model.context("Select an LLM model first")?;
            client.prepare_model(&id).await?;
            return Ok(format!("Model ready: {id}"));
        }
        Operation::Generate {
            prompt,
            text,
            options,
        } => {
            let models = client.models().await?;
            let id = options
                .model
                .context("Select an LLM model in Settings first")?;
            let model = models
                .iter()
                .find(|m| m.id == id)
                .context("Selected model is unavailable; refresh the model list")?;
            ensure!(
                !model.needs_load,
                "Model is not ready. Open Settings → LLM and click Apply to load it first"
            );
            send(tx, Event::Status("Generating…".into()), cancel, false).await?;
            (id, options.reasoning, prompt, text)
        }
        Operation::LegacyFirstModel(rewrite) => {
            let model = client.legacy_first_model().await?;
            let Some((prompt, text)) = rewrite else {
                return Ok(format!("Connected. Loaded model: {model}"));
            };
            (model, Reasoning::Default, prompt, text)
        }
    };
    client
        .generate(&model, reasoning, prompt, text, async |text| {
            send(tx, Event::Text(text), cancel, false).await
        })
        .await?;
    Ok("Rewrite complete.".to_owned())
}
