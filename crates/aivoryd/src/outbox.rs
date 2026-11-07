//! The output channel of one connection. Responses and notifications go through it in order;
//! one writer thread per connection drains it. Feed subscriptions keep a clone and use
//! [`Outbox::try_send`], so a slow client never blocks the daemon: a full outbox drops the
//! subscription and the client subscribes again with its cursor.

use std::io;
use std::sync::mpsc;

use serde_json::Value;

/// Messages that a connection buffers before a feed drops it.
pub const OUTBOX_CAPACITY: usize = 1024;

#[derive(Debug)]
pub enum Out {
    Message(Value),
    /// The read side ended: write what is queued, then stop.
    Close,
}

#[derive(Debug, Clone)]
pub struct Outbox(mpsc::SyncSender<Out>);

impl Outbox {
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<Out>) {
        let (tx, rx) = mpsc::sync_channel(capacity);
        (Self(tx), rx)
    }

    /// Queue a message. Blocks while the outbox is full. Fails when the connection is gone.
    pub fn send(&self, v: Value) -> io::Result<()> {
        self.0
            .send(Out::Message(v))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the connection is closed"))
    }

    /// Queue a message without a wait. `false`: the outbox is full or the connection is gone.
    pub fn try_send(&self, v: Value) -> bool {
        self.0.try_send(Out::Message(v)).is_ok()
    }

    pub fn close(&self) {
        let _ = self.0.send(Out::Close);
    }
}
