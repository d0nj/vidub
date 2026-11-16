use std::ffi::OsString;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};
use vidub_process::hide_console;

use crate::protocol::{seconds_to_ms, IpcCodec};
use crate::transport::{cleanup, connect, IpcStream, MpvTransport};

const CONNECT_ATTEMPTS: u32 = 50;
const CONNECT_RETRY: Duration = Duration::from_millis(100);
const EXIT_GRACE: Duration = Duration::from_secs(3);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[async_trait::async_trait]
pub trait MpvControl: Send + Sync {
    async fn load(&self, video: &Path, dub_wav: Option<&Path>) -> Result<()>;
    async fn seek_ms(&self, pos_ms: u64, exact: bool) -> Result<()>;
    async fn set_audio(&self, wav: Option<&Path>) -> Result<()>;
    async fn set_pause(&self, paused: bool) -> Result<()>;
    async fn time_pos_ms(&self) -> Result<Option<u64>>;
    async fn duration_ms(&self) -> Result<Option<u64>>;
    async fn close(&self) -> Result<()>;
}

struct CmdReq {
    cmd: Vec<Value>,
    reply: oneshot::Sender<Result<Value>>,
}

async fn drive(
    mut codec: IpcCodec<Box<dyn IpcStream>>,
    mut cmd_rx: mpsc::UnboundedReceiver<CmdReq>,
) {
    while let Some(req) = cmd_rx.recv().await {
        let res = codec.send(&req.cmd).await;
        let _ = req.reply.send(res);
    }
}

pub struct MpvHandle {
    child: Mutex<Option<Child>>,
    cmd_tx: mpsc::UnboundedSender<CmdReq>,
    transport: MpvTransport,
    driver: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl MpvHandle {
    #[tracing::instrument(skip(mpv, video, dub_wav))]
    pub async fn spawn(mpv: &Path, video: &Path, dub_wav: Option<&Path>) -> Result<Self> {
        let transport = MpvTransport::for_current_platform();
        cleanup(&transport);

        let mut cmd = tokio::process::Command::new(mpv);
        cmd.arg("--idle=yes")
            .arg(transport.ipc_server_arg())
            .arg("--no-osc")
            .arg("--keep-open=yes")
            .arg("--hwdec=no")
            .arg(video.as_os_str());
        if let Some(dub) = dub_wav {
            let mut arg = OsString::from("--audio-file=");
            arg.push(dub);
            cmd.arg(arg);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        hide_console(&mut cmd);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn mpv at {}", mpv.display()))?;

        let stream = match connect_with_retry(&transport, &mut child).await {
            Ok(s) => s,
            Err(e) => {
                let _ = child.start_kill();
                cleanup(&transport);
                return Err(e);
            }
        };

        let codec = IpcCodec::new(stream);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let driver = tokio::spawn(drive(codec, cmd_rx));

        Ok(Self {
            child: Mutex::new(Some(child)),
            cmd_tx,
            transport,
            driver: Mutex::new(Some(driver)),
        })
    }

    async fn send_cmd(&self, cmd: Vec<Value>) -> Result<Value> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(CmdReq {
                cmd,
                reply: reply_tx,
            })
            .map_err(|_| anyhow!("mpv driver task is gone"))?;
        reply_rx
            .await
            .map_err(|_| anyhow!("mpv driver task dropped the reply"))?
    }

    async fn shutdown(&self) {
        let _ = self.send_cmd(vec![Value::from("quit")]).await;
        let child = lock(&self.child).take();
        if let Some(mut child) = child {
            if tokio::time::timeout(EXIT_GRACE, child.wait())
                .await
                .is_err()
            {
                let _ = child.start_kill();
                let _ = tokio::time::timeout(EXIT_GRACE, child.wait()).await;
            }
        }
        if let Some(driver) = lock(&self.driver).take() {
            driver.abort();
        }
        cleanup(&self.transport);
    }
}

async fn connect_with_retry(
    transport: &MpvTransport,
    child: &mut Child,
) -> Result<Box<dyn IpcStream>> {
    let mut last = None;
    for _ in 0..CONNECT_ATTEMPTS {
        if let Some(status) = child.try_wait().context("checking spawned mpv child")? {
            return Err(anyhow!("mpv exited during startup (status {status})"));
        }
        match connect(transport).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
        tokio::time::sleep(CONNECT_RETRY).await;
    }
    Err(last.unwrap_or_else(|| anyhow!("mpv IPC never became connectable")))
        .context("connecting to mpv IPC server")
}

#[async_trait::async_trait]
impl MpvControl for MpvHandle {
    #[tracing::instrument(skip(self, video, dub_wav))]
    async fn load(&self, video: &Path, dub_wav: Option<&Path>) -> Result<()> {
        self.send_cmd(vec![
            Value::from("loadfile"),
            Value::from(path_str(video)),
            Value::from("replace"),
        ])
        .await?;
        if let Some(dub) = dub_wav {
            self.send_cmd(vec![
                Value::from("audio-add"),
                Value::from(path_str(dub)),
                Value::from("select"),
            ])
            .await?;
        }
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn seek_ms(&self, pos_ms: u64, exact: bool) -> Result<()> {
        let mode = if exact {
            "absolute+exact"
        } else {
            "absolute+keyframes"
        };
        self.send_cmd(vec![
            Value::from("seek"),
            Value::from(pos_ms as f64 / 1000.0),
            Value::from(mode),
        ])
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self, wav))]
    async fn set_audio(&self, wav: Option<&Path>) -> Result<()> {
        match wav {
            Some(dub) => {
                self.send_cmd(vec![
                    Value::from("audio-add"),
                    Value::from(path_str(dub)),
                    Value::from("select"),
                ])
                .await?;
            }
            None => {
                self.send_cmd(vec![
                    Value::from("set_property"),
                    Value::from("aid"),
                    Value::from("auto"),
                ])
                .await?;
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn set_pause(&self, paused: bool) -> Result<()> {
        self.send_cmd(vec![
            Value::from("set_property"),
            Value::from("pause"),
            Value::from(paused),
        ])
        .await?;
        Ok(())
    }

    async fn time_pos_ms(&self) -> Result<Option<u64>> {
        let data = self
            .send_cmd(vec![Value::from("get_property"), Value::from("time-pos")])
            .await?;
        Ok(seconds_to_ms(&data))
    }

    async fn duration_ms(&self) -> Result<Option<u64>> {
        let data = self
            .send_cmd(vec![Value::from("get_property"), Value::from("duration")])
            .await?;
        Ok(seconds_to_ms(&data))
    }

    #[tracing::instrument(skip(self))]
    async fn close(&self) -> Result<()> {
        self.shutdown().await;
        Ok(())
    }
}

impl Drop for MpvHandle {
    fn drop(&mut self) {
        if let Some(mut child) = lock(&self.child).take() {
            let _ = child.start_kill();
        }
        if let Some(driver) = lock(&self.driver).take() {
            driver.abort();
        }
        cleanup(&self.transport);
    }
}
