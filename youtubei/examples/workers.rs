//! A cloneable worker safely shares one session between Rust caller threads.
use youtubei::{Innertube, SessionOptions, Text, Worker, json};

struct Request {
    text: String,
    reply: tokio::sync::oneshot::Sender<youtubei::Result<String>>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let worker = Worker::new(
        || async { Innertube::create(SessionOptions::local()).await },
        |session, request: Request| {
            Box::pin(async move {
                let result = async {
                    let yt = session
                        .as_ref()
                        .map_err(|error| youtubei::Error::new(error.to_string()))?;
                    Text::new(yt.engine(), json!({"simpleText": request.text}))
                        .await?
                        .to_string()
                        .await
                }
                .await;
                let _ = request.reply.send(result);
            })
        },
    )?;
    for text in ["First call", "Same engine, second call"] {
        let shared = worker.clone();
        let (reply, result) = tokio::sync::oneshot::channel();
        shared
            .send(Request {
                text: text.into(),
                reply,
            })
            .await?;
        println!("{}", result.await??);
    }
    Ok(())
}
