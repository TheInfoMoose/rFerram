//! QUIC Connection Manager for Multiplexed Streams, Datagrams & RTT Monitoring
//!
//! Implements [`QuicConnectionManager`] wrapping an active `quinn::Connection` with
//! `postcard` datagram framing (`send_datagram` / `recv_datagram`), 4-byte little-endian
//! length-prefixed unidirectional stream framing (`send_uni_message` / `recv_uni_message`),
//! bidirectional stream allocation (`open_bidirectional_stream`), TLS 1.3 leaf certificate
//! SHA-256 channel binding accessor, and background RTT monitoring.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use quinn::{Connection, RecvStream, SendStream};
use tokio::sync::mpsc;

use rferram_protocol::{
    WireMessage, decode_length_prefixed, decode_wire_message, encode_length_prefixed,
    encode_wire_message,
};

use crate::{NetworkMetrics, QuicTransportConfig};

/// Primary Connection Controller for active QUIC Sessions (Client or Host)
pub struct QuicConnectionManager {
    pub connection: Connection,
    pub metrics: Arc<NetworkMetrics>,
    pub config: QuicTransportConfig,
    peer_cert_sha256: Option<[u8; 32]>,
}

impl QuicConnectionManager {
    pub fn new(
        connection: Connection,
        config: QuicTransportConfig,
        peer_cert_sha256: Option<[u8; 32]>,
    ) -> Self {
        Self {
            connection,
            metrics: Arc::new(NetworkMetrics::default()),
            config,
            peer_cert_sha256,
        }
    }

    /// Returns the SHA-256 fingerprint of the peer's TLS 1.3 leaf certificate for channel binding.
    pub fn peer_cert_sha256(&self) -> Option<[u8; 32]> {
        self.peer_cert_sha256
    }

