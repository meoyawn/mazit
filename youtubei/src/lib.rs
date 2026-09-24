//! Rust object handles for the bundled youtubei.js library.
//!
//! Create one [`Engine`], then reuse [`Innertube`] and the objects it returns.
//! Handles retain their engine and preserve JavaScript identity and prototypes.
//! They are deliberately `!Send` and `!Sync`: use a Tokio `LocalSet` or a
//! current-thread executor. Calls can overlap on that thread while awaiting I/O.
//! See the crate README for the boundary between typed and dynamic bindings.

mod api;
mod engine;
mod error;
mod fetch;
pub mod models;
mod options;
mod platform;
mod value;

pub use api::*;
pub use engine::{Engine, EngineOptions};
pub use error::{Error, Result};
pub use fetch::{FetchRequest, FetchResponse};
pub use options::{BrowseOptions, Client, GetVideoInfoOptions, SessionOptions};
pub use value::{Argument, JsValue};

/// The exact QuickJS binding used by [`Engine::value_with`] and [`JsValue::with`].
pub use rquickjs;
pub use serde_json::{Value as Json, json};
