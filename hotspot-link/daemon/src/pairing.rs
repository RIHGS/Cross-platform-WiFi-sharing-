use rand::{Rng, RngCore};
use std::fmt;
use std::time::{Duration, Instant};

pub const PAIRING_TTL: Duration = Duration::from_secs(120);
pub const MAX_ATTEMPTS: u8 = 5;
pub const SECRET_BYTES: usize = 32;

#[derive(Debug)]
pub struct PairingSession {
    pin: String,
    expires_at: Instant,
    attempts_left: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    Expired,
    AttemptsExhausted,
    InvalidPin { attempts_left: u8 },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expired => write!(f, "pairing window has expired; run 'pair' again"),
            Self::AttemptsExhausted => {
                write!(f, "too many incorrect PIN attempts; run 'pair' again")
            }
            Self::InvalidPin { attempts_left } => {
                write!(f, "incorrect PIN ({attempts_left} attempts left)")
            }
        }
    }
}

impl PairingSession {
    pub fn start() -> Self {
        Self::start_with_ttl(PAIRING_TTL)
    }

    pub fn start_with_ttl(ttl: Duration) -> Self {
        let pin = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32));
        Self {
            pin,
            expires_at: Instant::now() + ttl,
            attempts_left: MAX_ATTEMPTS,
        }
    }

    pub fn pin(&self) -> &str {
        &self.pin
    }

    pub fn expires_in(&self) -> Duration {
        self.expires_at.saturating_duration_since(Instant::now())
    }

    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }

    pub fn attempts_left(&self) -> u8 {
        self.attempts_left
    }

    pub fn verify(&mut self, pin: &str) -> Result<(), VerifyError> {
        if self.is_expired() {
            return Err(VerifyError::Expired);
        }
        if self.attempts_left == 0 {
            return Err(VerifyError::AttemptsExhausted);
        }

        if constant_time_eq(self.pin.as_bytes(), pin.as_bytes()) {
            Ok(())
        } else {
            self.attempts_left -= 1;
            Err(VerifyError::InvalidPin {
                attempts_left: self.attempts_left,
            })
        }
    }
}

pub fn new_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    to_hex(&bytes)
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b) {
        diff |= left ^ right;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_is_six_digits() {
        let session = PairingSession::start();
        assert_eq!(session.pin().len(), 6);
        assert!(session.pin().chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn correct_pin_verifies() {
        let mut session = PairingSession::start();
        let pin = session.pin().to_string();
        assert_eq!(session.verify(&pin), Ok(()));
    }

    #[test]
    fn wrong_pin_decrements_attempts() {
        let mut session = PairingSession::start();
        let wrong = if session.pin() == "000000" {
            "000001"
        } else {
            "000000"
        };

        assert_eq!(
            session.verify(wrong),
            Err(VerifyError::InvalidPin {
                attempts_left: MAX_ATTEMPTS - 1
            })
        );
    }

    #[test]
    fn attempts_are_exhausted() {
        let mut session = PairingSession::start();
        let wrong = if session.pin() == "000000" {
            "000001"
        } else {
            "000000"
        };

        for _ in 0..MAX_ATTEMPTS {
            let _ = session.verify(wrong);
        }

        assert_eq!(session.verify(wrong), Err(VerifyError::AttemptsExhausted));
    }

    #[test]
    fn expired_session_rejects_correct_pin() {
        let mut session = PairingSession::start_with_ttl(Duration::ZERO);
        let pin = session.pin().to_string();
        assert_eq!(session.verify(&pin), Err(VerifyError::Expired));
    }

    #[test]
    fn secret_is_random_hex() {
        let first = new_secret();
        let second = new_secret();

        assert_eq!(first.len(), SECRET_BYTES * 2);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }
}
