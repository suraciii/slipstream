use crate::{DerivativeTarget, PreviewState};

pub(crate) fn should_retry(state: PreviewState, target: DerivativeTarget) -> bool {
    target == DerivativeTarget::Review2560 && state == PreviewState::Failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_failure_only_enables_review_retry() {
        assert!(should_retry(
            PreviewState::Failed,
            DerivativeTarget::Review2560
        ));
        assert!(!should_retry(
            PreviewState::Failed,
            DerivativeTarget::Thumbnail512
        ));
    }
}
