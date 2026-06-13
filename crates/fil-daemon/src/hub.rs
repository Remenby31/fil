use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use url::Url;

use fil_protocol::proto;

pub struct HubConnection {
    hub_url: String,
    device_id: String,
    tx: mpsc::Sender<proto::DaemonMessage>,
}

pub enum HubEvent {
    SessionInput { session_id: String, data: Vec<u8> },
    Disconnected,
}

impl HubConnection {
    pub fn new(hub_url: &str, device_id: &str) -> (Self, mpsc::Receiver<proto::DaemonMessage>) {
        let (tx, rx) = mpsc::channel(256);
        (
            Self {
                hub_url: hub_url.to_string(),
                device_id: device_id.to_string(),
                tx,
            },
            rx,
        )
    }

    pub fn sender(&self) -> mpsc::Sender<proto::DaemonMessage> {
        self.tx.clone()
    }

    pub async fn connect_and_run(
        &self,
        mut outgoing: mpsc::Receiver<proto::DaemonMessage>,
        incoming_tx: mpsc::Sender<HubEvent>,
    ) -> Result<()> {
        let ws_url = self.build_ws_url()?;
        info!(url = %ws_url, "connecting to hub");

        let (ws_stream, _) = connect_async(ws_url.as_str())
            .await
            .context("failed to connect to hub")?;

        info!("connected to hub");

        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        // Two tasks: send outgoing messages and receive incoming
        let send_task = async {
            while let Some(msg) = outgoing.recv().await {
                let mut buf = Vec::new();
                msg.encode(&mut buf).ok();
                if ws_sender.send(Message::Binary(buf.into())).await.is_err() {
                    break;
                }
            }
        };

        let recv_task = async {
            while let Some(msg) = ws_receiver.next().await {
                match msg {
                    Ok(Message::Binary(data)) => {
                        if let Ok(hub_msg) = proto::HubMessage::decode(data.as_ref()) {
                            if let Some(payload) = hub_msg.payload {
                                match payload {
                                    proto::hub_message::Payload::SessionInput(input) => {
                                        let _ = incoming_tx
                                            .send(HubEvent::SessionInput {
                                                session_id: input.session_id,
                                                data: input.data,
                                            })
                                            .await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(e) => {
                        warn!(error = %e, "hub WebSocket error");
                        break;
                    }
                    _ => {}
                }
            }
            let _ = incoming_tx.send(HubEvent::Disconnected).await;
        };

        tokio::select! {
            _ = send_task => {},
            _ = recv_task => {},
        }

        Ok(())
    }

    fn build_ws_url(&self) -> Result<Url> {
        let mut url = Url::parse(&self.hub_url)?;

        // Convert http(s) to ws(s)
        match url.scheme() {
            "http" => url.set_scheme("ws").ok(),
            "https" => url.set_scheme("wss").ok(),
            _ => None,
        };

        url.set_path("/ws");
        url.query_pairs_mut()
            .append_pair("device_id", &self.device_id)
            .append_pair("device_token", &self.device_id); // TODO: use real token

        Ok(url)
    }
}

pub fn build_session_created(
    session_id: &str,
    shell: &str,
    cwd: &str,
    cols: u32,
    rows: u32,
) -> proto::DaemonMessage {
    proto::DaemonMessage {
        payload: Some(proto::daemon_message::Payload::SessionCreated(
            proto::SessionCreated {
                session_id: session_id.to_string(),
                device_id: String::new(), // filled by hub
                shell: shell.to_string(),
                cwd: cwd.to_string(),
                created_at: chrono::Utc::now().timestamp(),
                cols,
                rows,
            },
        )),
    }
}

pub fn build_session_destroyed(session_id: &str, exit_code: i32) -> proto::DaemonMessage {
    proto::DaemonMessage {
        payload: Some(proto::daemon_message::Payload::SessionDestroyed(
            proto::SessionDestroyed {
                session_id: session_id.to_string(),
                device_id: String::new(),
                exit_code,
                destroyed_at: chrono::Utc::now().timestamp(),
            },
        )),
    }
}

pub fn build_session_data(session_id: &str, data: Vec<u8>) -> proto::DaemonMessage {
    proto::DaemonMessage {
        payload: Some(proto::daemon_message::Payload::SessionData(
            proto::SessionData {
                session_id: session_id.to_string(),
                data,
            },
        )),
    }
}

pub fn build_heartbeat(
    device_id: &str,
    sessions: Vec<proto::SessionInfo>,
) -> proto::DaemonMessage {
    proto::DaemonMessage {
        payload: Some(proto::daemon_message::Payload::Heartbeat(
            proto::Heartbeat {
                device_id: device_id.to_string(),
                sessions,
                timestamp: chrono::Utc::now().timestamp(),
            },
        )),
    }
}
