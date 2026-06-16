use std::fmt;

pub type Result<T> = std::result::Result<T, AgenticSshError>;

#[derive(Debug)]
pub enum AgenticSshError {
    Config { message: String },
}

impl std::error::Error for AgenticSshError {}

impl fmt::Display for AgenticSshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgenticSshError::Config { message } => write!(f, "Configuration error: {}", message),
        }
    }
}
