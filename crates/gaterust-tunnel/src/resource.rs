use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::config::{MAX_DATA_STREAMS, MAX_QUEUED_UDP_BYTES, MAX_UDP_SESSIONS};

#[derive(Clone)]
pub(crate) struct ResourceBudget {
    data_streams: Arc<Semaphore>,
    udp_sessions: Arc<Semaphore>,
    queued_udp_bytes: Arc<Semaphore>,
}

impl ResourceBudget {
    pub(crate) fn new() -> Self {
        Self {
            data_streams: Arc::new(Semaphore::new(MAX_DATA_STREAMS)),
            udp_sessions: Arc::new(Semaphore::new(MAX_UDP_SESSIONS)),
            queued_udp_bytes: Arc::new(Semaphore::new(MAX_QUEUED_UDP_BYTES)),
        }
    }

    pub(crate) fn try_data_stream(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.data_streams).try_acquire_owned().ok()
    }

    pub(crate) fn try_udp_session(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.udp_sessions).try_acquire_owned().ok()
    }

    pub(crate) fn try_queue_udp_bytes(&self, bytes: usize) -> Option<OwnedSemaphorePermit> {
        let bytes = u32::try_from(bytes).ok()?;
        Arc::clone(&self.queued_udp_bytes)
            .try_acquire_many_owned(bytes)
            .ok()
    }
}
