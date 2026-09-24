use std::{
    future::{Future, poll_fn},
    pin::pin,
    sync::{Arc, Mutex},
    task::{Context, Wake, Waker},
};

/// QuickJS stores one scheduler waker for the whole runtime. Keep that waker
/// stable and forward notifications to every pending Rust caller, including
/// callers in separate tasks or FuturesUnordered entries.
#[derive(Default)]
pub(crate) struct RuntimeWake {
    callers: Mutex<Vec<Option<Waker>>>,
}

impl RuntimeWake {
    pub(crate) async fn run<F: Future>(self: &Arc<Self>, future: F) -> F::Output {
        let mut future = pin!(future);
        let waker = Waker::from(self.clone());
        let mut registration = Registration {
            wake: self.clone(),
            slot: None,
        };
        poll_fn(|cx| {
            registration.update(cx.waker());
            future.as_mut().poll(&mut Context::from_waker(&waker))
        })
        .await
    }
}

impl Wake for RuntimeWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let callers: Vec<_> = self
            .callers
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .cloned()
            .collect();
        // Never invoke executor callbacks while holding the registration lock.
        for caller in callers {
            caller.wake();
        }
    }
}

struct Registration {
    wake: Arc<RuntimeWake>,
    slot: Option<usize>,
}

impl Registration {
    fn update(&mut self, waker: &Waker) {
        let mut callers = self.wake.callers.lock().unwrap();
        let slot = *self.slot.get_or_insert_with(|| {
            callers.iter().position(Option::is_none).unwrap_or_else(|| {
                callers.push(None);
                callers.len() - 1
            })
        });
        if !callers[slot]
            .as_ref()
            .is_some_and(|old| old.will_wake(waker))
        {
            callers[slot] = Some(waker.clone());
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(slot) = self.slot {
            self.wake.callers.lock().unwrap()[slot] = None;
        }
    }
}
