//! Waiting for a future on a plain thread.
//!
//! A call has one thread of its own (it asks for admission, then passes the
//! engine's events on); this is all the executor it needs.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `future` to its end on this thread, sleeping while it waits.
pub(crate) fn on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn waits_for_another_thread() {
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            tx.send_blocking(7).unwrap();
        });
        assert_eq!(super::on(async { rx.recv().await.unwrap() }), 7);
    }
}
