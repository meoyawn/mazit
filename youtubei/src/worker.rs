use std::{future::Future, pin::Pin};

use futures_util::{StreamExt, stream::FuturesUnordered};
use tokio::sync::mpsc;

use crate::{Error, Result};

/// A cloneable, thread-safe queue for operations on reusable local JS state.
///
/// Initialization, handlers, and state destruction run on a crate-owned thread.
/// State and handler futures may contain non-Send engine/object handles; only
/// requests (and any reply channels they contain) cross the thread boundary.
/// Up to 16 handlers overlap, with 32 additional requests buffered. Dropping all
/// worker handles drains accepted requests before destroying the local state.
pub struct Worker<Q> {
    sender: mpsc::Sender<Q>,
}

impl<Q> Clone for Worker<Q> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<Q: Send + 'static> Worker<Q> {
    pub fn new<S, I, F, H>(initialize: I, handle: H) -> Result<Self>
    where
        S: 'static,
        I: FnOnce() -> F + Send + 'static,
        F: Future<Output = S> + 'static,
        H: for<'a> Fn(&'a S, Q) -> Pin<Box<dyn Future<Output = ()> + 'a>> + Send + 'static,
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| Error::new(format!("Create YouTube worker runtime: {error}")))?;
        let (sender, mut receiver) = mpsc::channel(32);
        std::thread::Builder::new()
            .name("youtubei".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let state = initialize().await;
                    let mut active = FuturesUnordered::new();
                    loop {
                        tokio::select! {
                            request = receiver.recv(), if active.len() < 16 => {
                                match request {
                                    Some(request) => active.push(handle(&state, request)),
                                    None => break,
                                }
                            }
                            Some(()) = active.next(), if !active.is_empty() => {}
                        }
                    }
                    while active.next().await.is_some() {}
                })
            })
            .map_err(|error| Error::new(format!("Start YouTube worker: {error}")))?;
        Ok(Self { sender })
    }

    /// Queue an owned request from any executor thread, with backpressure.
    pub async fn send(&self, request: Q) -> Result<()> {
        self.sender
            .send(request)
            .await
            .map_err(|_| Error::new("YouTube worker stopped"))
    }
}