    /// Sends an un-ordered, un-reliable UDP Datagram (ideal for ultra-low latency Mouse/Keyboard events)
    pub fn send_datagram(&self, msg: &WireMessage) -> Result<()> {
        let bytes =
            encode_wire_message(msg).context("Failed to serialize wire message for datagram")?;
        let byte_len = bytes.len() as u64;
        self.connection
            .send_datagram(bytes.into())
            .map_err(|e| anyhow!("Datagram transmission error: {e}"))?;

        self.metrics
            .bytes_sent
            .fetch_add(byte_len, Ordering::Relaxed);
        self.metrics.datagrams_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Receives and decodes the next incoming QUIC datagram into a [`WireMessage`].
    pub async fn recv_datagram(&self) -> Result<WireMessage> {
        let data = self
            .connection
            .read_datagram()
            .await
            .context("Failed to read incoming QUIC datagram")?;
        self.metrics
            .bytes_received
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        self.metrics
            .datagrams_received
            .fetch_add(1, Ordering::Relaxed);
        decode_wire_message(&data).context("Failed to decode incoming QUIC datagram")
    }

    /// Opens a high-throughput uni-directional stream (ideal for Video/Audio frames)
    pub async fn open_unidirectional_stream(&self) -> Result<SendStream> {
        let send_stream = self
            .connection
            .open_uni()
            .await
            .context("Failed to open uni-directional QUIC stream")?;
        Ok(send_stream)
    }

    /// Sends a single length-prefixed [`WireMessage`] over a unidirectional stream.
    pub async fn send_uni_message(&self, msg: &WireMessage) -> Result<()> {
        let framed = encode_length_prefixed(msg)?;
        let mut stream = self.open_unidirectional_stream().await?;
        stream.write_all(&framed).await?;
        stream.finish()?;
        self.metrics
            .bytes_sent
            .fetch_add(framed.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Accepts and decodes a single length-prefixed [`WireMessage`] from an incoming unidirectional stream.
    pub async fn recv_uni_message(&self) -> Result<WireMessage> {
        let mut recv = self.connection.accept_uni().await?;
        let data = recv
            .read_to_end(rferram_protocol::MAX_WIRE_FRAME_BYTES + 4)
            .await?;
        self.metrics
            .bytes_received
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        Ok(decode_length_prefixed(&data)?)
    }

    /// Opens a reliable bi-directional stream (ideal for Virtual Channels / ST v2 snapshot transfers)
    pub async fn open_bidirectional_stream(&self) -> Result<(SendStream, RecvStream)> {
        let (send_stream, recv_stream) = self
            .connection
            .open_bi()
            .await
            .context("Failed to open bi-directional QUIC stream")?;
        Ok((send_stream, recv_stream))
    }

    /// Background task monitoring Round-Trip Time (RTT) and connection state
    pub async fn start_metrics_monitor(&self, update_tx: mpsc::Sender<u32>) -> Result<()> {
        let connection = self.connection.clone();
        let metrics = self.metrics.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            loop {
                interval.tick().await;

                let rtt = connection.rtt().as_millis() as u64;
                metrics.rtt_ms.store(rtt, Ordering::Relaxed);

                if update_tx.send(rtt as u32).await.is_err() {
                    break;
                }
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QuicClientTransport, QuicServerListener};
    use rferram_protocol::{
        ControlMessage, ElementState, HandshakeMessage, InputEvent, MouseButtonCode, PixelFormat,
        VideoCodec, VideoFrameMessage,
    };

    #[tokio::test]
    async fn test_quic_loopback_handshake_and_datagram() {
        let server_cfg = QuicTransportConfig::default();
        let listener = QuicServerListener::bind("127.0.0.1:0".parse().unwrap(), server_cfg)
            .expect("Failed to bind QUIC listener");
        let addr = listener.local_addr().unwrap();

        let server_handle = tokio::spawn(async move {
            let conn = listener.accept().await.expect("Server failed to accept");
            let msg = conn
                .recv_datagram()
                .await
                .expect("Server failed to read datagram");
            assert_eq!(conn.metrics.datagrams_received.load(Ordering::Relaxed), 1);
            assert!(conn.metrics.bytes_received.load(Ordering::Relaxed) > 0);
            (conn.peer_cert_sha256(), msg)
        });

        let client = QuicClientTransport::new(QuicTransportConfig::default());
        let client_conn = client
            .connect(addr, "rferram.host")
            .await
            .expect("Client failed to connect");

        let input_msg = WireMessage::Input(InputEvent::MouseButton {
            button: MouseButtonCode::Left,
            state: ElementState::Pressed,
            x: 512.0,
            y: 384.0,
        });
        client_conn
            .send_datagram(&input_msg)
            .expect("Client failed to send datagram");
        assert_eq!(
            client_conn.metrics.datagrams_sent.load(Ordering::Relaxed),
            1
        );
        assert!(client_conn.metrics.bytes_sent.load(Ordering::Relaxed) > 0);

        let (server_cert_hash, received) = server_handle.await.unwrap();
        assert_eq!(received, input_msg);
        assert_eq!(client_conn.peer_cert_sha256(), server_cert_hash);
    }

    #[tokio::test]
    async fn test_quic_loopback_length_prefixed_unidirectional_stream() {
        let server_cfg = QuicTransportConfig::default();
        let listener = QuicServerListener::bind("127.0.0.1:0".parse().unwrap(), server_cfg)
            .expect("Failed to bind QUIC listener");
        let addr = listener.local_addr().unwrap();

        let expected_video = WireMessage::Video(VideoFrameMessage {
            frame_id: 42,
            monitor_id: 1,
            timestamp_us: 16_666,
            codec: VideoCodec::AV1,
            is_keyframe: true,
            width: 1920,
            height: 1080,
            stride: 1920 * 4,
            format: PixelFormat::Bgra8,
            payload: vec![0xAB; 4096],
        });
        let video_for_server = expected_video.clone();

        let server_handle = tokio::spawn(async move {
            let server_conn = listener.accept().await.expect("Server failed to accept");

            // Receive ClientHello on unidirectional stream from client
            let client_hello = server_conn
                .recv_uni_message()
                .await
                .expect("Server failed to receive unidirectional message");

            // Send length-prefixed VideoFrameMessage on unidirectional stream to client
            server_conn
                .send_uni_message(&video_for_server)
                .await
                .expect("Server failed to send unidirectional video frame");

            assert!(server_conn.metrics.bytes_received.load(Ordering::Relaxed) > 4);
            assert!(server_conn.metrics.bytes_sent.load(Ordering::Relaxed) > 4096);
            (listener, server_conn, client_hello)
        });

        let client = QuicClientTransport::new(QuicTransportConfig::default());
        let client_conn = client
            .connect(addr, "rferram.host")
            .await
            .expect("Client failed to connect");

        let hello_msg = WireMessage::Handshake(HandshakeMessage::ClientHello {
            protocol_version: 1,
            client_id: "client-loopback-01".to_string(),
            client_name: "rFerram Test Viewer".to_string(),
            supported_codecs: vec![VideoCodec::AV1],
            auth_methods: vec![],
        });

        client_conn
            .send_uni_message(&hello_msg)
            .await
            .expect("Client failed to send unidirectional handshake");

        let received_video = client_conn
            .recv_uni_message()
            .await
            .expect("Client failed to receive unidirectional video frame");

        let (_listener, _server_conn, received_hello) = server_handle.await.unwrap();
        assert_eq!(received_hello, hello_msg);
        assert_eq!(received_video, expected_video);
        assert!(client_conn.metrics.bytes_sent.load(Ordering::Relaxed) > 4);
        assert!(client_conn.metrics.bytes_received.load(Ordering::Relaxed) > 4096);
    }

    #[tokio::test]
    async fn test_quic_loopback_combined_datagram_and_uni_stream_with_metrics_monitor() {
        let listener = QuicServerListener::bind(
            "127.0.0.1:0".parse().unwrap(),
            QuicTransportConfig::default(),
        )
        .expect("Failed to bind QUIC listener");
        let addr = listener.local_addr().unwrap();

        let server_handle = tokio::spawn(async move {
            let server_conn = listener.accept().await.expect("Server failed to accept");

            let uni_msg = server_conn
                .recv_uni_message()
                .await
                .expect("Server failed to recv uni stream");
            let dgram_msg = server_conn
                .recv_datagram()
                .await
                .expect("Server failed to recv datagram");

            (uni_msg, dgram_msg)
        });

        let client = QuicClientTransport::new(QuicTransportConfig::default());
        let client_conn = client
            .connect(addr, "rferram.host")
            .await
            .expect("Client failed to connect");

        let (rtt_tx, mut rtt_rx) = mpsc::channel(4);
        client_conn
            .start_metrics_monitor(rtt_tx)
            .await
            .expect("Failed to start metrics monitor");

        let first_rtt = rtt_rx.recv().await.expect("Expected initial RTT sample");
        assert!(first_rtt < 5_000);

        let video_msg = WireMessage::Video(VideoFrameMessage {
            frame_id: 1,
            monitor_id: 0,
            timestamp_us: 1000,
            codec: VideoCodec::AV1,
            is_keyframe: true,
            width: 1280,
            height: 720,
            stride: 1280 * 4,
            format: PixelFormat::Rgba8,
            payload: vec![0x55; 512],
        });
        let ack_msg = WireMessage::Control(ControlMessage::FrameAck {
            frame_id: 1,
            monitor_id: 0,
            render_delay_us: 450,
        });

        client_conn
            .send_uni_message(&video_msg)
            .await
            .expect("Failed to send uni stream");
        client_conn
            .send_datagram(&ack_msg)
            .expect("Failed to send datagram");

        let (srv_uni, srv_dgram) = server_handle.await.unwrap();
        assert_eq!(srv_uni, video_msg);
        assert_eq!(srv_dgram, ack_msg);
    }
}
