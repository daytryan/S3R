//! Offline stub of skate2-net. `port/*.rs` are byte-for-byte copies of the port's
//! codec/protocol/frames/range modules (v0.2.3, f72f2c13). `client` and `crypto`
//! are offline stand-ins: no socket is ever opened.

#[path = "port/codec.rs"]
pub mod codec;
#[path = "port/frames.rs"]
pub mod frames;
#[path = "port/protocol.rs"]
pub mod protocol;
#[path = "port/range.rs"]
pub mod range;

pub mod client {
    //! Offline stand-in for the port's `client`: `connect` never touches the network and
    //! reports the connection as failed on the first poll.
    use crate::frames::NetFrame;
    use crate::protocol::{ClientMessage, FactsNotes, ServerMessage};
    use std::cell::Cell;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Disconnect {
        Unreachable,
        Refused(u8),
        Closed(u8),
        TimedOut,
        Network(String),
    }

    #[derive(Clone, Debug, PartialEq)]
    pub enum ClientEvent {
        SignedIn { user_id: u64 },
        Disconnected(Disconnect),
        Message(ServerMessage),
        Frame { user_id: u64, frame: Box<NetFrame>, run_end: bool },
        Facts { user_id: u64, tick: u32, stint: u32, notes: FactsNotes, data: Vec<u8> },
    }

    pub struct Client {
        reported: Cell<bool>,
    }

    impl Client {
        #[must_use]
        pub fn connect(_server: &str, _nickname: &str, _game_version: &str) -> Self {
            Self { reported: Cell::new(false) }
        }
        pub fn send(&self, _message: ClientMessage) {}
        pub fn send_tick(&self, _messages: Vec<ClientMessage>, _frames: Vec<NetFrame>) {}
        #[must_use]
        pub fn poll(&self) -> Vec<ClientEvent> {
            if self.reported.replace(true) {
                Vec::new()
            } else {
                vec![ClientEvent::Disconnected(Disconnect::Network("offline stub: networking stubbed out".into()))]
            }
        }
    }
}

pub mod crypto {
    //! Offline stand-in for the port's `crypto::Build`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Build([u8; 32]);
    impl Build {
        pub const DEVELOPMENT: Self = Self([0; 32]);
        #[must_use]
        pub fn current() -> Self {
            Self::DEVELOPMENT
        }
        #[must_use]
        pub fn id(self) -> String {
            "offline stub".into()
        }
    }
}
