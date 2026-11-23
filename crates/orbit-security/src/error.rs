#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("secret error: {0}")]
    Secret(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("dependency unavailable: {0}")]
    Unavailable(String),
    #[error("internal security error: {0}")]
    Internal(String),
}

impl SecurityError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Config(_) | Self::Secret(_) | Self::Internal(_) => "internal",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden(_) => "forbidden",
            Self::Unavailable(_) => "unavailable",
        }
    }
}
