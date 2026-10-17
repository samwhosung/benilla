//! mlua's `Error`, the variants benilla's code builds or matches, with mlua's `Display`. It is
//! `Send` and `Sync`, as mlua's is: a Lua error value it carries waits on its state's thread
//! ([`crate::ErrorValue`]), and the error holds only its tag.

use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Error {
    SyntaxError {
        message: String,
        incomplete_input: bool,
        /// The message's bytes, as the compiler wrote them: `message` is their lossy text.
        raw: Vec<u8>,
    },
    RuntimeError(String),
    /// A Lua error value, raised in Lua as it is, with no position: one a call into Lua answered
    /// (`message` its text), which a callback that passes it on raises again unchanged, as a C
    /// function `lua_error`s the value its `lua_pcall` left; or, with no value, `message` raised
    /// bare, as the client's `PlaySound` raises its usage text (`0x458708`). Its `Display` is a
    /// [`Error::RuntimeError`]'s.
    LuaValue {
        message: String,
        value: Option<crate::ErrorValue>,
    },
    MemoryError(String),
    /// A callback's arguments and results overflow the stack a C function may use.
    StackError,
    BadArgument {
        to: Option<String>,
        pos: usize,
        name: Option<String>,
        cause: Arc<Error>,
    },
    ToLuaConversionError {
        from: String,
        to: &'static str,
        message: Option<String>,
    },
    FromLuaConversionError {
        from: &'static str,
        to: String,
        message: Option<String>,
    },
    CallbackError {
        traceback: String,
        cause: Arc<Error>,
    },
    WithContext {
        context: String,
        cause: Arc<Error>,
    },
    ExternalError(Arc<dyn std::error::Error + Send + Sync>),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn runtime<T: fmt::Display>(message: T) -> Self {
        Error::RuntimeError(message.to_string())
    }

    /// `message` raised as it is, with no position: `lua_error`, not `luaL_error`.
    pub fn bare<T: fmt::Display>(message: T) -> Self {
        Error::LuaValue {
            message: message.to_string(),
            value: None,
        }
    }

    pub fn external<T: Into<Box<dyn std::error::Error + Send + Sync>>>(err: T) -> Self {
        Error::ExternalError(err.into().into())
    }

    /// The message Lua sees when this error is raised into it: the text, without mlua's
    /// category words, as a 1.12 C function raises a string.
    pub(crate) fn lua_message(&self) -> String {
        match self {
            Error::SyntaxError { message, .. } => message.clone(),
            Error::RuntimeError(m) | Error::LuaValue { message: m, .. } => m.clone(),
            Error::CallbackError { cause, .. } | Error::WithContext { cause, .. } => {
                cause.lua_message()
            }
            other => other.to_string(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::SyntaxError { message, .. } => write!(f, "syntax error: {message}"),
            Error::RuntimeError(msg) | Error::LuaValue { message: msg, .. } => {
                write!(f, "runtime error: {msg}")
            }
            Error::MemoryError(msg) => write!(f, "memory error: {msg}"),
            Error::StackError => write!(
                f,
                "out of Lua stack, too many arguments to a Lua function or too many return values from a callback"
            ),
            Error::BadArgument {
                to,
                pos,
                name,
                cause,
            } => {
                if let Some(name) = name {
                    write!(f, "bad argument `{name}`")?;
                } else {
                    write!(f, "bad argument #{pos}")?;
                }
                if let Some(to) = to {
                    write!(f, " to `{to}`")?;
                }
                write!(f, ": {cause}")
            }
            Error::ToLuaConversionError { from, to, message } => {
                write!(f, "error converting {from} to Lua {to}")?;
                match message {
                    None => Ok(()),
                    Some(m) => write!(f, " ({m})"),
                }
            }
            Error::FromLuaConversionError { from, to, message } => {
                write!(f, "error converting Lua {from} to {to}")?;
                match message {
                    None => Ok(()),
                    Some(m) => write!(f, " ({m})"),
                }
            }
            Error::CallbackError { cause, traceback } => {
                writeln!(f, "{cause}")?;
                write!(f, "{traceback}")
            }
            Error::WithContext { context, cause } => {
                writeln!(f, "{context}")?;
                write!(f, "{cause}")
            }
            Error::ExternalError(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

/// mlua's `ErrorContext` for `Result`s.
pub trait ErrorContext: Sized {
    fn context<C: fmt::Display>(self, context: C) -> Self;
    fn with_context<C: fmt::Display>(self, f: impl FnOnce(&Error) -> C) -> Self;
}

impl<T> ErrorContext for Result<T> {
    fn context<C: fmt::Display>(self, context: C) -> Self {
        self.map_err(|e| Error::WithContext {
            context: context.to_string(),
            cause: Arc::new(e),
        })
    }

    fn with_context<C: fmt::Display>(self, f: impl FnOnce(&Error) -> C) -> Self {
        self.map_err(|e| Error::WithContext {
            context: f(&e).to_string(),
            cause: Arc::new(e),
        })
    }
}

/// mlua's `ExternalError`: `.into_lua_err()` on any error.
pub trait ExternalError {
    fn into_lua_err(self) -> Error;
}

impl<E: Into<Box<dyn std::error::Error + Send + Sync>>> ExternalError for E {
    fn into_lua_err(self) -> Error {
        Error::external(self)
    }
}
