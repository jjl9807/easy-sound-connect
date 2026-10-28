//! Delayed, operation-local feedback that does not change control availability.

use ezsc_core::{audio::AudioKind, equalizer::EqualizerKind, noise::NoiseKind, system::SystemKind};
use std::time::Duration;

pub(super) const FEEDBACK_DELAY: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OperationTarget {
    Noise(NoiseKind),
    Equalizer(EqualizerKind),
    Audio(AudioKind),
    System(SystemKind),
    Refresh,
}

#[derive(Default)]
pub(super) struct OperationFeedback {
    generation: u64,
    target: Option<OperationTarget>,
    visible: bool,
}

impl OperationFeedback {
    pub(super) fn start(&mut self, target: OperationTarget) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.target = Some(target);
        self.visible = false;
        self.generation
    }

    pub(super) fn finish(&mut self) {
        self.target = None;
        self.visible = false;
    }

    pub(super) fn reveal(&mut self, generation: u64) {
        if generation == self.generation && self.target.is_some() {
            self.visible = true;
        }
    }

    pub(super) fn is_visible(&self, target: OperationTarget) -> bool {
        self.visible && self.target == Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_completion_never_shows_waiting_feedback() {
        let mut feedback = OperationFeedback::default();
        let timer = feedback.start(OperationTarget::Refresh);
        assert!(!feedback.is_visible(OperationTarget::Refresh));
        feedback.finish();
        feedback.reveal(timer);
        assert!(!feedback.is_visible(OperationTarget::Refresh));
    }

    #[test]
    fn late_timer_cannot_reveal_a_new_operation() {
        let mut feedback = OperationFeedback::default();
        let previous = feedback.start(OperationTarget::Refresh);
        feedback.finish();
        let current = feedback.start(OperationTarget::Audio(AudioKind::Dsee));
        feedback.reveal(previous);
        assert!(!feedback.is_visible(OperationTarget::Audio(AudioKind::Dsee)));
        feedback.reveal(current);
        assert!(feedback.is_visible(OperationTarget::Audio(AudioKind::Dsee)));
        assert!(!feedback.is_visible(OperationTarget::Refresh));
        assert!(!feedback.is_visible(OperationTarget::Audio(AudioKind::DseeWithReasons)));
    }
}
