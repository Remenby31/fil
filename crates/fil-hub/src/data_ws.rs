//! HTTPS data-plane fallback. Authentication is exclusively in the bearer header.
//! Client output starts with one binary u64 BE cursor, then raw replay/live bytes.
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};
use tracing::debug;

use crate::auth::AuthUser;
use crate::quic::{DaemonCommand, QuicRouter};
use crate::sessions::SessionRegistry;
use crate::state::AppState;

const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = MAX_INPUT_BYTES + 5;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const KEEPALIVE: Duration = Duration::from_secs(5);
const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Daemon,
    Client,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataWsParams {
    role: Role,
    resume_from: Option<u64>,
}

pub async fn data_ws_handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<DataWsParams>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Err(status) = authorize_session(&state.sessions, &auth.user_id, &session_id) {
        return status.into_response();
    }
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .write_buffer_size(0)
        .max_write_buffer_size(2 * MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            // Ownership can change while the HTTP upgrade is in flight.
            if authorize_session(&state.sessions, &auth.user_id, &session_id).is_err() {
                return;
            }
            match params.role {
                Role::Daemon => {
                    daemon_socket(socket, &state.quic_router, &session_id).await;
                }
                Role::Client => {
                    client_socket(
                        socket,
                        &state.quic_router,
                        &state.sessions,
                        &session_id,
                        params.resume_from,
                    )
                    .await;
                }
            }
        })
}

fn authorize_session(
    sessions: &SessionRegistry,
    user_id: &str,
    session_id: &str,
) -> Result<(), StatusCode> {
    sessions
        .owns_session(user_id, session_id)
        .then_some(())
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Debug, PartialEq, Eq)]
enum ClientCommand {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Detach,
}

fn parse_client_command(data: &[u8]) -> Result<ClientCommand, &'static str> {
    match data.first() {
        Some(0x00) if data.len() >= 5 => {
            let len = u32::from_be_bytes(data[1..5].try_into().unwrap()) as usize;
            if len > MAX_INPUT_BYTES || data.len() - 5 != len {
                return Err("invalid input frame length");
            }
            Ok(ClientCommand::Input(data[5..].to_vec()))
        }
        Some(0x01) if data.len() == 5 => Ok(ClientCommand::Resize {
            cols: u16::from_be_bytes([data[1], data[2]]),
            rows: u16::from_be_bytes([data[3], data[4]]),
        }),
        Some(0x02) if data.len() == 1 => Ok(ClientCommand::Detach),
        _ => Err("invalid client command frame"),
    }
}

fn encode_daemon_command(command: DaemonCommand) -> Result<Vec<u8>, &'static str> {
    Ok(match command {
        DaemonCommand::Input(data) => {
            if data.len() > MAX_INPUT_BYTES {
                return Err("input frame too large");
            }
            let mut frame = Vec::with_capacity(data.len() + 5);
            frame.push(0x00);
            frame.extend_from_slice(&(data.len() as u32).to_be_bytes());
            frame.extend_from_slice(&data);
            frame
        }
        DaemonCommand::Resize { cols, rows } => {
            let mut frame = vec![0x01];
            frame.extend_from_slice(&cols.to_be_bytes());
            frame.extend_from_slice(&rows.to_be_bytes());
            frame
        }
        DaemonCommand::ClientAttached => vec![0x02],
        DaemonCommand::ClientDetached => vec![0x03],
    })
}

async fn send<S: Sink<Message> + Unpin>(
    socket: &mut S,
    message: Message,
) -> Result<(), &'static str> {
    timeout(IO_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| "WebSocket write timed out")?
        .map_err(|_| "WebSocket write failed")
}

