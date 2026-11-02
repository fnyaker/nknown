use std::{fmt, io};

#[derive(Debug)]
pub enum Error {
    Candle(candle_core::Error),
    Tokenizer(String),
    Io(io::Error),
    Json(serde_json::Error),
    Format(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Candle(e) => write!(f, "calcul : {e}"),
            Error::Tokenizer(e) => write!(f, "tokeniseur : {e}"),
            Error::Io(e) => write!(f, "E/S : {e}"),
            Error::Json(e) => write!(f, "configuration : {e}"),
            Error::Format(m) => write!(f, "modèle invalide : {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Candle(e) => Some(e),
            Error::Io(e) => Some(e),
            Error::Json(e) => Some(e),
            Error::Tokenizer(_) | Error::Format(_) => None,
        }
    }
}

impl From<candle_core::Error> for Error {
    fn from(e: candle_core::Error) -> Self {
        Error::Candle(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}

impl From<tokenizers::Error> for Error {
    fn from(e: tokenizers::Error) -> Self {
        Error::Tokenizer(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
