//! Cooperative cancellation, without a runtime.
//!
//! A transport trips the token when the client disconnects or a deadline
//! passes; the run notices and unwinds. There is deliberately no
//! `tokio_util::CancellationToken` here — this crate must build for wasm and
//! for non-tokio executors, so the token is an [`AtomicBool`] plus a waker list.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// A shared "stop now" flag.
///
/// Cloning is cheap and every clone refers to the same flag, so a transport can
/// keep one and hand another to the run.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    cancelled: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

impl CancellationToken {
    /// A fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Trips the token and wakes everything waiting on it.
    ///
    /// Idempotent — cancelling twice is a no-op.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            let mut wakers = lock(&self.inner.wakers);
            for waker in wakers.drain(..) {
                waker.wake();
            }
        }
    }

    /// Whether the token has been tripped.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once the token is tripped.
    ///
    /// Use it to race an in-flight model call:
    ///
    /// ```
    /// # use ag_ui::server::CancellationToken;
    /// # let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    /// # rt.block_on(async {
    /// let token = CancellationToken::new();
    /// token.cancel();
    /// token.cancelled().await;
    /// # });
    /// ```
    pub fn cancelled(&self) -> Cancelled {
        Cancelled {
            token: self.clone(),
        }
    }
}

/// The future returned by [`CancellationToken::cancelled`].
///
/// It owns a clone of the token rather than borrowing one — one `Arc` bump, in
/// exchange for a `'static` future that an agent can hold across an await
/// without dragging a borrow of the run context along with it.
#[derive(Clone, Debug)]
#[must_use = "a future does nothing unless awaited"]
pub struct Cancelled {
    token: CancellationToken,
}

impl Future for Cancelled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.token.is_cancelled() {
            return Poll::Ready(());
        }
        let mut wakers = lock(&self.token.inner.wakers);
        // Re-check under the lock: `cancel` may have drained the list between
        // the load above and here, and would then never see our waker.
        if self.token.is_cancelled() {
            return Poll::Ready(());
        }
        if !wakers.iter().any(|waker| waker.will_wake(cx.waker())) {
            wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// A poisoned waker list is still a perfectly good waker list: the only code
/// that touches it cannot panic while holding the guard.
fn lock(mutex: &Mutex<Vec<Waker>>) -> std::sync::MutexGuard<'_, Vec<Waker>> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_visible_through_clones() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_future_resolves() {
        let token = CancellationToken::new();
        let waiter = token.clone();
        let handle = tokio::spawn(async move { waiter.cancelled().await });
        token.cancel();
        handle.await.expect("waiter task panicked");
    }

    #[tokio::test]
    async fn cancelled_future_is_ready_when_already_cancelled() {
        let token = CancellationToken::new();
        token.cancel();
        token.cancelled().await;
    }
}
