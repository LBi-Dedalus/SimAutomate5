use std::num::NonZeroU64;

pub const MAX_SESSION_ID_LEN: usize = 64;

/// A frontend supplied session id, known to be usable (and safe to log).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() || raw.len() > MAX_SESSION_ID_LEN {
            return Err(format!(
                "Invalid session id: it must contain 1 to {MAX_SESSION_ID_LEN} characters"
            ));
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("Invalid session id: only letters, digits, '-' and '_' are allowed".into());
        }
        Ok(Self(raw.to_string()))
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A connection attempt number of a session; always greater than zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Attempt(NonZeroU64);

impl Attempt {
    pub fn parse(raw: u64) -> Result<Self, String> {
        NonZeroU64::new(raw)
            .map(Self)
            .ok_or_else(|| "Invalid attempt: it must be greater than zero".to_string())
    }

    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl std::fmt::Display for Attempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One connection attempt of one session: what every event and log line belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionId {
    pub session_id: SessionId,
    pub attempt: Attempt,
}

impl ConnectionId {
    pub fn new(session_id: SessionId, attempt: Attempt) -> Self {
        Self {
            session_id,
            attempt,
        }
    }

    pub fn parse(session_id: &str, attempt: u64) -> Result<Self, String> {
        Ok(Self::new(
            SessionId::parse(session_id)?,
            Attempt::parse(attempt)?,
        ))
    }
}

impl std::fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "session={} attempt={}", self.session_id, self.attempt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_accepts_simple_ids() {
        assert_eq!(SessionId::parse("a-B_9").unwrap().to_string(), "a-B_9");
        assert!(SessionId::parse(&"x".repeat(MAX_SESSION_ID_LEN)).is_ok());
    }

    #[test]
    fn session_id_rejects_bad_ids() {
        let length = "Invalid session id: it must contain 1 to 64 characters";
        assert_eq!(SessionId::parse("").unwrap_err(), length);
        assert_eq!(
            SessionId::parse(&"x".repeat(MAX_SESSION_ID_LEN + 1)).unwrap_err(),
            length
        );
        let chars = "Invalid session id: only letters, digits, '-' and '_' are allowed";
        assert_eq!(SessionId::parse("bad id").unwrap_err(), chars);
        assert_eq!(SessionId::parse("a\nb").unwrap_err(), chars);
    }

    #[test]
    fn attempt_must_be_positive() {
        assert_eq!(
            Attempt::parse(0).unwrap_err(),
            "Invalid attempt: it must be greater than zero"
        );
        let attempt = Attempt::parse(3).unwrap();
        assert_eq!(attempt.get(), 3);
        assert!(Attempt::parse(2).unwrap() < attempt);
    }
}
