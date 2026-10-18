use std::fmt::{Display, Formatter};
use std::io::{Read, Seek, SeekFrom, Write};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::SpooledTempFile;
use tokio::sync::Notify;
use uuid::Uuid;

pub(super) const SPOOL_MEMORY_THRESHOLD_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct StreamId(Uuid);

impl StreamId {
    pub(super) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Display for StreamId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "stream_{}", self.0.simple())
    }
}

impl FromStr for StreamId {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let uuid_text = text.strip_prefix("stream_").ok_or(())?;
        Uuid::parse_str(uuid_text).map(Self).map_err(|_| ())
    }
}

pub(super) struct SharedStream {
    storage: Arc<Mutex<SpooledTempFile>>,
    written: Arc<AtomicU64>,
    update: Notify,
    closed: AtomicBool,
}

impl SharedStream {
    pub(super) fn new(maximum_memory_bytes: usize) -> Self {
        Self {
            storage: Arc::new(Mutex::new(SpooledTempFile::new(maximum_memory_bytes))),
            written: Arc::new(AtomicU64::new(0)),
            update: Notify::new(),
            closed: AtomicBool::new(false),
        }
    }

    pub(super) async fn append(&self, bytes: Vec<u8>) -> Result<(), ()> {
        let storage = Arc::clone(&self.storage);
        let written = Arc::clone(&self.written);
        let write = tokio::task::spawn_blocking(move || {
            let mut spool = storage.lock().expect("the stream storage should lock");
            spool.seek(SeekFrom::End(0))?;
            spool.write_all(&bytes)?;
            written.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            Ok::<(), std::io::Error>(())
        })
        .await;
        match write {
            Ok(Ok(())) => {
                self.update.notify_one();
                Ok(())
            }
            Ok(Err(_)) | Err(_) => Err(()),
        }
    }

    pub(super) async fn read(&self, offset: u64, limit: u64) -> StreamRead {
        let storage = Arc::clone(&self.storage);
        let written = Arc::clone(&self.written);
        let limit = limit as usize;
        let read = tokio::task::spawn_blocking(move || {
            let mut spool = storage.lock().expect("the stream storage should lock");
            let available = written.load(Ordering::Relaxed).saturating_sub(offset) as usize;
            let to_read = available.min(limit);
            let mut bytes = vec![0_u8; to_read];
            spool.seek(SeekFrom::Start(offset))?;
            spool.read_exact(&mut bytes)?;
            Ok::<(Vec<u8>, bool), std::io::Error>((bytes, to_read < available))
        })
        .await;
        match read {
            Ok(Ok((bytes, more))) => StreamRead { bytes, more },
            Ok(Err(_)) | Err(_) => StreamRead {
                bytes: Vec::new(),
                more: false,
            },
        }
    }

    pub(super) async fn read_with_wait(
        &self,
        offset: u64,
        limit: u64,
        timeout: Duration,
    ) -> StreamRead {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let read = self.read(offset, limit).await;
            let eof = self.is_closed() && !read.more;
            if read.more || eof {
                return read;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return read;
            }
            let remaining = deadline.saturating_duration_since(now);
            if tokio::time::timeout(remaining, self.follow())
                .await
                .is_err()
            {
                return read;
            }
        }
    }

    pub(super) fn available(&self, offset: u64) -> bool {
        self.written.load(Ordering::Relaxed) > offset
    }

    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.update.notify_one();
    }

    pub(super) async fn follow(&self) {
        self.update.notified().await;
    }
}

pub(super) struct StreamRead {
    pub(super) bytes: Vec<u8>,
    pub(super) more: bool,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::StreamId;

    #[test]
    fn stream_identifiers_parse_from_and_display_their_prefix_form() {
        let identifier = StreamId::new();

        assert!(identifier.to_string().starts_with("stream_"));
        assert_eq!(StreamId::from_str(&identifier.to_string()), Ok(identifier));
        assert!(StreamId::from_str("exec_foo").is_err());
        assert!(StreamId::from_str("stream_not-a-uuid").is_err());
        assert!(StreamId::from_str("stream_123").is_err());
    }
}
