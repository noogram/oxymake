//! # Event System
//!
//! Structured events emitted during workflow execution. Every state
//! transition produces an event that flows to `Reporter` implementations.
//!
//! The event types are defined in [`crate::model::Event`]. This module
//! provides the channel infrastructure for dispatching events from the
//! scheduler to reporters.

use std::sync::Arc;
use tokio::sync::broadcast;

use crate::model::Event;

/// Default channel capacity for event broadcasting.
const DEFAULT_CHANNEL_CAPACITY: usize = 1024;

/// An event emitter that broadcasts events to all subscribers.
///
/// Uses `tokio::broadcast` — multiple receivers (reporters) independently
/// consume events. Events not consumed before the channel fills are dropped.
///
/// # Example
/// ```
/// use ox_core::event::EventBus;
///
/// let bus = EventBus::new();
/// let mut rx = bus.subscribe();
/// assert_eq!(bus.subscriber_count(), 1);
/// ```
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: Arc<broadcast::Sender<Event>>,
}

impl EventBus {
    /// Create a new event bus with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CHANNEL_CAPACITY)
    }

    /// Create a new event bus with the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender: Arc::new(sender),
        }
    }

    /// Subscribe to events.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }

    /// Emit an event to all subscribers. Returns the number of receivers.
    pub fn emit(&self, event: Event) -> usize {
        self.sender.send(event).unwrap_or(0)
    }

    /// Returns the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_emit_and_receive() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let event = Event::RunStarted {
            total_jobs: 100,
            to_run: 10,
            cached: 90,
        };
        bus.emit(event.clone());

        let received = rx.recv().await.unwrap();
        assert_eq!(received, event);
    }

    #[test]
    fn test_no_subscribers() {
        let bus = EventBus::new();
        assert_eq!(
            bus.emit(Event::RunStarted {
                total_jobs: 1,
                to_run: 1,
                cached: 0
            }),
            0
        );
    }

    #[test]
    fn test_subscriber_count() {
        let bus = EventBus::new();
        assert_eq!(bus.subscriber_count(), 0);
        let _rx1 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);
        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
    }

    #[test]
    fn test_default_impl() {
        let bus = EventBus::default();
        assert_eq!(bus.subscriber_count(), 0);
        let _rx = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);
    }
}
