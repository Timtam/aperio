//! What reading iCalendar text can fail with.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IcalError {
    /// The text is no iCalendar, or a component lacks what every one must
    /// carry (a VEVENT without UID or DTSTART).
    #[error("malformed iCalendar: {0}")]
    Protocol(String),
}

pub type IcalResult<T> = Result<T, IcalError>;
