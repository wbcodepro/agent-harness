//! Events for the page wait until it can receive them.
//!
//! The shell tells the page things with `hame:desktop` DOM events (open a
//! session, start a new one). A link that launches the app, or arrives
//! while the page loads or reloads, would reach a page whose listener does
//! not exist yet and be lost. So events are queued until the page says it
//! is listening (`desktop_ready`, sent by the interface once its listener
//! is attached), and the queue starts over whenever a page load begins.

use serde_json::Value;

/// At most this many events wait; older ones give way.
const MOST: usize = 16;

#[derive(Debug, Default)]
pub struct PageQueue {
    ready: bool,
    waiting: Vec<Value>,
}

impl PageQueue {
    /// An event to hand over: returned when the page listens now, else kept.
    pub fn send(&mut self, event: Value) -> Option<Value> {
        if self.ready {
            return Some(event);
        }
        if self.waiting.len() == MOST {
            self.waiting.remove(0);
        }
        self.waiting.push(event);
        None
    }

    /// A page load began: what it had is gone, and it does not listen yet.
    pub fn loading(&mut self) {
        self.ready = false;
    }

    /// The page listens: the events kept for it, in order.
    pub fn ready(&mut self) -> Vec<Value> {
        self.ready = true;
        std::mem::take(&mut self.waiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_wait_for_the_page_and_arrive_in_order() {
        let mut queue = PageQueue::default();
        assert_eq!(queue.send(json!(1)), None);
        assert_eq!(queue.send(json!(2)), None);
        assert_eq!(queue.ready(), vec![json!(1), json!(2)]);
        assert_eq!(queue.send(json!(3)), Some(json!(3)));
        assert_eq!(queue.ready(), Vec::<Value>::new());
    }

    #[test]
    fn a_reload_waits_again() {
        let mut queue = PageQueue::default();
        queue.ready();
        queue.loading();
        assert_eq!(queue.send(json!("new")), None);
        assert_eq!(queue.ready(), vec![json!("new")]);
    }

    #[test]
    fn the_queue_is_bounded() {
        let mut queue = PageQueue::default();
        for n in 0..40 {
            queue.send(json!(n));
        }
        let kept = queue.ready();
        assert_eq!(kept.len(), MOST);
        assert_eq!(kept.first(), Some(&json!(40 - MOST)));
    }
}
