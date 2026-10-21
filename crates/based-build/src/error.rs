//! Build failures retain compiler diagnostics and filesystem causes.
use std::fmt;

#[derive(Debug)]
pub enum Error {
    Environment(&'static str),
    Io(std::io::Error),
    Compile(based_project::Error),
    Publish(based_artifacts::Error),
    /// `migrations/` failed the same checks as `based migrate verify`.
    Migrations(Vec<based_project::Finding>),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment(name) => write!(f, "{name} must be an absolute path supplied by Cargo; call based_build::generate from build.rs"),
            Self::Io(error) => write!(f, "build artifact I/O: {error}"),
            Self::Compile(error) => write!(f, "{error}"),
            Self::Publish(error) => write!(f, "{error}"),
            Self::Migrations(problems) => {
                for problem in problems {
                    writeln!(f, "migrations/{problem}")?;
                }
                write!(f, "migration verify failed: {} problem(s); fix them or re-run `based migrate gen`", problems.len())
            }
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Environment(_) => None,
            Self::Io(error) => Some(error),
            Self::Compile(error) => Some(error),
            Self::Publish(error) => Some(error),
            Self::Migrations(_) => None,
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<based_project::Error> for Error {
    fn from(error: based_project::Error) -> Self {
        Self::Compile(error)
    }
}
impl From<based_artifacts::Error> for Error {
    fn from(error: based_artifacts::Error) -> Self {
        Self::Publish(error)
    }
}
