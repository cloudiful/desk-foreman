//! Fake control-plane scripting side (issue 749 P2d, `#[cfg(test)]`).
//!
//! Split from [`super::wire_harness`]: the test drives DF's side of the
//! tunnel here — opening streams, feeding bodies, cancelling — while the
//! manager answers through the code under test. Every control frame carries
//! its own stream identifier (`df-env-http.v2`).

#[cfg(test)]
pub(crate) mod plane {
    use std::net::SocketAddr;

    use futures_util::{SinkExt, StreamExt};
    use runner_protocol::service_relay::{
        OpenFrame, RelayControl, RelayId, StreamFrame, decode_control, encode_chunk, encode_control,
    };
    use tokio::net::TcpListener;
    use uuid::Uuid;

    /// One fake-control-plane connection: the test scripts DF's side.
    pub struct FakePlane {
        ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    }

    pub async fn fake_plane() -> (SocketAddr, tokio::task::JoinHandle<FakePlane>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("loopback control plane");
        let addr = listener.local_addr().expect("addr");
        let accept = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept");
            let ws = tokio_tungstenite::accept_async(socket)
                .await
                .expect("handshake");
            FakePlane { ws }
        });
        (addr, accept)
    }

    impl FakePlane {
        pub async fn send_open(
            &mut self,
            stream: RelayId,
            method: &str,
            path: &str,
            body_length: u64,
        ) {
            self.send_open_env(
                stream,
                RelayId::from_bytes(Uuid::new_v4().into_bytes()),
                method,
                path,
                body_length,
            )
            .await;
        }

        pub async fn send_open_env(
            &mut self,
            stream: RelayId,
            environment: RelayId,
            method: &str,
            path: &str,
            body_length: u64,
        ) {
            use tokio_tungstenite::tungstenite::Message;
            let open = OpenFrame {
                stream_id: stream,
                environment_id: environment,
                generation: 4,
                method: method.to_string(),
                path: path.to_string(),
                query: None,
                allowed_headers: vec!["content-type: application/json".to_string()],
                body_length,
            };
            self.ws
                .send(Message::Text(
                    encode_control(&RelayControl::Open(open))
                        .expect("open")
                        .into(),
                ))
                .await
                .expect("send open");
        }

        pub async fn send_data(&mut self, stream: RelayId, payload: &[u8]) {
            use tokio_tungstenite::tungstenite::Message;
            self.ws
                .send(Message::Binary(
                    encode_chunk(stream, payload).expect("chunk").into(),
                ))
                .await
                .expect("send data");
        }

        pub async fn send_request_end(&mut self, stream: RelayId) {
            use tokio_tungstenite::tungstenite::Message;
            self.ws
                .send(Message::Text(
                    encode_control(&RelayControl::RequestEnd(StreamFrame { stream_id: stream }))
                        .expect("end")
                        .into(),
                ))
                .await
                .expect("send request end");
        }

        pub async fn send_cancel(&mut self, stream: RelayId) {
            use tokio_tungstenite::tungstenite::Message;
            self.ws
                .send(Message::Text(
                    encode_control(&RelayControl::Cancel(StreamFrame { stream_id: stream }))
                        .expect("cancel")
                        .into(),
                ))
                .await
                .expect("send cancel");
        }

        pub async fn next_frame(&mut self) -> tokio_tungstenite::tungstenite::Message {
            tokio::time::timeout(std::time::Duration::from_secs(10), self.ws.next())
                .await
                .expect("a frame arrives")
                .expect("socket open")
                .expect("message")
        }

        /// Raw read with a bounded wait: `None` on timeout, for asserting
        /// the peer closed the socket or went quiet.
        pub async fn next_raw(
            &mut self,
        ) -> Option<
            Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>,
        > {
            tokio::time::timeout(std::time::Duration::from_secs(10), self.ws.next())
                .await
                .ok()
                .flatten()
        }

        pub async fn next_text(&mut self) -> String {
            match self.next_frame().await {
                tokio_tungstenite::tungstenite::Message::Text(text) => text.to_string(),
                other => panic!("expected a text frame, got {other:?}"),
            }
        }
    }

    pub fn decode_text(raw: &str) -> RelayControl {
        decode_control(raw).expect("decodable control frame")
    }

    pub fn fresh_id() -> RelayId {
        RelayId::from_bytes(Uuid::new_v4().into_bytes())
    }
}
