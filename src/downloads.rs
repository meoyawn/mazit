use parking_lot::RwLock;
use std::{sync::Arc, time::Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

/// One budget for the entire library, including resolution, conversion and upload.
pub const CONCURRENT_TRANSFERS: usize = 4;
pub const RANGES_PER_TRANSFER: usize = 4;
pub const RANGE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Resolving,
    Downloading,
    Preparing,
    Uploading,
    Retrying,
    Complete,
    Failed,
}

impl Phase {
    pub fn active(self) -> bool {
        !matches!(self, Self::Queued | Self::Complete | Self::Failed)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Resolving => "Resolving audio",
            Self::Downloading => "Downloading",
            Self::Preparing => "Preparing M4A",
            Self::Uploading => "Uploading",
            Self::Retrying => "Retrying",
            Self::Complete => "Complete",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangePhase {
    Waiting,
    Active,
    Retrying,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
    pub received: u64,
    pub phase: RangePhase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    pub id: String,
    pub source_id: String,
    pub source_title: String,
    pub title: String,
    pub phase: Phase,
    pub attempt: usize,
    pub total: u64,
    pub ranges: Vec<ByteRange>,
    pub error: Option<String>,
    started: Option<Instant>,
}

impl Download {
    pub fn received(&self) -> u64 {
        self.ranges.iter().map(|range| range.received).sum()
    }

    pub fn bytes_per_second(&self) -> u64 {
        if self.phase != Phase::Downloading {
            return 0;
        }
        self.started
            .map(|started| {
                (self.received() as f64 / started.elapsed().as_secs_f64().max(0.1)) as u64
            })
            .unwrap_or(0)
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub items: Vec<Download>,
    pub paused: bool,
}

#[derive(Clone)]
pub struct DownloadManager {
    state: Arc<RwLock<Snapshot>>,
    slots: Arc<Semaphore>,
    paused: watch::Sender<bool>,
}

impl Default for DownloadManager {
    fn default() -> Self {
        Self {
            state: Arc::default(),
            slots: Arc::new(Semaphore::new(CONCURRENT_TRANSFERS)),
            paused: watch::channel(false).0,
        }
    }
}

impl DownloadManager {
    pub fn snapshot(&self) -> Snapshot {
        self.state.read().clone()
    }

    /// Pause only admissions; in-flight transfers finish and keep their checkpoints.
    pub fn set_paused(&self, paused: bool) {
        let mut state = self.state.write();
        state.paused = paused;
        self.paused.send_replace(paused);
    }

    /// Replace the previous sync's history for this source, retaining other podcasts.
    pub fn enqueue(
        &self,
        source_id: &str,
        source_title: &str,
        episodes: &[(String, String)],
    ) -> Vec<Transfer> {
        let mut state = self.state.write();
        state.items.retain(|item| item.source_id != source_id);
        episodes
            .iter()
            .map(|(video_id, title)| {
                let id = format!("{source_id}/{video_id}");
                state.items.push(Download {
                    id: id.clone(),
                    source_id: source_id.into(),
                    source_title: source_title.into(),
                    title: title.clone(),
                    phase: Phase::Queued,
                    attempt: 0,
                    total: 0,
                    ranges: Vec::new(),
                    error: None,
                    started: None,
                });
                Transfer {
                    manager: self.clone(),
                    id,
                }
            })
            .collect()
    }
}

#[derive(Clone)]
pub struct Transfer {
    manager: DownloadManager,
    id: String,
}

impl Transfer {
    fn update(&self, update: impl FnOnce(&mut Download)) {
        if let Some(item) = self
            .manager
            .state
            .write()
            .items
            .iter_mut()
            .find(|item| item.id == self.id)
        {
            update(item);
        }
    }

    pub async fn acquire(&self) -> ActiveTransfer {
        let mut paused = self.manager.paused.subscribe();
        loop {
            paused
                .wait_for(|paused| !paused)
                .await
                .expect("Manager owns pause channel");
            let permit = self
                .manager
                .slots
                .clone()
                .acquire_owned()
                .await
                .expect("Download slots stay open");
            let mut state = self.manager.state.write();
            if !state.paused {
                if let Some(item) = state.items.iter_mut().find(|item| item.id == self.id) {
                    item.phase = Phase::Resolving;
                }
                return ActiveTransfer {
                    transfer: self.clone(),
                    _permit: permit,
                    finished: false,
                };
            }
            // Pause may have arrived while this item waited for a slot.
            drop(state);
            drop(permit);
        }
    }

    pub fn attempt(&self, attempt: usize) {
        self.update(|item| {
            item.phase = Phase::Resolving;
            item.attempt = attempt;
            item.error = None;
            item.total = 0;
            item.ranges.clear();
            item.started = None;
        });
    }

    pub fn phase(&self, phase: Phase) {
        self.update(|item| item.phase = phase);
    }

    pub fn error(&self, error: &anyhow::Error) {
        self.update(|item| item.error = Some(crate::redact(&format!("{error:#}"))));
    }

    pub fn start_download(&self, total: u64, chunk_bytes: usize) {
        self.update(|item| {
            item.phase = Phase::Downloading;
            item.total = total;
            item.started = Some(Instant::now());
            item.ranges = (0..total)
                .step_by(chunk_bytes)
                .map(|start| ByteRange {
                    start,
                    end: (start + chunk_bytes as u64).min(total) - 1,
                    received: 0,
                    phase: RangePhase::Waiting,
                })
                .collect();
        });
    }

    pub fn range(&self, start: u64, received: u64, phase: RangePhase) {
        self.update(|item| {
            if let Ok(index) = item
                .ranges
                .binary_search_by_key(&start, |range| range.start)
            {
                let range = &mut item.ranges[index];
                range.received = received.min(range.end - range.start + 1);
                range.phase = phase;
            }
        });
    }
}

pub struct ActiveTransfer {
    transfer: Transfer,
    _permit: OwnedSemaphorePermit,
    finished: bool,
}

impl ActiveTransfer {
    pub fn finish(mut self, result: &anyhow::Result<()>) {
        if let Err(error) = result {
            self.transfer.error(error);
        }
        self.transfer.phase(if result.is_ok() {
            Phase::Complete
        } else {
            Phase::Failed
        });
        self.finished = true;
    }
}

impl Drop for ActiveTransfer {
    fn drop(&mut self) {
        if !self.finished {
            self.transfer.update(|item| {
                item.phase = Phase::Failed;
                item.error = Some("Transfer interrupted; refresh the subscription to retry".into());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::poll;

    fn enqueue(manager: &DownloadManager, source: &str, count: usize) -> Vec<Transfer> {
        manager.enqueue(
            source,
            source,
            &(0..count)
                .map(|index| (index.to_string(), format!("Episode {index}")))
                .collect::<Vec<_>>(),
        )
    }

    #[tokio::test]
    async fn podcasts_share_one_budget_and_pause_applies_to_waiting_admissions() {
        let manager = DownloadManager::default();
        let first = enqueue(&manager, "first", CONCURRENT_TRANSFERS);
        let second = enqueue(&manager, "second", 2);
        let mut active = Vec::new();
        for transfer in &first {
            active.push(transfer.acquire().await);
        }
        let mut waiting = Box::pin(second[0].acquire());
        assert!(poll!(&mut waiting).is_pending());
        assert_eq!(
            manager
                .snapshot()
                .items
                .iter()
                .filter(|item| item.phase.active())
                .count(),
            CONCURRENT_TRANSFERS
        );
        assert_eq!(
            manager.snapshot().items.last().unwrap().phase,
            Phase::Queued
        );
        manager.set_paused(true);
        active.pop().unwrap().finish(&Ok(()));
        assert!(poll!(&mut waiting).is_pending());
        manager.set_paused(false);
        let admitted = waiting.await;
        assert_eq!(
            manager
                .snapshot()
                .items
                .iter()
                .filter(|item| item.phase.active())
                .count(),
            CONCURRENT_TRANSFERS
        );
        // Cancellation releases its slot and reports an actionable failure.
        drop(admitted);
        let next = second[1].acquire().await;
        assert_eq!(
            manager.snapshot().items[CONCURRENT_TRANSFERS].phase,
            Phase::Failed
        );
        next.finish(&Ok(()));
        for item in active {
            item.finish(&Ok(()));
        }
        assert_eq!(manager.slots.available_permits(), CONCURRENT_TRANSFERS);
    }

    #[tokio::test]
    async fn retry_resets_ranges_and_failure_does_not_block_other_podcasts() {
        let manager = DownloadManager::default();
        let transfers = enqueue(&manager, "first", 1);
        let transfer = &transfers[0];
        let active = transfer.acquire().await;
        transfer.attempt(1);
        transfer.start_download(10, 4);
        transfer.range(4, 4, RangePhase::Complete);
        transfer.range(0, 2, RangePhase::Active);
        assert_eq!(manager.snapshot().items[0].received(), 6);
        transfer.range(0, 0, RangePhase::Retrying);
        assert_eq!(manager.snapshot().items[0].received(), 4);
        transfer.error(&anyhow::anyhow!(
            "Failed https://secret.example/audio?token=private"
        ));
        assert!(
            !manager.snapshot().items[0]
                .error
                .as_ref()
                .unwrap()
                .contains("token")
        );
        transfer.attempt(2);
        let item = &manager.snapshot().items[0];
        assert!(item.ranges.is_empty());
        assert!(item.error.is_none());
        assert_eq!(item.total, 0);
        active.finish(&Err(anyhow::anyhow!("Upload failed")));
        let second = enqueue(&manager, "second", 1);
        second[0].acquire().await.finish(&Ok(()));
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.items[0].phase, Phase::Failed);
        assert_eq!(snapshot.items[1].phase, Phase::Complete);
    }
}