async fn daemon_socket<S, E>(mut socket: S, router: &Arc<QuicRouter>, session_id: &str)
where
    S: Sink<Message> + Stream<Item = Result<Message, E>> + Unpin,
{
    let (input_tx, mut input_rx) = mpsc::channel(256);
    let generation = router.register_daemon_input(session_id, input_tx).await;
    let result: Result<(), &'static str> = async {
        let mut ticker = tokio::time::interval(KEEPALIVE);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_seen = Instant::now();
        loop {
            tokio::select! {
                command = input_rx.recv() => {
                    let Some(command) = command else { return Ok(()); };
                    send(&mut socket, Message::Binary(encode_daemon_command(command)?.into())).await?;
                }
                message = socket.next() => {
                    last_seen = Instant::now();
                    match message {
                        Some(Ok(Message::Binary(data))) => router.forward_to_clients(session_id, &data).await,
                        Some(Ok(Message::Ping(data))) => send(&mut socket, Message::Pong(data)).await?,
                        Some(Ok(Message::Pong(_))) => {},
                        Some(Ok(Message::Close(_))) | None => return Ok(()),
                        Some(Err(_)) => return Err("WebSocket read failed"),
                        _ => return Err("expected binary daemon output"),
                    }
                }
                _ = ticker.tick() => {
                    if last_seen.elapsed() >= IDLE_TIMEOUT { return Err("WebSocket peer timed out"); }
                    send(&mut socket, Message::Ping(Vec::new().into())).await?;
                }
            }
        }
    }.await;
    // Drop the receiver first: blocked router senders must release their locks.
    drop(input_rx);
    router.unregister_daemon(session_id, generation).await;
    let _ = timeout(Duration::from_secs(1), socket.close()).await;
    debug!(%session_id, reason = result.err().unwrap_or("closed"), "daemon data WebSocket ended");
}

