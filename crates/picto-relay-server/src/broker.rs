//! Bounded async broker for API-owned virtual stations.

use std::{collections::BTreeMap, sync::mpsc as std_mpsc, thread, time::Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use picto_relay_codec::{
    FcsMode, MacFrame, NintendoFlow, PictoChatEnvelope, PictoChatFragment, build_ack_frame,
    decode_nintendo_data,
};
use picto_relay_engine::{
    ClientCommand, ClientEvent, ClientId, Engine, EngineConfig, EngineEvent, EngineInput,
};
use picto_relay_join::{LiveCommand, LiveConfig, LiveEvent, run_live_backend};
use picto_relay_radio::{RadioConfig, ReceivedFrame, ScriptedRadio};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use crate::{
    ClientNotification, Runner, RunnerError, RunnerOutput,
    protocol::{BITMAP_BYTES, Participant, ServerEnvelope, ServerWireEvent},
};

const COMMAND_CAPACITY: usize = 64;
const CLIENT_EVENT_CAPACITY: usize = 32;

/// Handle used by HTTP tasks to communicate with the broker actor.
#[derive(Clone, Debug)]
pub struct BrokerHandle {
    commands: mpsc::Sender<BrokerCommand>,
}

/// An API session registered with the broker.
#[derive(Debug)]
pub struct RelaySession {
    /// Assigned virtual station identity.
    pub client_id: ClientId,
    /// Bounded stream of events for this session.
    pub events: mpsc::Receiver<ServerEnvelope>,
}

/// Failures returned to an API task.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BrokerError {
    /// The single broker actor has stopped.
    #[error("broker is unavailable")]
    Unavailable,
    /// The client's command queue is full.
    #[error("broker command queue is full")]
    Busy,
    /// A bitmap does not have the required packed size.
    #[error("bitmap must contain exactly {BITMAP_BYTES} bytes, received {0}")]
    InvalidBitmapSize(usize),
}

impl BrokerHandle {
    /// Starts the broker actor with an interactive [`ScriptedRadio`].
    #[must_use]
    pub fn spawn() -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        tokio::spawn(async move {
            let radio = ScriptedRadio::recording();
            let engine = Engine::new(EngineConfig::default());
            let runner = Runner::new(engine, radio, RadioConfig { channel: 1 });
            match runner {
                Ok(runner) => Broker::new(runner, receiver).run().await,
                Err(error) => warn!(%error, "failed to initialize scripted broker"),
            }
        });
        Self { commands }
    }

    /// Starts a single-station broker backed by a physical AR9271 radio worker.
    #[must_use]
    pub fn spawn_live(config: LiveConfig) -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        tokio::spawn(LiveBroker::new(config, receiver).run());
        Self { commands }
    }

    /// Registers a caller and allocates its virtual station.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError`] if the actor is busy or unavailable.
    pub async fn register(&self, name: String) -> Result<RelaySession, BrokerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .try_send(BrokerCommand::Register { name, reply })
            .map_err(|error| map_send_error(&error))?;
        response.await.map_err(|_| BrokerError::Unavailable)?
    }

    /// Submits one packed monochrome message.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError`] for an invalid bitmap or unavailable actor.
    pub fn send_message(&self, client_id: ClientId, bitmap: Vec<u8>) -> Result<(), BrokerError> {
        if bitmap.len() != BITMAP_BYTES {
            return Err(BrokerError::InvalidBitmapSize(bitmap.len()));
        }
        self.commands
            .try_send(BrokerCommand::SendMessage { client_id, bitmap })
            .map_err(|error| map_send_error(&error))
    }

    /// Requests removal of a relay session.
    pub fn unregister(&self, client_id: ClientId) {
        if let Err(error) = self
            .commands
            .try_send(BrokerCommand::Unregister { client_id })
        {
            debug!(?error, ?client_id, "could not enqueue client disconnect");
        }
    }
}

fn map_send_error<T>(error: &mpsc::error::TrySendError<T>) -> BrokerError {
    match error {
        mpsc::error::TrySendError::Full(_) => BrokerError::Busy,
        mpsc::error::TrySendError::Closed(_) => BrokerError::Unavailable,
    }
}

#[derive(Debug)]
enum BrokerCommand {
    Register {
        name: String,
        reply: oneshot::Sender<Result<RelaySession, BrokerError>>,
    },
    SendMessage {
        client_id: ClientId,
        bitmap: Vec<u8>,
    },
    Unregister {
        client_id: ClientId,
    },
}

#[derive(Debug)]
struct ConnectedClient {
    name: String,
    events: mpsc::Sender<ServerEnvelope>,
}

