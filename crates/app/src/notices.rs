//! Transient warnings and errors, with expiry owned by the application session.

use std::time::Duration;

pub(super) const DISPLAY_DURATION: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Severity {
    Warning,
    Error,
}

#[derive(Debug)]
pub(super) struct Notice {
    pub(super) id: u64,
    pub(super) severity: Severity,
    pub(super) message: String,
}

#[derive(Default)]
pub(super) struct Notices {
    current: Option<Notice>,
    next_id: u64,
}

impl Notices {
    pub(super) fn current(&self) -> Option<&Notice> {
        self.current.as_ref()
    }

    /// Returns an identifier for a new expiry timer, or None when suppressed.
    /// Repeated messages keep their original expiry, and warnings never hide an error.
    pub(super) fn show(&mut self, severity: Severity, message: String) -> Option<u64> {
        if let Some(current) = &self.current
            && ((current.severity == Severity::Error && severity == Severity::Warning)
                || (current.severity == severity && current.message == message))
        {
            return None;
        }
        self.next_id = self.next_id.checked_add(1)?;
        let id = self.next_id;
        self.current = Some(Notice {
            id,
            severity,
            message,
        });
        Some(id)
    }

    /// Ignores expiry or dismissal requests for a previously replaced notice.
    pub(super) fn dismiss(&mut self, id: u64) -> bool {
        if self.current.as_ref().is_some_and(|notice| notice.id == id) {
            self.current = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Notices, Severity};

    #[test]
    fn repeated_notice_does_not_restart_expiry_but_can_reappear_after_dismissal() {
        for severity in [Severity::Warning, Severity::Error] {
            let mut notices = Notices::default();
            let first = notices
                .show(severity, "Connection interrupted".into())
                .unwrap();
            assert_eq!(
                notices.show(severity, "Connection interrupted".into()),
                None
            );
            assert_eq!(notices.current().unwrap().id, first);
            assert!(notices.dismiss(first));
            let repeated = notices
                .show(severity, "Connection interrupted".into())
                .unwrap();
            assert!(repeated > first);
        }
    }

    #[test]
    fn warning_does_not_replace_visible_error() {
        let mut notices = Notices::default();
        let error = notices
            .show(Severity::Error, "Cannot save preferences".into())
            .unwrap();
        assert_eq!(
            notices.show(Severity::Warning, "Tray unavailable".into()),
            None
        );
        let current = notices.current().unwrap();
        assert_eq!(current.id, error);
        assert_eq!(current.severity, Severity::Error);
        assert_eq!(current.message, "Cannot save preferences");
    }

    #[test]
    fn error_replaces_warning_with_a_new_expiry() {
        let mut notices = Notices::default();
        let warning = notices
            .show(Severity::Warning, "Connection interrupted".into())
            .unwrap();
        let error = notices
            .show(Severity::Error, "Unable to reconnect".into())
            .unwrap();
        assert!(error > warning);
        let current = notices.current().unwrap();
        assert_eq!(current.id, error);
        assert_eq!(current.severity, Severity::Error);
        assert_eq!(current.message, "Unable to reconnect");
    }

    #[test]
    fn old_expiry_cannot_clear_a_replacement() {
        let mut notices = Notices::default();
        let first = notices
            .show(Severity::Error, "Unable to connect".into())
            .unwrap();
        let replacement = notices
            .show(Severity::Error, "Cannot save preferences".into())
            .unwrap();
        assert!(!notices.dismiss(first));
        assert_eq!(notices.current().unwrap().id, replacement);
        assert!(notices.dismiss(replacement));
        assert!(notices.current().is_none());
        assert!(!notices.dismiss(replacement));
    }
}
