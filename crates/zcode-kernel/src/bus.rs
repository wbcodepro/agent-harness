use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::Registration;

type Listener<E> = Arc<dyn Fn(&E) + Send + Sync>;

/// Synchronous notifications: every listener observes each event, in
/// registration order. Listeners cannot change or veto the event; use a
/// [`crate::Chain`] for that.
pub struct Bus<E> {
    listeners: RwLock<Vec<(u64, Listener<E>)>>,
    next_id: AtomicU64,
}

impl<E> Default for Bus<E> {
    fn default() -> Self {
        Self {
            listeners: RwLock::new(Vec::new()),
            next_id: AtomicU64::new(0),
        }
    }
}

impl<E> Bus<E> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self, listener: impl Fn(&E) + Send + Sync + 'static) -> Registration {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.listeners
            .write()
            .expect("bus lock poisoned")
            .push((id, Arc::new(listener)));
        Registration(id)
    }

    /// Returns whether the listener was still registered.
    pub fn unsubscribe(&self, registration: Registration) -> bool {
        let mut listeners = self.listeners.write().expect("bus lock poisoned");
        let before = listeners.len();
        listeners.retain(|(id, _)| *id != registration.0);
        listeners.len() != before
    }

    /// Delivers `event` to the listeners registered when the call started.
    /// The lock is not held while listeners run, so a listener may subscribe
    /// or unsubscribe.
    pub fn emit(&self, event: &E) {
        let snapshot: Vec<Listener<E>> = self
            .listeners
            .read()
            .expect("bus lock poisoned")
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        for listener in snapshot {
            listener(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn delivers_in_order_until_unsubscribed() {
        let bus = Bus::<u32>::new();
        let seen = Arc::new(Mutex::new(Vec::new()));

        let first = {
            let seen = Arc::clone(&seen);
            bus.subscribe(move |n| seen.lock().unwrap().push(("first", *n)))
        };
        {
            let seen = Arc::clone(&seen);
            bus.subscribe(move |n| seen.lock().unwrap().push(("second", *n)));
        }

        bus.emit(&1);
        assert!(bus.unsubscribe(first));
        assert!(!bus.unsubscribe(first));
        bus.emit(&2);

        assert_eq!(
            *seen.lock().unwrap(),
            vec![("first", 1), ("second", 1), ("second", 2)]
        );
    }
}
