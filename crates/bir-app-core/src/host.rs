//! The JSON-shaped call surface the ported screen, taxpayer and dashboard
//! functions were written against.
//!
//! The old shell passed these values across a script boundary; here they
//! stay in process. `HostValue` is a JSON value with insertion-ordered
//! objects, `HostArguments` the positional arguments of one call, and
//! `HostModule` a name -> function table, so every ported function stays
//! reachable by name (`BirApp::call`) until the typed facade covers it.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

/// One value crossing the JSON-shaped call surface.
#[derive(Clone, Debug, PartialEq)]
pub enum HostValue {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Array(Vec<HostValue>),
    /// Insertion-ordered fields.
    Object(Vec<(String, HostValue)>),
}

impl HostValue {
    pub fn is_null(&self) -> bool {
        matches!(self, HostValue::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            HostValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            HostValue::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            HostValue::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[HostValue]> {
        match self {
            HostValue::Array(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, HostValue)]> {
        match self {
            HostValue::Object(fields) => Some(fields),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&HostValue> {
        self.as_object()?
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// The type name used in error messages.
    pub fn describe(&self) -> &'static str {
        match self {
            HostValue::Null => "null",
            HostValue::Bool(_) => "a boolean",
            HostValue::Number(_) => "a number",
            HostValue::Str(_) => "a string",
            HostValue::Array(_) => "an array",
            HostValue::Object(_) => "an object",
        }
    }
}

impl From<bool> for HostValue {
    fn from(value: bool) -> Self {
        HostValue::Bool(value)
    }
}

macro_rules! from_number {
    ($($type:ty),* $(,)?) => {
        $(
            impl From<$type> for HostValue {
                fn from(value: $type) -> Self {
                    HostValue::Number(value as f64)
                }
            }
        )*
    };
}

from_number!(f32, f64, i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl From<String> for HostValue {
    fn from(value: String) -> Self {
        HostValue::Str(value)
    }
}

impl From<&str> for HostValue {
    fn from(value: &str) -> Self {
        HostValue::Str(value.to_owned())
    }
}

impl<T: Into<HostValue>> From<Vec<T>> for HostValue {
    fn from(values: Vec<T>) -> Self {
        HostValue::Array(values.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<HostValue>> From<Option<T>> for HostValue {
    fn from(value: Option<T>) -> Self {
        value.map(Into::into).unwrap_or(HostValue::Null)
    }
}

/// Builds a [`HostValue::Object`] one field at a time, in order.
#[derive(Clone, Debug, Default)]
pub struct HostObject {
    fields: Vec<(String, HostValue)>,
}

impl HostObject {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a field; a repeated name replaces the earlier value in place.
    pub fn field(mut self, name: impl Into<String>, value: impl Into<HostValue>) -> Self {
        let name = name.into();
        let value = value.into();
        match self.fields.iter_mut().find(|(key, _)| *key == name) {
            Some(existing) => existing.1 = value,
            None => self.fields.push((name, value)),
        }
        self
    }
}

impl From<HostObject> for HostValue {
    fn from(object: HostObject) -> Self {
        HostValue::Object(object.fields)
    }
}

/// The positional arguments of one call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostArguments {
    values: Vec<HostValue>,
}

impl HostArguments {
    pub fn new(values: impl IntoIterator<Item = HostValue>) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&HostValue> {
        self.values.get(index)
    }

    pub fn value(&self, index: usize) -> Result<&HostValue, HostError> {
        self.values.get(index).ok_or_else(|| {
            HostError::new(format!(
                "argument {} is missing; {} were passed",
                index + 1,
                self.values.len()
            ))
        })
    }

    pub fn string(&self, index: usize) -> Result<&str, HostError> {
        let value = self.value(index)?;
        value
            .as_str()
            .ok_or_else(|| mistyped(index, "a string", value))
    }

    pub fn number(&self, index: usize) -> Result<f64, HostError> {
        let value = self.value(index)?;
        value
            .as_number()
            .ok_or_else(|| mistyped(index, "a number", value))
    }

    /// A number that has to be whole.
    pub fn integer(&self, index: usize) -> Result<i64, HostError> {
        let number = self.number(index)?;
        if number.fract() != 0. {
            return Err(HostError::new(format!(
                "argument {} must be a whole number, got {number}",
                index + 1
            )));
        }
        Ok(number as i64)
    }

    pub fn boolean(&self, index: usize) -> Result<bool, HostError> {
        let value = self.value(index)?;
        value
            .as_bool()
            .ok_or_else(|| mistyped(index, "a boolean", value))
    }
}

fn mistyped(index: usize, expected: &str, got: &HostValue) -> HostError {
    HostError::new(format!(
        "argument {} must be {expected}, got {}",
        index + 1,
        got.describe()
    ))
}

/// A function said no, in one sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostError {
    message: String,
}

impl HostError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HostError {}

impl From<String> for HostError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for HostError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

pub type HostResult = Result<HostValue, HostError>;

type HostFunction = Arc<dyn Fn(&HostArguments) -> HostResult + Send + Sync>;

/// Name -> function table of the ported JSON-shaped functions.
///
/// The old asynchronous functions read the database first and hand back a
/// future that does the CPU work; here the future is run to completion on
/// the calling thread, so every call is synchronous. Call from a worker
/// thread, never the UI thread, for anything that touches the database.
#[derive(Clone, Default)]
pub struct HostModule {
    functions: BTreeMap<String, HostFunction>,
}

impl HostModule {
    pub fn new(_name: impl Into<String>) -> Self {
        Self::default()
    }

    pub fn function(
        mut self,
        name: impl Into<String>,
        body: impl Fn(&HostArguments) -> HostResult + Send + Sync + 'static,
    ) -> Self {
        self.functions.insert(name.into(), Arc::new(body));
        self
    }

    pub fn async_function<F>(
        mut self,
        name: impl Into<String>,
        body: impl Fn(&HostArguments) -> Result<F, HostError> + Send + Sync + 'static,
    ) -> Self
    where
        F: Future<Output = HostResult> + Send + 'static,
    {
        self.functions.insert(
            name.into(),
            Arc::new(move |arguments| block_on(body(arguments)?)),
        );
        self
    }

    pub fn function_names(&self) -> impl Iterator<Item = &str> {
        self.functions.keys().map(String::as_str)
    }

    pub fn has(&self, function: &str) -> bool {
        self.functions.contains_key(function)
    }

    pub fn call(&self, function: &str, arguments: &HostArguments) -> HostResult {
        let body = self
            .functions
            .get(function)
            .ok_or_else(|| HostError::new(format!("no function `{function}`")))?;
        body(arguments)
    }
}

struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Run a future to completion on this thread. The ported futures do plain
/// CPU work and never wait on I/O, so this returns on the first poll.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        match Pin::as_mut(&mut future).poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}
