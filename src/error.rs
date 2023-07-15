use std::result::Result as StdResult;
use futures::channel::mpsc::SendError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum LazyPoolError {
    #[error("failed to release object")]
    Release,
    #[error("failed to send to channel")]
    Send(#[from] SendError)
}

pub type Result<T> = StdResult<T, LazyPoolError>;
