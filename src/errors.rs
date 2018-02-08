use std::error::Error;
use std::fmt::{Display, Formatter, Error as FmtError};


#[derive(Debug)]
pub enum PoolError {
    PollError,
}


impl Error for PoolError {
    fn description(&self) -> &str {
        "PoolError Occurred"
    }
}

impl Display for PoolError {
    fn fmt(&self, f: &mut Formatter) -> Result<(), FmtError> {
        write!(f, "{}", self.description())
    }
}