struct Broker {
    runner: Runner<ScriptedRadio>,
    commands: mpsc::Receiver<BrokerCommand>,
    clients: BTreeMap<ClientId, ConnectedClient>,
    next_client_id: u32,
}

impl Broker {
    fn new(runner: Runner<ScriptedRadio>, commands: mpsc::Receiver<BrokerCommand>) -> Self {
        Self {
            runner,
            commands,
            clients: BTreeMap::new(),
            next_client_id: 1,
        }
    }

    async fn run(mut self) {
        while let Some(command) = self.commands.recv().await {
            if let Err(error) = self.handle(command) {
                warn!(%error, "broker command failed");
            }
        }
    }

    fn handle(&mut self, command: BrokerCommand) -> Result<(), BrokerActorError> {
        match command {
            BrokerCommand::Register { name, reply } => self.register(name, reply),
            BrokerCommand::SendMessage { client_id, bitmap } => {
                self.send_message(client_id, bitmap)
            }
            BrokerCommand::Unregister { client_id } => self.unregister(client_id),
        }
    }

    fn register(
        &mut self,
        name: String,
        reply: oneshot::Sender<Result<RelaySession, BrokerError>>,
    ) -> Result<(), BrokerActorError> {
        let client_id = ClientId(self.next_client_id);
        self.next_client_id = self.next_client_id.wrapping_add(1).max(1);
        let (events, receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        self.clients.insert(
            client_id,
            ConnectedClient {
                name,
                events: events.clone(),
            },
        );
        let output = self.runner.apply(client_input(
            client_id,
            ClientCommand::Connect {
                client_id,
                name: self.clients[&client_id].name.clone(),
            },
        ))?;
        self.route(output);
        self.send_to(
            client_id,
            ServerWireEvent::SessionReady {
                client_id: client_id.0,
            },
        );
        self.broadcast_participants();

        if reply
            .send(Ok(RelaySession {
                client_id,
                events: receiver,
            }))
            .is_err()
        {
            self.unregister(client_id)?;
        }
        Ok(())
    }

    fn send_message(
        &mut self,
        client_id: ClientId,
        bitmap: Vec<u8>,
    ) -> Result<(), BrokerActorError> {
        let output = self.runner.apply(client_input(
            client_id,
            ClientCommand::SendMessage {
                client_id,
                payload: bitmap,
            },
        ))?;
        self.route(output);
        self.loop_back_recorded_transmissions()?;
        Ok(())
    }

    fn unregister(&mut self, client_id: ClientId) -> Result<(), BrokerActorError> {
        if !self.clients.contains_key(&client_id) {
            return Ok(());
        }
        let output = self.runner.apply(client_input(
            client_id,
            ClientCommand::Disconnect { client_id },
        ))?;
        self.route(output);
        self.clients.remove(&client_id);
        self.broadcast_participants();
        Ok(())
    }

    fn loop_back_recorded_transmissions(&mut self) -> Result<(), BrokerActorError> {
        let transmitted = self.runner.radio_mut().take_transmissions();
        let mut injected = 0;
        for frame in transmitted {
            let now = Instant::now();
            let mac_frame = MacFrame::parse(&frame.bytes, FcsMode::Absent)?;
            let data = decode_nintendo_data(&mac_frame)?;
            let is_final_fragment = if data.flow == NintendoFlow::ClientToHost {
                let envelope = PictoChatEnvelope::parse(data.flow, &data.payload)?;
                let fragment = PictoChatFragment::parse(&envelope.payload, data.flow)?;
                fragment.final_fragment
            } else {
                false
            };
            self.runner.radio_mut().inject(ReceivedFrame {
                received_at: now,
                tsft_us: None,
                bytes: frame.bytes,
                rssi_dbm: None,
                antenna: None,
            });
            injected += 1;
            if is_final_fragment {
                let bssid = data.bssid.unwrap_or(data.source);
                self.runner.radio_mut().inject(ReceivedFrame {
                    received_at: now,
                    tsft_us: None,
                    bytes: build_ack_frame(bssid, bssid, data.sequence_number),
                    rssi_dbm: None,
                    antenna: None,
                });
                injected += 1;
            }
        }
        for _ in 0..injected {
            let output = self.runner.poll_radio(Instant::now())?;
            self.route(output);
        }
        Ok(())
    }

    fn route(&mut self, output: RunnerOutput) {
        for notification in output.notifications {
            self.route_notification(notification);
        }
    }

    fn route_notification(&mut self, notification: ClientNotification) {
        let event = match notification.event {
            ClientEvent::Connected | ClientEvent::Disconnected => return,
            ClientEvent::MessageReceived {
                station_id,
                message_id,
                payload,
            } => ServerWireEvent::Message {
                station_id,
                name: self
                    .clients
                    .get(&ClientId(station_id))
                    .map_or_else(|| format!("DS {station_id}"), |client| client.name.clone()),
                message_id,
                bitmap: STANDARD.encode(payload),
            },
            ClientEvent::Delivered { message_id } => ServerWireEvent::Delivered { message_id },
            ClientEvent::DeliveryTimedOut { message_id } => {
                ServerWireEvent::DeliveryTimedOut { message_id }
            }
        };
        self.send_to(notification.client_id, event);
    }

    fn broadcast_participants(&mut self) {
        let participants = self
            .clients
            .iter()
            .map(|(&client_id, client)| Participant {
                client_id: client_id.0,
                name: client.name.clone(),
            })
            .collect::<Vec<_>>();
        let event = ServerEnvelope::new(ServerWireEvent::Participants { participants });
        self.clients
            .retain(|_, client| client.events.try_send(event.clone()).is_ok());
    }

    fn send_to(&mut self, client_id: ClientId, event: ServerWireEvent) {
        let should_remove = self
            .clients
            .get(&client_id)
            .is_some_and(|client| client.events.try_send(ServerEnvelope::new(event)).is_err());
        if should_remove {
            self.clients.remove(&client_id);
        }
    }
}

struct LiveBroker {
    config: LiveConfig,
    commands: mpsc::Receiver<BrokerCommand>,
    hardware_events: mpsc::UnboundedReceiver<LiveEvent>,
    hardware_event_sender: mpsc::UnboundedSender<LiveEvent>,
    hardware_commands: Option<std_mpsc::Sender<LiveCommand>>,
    hardware_running: bool,
    client: Option<(ClientId, ConnectedClient)>,
    next_message_id: u32,
}

impl LiveBroker {
    fn new(config: LiveConfig, commands: mpsc::Receiver<BrokerCommand>) -> Self {
        let (hardware_event_sender, hardware_events) = mpsc::unbounded_channel();
        Self {
            config,
            commands,
            hardware_events,
            hardware_event_sender,
            hardware_commands: None,
            hardware_running: false,
            client: None,
            next_message_id: 0,
        }
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                command = self.commands.recv() => {
                    let Some(command) = command else {
                        self.shutdown_hardware();
                        break;
                    };
                    self.handle_command(command);
                }
                event = self.hardware_events.recv(), if self.hardware_running => {
                    let Some(event) = event else {
                        self.hardware_running = false;
                        self.hardware_commands = None;
                        continue;
                    };
                    self.handle_hardware_event(event);
                }
            }
        }
    }

    fn handle_command(&mut self, command: BrokerCommand) {
        match command {
            BrokerCommand::Register { name, reply } => self.register(name, reply),
            BrokerCommand::SendMessage { client_id, bitmap } => {
                self.send_message(client_id, bitmap);
            }
            BrokerCommand::Unregister { client_id } => self.unregister(client_id),
        }
    }

    fn register(
        &mut self,
        name: String,
        reply: oneshot::Sender<Result<RelaySession, BrokerError>>,
    ) {
        if self.client.is_some() || self.hardware_running {
            let _ = reply.send(Err(BrokerError::Busy));
            return;
        }
        let client_id = ClientId(1);
        let (events, receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        self.client = Some((
            client_id,
            ConnectedClient {
                name: name.clone(),
                events,
            },
        ));
        let (hardware_commands, hardware_receiver) = std_mpsc::channel();
        self.hardware_commands = Some(hardware_commands);
        self.hardware_running = true;
        let config = self.config.clone();
        let hardware_events = self.hardware_event_sender.clone();
        let spawn_result = thread::Builder::new()
            .name("picto-relay-radio".into())
            .spawn(move || {
                let emit = |event| {
                    let _ = hardware_events.send(event);
                };
                if let Err(error) = run_live_backend(&config, &name, &hardware_receiver, &emit) {
                    emit(LiveEvent::Error {
                        code: "hardware_failed",
                        message: error.to_string(),
                    });
                    emit(LiveEvent::Left);
                }
            });
        if let Err(error) = spawn_result {
            self.hardware_running = false;
            self.hardware_commands = None;
            self.client = None;
            let _ = reply.send(Err(BrokerError::Unavailable));
            warn!(%error, "failed to start physical radio thread");
            return;
        }
        let _ = reply.send(Ok(RelaySession {
            client_id,
            events: receiver,
        }));
    }

    fn send_message(&mut self, client_id: ClientId, bitmap: Vec<u8>) {
        if self.client.as_ref().map(|(id, _)| *id) != Some(client_id) {
            return;
        }
        let message_id = self.next_message_id;
        self.next_message_id = self.next_message_id.wrapping_add(1);
        let result = self
            .hardware_commands
            .as_ref()
            .ok_or("physical radio is not running")
            .and_then(|sender| {
                sender
                    .send(LiveCommand::SendMessage { message_id, bitmap })
                    .map_err(|_| "physical radio command channel closed")
            });
        if let Err(message) = result {
            self.send_to(ServerWireEvent::Error {
                code: "hardware_unavailable",
                message: message.into(),
            });
        }
    }

    fn unregister(&mut self, client_id: ClientId) {
        if self.client.as_ref().map(|(id, _)| *id) != Some(client_id) {
            return;
        }
        self.client = None;
        self.shutdown_hardware();
    }

    fn shutdown_hardware(&mut self) {
        if let Some(sender) = self.hardware_commands.take() {
            let _ = sender.send(LiveCommand::Shutdown);
        }
    }

    fn handle_hardware_event(&mut self, event: LiveEvent) {
        let wire_event = match event {
            LiveEvent::State { state, detail } => ServerWireEvent::BackendState { state, detail },
            LiveEvent::Ready { host, room, slot } => {
                let detail = format!(
                    "joined Room {} hosted by {host} in slot {slot}",
                    char::from(b'A' + room)
                );
                self.send_to(ServerWireEvent::BackendState {
                    state: "ready",
                    detail,
                });
                let Some((client_id, _)) = &self.client else {
                    return;
                };
                ServerWireEvent::SessionReady {
                    client_id: client_id.0,
                }
            }
            LiveEvent::Message {
                station_id,
                name,
                message_id,
                bitmap,
            } => ServerWireEvent::Message {
                station_id,
                name,
                message_id,
                bitmap: STANDARD.encode(bitmap),
            },
            LiveEvent::Delivered { message_id } => ServerWireEvent::Delivered { message_id },
            LiveEvent::DeliveryTimedOut { message_id } => {
                ServerWireEvent::DeliveryTimedOut { message_id }
            }
            LiveEvent::Error { code, message } => ServerWireEvent::Error { code, message },
            LiveEvent::Left => {
                self.hardware_running = false;
                self.hardware_commands = None;
                ServerWireEvent::BackendState {
                    state: "left",
                    detail: "physical station left the PictoChat room".into(),
                }
            }
        };
        self.send_to(wire_event);
    }

    fn send_to(&mut self, event: ServerWireEvent) {
        let should_remove = self
            .client
            .as_ref()
            .is_some_and(|(_, client)| client.events.try_send(ServerEnvelope::new(event)).is_err());
        if should_remove {
            self.client = None;
            self.shutdown_hardware();
        }
    }
}

