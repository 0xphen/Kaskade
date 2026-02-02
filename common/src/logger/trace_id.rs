use uuid::Uuid;

/// Correlation ID that follows a request / batch / transaction
#[derive(Clone, Debug)]
pub struct TraceId(Uuid);

impl TraceId {
    pub fn as_str(&self) -> &str {
        // safe: UUID lives as long as self
        self.0.as_hyphenated().to_string().leak()
    }
}

impl Default for TraceId {
    fn default() -> Self {
        Self(Uuid::new_v4())
    }
}
