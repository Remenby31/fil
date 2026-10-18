use crate::config::DaemonConfig;
use anyhow::{Context, Result, bail};
use fil_protocol::agent::{AgentReply, AgentRequest};
use fil_protocol::ipc::{self, FrameReader};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

pub struct DaemonClient {
    socket: PathBuf,
}

impl DaemonClient {
    pub fn from_env() -> Self {
        Self::new(DaemonConfig::socket_path())
    }

    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    /// One connection per request. The daemon answers once, possibly after
    /// waiting minutes for the user; dropping the future hangs up.
    pub async fn request(&self, request: &AgentRequest) -> Result<AgentReply> {
        let mut stream = UnixStream::connect(&self.socket).await.with_context(
            || "fil-daemon is not running. Run `fil setup` (or `fil status`) on this machine",
        )?;
        let mut frame = Vec::new();
        ipc::write_frame(
            &mut frame,
            ipc::MSG_AGENT_REQUEST,
            &serde_json::to_vec(request)?,
        )?;
        stream.write_all(&frame).await?;

        let mut reader = FrameReader::new();
        let mut buf = [0u8; 4096];
        loop {
            if let Some((msg_type, payload)) = reader.try_parse_frame() {
                if msg_type != ipc::MSG_AGENT_REPLY {
                    bail!("unexpected reply from fil-daemon");
                }
                return Ok(serde_json::from_slice(&payload)?);
            }
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                bail!("fil-daemon closed the connection (it may need updating: `fil status`)");
            }
            reader.feed(&buf[..n]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn request_reply_round_trip_over_the_socket() {
        // sockaddr_un paths are short; macOS temp_dir() is not.
        let dir = PathBuf::from(format!(
            "/tmp/fil-mcp-{}",
            &uuid::Uuid::new_v4().to_string()[..8]
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut reader = FrameReader::new();
            let mut buf = [0u8; 1024];
            let (msg_type, payload) = loop {
                let n = stream.read(&mut buf).await.unwrap();
                reader.feed(&buf[..n]);
                if let Some(frame) = reader.try_parse_frame() {
                    break frame;
                }
            };
            assert_eq!(msg_type, ipc::MSG_AGENT_REQUEST);
            let request: AgentRequest = serde_json::from_slice(&payload).unwrap();
            assert!(matches!(request, AgentRequest::Notify { .. }));
            let reply = serde_json::to_vec(&AgentReply::Sent {
                session_id: Some("s".into()),
                delivered: Some(1),
            })
            .unwrap();
            let mut frame = Vec::new();
            ipc::write_frame(&mut frame, ipc::MSG_AGENT_REPLY, &reply).unwrap();
            stream.write_all(&frame).await.unwrap();
        });

        let reply = DaemonClient::new(socket)
            .request(&AgentRequest::Notify {
                title: "t".into(),
                message: "m".into(),
                session_hint: None,
            })
            .await
            .unwrap();
        assert_eq!(
            reply,
            AgentReply::Sent {
                session_id: Some("s".into()),
                delivered: Some(1)
            }
        );
        let missing = DaemonClient::new(dir.join("missing.sock"))
            .request(&AgentRequest::Status { session_hint: None })
            .await;
        assert!(missing.is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
