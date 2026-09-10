//! Small async primitives shared by the app's bounded bridge requests.

use std::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

/// Races `future` against `timeout`, returning `None` when the deadline wins.
///
/// `future` is polled first, so an answer that lands on the same wake as an expired timer is
/// kept rather than discarded — the tie-break both call sites depend on. The bridge does not
/// drop-cancel, so this racer only stops the *waiting*; the daemon request stays in flight.
pub(crate) async fn before_timeout<T>(
    future: impl Future<Output = T>,
    timeout: impl Future<Output = ()>,
) -> Option<T> {
    let mut future = pin!(future);
    let mut timeout = pin!(timeout);
    poll_fn(|cx| {
        if let Poll::Ready(output) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(output));
        }
        if timeout.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const DEADLINE: Duration = Duration::from_millis(50);

    #[gpui::test]
    async fn an_expired_deadline_gives_up_on_a_pending_future(cx: &mut gpui::TestAppContext) {
        let timer = cx.executor().timer(DEADLINE);
        let race = cx
            .executor()
            .spawn(async move { before_timeout(std::future::pending::<u8>(), timer).await });
        cx.executor().advance_clock(2 * DEADLINE);
        assert_eq!(race.await, None);
    }

    #[gpui::test]
    async fn a_ready_answer_wins_over_the_deadline(cx: &mut gpui::TestAppContext) {
        let timer = cx.executor().timer(DEADLINE);
        let answered = before_timeout(std::future::ready(7_u8), timer).await;
        assert_eq!(answered, Some(7));
    }
}
