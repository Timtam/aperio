//! A move to another calendar or list, as both hosts make it: the copy is
//! created at the target, then the original deleted at the source.

/// Whether a move's source delete, after the copy at the target landed, left
/// the original standing for a reason the user must hear: the provider was
/// busy and turned the delete away (decision 277; Toni, 2026-10-23). The move
/// stands either way, and is never an error, which would invite moving again
/// and a second copy; the returned row names the container the original
/// stayed in (`original_left_in`), and the surfaces say to delete it. Any other
/// failure stays silent, as before: the caller logs it.
pub fn original_kept(delete: &cal_core::Result<()>) -> bool {
    matches!(delete, Err(err) if err.is_provider_busy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_busy_provider_keeps_the_original_said() {
        let busy = cal_core::Error::provider_busy(cal_core::BusyDetail::TooManyRequests);
        assert!(original_kept(&Err(busy)));
        assert!(!original_kept(&Ok(())));
        assert!(!original_kept(&Err(cal_core::Error::Network(
            "down".into()
        ))));
    }
}
