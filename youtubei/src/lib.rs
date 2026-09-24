//! Rust object handles for the bundled youtubei.js library.
//!
//! Create one [`Engine`], then reuse [`Innertube`] and the objects it returns.
//! Handles retain their engine and preserve JavaScript identity and prototypes.
//! Use the cloneable [`Worker`] to share local engine state safely across caller
//! threads. It owns initialization, scheduling, and destruction. Raw handles
//! remain `!Send` and `!Sync`; advanced callers can use them on a Tokio `LocalSet`
//! or current-thread executor. Concurrent calls retain independent wake-ups.
//! See the crate README for the boundary between typed and dynamic bindings.

mod api;
mod engine;
mod error;
mod fetch;
pub mod models;
mod options;
mod platform;
mod value;
mod wake;
mod worker;

pub use api::*;
pub use engine::{Engine, EngineOptions};
pub use error::{Error, Result};
pub use fetch::{FetchRequest, FetchResponse};
pub use options::{BrowseOptions, Client, GetVideoInfoOptions, SessionOptions};
pub use value::{Argument, JsValue};
pub use worker::Worker;

/// The exact QuickJS binding used by [`Engine::value_with`] and [`JsValue::with`].
pub use rquickjs;
pub use serde_json::{Value as Json, json};
