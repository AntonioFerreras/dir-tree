//! Terminal event abstraction.
//!
//! Wraps crossterm events into a simpler enum and runs a background task that
//! forwards them over a channel so the main loop stays non-blocking.

use std::time::Duration;

use crossterm::event::{self, Event as CtEvent, KeyEvent, MouseEvent};
use tokio::sync::mpsc;

/// High-level events consumed by the application.
#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    Tick,
}

/// Spawns a background task that polls the terminal for events and sends them
/// through the returned channel.
pub fn spawn_event_reader(tick_rate: Duration) -> mpsc::UnboundedReceiver<AppEvent> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            // Use crossterm's poll with the tick rate so we can send Tick
            // events even when nothing is happening.
            let has_event = event::poll(tick_rate).unwrap_or(false);
            if has_event {
                if let Ok(ev) = event::read() {
                    let app_event = match ev {
                        CtEvent::Key(k) => AppEvent::Key(k),
                        CtEvent::Mouse(m) => AppEvent::Mouse(m),
                        CtEvent::Resize(w, h) => AppEvent::Resize(w, h),
                        _ => continue,
                    };
                    if tx.send(app_event).is_err() {
                        break; // receiver dropped
                    }
                }
            } else {
                // No event within tick_rate — send a tick.
                if tx.send(AppEvent::Tick).is_err() {
                    break;
                }
            }
        }
    });

    rx
}