async fn client_socket<S, E>(
    mut socket: S,
    router: &Arc<QuicRouter>,
    sessions: &SessionRegistry,
    session_id: &str,
    resume_from: Option<u64>,
) where
    S: Sink<Message> + Stream<Item = Result<Message, E>> + Unpin,
{
    let (client_id, first, mut output_rx, catchup, end_offset) =
        router.attach_client(session_id, resume_from).await;
    // Every fallible operation after registration is inside this block.
    let result: Result<(), &'static str> = async {
        if first {
            timeout(IO_TIMEOUT, router.notify_daemon_client_attached(session_id))
                .await.map_err(|_| "daemon notification timed out")?;
        }
        let start = end_offset - catchup.len() as u64;
        send(&mut socket, Message::Binary(start.to_be_bytes().to_vec().into())).await?;
        if !catchup.is_empty() {
            send(&mut socket, Message::Binary(catchup.into())).await?;
        }
        let mut ticker = tokio::time::interval(KEEPALIVE);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_seen = Instant::now();
        loop {
            tokio::select! {
                data = output_rx.recv() => {
                    let Some(data) = data else { return Ok(()); };
                    send(&mut socket, Message::Binary(data.into())).await?;
                }
                message = socket.next() => {
                    last_seen = Instant::now();
                    match message {
                        Some(Ok(Message::Binary(data))) => match parse_client_command(&data)? {
                            ClientCommand::Input(data) => {
                                timeout(IO_TIMEOUT, router.send_to_daemon(session_id, &data))
                                    .await.map_err(|_| "daemon input timed out")?;
                            }
                            ClientCommand::Resize { cols, rows } => {
                                sessions.update_session_size(session_id, u32::from(cols), u32::from(rows));
                                timeout(IO_TIMEOUT, router.resize_daemon(session_id, cols, rows))
                                    .await.map_err(|_| "daemon resize timed out")?;
                            }
                            ClientCommand::Detach => return Ok(()),
                        },
                        Some(Ok(Message::Ping(data))) => send(&mut socket, Message::Pong(data)).await?,
                        Some(Ok(Message::Pong(_))) => {},
                        Some(Ok(Message::Close(_))) | None => return Ok(()),
                        Some(Err(_)) => return Err("WebSocket read failed"),
                        _ => return Err("expected binary client command"),
                    }
                }
                _ = ticker.tick() => {
                    if last_seen.elapsed() >= IDLE_TIMEOUT { return Err("WebSocket peer timed out"); }
                    send(&mut socket, Message::Ping(Vec::new().into())).await?;
                }
            }
        }
    }.await;
    drop(output_rx);
    if router.detach_client(session_id, client_id).await {
        let _ = timeout(IO_TIMEOUT, router.notify_daemon_client_detached(session_id)).await;
    }
    let _ = timeout(Duration::from_secs(1), socket.close()).await;
    debug!(%session_id, reason = result.err().unwrap_or("closed"), "client data WebSocket ended");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    struct TestSocket {
        incoming: mpsc::UnboundedReceiver<Result<Message, ()>>,
        outgoing: mpsc::UnboundedSender<Message>,
        fail_after: usize,
        writes: usize,
    }

    impl Stream for TestSocket {
        type Item = Result<Message, ()>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.incoming.poll_recv(cx)
        }
    }

    impl Sink<Message> for TestSocket {
        type Error = ();

        fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), ()>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(mut self: Pin<&mut Self>, message: Message) -> Result<(), ()> {
            if self.writes >= self.fail_after {
                return Err(());
            }
            self.writes += 1;
            self.outgoing.send(message).map_err(|_| ())
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), ()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), ()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn socket_pair() -> (
        TestSocket,
        mpsc::UnboundedSender<Result<Message, ()>>,
        mpsc::UnboundedReceiver<Message>,
    ) {
        let (input_tx, incoming) = mpsc::unbounded_channel();
        let (outgoing, output_rx) = mpsc::unbounded_channel();
        (
            TestSocket {
                incoming,
                outgoing,
                fail_after: usize::MAX,
                writes: 0,
            },
            input_tx,
            output_rx,
        )
    }

    async fn binary(rx: &mut mpsc::UnboundedReceiver<Message>) -> Vec<u8> {
        timeout(Duration::from_secs(2), async {
            loop {
                match rx.recv().await.unwrap() {
                    Message::Binary(bytes) => return bytes.to_vec(),
                    Message::Ping(_) => {}
                    _ => panic!("unexpected output message"),
                }
            }
        })
        .await
        .unwrap()
    }

    fn sessions() -> SessionRegistry {
        let sessions = SessionRegistry::new();
        sessions.register_device("device", "owner", "Test device");
        sessions.add_session(
            "device",
            crate::sessions::SessionInfo {
                session_id: "sid".into(),
                device_id: "device".into(),
                shell: "sh".into(),
                command: String::new(),
                cwd: "/tmp".into(),
                cols: 80,
                rows: 24,
                status: crate::sessions::SessionStatus::Online,
                created_at: chrono::Utc::now(),
            },
        );
        sessions
    }

    #[test]
    fn client_frame_validation_is_exact_and_bounded() {
        assert_eq!(
            parse_client_command(&[0, 0, 0, 0, 0]),
            Ok(ClientCommand::Input(vec![]))
        );
        assert_eq!(
            parse_client_command(&[0, 0, 0, 0, 2, 0xff, 0]),
            Ok(ClientCommand::Input(vec![0xff, 0]))
        );
        assert_eq!(
            parse_client_command(&[1, 0, 120, 0, 40]),
            Ok(ClientCommand::Resize {
                cols: 120,
                rows: 40
            })
        );
        assert_eq!(parse_client_command(&[2]), Ok(ClientCommand::Detach));
        for invalid in [
            &[][..],
            &[0],
            &[0, 0, 0, 0, 1],
            &[0, 0, 0, 0, 0, 7],
            &[1, 0],
            &[1, 0, 1, 0, 1, 0],
            &[2, 0],
            &[3],
            &[0, 0xff, 0xff, 0xff, 0xff],
        ] {
            assert!(parse_client_command(invalid).is_err());
        }
        let largest =
            encode_daemon_command(DaemonCommand::Input(vec![7; MAX_INPUT_BYTES])).unwrap();
        assert!(parse_client_command(&largest).is_ok());
        assert!(encode_daemon_command(DaemonCommand::Input(vec![7; MAX_INPUT_BYTES + 1])).is_err());
        assert_eq!(
            encode_daemon_command(DaemonCommand::ClientAttached).unwrap(),
            [2]
        );
        assert_eq!(
            encode_daemon_command(DaemonCommand::ClientDetached).unwrap(),
            [3]
        );
    }

    #[test]
    fn only_the_session_owner_is_authorized() {
        let sessions = sessions();
        assert_eq!(authorize_session(&sessions, "owner", "sid"), Ok(()));
        assert_eq!(
            authorize_session(&sessions, "other", "sid"),
            Err(StatusCode::NOT_FOUND)
        );
        assert_eq!(
            authorize_session(&sessions, "owner", "missing"),
            Err(StatusCode::NOT_FOUND)
        );
        sessions.remove_session("device", "sid");
        assert_eq!(
            authorize_session(&sessions, "owner", "sid"),
            Err(StatusCode::NOT_FOUND)
        );
    }

    #[tokio::test]
    async fn initial_cursor_and_catchup_write_failures_always_detach() {
        for fail_after in [0, 1] {
            let router = Arc::new(QuicRouter::new());
            router.forward_to_clients("sid", b"history").await;
            let (daemon_tx, mut daemon_rx) = mpsc::channel(8);
            router.register_daemon_input("sid", daemon_tx).await;
            let (mut socket, _input, _output) = socket_pair();
            socket.fail_after = fail_after;
            client_socket(socket, &router, &sessions(), "sid", None).await;
            assert!(matches!(
                daemon_rx.try_recv(),
                Ok(DaemonCommand::ClientAttached)
            ));
            assert!(matches!(
                daemon_rx.try_recv(),
                Ok(DaemonCommand::ClientDetached)
            ));
            assert!(daemon_rx.try_recv().is_err());
            let (id, first, _, _, _) = router.attach_client("sid", None).await;
            assert!(first, "failed initial write leaked a client registration");
            router.detach_client("sid", id).await;
        }
    }

    #[tokio::test]
    async fn replay_live_input_resize_and_last_client_detach_share_the_router() {
        let router = Arc::new(QuicRouter::new());
        let sessions = sessions();
        router.forward_to_clients("sid", b"abcdef").await;
        let (daemon_tx, mut daemon_rx) = mpsc::channel(8);
        router.register_daemon_input("sid", daemon_tx).await;
        let (socket, input, mut output) = socket_pair();
        let task = {
            let router = router.clone();
            let sessions = sessions.clone();
            tokio::spawn(
                async move { client_socket(socket, &router, &sessions, "sid", Some(3)).await },
            )
        };
        assert_eq!(binary(&mut output).await, 3u64.to_be_bytes());
        assert_eq!(binary(&mut output).await, b"def");
        assert!(matches!(
            daemon_rx.recv().await,
            Some(DaemonCommand::ClientAttached)
        ));
        let (second_id, first, _, _, _) = router.attach_client("sid", None).await;
        assert!(!first);
        router.forward_to_clients("sid", b"ghi").await;
        assert_eq!(binary(&mut output).await, b"ghi");
        input
            .send(Ok(Message::Binary(vec![0, 0, 0, 0, 1, b'x'].into())))
            .unwrap();
        assert!(matches!(daemon_rx.recv().await, Some(DaemonCommand::Input(data)) if data == b"x"));
        input
            .send(Ok(Message::Binary(vec![1, 0, 120, 0, 40].into())))
            .unwrap();
        assert!(matches!(
            daemon_rx.recv().await,
            Some(DaemonCommand::Resize {
                cols: 120,
                rows: 40
            })
        ));
        assert_eq!(sessions.get_user_sessions("owner")[0].sessions[0].cols, 120);
        input.send(Ok(Message::Binary(vec![2].into()))).unwrap();
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            daemon_rx.try_recv().is_err(),
            "another client is still attached"
        );
        assert!(router.detach_client("sid", second_id).await);
    }

    #[tokio::test]
    async fn daemon_replacement_cannot_be_unregistered_by_the_old_socket() {
        let router = Arc::new(QuicRouter::new());
        let (socket, input, mut output) = socket_pair();
        let task = {
            let router = router.clone();
            tokio::spawn(async move { daemon_socket(socket, &router, "sid").await })
        };
        // First ping is sent only after registration.
        assert!(matches!(
            timeout(Duration::from_secs(2), output.recv())
                .await
                .unwrap(),
            Some(Message::Ping(_))
        ));
        input
            .send(Ok(Message::Binary(b"raw output".to_vec().into())))
            .unwrap();
        input.send(Ok(Message::Ping(vec![42].into()))).unwrap();
        assert!(
            matches!(timeout(Duration::from_secs(2), output.recv()).await.unwrap(), Some(Message::Pong(data)) if data.as_ref() == [42])
        );
        let (client_id, _, _, catchup, _) = router.attach_client("sid", None).await;
        assert_eq!(catchup, b"raw output");
        router.detach_client("sid", client_id).await;
        let (new_tx, mut new_rx) = mpsc::channel(8);
        let generation = router.register_daemon_input("sid", new_tx).await;
        timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        router.send_to_daemon("sid", b"still connected").await;
        assert!(
            matches!(new_rx.recv().await, Some(DaemonCommand::Input(data)) if data == b"still connected")
        );
        router.unregister_daemon("sid", generation).await;
        assert!(new_rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn daemon_writer_failure_cleans_up_registration() {
        let router = Arc::new(QuicRouter::new());
        let (mut socket, _input, _output) = socket_pair();
        socket.fail_after = 0;
        daemon_socket(socket, &router, "sid").await;
        // This must not block on an orphaned, full daemon command queue.
        timeout(Duration::from_secs(2), async {
            for _ in 0..300 {
                router.send_to_daemon("sid", b"x").await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn malformed_client_frames_close_and_detach() {
        let router = Arc::new(QuicRouter::new());
        let (socket, input, _output) = socket_pair();
        input
            .send(Ok(Message::Binary(vec![0, 0xff, 0xff, 0xff, 0xff].into())))
            .unwrap();
        client_socket(socket, &router, &sessions(), "sid", None).await;
        let (id, first, _, _, _) = router.attach_client("sid", None).await;
        assert!(first);
        router.detach_client("sid", id).await;
    }

    #[tokio::test]
    async fn endpoint_requires_bearer_and_checks_ownership_before_upgrade() {
        let mut state = crate::state::test_state().await;
        state.sessions = sessions();
        sqlx::query("INSERT INTO users (id, provider, provider_id) VALUES ('owner', 'github', 'owner'), ('other', 'github', 'other')")
            .execute(&state.db.pool).await.unwrap();
        let token = |subject: &str| {
            jsonwebtoken::encode(
                &jsonwebtoken::Header::default(),
                &serde_json::json!({"sub": subject, "iat": chrono::Utc::now().timestamp(), "exp": chrono::Utc::now().timestamp() + 60}),
                &jsonwebtoken::EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
            )
            .unwrap()
        };
        let other = token("other");
        let deleted = token("deleted");
        let owner = token("owner");
        let app = axum::Router::new()
            .route("/ws/data/{session_id}", axum::routing::get(data_ws_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        for role in ["client", "daemon"] {
            for (bearer, expected) in [
                (None, 401),
                (Some("invalid"), 401),
                (Some(deleted.as_str()), 401),
                (Some(other.as_str()), 404),
            ] {
                let mut request = client
                    .get(format!("http://{addr}/ws/data/sid?role={role}"))
                    .header("Connection", "upgrade")
                    .header("Upgrade", "websocket")
                    .header("Sec-WebSocket-Version", "13")
                    .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==");
                if let Some(bearer) = bearer {
                    request = request.bearer_auth(bearer);
                }
                assert_eq!(request.send().await.unwrap().status().as_u16(), expected);
            }
        }
        assert_eq!(
            client
                .get(format!(
                    "http://{addr}/ws/data/sid?role=client&token={owner}"
                ))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16(),
            401
        );
        server.abort();
    }
}
