//! Picto Relay application runner connecting its deterministic engine to a radio backend.

pub mod broker;
pub mod protocol;

use std::time::Instant;

use picto_relay_engine::{ClientEvent, ClientId, Engine, EngineEffect, EngineInput};
use picto_relay_radio::{RadioBackend, RadioConfig, RadioError};
use thiserror::Error;

/// A notification ready for delivery to a relay client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientNotification {
    /// Destination relay client.
    pub client_id: ClientId,
    /// Event to serialize and deliver.
    pub event: ClientEvent,
}

/// Effects observed after one runner operation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunnerOutput {
    /// Client notifications produced by the engine.
    pub notifications: Vec<ClientNotification>,
    /// Timer wakeups requested by the engine.
    pub wakeups: Vec<Instant>,
}

/// Errors from the engine or radio boundary.
#[derive(Debug, Error)]
pub enum RunnerError {
    /// The deterministic engine rejected an input.
    #[error(transparent)]
    Engine(#[from] picto_relay_engine::EngineError),
    /// The selected radio backend failed.
    #[error(transparent)]
    Radio(#[from] RadioError),
}

/// Synchronous application runner used by tests and the future radio thread.
#[derive(Debug)]
pub struct Runner<B> {
    engine: Engine,
    radio: B,
}

impl<B: RadioBackend> Runner<B> {
    /// Creates and configures an application runner.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError`] when radio configuration fails.
    pub fn new(engine: Engine, mut radio: B, config: RadioConfig) -> Result<Self, RunnerError> {
        radio.configure(config)?;
        Ok(Self { engine, radio })
    }

    /// Applies one engine input and executes its radio effects.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError`] when the engine rejects the input or a
    /// transmission fails.
    pub fn apply(&mut self, input: EngineInput) -> Result<RunnerOutput, RunnerError> {
        let effects = self.engine.handle(input)?;
        self.execute(effects)
    }

    /// Receives at most one frame from the radio and applies it to the engine.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError`] when reception, decoding, or resulting
    /// transmission fails.
    pub fn poll_radio(&mut self, deadline: Instant) -> Result<RunnerOutput, RunnerError> {
        let Some(frame) = self.radio.receive(deadline)? else {
            return Ok(RunnerOutput::default());
        };
        let observed_at = frame.received_at;
        self.apply(EngineInput {
            at: observed_at,
            event: picto_relay_engine::EngineEvent::Radio(frame),
        })
    }

    /// Returns a shared view of the protocol engine.
    #[must_use]
    pub const fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Splits the runner into its engine and backend after shutdown.
    #[must_use]
    pub fn into_parts(self) -> (Engine, B) {
        (self.engine, self.radio)
    }

    /// Returns exclusive access to the selected radio backend.
    #[must_use]
    pub const fn radio_mut(&mut self) -> &mut B {
        &mut self.radio
    }

    fn execute(&mut self, effects: Vec<EngineEffect>) -> Result<RunnerOutput, RunnerError> {
        let mut output = RunnerOutput::default();
        for effect in effects {
            match effect {
                EngineEffect::Transmit(frame) => self.radio.transmit(frame)?,
                EngineEffect::Notify { client_id, event } => {
                    output
                        .notifications
                        .push(ClientNotification { client_id, event });
                }
                EngineEffect::ScheduleWakeup(at) => output.wakeups.push(at),
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use picto_relay_codec::{MacAddress, NintendoFlow, build_fragment_frame, fragment_payload};
    use picto_relay_engine::{
        ClientCommand, ClientEvent, ClientId, Engine, EngineConfig, EngineEvent, EngineInput,
        virtual_mac,
    };
    use picto_relay_radio::{ExpectedFrame, RadioConfig, ReceivedFrame, ScriptStep, ScriptedRadio};

    use super::{ClientNotification, Runner};

    fn client_input(at: Instant, command: ClientCommand) -> EngineInput {
        EngineInput {
            at,
            event: EngineEvent::Client(command),
        }
    }

    fn scripted_runner(start: Instant, sender: ClientId) -> Runner<ScriptedRadio> {
        let bssid = MacAddress([0x00, 0x09, 0xbf, 0x52, 0x00, 0x00]);
        let outgoing_fragment = fragment_payload(b"ping", [0, 0], 160)
            .expect("outgoing fixture should fragment")
            .remove(0);
        let outgoing = build_fragment_frame(
            NintendoFlow::ClientToHost,
            virtual_mac(sender),
            bssid,
            0,
            false,
            &outgoing_fragment,
        )
        .expect("outgoing fixture should encode");
        let retry = build_fragment_frame(
            NintendoFlow::ClientToHost,
            virtual_mac(sender),
            bssid,
            0,
            true,
            &outgoing_fragment,
        )
        .expect("retry fixture should encode");
        let incoming_fragment = fragment_payload(b"from DS!", [12, 0], 160)
            .expect("incoming fixture should fragment")
            .remove(0);
        let incoming = build_fragment_frame(
            NintendoFlow::HostToClient,
            virtual_mac(ClientId(99)),
            bssid,
            12,
            false,
            &incoming_fragment,
        )
        .expect("incoming fixture should encode");
        let radio = ScriptedRadio::new([
            ScriptStep::ExpectTransmit(ExpectedFrame::exact(outgoing)),
            ScriptStep::Receive {
                at: start + Duration::from_millis(5),
                frame: ReceivedFrame {
                    received_at: start + Duration::from_millis(5),
                    tsft_us: None,
                    bytes: incoming,
                    rssi_dbm: Some(-35),
                    antenna: None,
                },
            },
            ScriptStep::ExpectTransmit(ExpectedFrame::exact(retry)),
        ]);
        let engine = Engine::new(EngineConfig {
            retry_interval: Duration::from_millis(10),
            max_attempts: 2,
        });
        Runner::new(engine, radio, RadioConfig { channel: 1 })
            .expect("scripted radio should configure")
    }

    #[test]
    fn scripted_runner_should_connect_exchange_retry_and_disconnect() {
        let start = Instant::now();
        let alice = ClientId(1);
        let bob = ClientId(2);
        let retry_at = start + Duration::from_millis(10);
        let mut runner = scripted_runner(start, alice);

        let alice_connected = runner
            .apply(client_input(
                start,
                ClientCommand::Connect {
                    client_id: alice,
                    name: "Alice".into(),
                },
            ))
            .expect("Alice should connect");
        assert_eq!(
            alice_connected.notifications,
            vec![ClientNotification {
                client_id: alice,
                event: ClientEvent::Connected,
            }]
        );

        runner
            .apply(client_input(
                start,
                ClientCommand::Connect {
                    client_id: bob,
                    name: "Bob".into(),
                },
            ))
            .expect("Bob should connect");
        assert!(runner.engine().contains_client(alice));
        assert!(runner.engine().contains_client(bob));

        let sent = runner
            .apply(client_input(
                start,
                ClientCommand::SendMessage {
                    client_id: alice,
                    payload: b"ping".to_vec(),
                },
            ))
            .expect("Alice's message should be transmitted");
        assert_eq!(sent.wakeups, vec![retry_at]);

        let received = runner
            .poll_radio(start + Duration::from_millis(5))
            .expect("scripted frame should be received");
        assert_eq!(received.notifications.len(), 2);
        assert!(received.notifications.iter().any(|notification| {
            notification.client_id == bob
                && notification.event
                    == ClientEvent::MessageReceived {
                        station_id: 99,
                        message_id: 12,
                        payload: b"from DS!".to_vec(),
                    }
        }));

        let retry = runner
            .apply(EngineInput {
                at: retry_at,
                event: EngineEvent::Tick,
            })
            .expect("timer should trigger a retry");
        assert_eq!(retry.wakeups, vec![retry_at + Duration::from_millis(10)]);

        let disconnected = runner
            .apply(client_input(
                retry_at,
                ClientCommand::Disconnect { client_id: alice },
            ))
            .expect("Alice should disconnect");
        assert_eq!(
            disconnected.notifications,
            vec![ClientNotification {
                client_id: alice,
                event: ClientEvent::Disconnected,
            }]
        );
        assert!(!runner.engine().contains_client(alice));
        assert!(runner.engine().contains_client(bob));

        let (_, radio) = runner.into_parts();
        radio.finish().expect("script should be fully consumed");
    }
}
