//! The program a command line runs, named in messages without its arguments, which may hold a secret.
use std::ffi::OsStr;
use std::fmt;

/// The first word of a command line: what pgbg runs and names, never its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Program(String);

impl Program {
    pub(crate) fn new(name: &str) -> Self {
        Self(name.to_owned())
    }
}

impl AsRef<OsStr> for Program {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(&self.0)
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
