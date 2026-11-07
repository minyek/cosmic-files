#[derive(Default)]
pub(crate) struct ScanState {
    generation: u64,
    current: Option<u64>,
    pending: bool,
}

impl ScanState {
    pub(crate) fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.current = Some(self.generation);
        self.pending = false;
        self.generation
    }

    pub(crate) fn defer_if_running(&mut self) -> bool {
        if self.current.is_some() {
            self.pending = true;
            true
        } else {
            false
        }
    }

    pub(crate) fn finish(&mut self, request: u64) -> Option<bool> {
        if self.current != Some(request) {
            return None;
        }
        self.current = None;
        Some(std::mem::take(&mut self.pending))
    }
}

#[cfg(test)]
mod tests {
    use super::ScanState;

    #[test]
    fn an_old_completion_cannot_release_the_current_scan() {
        let mut scans = ScanState::default();
        let old = scans.begin();
        let current = scans.begin();
        assert!(scans.defer_if_running());
        assert_eq!(scans.finish(old), None);
        assert!(scans.defer_if_running());
        assert_eq!(scans.finish(current), Some(true));
        assert!(!scans.defer_if_running());
    }

    #[test]
    fn bursts_request_only_one_followup() {
        let mut scans = ScanState::default();
        assert!(!scans.defer_if_running());
        let request = scans.begin();
        assert!(scans.defer_if_running());
        assert!(scans.defer_if_running());
        assert_eq!(scans.finish(request), Some(true));
        assert_eq!(scans.finish(request), None);
    }

    #[test]
    fn a_new_scan_covers_requests_deferred_before_it_started() {
        let mut scans = ScanState::default();
        let old = scans.begin();
        assert!(scans.defer_if_running());
        let current = scans.begin();
        assert_eq!(scans.finish(old), None);
        assert_eq!(scans.finish(current), Some(false));
    }
}
