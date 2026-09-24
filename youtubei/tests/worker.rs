use std::{
    sync::{Arc, Mutex},
    thread::ThreadId,
    time::Duration,
};
use tokio::sync::oneshot;
use youtubei::{Engine, Text, Worker, json};

struct Request {
    text: String,
    reply: oneshot::Sender<youtubei::Result<String>>,
}

static_assertions::assert_impl_all!(Worker<Request>: Send, Sync, Clone);

struct State {
    engine: Engine,
    owner: ThreadId,
    barrier: tokio::sync::Barrier,
    dropped: Option<oneshot::Sender<ThreadId>>,
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = self
            .dropped
            .take()
            .unwrap()
            .send(std::thread::current().id());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn shared_worker_overlaps_calls_from_other_threads_and_drains_on_drop() {
    let (dropped, drop_thread) = oneshot::channel();
    let owner = Arc::new(Mutex::new(None));
    let initialized_owner = owner.clone();
    let worker = Worker::new(
        move || async move {
            let owner = std::thread::current().id();
            *initialized_owner.lock().unwrap() = Some(owner);
            State {
                engine: Engine::new().await.unwrap(),
                owner,
                barrier: tokio::sync::Barrier::new(2),
                dropped: Some(dropped),
            }
        },
        |state, request: Request| {
            Box::pin(async move {
                assert_eq!(std::thread::current().id(), state.owner);
                state.barrier.wait().await;
                // Hold accepted work while all sender handles are dropped.
                tokio::time::sleep(Duration::from_millis(30)).await;
                let result = async {
                    Text::new(&state.engine, json!({"simpleText": request.text}))
                        .await?
                        .to_string()
                        .await
                }
                .await;
                let _ = request.reply.send(result);
            })
        },
    )
    .unwrap();
    let mut callers = Vec::new();
    for text in ["First caller", "Second caller"] {
        let worker = worker.clone();
        callers.push(std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let (reply, response) = oneshot::channel();
                worker
                    .send(Request {
                        text: text.into(),
                        reply,
                    })
                    .await
                    .unwrap();
                drop(worker);
                let result = tokio::time::timeout(Duration::from_secs(2), response)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(result, text);
            });
        }));
    }
    drop(worker);
    for caller in callers {
        caller.join().unwrap();
    }
    let actual = tokio::time::timeout(Duration::from_secs(2), drop_thread)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Some(actual), *owner.lock().unwrap());
    assert_ne!(actual, std::thread::current().id());
}
