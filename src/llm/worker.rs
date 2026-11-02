use super::{Connection, client::Client};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
};

pub enum Event {
    Text(String),
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
                    let model = client.model().await?;
                    if let Some((prompt, text)) = rewrite {
                        client
                            .generate(&model, prompt, text, async |text| {
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
