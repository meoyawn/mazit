use rquickjs::{Coerced, Ctx, FromJs};

pub type Result<T> = std::result::Result<T, Error>;

/// An owned error; no JavaScript value escapes a locked context on failure.
#[derive(Debug)]
pub struct Error {
    pub message: String,
    pub stack: Option<String>,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self::message(message)
    }
    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            stack: None,
        }
    }

    pub(crate) fn caught(ctx: &Ctx<'_>, error: rquickjs::Error) -> Self {
        if !error.is_exception() {
            return Self::message(error.to_string());
        }
        let value = ctx.catch();
        let stack = value.as_exception().and_then(|e| e.stack());
        let message = Coerced::<String>::from_js(ctx, value)
            .map(|s| s.0)
            .unwrap_or_else(|_| "JavaScript exception".into());
        Self { message, stack }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<rquickjs::Error> for Error {
    fn from(value: rquickjs::Error) -> Self {
        Self::message(value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::message(value.to_string())
    }
}
