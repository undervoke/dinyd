//! dinyd ties documentation claims to the code regions they restate.

pub mod anchor;
pub mod check;
pub mod edit;
pub mod git;
pub mod link;
pub mod target;
pub mod unit;

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Bad arguments; exit 2.
    Usage(String),
    /// Git or file read failure; exit 2.
    Read(String),
    /// apply or update declined to write; exit 1.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Usage(m) | Error::Read(m) | Error::Refused(m) => f.write_str(m),
        }
    }
}
