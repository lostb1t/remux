use anyhow::Result;
use core::fmt::Debug;
use tracing::error;

pub trait LogErr<T, E> {
    fn log_err(self, msg: &str) -> Self;
}

impl<T, E: Debug> LogErr<T, E> for Result<T, E> {
    fn log_err(self, msg: &str) -> Self {
        if let Err(ref e) = self {
            error!("{}: {:?}", msg, e);
        }
        self
    }
}
