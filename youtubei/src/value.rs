use rquickjs::{
    Array, Ctx, FromJs, Function, IntoJs, Object, Persistent, Value, async_with,
    function::{Args, Constructor},
    promise::MaybePromise,
};
use serde::de::DeserializeOwned;

use crate::{Engine, Error, Json, Result};

/// A reference to an actual JS value, retaining its engine, prototype and identity.
/// Cloning clones the reference, not the JavaScript object.
#[derive(Clone)]
pub struct JsValue {
    // Drop the rooted value before releasing the last engine reference.
    pub(crate) value: Persistent<Value<'static>>,
    pub(crate) engine: Engine,
}

impl std::fmt::Debug for JsValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsValue").finish_non_exhaustive()
    }
}

/// Arguments retain JS objects and distinguish `undefined` from JSON `null`.
#[derive(Clone, Debug)]
pub enum Argument {
    Undefined,
    Json(Json),
    Value(JsValue),
}

impl From<Json> for Argument {
    fn from(v: Json) -> Self {
        Self::Json(v)
    }
}
impl From<JsValue> for Argument {
    fn from(v: JsValue) -> Self {
        Self::Value(v)
    }
}
impl From<&JsValue> for Argument {
    fn from(v: &JsValue) -> Self {
        Self::Value(v.clone())
    }
}
impl From<&str> for Argument {
    fn from(v: &str) -> Self {
        Self::Json(v.into())
    }
}
impl From<String> for Argument {
    fn from(v: String) -> Self {
        Self::Json(v.into())
    }
}
impl From<bool> for Argument {
    fn from(v: bool) -> Self {
        Self::Json(v.into())
    }
}
impl From<u32> for Argument {
    fn from(v: u32) -> Self {
        Self::Json(v.into())
    }
}

impl Argument {
    pub(crate) fn to_js<'js>(&self, ctx: &Ctx<'js>) -> rquickjs::Result<Value<'js>> {
        match self {
            Self::Undefined => Ok(Value::new_undefined(ctx.clone())),
            Self::Json(v) => json_to_js(ctx, v),
            Self::Value(v) => v.value.clone().restore(ctx),
        }
    }
}

pub(crate) fn json_to_js<'js>(ctx: &Ctx<'js>, v: &Json) -> rquickjs::Result<Value<'js>> {
    match v {
        Json::Null => Ok(Value::new_null(ctx.clone())),
        Json::Bool(v) => v.into_js(ctx),
        Json::String(v) => v.as_str().into_js(ctx),
        Json::Number(v) => v.as_f64().unwrap_or(f64::NAN).into_js(ctx),
        Json::Array(values) => {
            let array = Array::new(ctx.clone())?;
            for (i, value) in values.iter().enumerate() {
                array.set(i, json_to_js(ctx, value)?)?;
            }
            Ok(array.into_value())
        }
        Json::Object(values) => {
            let object = Object::new(ctx.clone())?;
            for (key, value) in values {
                object.set(key.as_str(), json_to_js(ctx, value)?)?;
            }
            Ok(object.into_value())
        }
    }
}

impl JsValue {
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Work with the native value from Rust, including symbols and callbacks.
    /// The return type must be owned; JS values cannot escape the context.
    pub async fn with<T, F>(&self, f: F) -> Result<T>
    where
        F: for<'js> FnOnce(Ctx<'js>, Value<'js>) -> rquickjs::Result<T>,
    {
        self.engine
            .0
            .context
            .with(|ctx| {
                self.value
                    .clone()
                    .restore(&ctx)
                    .and_then(|v| f(ctx.clone(), v))
                    .map_err(|e| Error::caught(&ctx, e))
            })
            .await
    }

    pub async fn get(&self, key: &str) -> Result<Self> {
        let value = self
            .with(|ctx, v| {
                let object = Object::from_js(&ctx, v)?;
                Ok(Persistent::save(&ctx, object.get::<_, Value>(key)?))
            })
            .await?;
        Ok(self.engine.retain(value))
    }

    pub async fn set(&self, key: &str, value: impl Into<Argument>) -> Result<()> {
        let value = value.into();
        self.with(|ctx, v| Object::from_js(&ctx, v)?.set(key, value.to_js(&ctx)?))
            .await
    }

