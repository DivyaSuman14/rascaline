use std::str::Utf8Error;

#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// Got an invalid parameter value in a function
    InvalidParameter(String),
    /// Error while serializing/deserializing data
    Json(serde_json::Error),
    /// Error due to C strings containing non-utf8 data
    Utf8(Utf8Error),
    /// Error related to reading files with chemfiles
    Chemfiles(String),
    /// Errors coming from equistore
    Equistore(equistore::Error),
    /// Errors coming from external callbacks, typically inside the System
    /// implementation
    External {
        status: i32,
        message: String
    },
    /// Error used when a memory buffer is too small to fit the requested data,
    /// usually in the C API.
    BufferSize(String),
    /// Error used for failed internal consistency check and panics, i.e. bugs
    /// in rascaline.
    Internal(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidParameter(e) => write!(f, "invalid parameter: {}", e),
            Error::Json(e) => write!(f, "json error: {}", e),
            Error::Utf8(e) => write!(f, "utf8 decoding error: {}", e),
            Error::Chemfiles(e) => write!(f, "chemfiles error: {}", e),
            Error::Equistore(e) => write!(f, "equistore error: {}", e),
            Error::BufferSize(e) => write!(f, "buffer is not big enough: {}", e),
            Error::External{status, message} => write!(f, "error from external code (status {}): {}", status, message),
            Error::Internal(e) => write!(f, "internal error (this is likely a bug, please report it): {}", e),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::InvalidParameter(_) |
            Error::Internal(_) |
            Error::Chemfiles(_) |
            Error::BufferSize(_) |
            Error::External{..} => None,
            Error::Equistore(e) => Some(e),
            Error::Json(e) => Some(e),
            Error::Utf8(e) => Some(e),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Error {
        Error::Json(error)
    }
}

impl From<Utf8Error> for Error {
    fn from(error: Utf8Error) -> Error {
        Error::Utf8(error)
    }
}

impl From<equistore::Error> for Error {
    fn from(error: equistore::Error) -> Error {
        return Error::Equistore(error);
    }
}


// Box<dyn Any + Send + 'static> is the error type in std::panic::catch_unwind
impl From<Box<dyn std::any::Any + Send + 'static>> for Error {
    fn from(error: Box<dyn std::any::Any + Send + 'static>) -> Error {
        let message = if let Some(message) = error.downcast_ref::<String>() {
            message.clone()
        } else if let Some(message) = error.downcast_ref::<&str>() {
            (*message).to_owned()
        } else {
            panic!("panic message is not a string, something is very wrong")
        };

        Error::Internal(message)
    }
}
