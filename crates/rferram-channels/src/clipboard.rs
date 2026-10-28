//! Cross-platform native clipboard synchronization (`arboard`) with dual-layer loopback echo suppression.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use arboard::{Clipboard, ImageData};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info};

use rferram_protocol::{ClipboardPayload, VirtualChannelMessage, WireMessage};

/// Represents a local clipboard snapshot to prevent echo-back loops.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipboardContent {
    Text(String),
    ImageHash(u64),
    Files(Vec<String>),
    Empty,
}

/// Cross-Platform Bi-Directional Clipboard Synchronization Manager (`arboard`)
pub struct ClipboardChannelHandler {
    clipboard: Arc<Mutex<Option<Clipboard>>>,
    last_sent_content: Arc<Mutex<ClipboardContent>>,
    is_suppressed: Arc<AtomicBool>,
}

impl Default for ClipboardChannelHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardChannelHandler {
    pub fn new() -> Self {
        let clipboard = match Clipboard::new() {
            Ok(cb) => Some(cb),
            Err(e) => {
                debug!(
                    "Native system clipboard unavailable in current headless/display environment: {e}"
                );
                None
            }
        };

        Self {
            clipboard: Arc::new(Mutex::new(clipboard)),
            last_sent_content: Arc::new(Mutex::new(ClipboardContent::Empty)),
            is_suppressed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawns background polling loop to detect local clipboard updates and emit outgoing wire messages.
    pub async fn start_clipboard_monitor(
        &self,
        outgoing_tx: mpsc::Sender<WireMessage>,
    ) -> Result<()> {
        let clipboard = self.clipboard.clone();
        let last_sent = self.last_sent_content.clone();
        let is_suppressed = self.is_suppressed.clone();

        tokio::spawn(async move {
            let mut check_interval = tokio::time::interval(Duration::from_millis(300));

            loop {
                check_interval.tick().await;

                if is_suppressed.load(Ordering::SeqCst) {
                    continue;
                }

                let mut cb_lock = clipboard.lock().await;
                if let Some(cb) = cb_lock.as_mut() {
                    // 1. Check for Text Content
                    if let Ok(text) = cb.get_text()
                        && !text.is_empty()
                    {
                        let mut last_lock = last_sent.lock().await;
                        if *last_lock != ClipboardContent::Text(text.clone()) {
                            *last_lock = ClipboardContent::Text(text.clone());
                            debug!(bytes = text.len(), "Local text clipboard change detected");

                            let payload = ClipboardPayload::Text(text);
                            let msg =
                                WireMessage::Channel(VirtualChannelMessage::Clipboard(payload));
                            if outgoing_tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                        continue;
                    }

                    // 2. Check for Image Content
                    if let Ok(image) = cb.get_image() {
                        let hash = calculate_image_hash(&image.bytes);
                        let mut last_lock = last_sent.lock().await;
                        if *last_lock != ClipboardContent::ImageHash(hash) {
                            *last_lock = ClipboardContent::ImageHash(hash);
                            debug!(
                                width = image.width,
                                height = image.height,
                                "Local image clipboard change detected"
                            );

                            let payload = ClipboardPayload::Image {
                                width: image.width as u32,
                                height: image.height as u32,
                                format: String::from("raw/rgba8"),
                                data: image.bytes.into_owned(),
                            };
                            let msg =
                                WireMessage::Channel(VirtualChannelMessage::Clipboard(payload));
                            if outgoing_tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// Ingests incoming remote clipboard payloads and updates the local OS pasteboard.
    pub async fn handle_remote_clipboard(&self, payload: ClipboardPayload) -> Result<()> {
        self.is_suppressed.store(true, Ordering::SeqCst);

        let mut cb_lock = self.clipboard.lock().await;
        if let Some(cb) = cb_lock.as_mut() {
            match payload {
                ClipboardPayload::Text(text) => {
                    info!(
                        text_len = text.len(),
                        "Injecting remote text into system clipboard"
                    );
                    let mut last = self.last_sent_content.lock().await;
                    *last = ClipboardContent::Text(text.clone());
                    let _ = cb.set_text(text);
                }
                ClipboardPayload::Image {
                    width,
                    height,
                    format: _,
                    data,
                } => {
                    info!(
                        width,
                        height,
                        data_bytes = data.len(),
                        "Injecting remote image into system clipboard"
                    );
                    let hash = calculate_image_hash(&data);
                    let mut last = self.last_sent_content.lock().await;
                    *last = ClipboardContent::ImageHash(hash);

                    let img_data = ImageData {
                        width: width.max(1) as usize,
                        height: height.max(1) as usize,
                        bytes: std::borrow::Cow::Owned(data),
                    };
                    let _ = cb.set_image(img_data);
                }
                ClipboardPayload::Files(files) => {
                    info!(
                        count = files.len(),
                        "Injecting remote file path list into system clipboard"
                    );
                    let mut last = self.last_sent_content.lock().await;
                    *last = ClipboardContent::Files(files);
                }
            }
        }
        drop(cb_lock);

        tokio::time::sleep(Duration::from_millis(100)).await;
        self.is_suppressed.store(false, Ordering::SeqCst);
        Ok(())
    }
}

pub fn calculate_image_hash(data: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub struct MockClipboardHandler {
        pub is_suppressed: AtomicBool,
        pub last_hash: u64,
    }

    impl Default for MockClipboardHandler {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockClipboardHandler {
        pub fn new() -> Self {
            Self {
                is_suppressed: AtomicBool::new(false),
                last_hash: 0,
            }
        }

        pub fn process_incoming_text(&mut self, text: &str) -> bool {
            let text_hash = calculate_image_hash(text.as_bytes());

            if self.is_suppressed.swap(false, Ordering::SeqCst) {
                return false;
            }

            if text_hash == self.last_hash {
                return false;
            }

            self.last_hash = text_hash;
            true
        }

        pub fn process_incoming_image(&mut self, data: &[u8]) -> bool {
            let img_hash = calculate_image_hash(data);

            if self.is_suppressed.swap(false, Ordering::SeqCst) {
                return false;
            }

            if img_hash == self.last_hash {
                return false;
            }

            self.last_hash = img_hash;
            true
        }
    }

    #[test]
    fn test_clipboard_loopback_echo_suppression() {
        let mut handler = MockClipboardHandler::new();
        assert!(handler.process_incoming_text("Hello rFerram"));
        assert!(!handler.process_incoming_text("Hello rFerram"));

        handler.is_suppressed.store(true, Ordering::SeqCst);
        assert!(!handler.process_incoming_text("External Change"));
        assert!(!handler.is_suppressed.load(Ordering::SeqCst));
    }

    #[test]
    fn test_duplicate_image_hash_filtering() {
        let mut handler = MockClipboardHandler::new();
        let frame_a = vec![255u8, 0, 128, 255, 10, 20, 30, 40];
        let frame_b = vec![0u8, 255, 64, 255, 50, 60, 70, 80];

        assert_eq!(
            calculate_image_hash(&frame_a),
            calculate_image_hash(&frame_a)
        );
        assert_ne!(
            calculate_image_hash(&frame_a),
            calculate_image_hash(&frame_b)
        );

        // First image payload passes through; identical consecutive hash is filtered
        assert!(handler.process_incoming_image(&frame_a));
        assert!(!handler.process_incoming_image(&frame_a));

        // Distinct image payload passes through; subsequent duplicate is filtered
        assert!(handler.process_incoming_image(&frame_b));
        assert!(!handler.process_incoming_image(&frame_b));
    }
}
