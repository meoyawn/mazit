use super::{YouTube, client::Request, listing};
use serde_json::{Value, json};
use std::collections::VecDeque;

struct Responses(VecDeque<(&'static str, Value, Value)>);

impl Drop for Responses {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(self.0.is_empty(), "Unused YouTube responses");
        }
    }
}

struct Source<'a> {
    responses: &'a mut VecDeque<(&'static str, Value, Value)>,
    id: String,
    continuation: bool,
}

fn take(
    responses: &mut VecDeque<(&'static str, Value, Value)>,
    method: &str,
    args: Value,
) -> Value {
    let (expected_method, expected_args, result) =
        responses.pop_front().expect("Unexpected YouTube operation");
    assert_eq!(method, expected_method);
    assert_eq!(args, expected_args);
    result
}

impl listing::Source for Source<'_> {
    async fn next_page(&mut self) -> anyhow::Result<listing::Page> {
        let result = take(
            self.responses,
            "page",
            json!({"id": self.id, "continuation": self.continuation}),
        );
        self.continuation = true;
        Ok(serde_json::from_value(result)?)
    }
    async fn channel(&mut self, id: &str) -> anyhow::Result<listing::Channel> {
        Ok(serde_json::from_value(take(
            self.responses,
            "channel",
            json!({"id": id}),
        ))?)
    }
}

impl YouTube {
    pub(crate) fn with_responses(responses: Vec<(&'static str, Value, Value)>) -> Self {
        let sender = youtubei::Worker::new(
            move || async move { tokio::sync::Mutex::new(Responses(responses.into())) },
            |responses, request| {
                Box::pin(async move {
                    let mut responses = responses.lock().await;
                    match request {
                        Request::Resolve { url, reply } => {
                            let response = take(&mut responses.0, "resolve", json!({"url": url}));
                            let _ = reply.send(Ok(response["id"].as_str().unwrap().into()));
                        }
                        Request::Snapshot { kind, id, reply } => {
                            let playlist = if kind == "channel" {
                                format!("UU{}", &id[2..])
                            } else {
                                id.clone()
                            };
                            let mut source = Source {
                                responses: &mut responses.0,
                                id: playlist,
                                continuation: false,
                            };
                            let _ = reply.send(listing::snapshot(&mut source, &kind, &id).await);
                        }
                        Request::Media { id, reply } => {
                            let value = take(
                                &mut responses.0,
                                "media",
                                json!({"id": id, "client": "VISIONOS"}),
                            );
                            let _ = reply.send(serde_json::from_value(value).map_err(Into::into));
                        }
                    }
                })
            },
        )
        .unwrap();
        Self { sender }
    }
}
