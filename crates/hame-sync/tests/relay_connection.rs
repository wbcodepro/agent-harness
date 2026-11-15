//! The relay connection renews its access token on the open socket.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hame_sync::mock_relay::MockRelay;
use hame_sync::relay::{RelayOptions, TokenSource, connect};

#[tokio::test]
async fn the_connection_renews_its_token() {
    let relay = MockRelay::start().await;
    let account = relay.account();
    let device = relay.register(&account, "host", "host", None);
    let asked = Arc::new(AtomicUsize::new(0));
    let tokens: Vec<String> = (0..20).map(|_| relay.token_for(&device)).collect();
    let token: TokenSource = {
        let asked = Arc::clone(&asked);
        Arc::new(move || {
            let n = asked.fetch_add(1, Ordering::SeqCst);
            let token = tokens[n.min(19)].clone();
            Box::pin(async move { Ok(token) })
        })
    };
    let mut options = RelayOptions::new(&relay.url, &device, token);
    options.renew_every = Duration::from_millis(50);
    let (handle, _events) = connect(options);
    for _ in 0..100 {
        if relay.renewals(&device) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(relay.renewals(&device) >= 2, "renewed on the same socket");
    assert!(asked.load(Ordering::SeqCst) >= 3);
    handle.stop();
}
