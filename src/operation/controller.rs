use atomic_float::AtomicF32;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use std::sync::Arc;
use std::sync::atomic::{self, AtomicU16};
use tokio::sync::Notify;

#[derive(Clone, Copy, Debug, PartialEq, Eq, IntoPrimitive, TryFromPrimitive)]
#[repr(u16)]
pub enum ControllerState {
    Cancelled,
    Failed,
    Paused,
    Running,
}

#[derive(Debug)]
struct ControllerInner {
    state: AtomicU16,
    progress: AtomicF32,
    notify: Notify,
}

#[derive(Debug)]
pub struct Controller {
    primary: bool,
    inner: Arc<ControllerInner>,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            primary: true,
            inner: Arc::new(ControllerInner {
                state: AtomicU16::new(ControllerState::Running.into()),
                progress: AtomicF32::new(0.0),
                notify: Notify::new(),
            }),
        }
    }
}

impl Controller {
    /// Waits until `ready` returns a value for the current state, and returns that value.
    async fn wait_for<T>(&self, mut ready: impl FnMut(ControllerState) -> Option<T>) -> T {
        loop {
            // Created before the state is read: it receives every notify_waiters() after this
            // point, so a change between the read and the wait still wakes it.
            let notified = self.inner.notify.notified();
            if let Some(value) = ready(self.state()) {
                return value;
            }
            notified.await;
        }
    }

    /// Waits while paused; then `Ok` when running, or the state that ended the operation.
    pub async fn check(&self) -> Result<(), ControllerState> {
        self.wait_for(|state| match state {
            ControllerState::Cancelled => Some(Err(ControllerState::Cancelled)),
            ControllerState::Failed => Some(Err(ControllerState::Failed)),
            ControllerState::Paused => None,
            ControllerState::Running => Some(Ok(())),
        })
        .await
    }

    pub fn progress(&self) -> f32 {
        self.inner.progress.load(atomic::Ordering::Relaxed)
    }

    pub fn set_progress(&self, progress: f32) {
        self.inner
            .progress
            .swap(progress, atomic::Ordering::Relaxed);
    }

    pub fn state(&self) -> ControllerState {
        ControllerState::try_from(self.inner.state.load(atomic::Ordering::Relaxed))
            .unwrap_or(ControllerState::Failed)
    }

    pub fn set_state(&self, state: ControllerState) {
        self.inner
            .state
            .store(state.into(), atomic::Ordering::Relaxed);
        self.inner.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self.state(), ControllerState::Cancelled)
    }

    pub fn cancel(&self) {
        self.set_state(ControllerState::Cancelled);
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.state(), ControllerState::Failed)
    }

    pub fn is_paused(&self) -> bool {
        matches!(self.state(), ControllerState::Paused)
    }

    /// Pauses a running operation. Any other state is left alone, so a cancel or failure is
    /// never turned back into a pause that a resume would restart.
    pub fn pause(&self) {
        self.transition(ControllerState::Running, ControllerState::Paused);
    }

    /// Returns when the state is paused.
    ///
    /// Use this to pause futures.
    pub async fn until_paused(&self) {
        self.wait_for(|state| (state == ControllerState::Paused).then_some(()))
            .await;
    }

    /// Returns when state is neither paused, cancelled, nor failed.
    ///
    /// Use this to resume futures.
    pub async fn until_unpaused(&self) {
        self.wait_for(|state| (state == ControllerState::Running).then_some(()))
            .await;
    }

    /// Resumes a paused operation. Any other state is left alone, so a cancel or failure that
    /// races a resume is never undone.
    pub fn unpause(&self) {
        self.transition(ControllerState::Paused, ControllerState::Running);
    }

    /// Moves the state from `from` to `to`, waking waiters, only if it is still `from`.
    fn transition(&self, from: ControllerState, to: ControllerState) {
        let changed = self
            .inner
            .state
            .compare_exchange(
                from.into(),
                to.into(),
                atomic::Ordering::Relaxed,
                atomic::Ordering::Relaxed,
            )
            .is_ok();
        if changed {
            self.inner.notify.notify_waiters();
        }
    }
}

