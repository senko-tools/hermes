use std::time::Duration;

use hermes_client_core::{CoreEvent, CoreEvents};

pub(super) const WAIT: Duration = Duration::from_secs(15);

pub(super) async fn wait_for(
    events: &mut CoreEvents,
    what: &str,
    mut f: impl FnMut(&CoreEvent) -> bool,
) {
    let result = tokio::time::timeout(WAIT, async {
        while let Some(event) = events.recv().await {
            if f(&event) {
                return;
            }
        }
        panic!("event stream ended");
    })
    .await;
    assert!(result.is_ok(), "timed out waiting for {what}");
}