    /// Invoke a member with the original receiver as `this`, then await it.
    pub async fn call(&self, method: &str, arguments: &[Argument]) -> Result<Self> {
        self.invoke(Some(method), arguments, false, None).await
    }

    /// Call a function value with an explicit receiver (or `undefined`).
    pub async fn apply(&self, this: Option<&Self>, arguments: &[Argument]) -> Result<Self> {
        self.invoke(None, arguments, false, this).await
    }

    /// Invoke an upstream constructor with `new`, preserving its prototype.
    pub async fn construct(&self, arguments: &[Argument]) -> Result<Self> {
        self.invoke(None, arguments, true, None).await
    }

    async fn invoke(
        &self,
        method: Option<&str>,
        arguments: &[Argument],
        construct: bool,
        this: Option<&Self>,
    ) -> Result<Self> {
        let value = async_with!(self.engine.0.context => |ctx| {
            let result = async {
                let value = self.value.clone().restore(&ctx)?;
                let mut args = Args::new(ctx.clone(), arguments.len());
                for arg in arguments { args.push_arg(arg.to_js(&ctx)?)?; }
                let result: Value = if construct {
                    args.construct(&Constructor::from_js(&ctx, value)?)?
                } else {
                    let function = if let Some(method) = method {
                        let object = Object::from_js(&ctx, value)?;
                        args.this(object.clone())?;
                        object.get::<_, Function>(method)?
                    } else {
                        if let Some(this) = this { args.this(this.value.clone().restore(&ctx)?)?; }
                        Function::from_js(&ctx, value)?
                    };
                    args.apply(&function)?
                };
                let value: Value = MaybePromise::from_value(result).into_future().await?;
                Ok(Persistent::save(&ctx, value))
            }.await;
            result.map_err(|e| Error::caught(&ctx, e))
        })
        .await?;
        Ok(self.engine.retain(value))
    }

    /// Await a promise value without invoking it.
    pub async fn resolve(&self) -> Result<Self> {
        let value = async_with!(self.engine.0.context => |ctx| {
            let result = async {
                let value = self.value.clone().restore(&ctx)?;
                let value: Value = MaybePromise::from_value(value).into_future().await?;
                Ok(Persistent::save(&ctx, value))
            }.await;
            result.map_err(|e| Error::caught(&ctx, e))
        })
        .await?;
        Ok(self.engine.retain(value))
    }

    pub async fn read<T>(&self) -> Result<T>
    where
        T: for<'js> FromJs<'js>,
    {
        self.with(|ctx, value| T::from_js(&ctx, value)).await
    }

    pub async fn is_nullish(&self) -> Result<bool> {
        self.with(|_, value| Ok(value.is_null() || value.is_undefined()))
            .await
    }

    pub async fn same_identity(&self, other: &Self) -> Result<bool> {
        self.with(|ctx, value| Ok(value == other.value.clone().restore(&ctx)?))
            .await
    }

    pub async fn elements(&self) -> Result<Vec<Self>> {
        let values = self
            .with(|ctx, value| {
                // youtubei.js ObservedArray is a Proxy, not a native QuickJS array value.
                let array_class: Object = ctx.globals().get("Array")?;
                let is_array: bool = array_class
                    .get::<_, Function>("isArray")?
                    .call((value.clone(),))?;
                if !is_array {
                    return Err(rquickjs::Error::new_from_js(
                        value.type_of().as_str(),
                        "array",
                    ));
                }
                let array = Object::from_js(&ctx, value)?;
                let length: u32 = array.get("length")?;
                (0..length)
                    .map(|index| {
                        array
                            .get::<_, Value>(index)
                            .map(|v| Persistent::save(&ctx, v))
                    })
                    .collect::<rquickjs::Result<Vec<_>>>()
            })
            .await?;
        Ok(values.into_iter().map(|v| self.engine.retain(v)).collect())
    }

    /// Explicit snapshot for data-only values. Cycles, BigInts, functions and
    /// `undefined` are not JSON; use native handles/`with` for these instead.
    pub async fn deserialize<T: DeserializeOwned>(&self) -> Result<T> {
        let text = self
            .with(|ctx, value| {
                ctx.json_stringify(value)?
                    .ok_or_else(|| rquickjs::Error::new_from_js("undefined", "JSON"))?
                    .to_string()
            })
            .await?;
        Ok(serde_json::from_str(&text)?)
    }
}
