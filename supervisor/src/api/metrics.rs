use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use tokio::sync::watch;
use tokio_stream::{wrappers::WatchStream, Stream, StreamExt};

const PERIOD: Duration = Duration::from_secs(1);
type Sample = (i32, Option<SystemTime>, Option<f64>, i64);
type Output = watch::Sender<Option<Sample>>;

#[derive(Clone, Default)]
pub struct MetricsFeed {
    active: Arc<Mutex<Option<Output>>>,
}

impl MetricsFeed {
    pub(super) fn subscribe(
        &self,
        sample: impl Fn() -> Sample + Send + Sync + 'static,
    ) -> impl Stream<Item = Sample> + Send + 'static {
        let mut active = self.active.lock().unwrap_or_else(|err| err.into_inner());
        let rx = match active.as_ref() {
            Some(output) => output.subscribe(),
            None => {
                let (output, rx) = watch::channel(None);
                *active = Some(output.clone());
                tokio::spawn(self.clone().run(output, sample));
                rx
            }
        };
        WatchStream::from_changes(rx).filter_map(|sample| sample)
    }

    async fn run(self, output: Output, sample: impl Fn() -> Sample + Send + Sync + 'static) {
        let sample = Arc::new(sample);
        let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + PERIOD, PERIOD);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = output.closed() => {
                    // A reconnect may have attached to this worker after closed() woke it.
                    let mut active = self.active.lock().unwrap_or_else(|err| err.into_inner());
                    if output.receiver_count() == 0 {
                        *active = None;
                        return;
                    }
                }
                _ = timer.tick(), if output.receiver_count() > 0 => {
                    let sample = sample.clone();
                    if let Ok(value) = tokio::task::spawn_blocking(move || sample()).await {
                        let _ = output.send(Some(value));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    #[tokio::test]
    async fn five_subscribers_receive_the_same_sample() {
        let feed = MetricsFeed::default();
        let calls = Arc::new(AtomicI64::new(0));
        let sample = move || (10, None, None, calls.fetch_add(1, Ordering::Relaxed) + 1);
        let [mut a, mut b, mut c, mut d, mut e] =
            std::array::from_fn(|_| Box::pin(feed.subscribe(sample.clone())));

        let (a, b, c, d, e) = tokio::time::timeout(3 * PERIOD, async {
            tokio::join!(a.next(), b.next(), c.next(), d.next(), e.next())
        })
        .await
        .expect("all subscribers should receive a sample");
        assert_eq!([a, b, c, d, e], [Some((10, None, None, 1)); 5]);
    }

    #[tokio::test]
    async fn sampler_is_idle_without_subscribers_and_resumes_on_demand() {
        let feed = MetricsFeed::default();
        let calls = Arc::new(AtomicI64::new(0));
        let sample = {
            let calls = calls.clone();
            move || (10, None, None, calls.fetch_add(1, Ordering::Relaxed) + 1)
        };
        let idle = PERIOD + Duration::from_millis(100);
        tokio::time::sleep(idle).await;
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        let mut first = Box::pin(feed.subscribe(sample.clone()));
        assert_eq!(
            tokio::time::timeout(3 * PERIOD, first.next())
                .await
                .unwrap(),
            Some((10, None, None, 1))
        );
        drop(first);
        tokio::time::sleep(idle).await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        let mut reopened = Box::pin(feed.subscribe(sample));
        assert_eq!(
            tokio::time::timeout(3 * PERIOD, reopened.next())
                .await
                .unwrap(),
            Some((10, None, None, 2))
        );
    }

    #[tokio::test]
    async fn joining_active_sampler_does_not_replay_its_last_sample() {
        let feed = MetricsFeed::default();
        let calls = Arc::new(AtomicI64::new(0));
        let sample = move || (10, None, None, calls.fetch_add(1, Ordering::Relaxed) + 1);
        let mut first = Box::pin(feed.subscribe(sample.clone()));
        assert_eq!(
            tokio::time::timeout(3 * PERIOD, first.next())
                .await
                .unwrap(),
            Some((10, None, None, 1))
        );

        let mut joined = Box::pin(feed.subscribe(sample));
        let (first, joined) = tokio::time::timeout(3 * PERIOD, async {
            tokio::join!(first.next(), joined.next())
        })
        .await
        .unwrap();
        assert_eq!([first, joined], [Some((10, None, None, 2)); 2]);
    }
}