fn client_input(client_id: ClientId, command: ClientCommand) -> EngineInput {
    debug!(?client_id, "applying client command");
    EngineInput {
        at: Instant::now(),
        event: EngineEvent::Client(command),
    }
}

#[derive(Debug, Error)]
enum BrokerActorError {
    #[error(transparent)]
    Runner(#[from] RunnerError),
    #[error(transparent)]
    Codec(#[from] picto_relay_codec::DecodeError),
}

#[cfg(test)]
mod tests {
    use super::{BITMAP_BYTES, BrokerHandle};
    use crate::protocol::ServerWireEvent;

    #[tokio::test]
    async fn broker_should_deliver_message_to_two_virtual_clients() {
        let broker = BrokerHandle::spawn();
        let mut alice = broker
            .register("Alice".into())
            .await
            .expect("Alice should register");
        let mut bob = broker
            .register("Bob".into())
            .await
            .expect("Bob should register");
        drain(&mut alice.events);
        drain(&mut bob.events);

        broker
            .send_message(alice.client_id, vec![0x55; BITMAP_BYTES])
            .expect("bitmap should enqueue");

        let alice_message = receive_message(&mut alice.events).await;
        let bob_message = receive_message(&mut bob.events).await;
        assert!(matches!(
            alice_message,
            ServerWireEvent::Message { station_id: 1, .. }
        ));
        assert!(matches!(
            bob_message,
            ServerWireEvent::Message { station_id: 1, .. }
        ));
    }

    fn drain(events: &mut tokio::sync::mpsc::Receiver<crate::protocol::ServerEnvelope>) {
        while events.try_recv().is_ok() {}
    }

    async fn receive_message(
        events: &mut tokio::sync::mpsc::Receiver<crate::protocol::ServerEnvelope>,
    ) -> ServerWireEvent {
        loop {
            let envelope = events.recv().await.expect("broker should remain available");
            if matches!(envelope.event, ServerWireEvent::Message { .. }) {
                return envelope.event;
            }
        }
    }
}
