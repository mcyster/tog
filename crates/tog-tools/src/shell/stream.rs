use std::fmt::{Display, Formatter};
use std::io::{Read, Seek, SeekFrom, Write};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::SpooledTempFile;
use tokio::sync::Notify;

pub(crate) const SPOOL_MEMORY_THRESHOLD_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct StreamId(u64);

impl StreamId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }
}

impl Display for StreamId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "stream_{}", self.0)
    }
}

impl FromStr for StreamId {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let digits = text.strip_prefix("stream_").ok_or(())?;
        let value = digits.parse::<u64>().map_err(|_| ())?;
        Ok(Self(value))
    }
}

pub(crate) struct SharedStream {
    storage: Arc<Mutex<SpooledTempFile>>,
    written: Arc<AtomicU64>,
    update: Notify,
    closed: AtomicBool,
}

impl SharedStream {
    pub(crate) fn new(maximum_memory_bytes: usize) -> Self {
        Self {
            storage: Arc::new(Mutex::new(SpooledTempFile::new(maximum_memory_bytes))),
            written: Arc::new(AtomicU64::new(0)),
            update: Notify::new(),
            closed: AtomicBool::new(false),
        }
    }

    pub(crate) async fn append(&self, bytes: Vec<u8>) -> Result<(), ()> {
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

    pub(crate) async fn read(&self, offset: u64, limit: u64) -> StreamRead {
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

    pub(crate) async fn read_with_wait(
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

    pub(crate) fn available(&self, offset: u64) -> bool {
        self.written.load(Ordering::Relaxed) > offset
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.update.notify_one();
    }

    pub(crate) async fn follow(&self) {
        self.update.notified().await;
    }
}

pub(crate) struct StreamRead {
    pub(crate) bytes: Vec<u8>,
    pub(crate) more: bool,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::StreamId;

    #[test]
    fn stream_identifiers_parse_from_and_display_their_prefix_form() {
        let identifier = StreamId::new(3);

        assert_eq!(identifier.to_string(), "stream_3");
        assert_eq!(StreamId::from_str("stream_3"), Ok(identifier));
        assert!(StreamId::from_str("exec_3").is_err());
        assert!(StreamId::from_str("stream_abc").is_err());
    }
}
