//! A Send channel connects multithreaded Rust callers to a thread-local engine.
//! Start one worker for shared state, or several workers for independent engines.
use youtubei::{Innertube, SessionOptions, Text, json};

struct Request {
    text: String,
    reply: tokio::sync::oneshot::Sender<youtubei::Result<String>>,
}

fn worker() -> (
    tokio::sync::mpsc::Sender<Request>,
    std::thread::JoinHandle<()>,
) {
    let (send, mut receive) = tokio::sync::mpsc::channel::<Request>(32);
    let thread = std::thread::spawn(move || {
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        executor.block_on(async move {
            // This instance is created, reused, and dropped on this worker.
            let yt = Innertube::create(SessionOptions::local()).await.unwrap();
            while let Some(request) = receive.recv().await {
                let result = async {
                    // Replace with get_basic_info/get_playlist/etc. as needed.
                    Text::new(yt.engine(), json!({"simpleText": request.text}))
                        .await?
                        .to_string()
                        .await
                }
                .await;
                // Only owned Rust data crosses the thread boundary.
                let _ = request.reply.send(result);
            }
        });
    });
    (send, thread)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (send, thread) = worker();
    for text in ["First call", "Same engine, second call"] {
        let (reply, result) = tokio::sync::oneshot::channel();
        send.send(Request {
            text: text.into(),
            reply,
        })
        .await?;
        println!("{}", result.await??);
    }
    drop(send);
    thread.join().expect("worker exited");
    Ok(())
}