impl Clone for Controller {
    fn clone(&self) -> Self {
        Self {
            primary: false,
            inner: self.inner.clone(),
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        // Cancel operations if primary controller is dropped and controller is still running
        if self.primary && self.state() != ControllerState::Failed {
            self.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::{Pin, pin};
    use std::task::{Context, Poll};

    use cosmic::iced::futures::task::noop_waker;

    use super::{Controller, ControllerState};

    /// Polls `future` once, without an executor, so a test can tell "woken and done" from
    /// "still waiting" without a clock.
    fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
        let waker = noop_waker();
        future.poll(&mut Context::from_waker(&waker))
    }

    // The cancel button stays offered next to Pause until the task sees the cancel, so pausing
    // must not turn a cancelled operation back into a paused one that a resume would restart.
    #[test]
    fn pausing_a_cancelled_operation_keeps_it_cancelled() {
        let controller = Controller::default();
        controller.cancel();
        controller.pause();
        assert_eq!(controller.state(), ControllerState::Cancelled);
    }

    #[test]
    fn pausing_a_failed_operation_keeps_it_failed() {
        let controller = Controller::default();
        controller.set_state(ControllerState::Failed);
        controller.pause();
        assert_eq!(controller.state(), ControllerState::Failed);
    }

    // A resume wakes an operation already waiting on its pause.
    #[test]
    fn a_resume_wakes_a_waiting_check() {
        let controller = Controller::default();
        controller.pause();
        let mut check = pin!(controller.check());
        assert!(poll_once(check.as_mut()).is_pending());

        controller.unpause();

        assert_eq!(poll_once(check.as_mut()), Poll::Ready(Ok(())));
    }

    // A state change made after the wait reads the state but before it starts waiting still
    // ends the wait: the predicate below resumes the controller on its first call, which lands
    // exactly in that gap.
    #[test]
    fn a_change_between_reading_the_state_and_waiting_is_not_lost() {
        let controller = Controller::default();
        controller.pause();
        let resumer = controller.clone();
        let mut calls = 0;
        let mut wait = pin!(controller.wait_for(|state| {
            calls += 1;
            if calls == 1 {
                resumer.unpause();
                return None;
            }
            (state == ControllerState::Running).then_some(())
        }));

        assert_eq!(poll_once(wait.as_mut()), Poll::Ready(()));
    }

    // A cancelled or failed operation stops waiting for a resume, so a paused copy that is
    // then cancelled ends instead of waiting for ever.
    #[test]
    fn a_cancel_ends_a_wait_for_resume() {
        let controller = Controller::default();
        controller.pause();
        let mut wait = pin!(controller.check());
        assert!(poll_once(wait.as_mut()).is_pending());

        controller.cancel();

        assert_eq!(
            poll_once(wait.as_mut()),
            Poll::Ready(Err(ControllerState::Cancelled))
        );
    }

    // "Resume all" unpauses every pending operation, including one the user just cancelled
    // that has not yet seen the cancel; it must stay cancelled.
    #[test]
    fn unpausing_a_cancelled_operation_keeps_it_cancelled() {
        let controller = Controller::default();
        controller.pause();
        controller.cancel();
        controller.unpause();
        assert_eq!(controller.state(), ControllerState::Cancelled);
    }

    #[test]
    fn unpausing_a_failed_operation_keeps_it_failed() {
        let controller = Controller::default();
        controller.set_state(ControllerState::Failed);
        controller.unpause();
        assert_eq!(controller.state(), ControllerState::Failed);
    }

    #[test]
    fn unpausing_a_paused_operation_resumes_it() {
        let controller = Controller::default();
        controller.pause();
        controller.unpause();
        assert_eq!(controller.state(), ControllerState::Running);
    }
}
