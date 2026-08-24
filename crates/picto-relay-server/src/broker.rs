//! Bounded async broker for API-owned virtual stations.

use std::{collections::BTreeMap, sync::mpsc as std_mpsc, thread, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use picto_relay_join::{
    LiveCommand, LiveConfig, LiveEvent, run_live_backend, run_live_room_observer,
};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, warn};

use crate::protocol::{
    BITMAP_BYTES, Participant, ROOM_COUNT, RoomSummary, ServerEnvelope, ServerWireEvent,
};

const COMMAND_CAPACITY: usize = 64;
const CLIENT_EVENT_CAPACITY: usize = 32;

/// Stable identity assigned to one API session.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ClientId(pub u32);

/// Handle used by HTTP tasks to communicate with the broker actor.
#[derive(Clone, Debug)]
pub struct BrokerHandle {
    commands: mpsc::Sender<BrokerCommand>,
    rooms: watch::Receiver<Vec<RoomSummary>>,
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
    /// A room outside A through D was requested.
    #[error("room must be between 0 (A) and 3 (D), received {0}")]
    InvalidRoom(u8),
}

impl BrokerHandle {
    /// Starts the in-memory development broker.
    ///
    /// This backend exercises the JSON session contract only. It does not
    /// simulate Nintendo wireless delivery or claim protocol compatibility.
    #[must_use]
    pub fn spawn_memory() -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (room_updates, rooms) = watch::channel(empty_room_summaries());
        tokio::spawn(Broker::new(receiver, room_updates).run());
        Self { commands, rooms }
    }

    /// Starts a single-station broker backed by a physical AR9271 radio worker.
    #[must_use]
    pub fn spawn_live(config: LiveConfig) -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (room_updates, rooms) = watch::channel(empty_room_summaries());
        tokio::spawn(LiveBroker::new(config, receiver, room_updates).run());
        Self { commands, rooms }
    }

    /// Registers a caller and allocates its virtual station.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError`] if the actor is busy or unavailable.
    pub async fn register(&self, name: String, room: u8) -> Result<RelaySession, BrokerError> {
        if room >= ROOM_COUNT {
            return Err(BrokerError::InvalidRoom(room));
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .try_send(BrokerCommand::Register { name, room, reply })
            .map_err(|error| map_send_error(&error))?;
        response.await.map_err(|_| BrokerError::Unavailable)?
    }

    /// Returns the participant rosters known to the selected backend.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerError`] if the actor is busy or unavailable.
    pub async fn rooms(&self) -> Result<Vec<RoomSummary>, BrokerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .try_send(BrokerCommand::Rooms { reply })
            .map_err(|error| map_send_error(&error))?;
        response.await.map_err(|_| BrokerError::Unavailable)
    }

    /// Subscribes to complete room snapshots, including the current value.
    #[must_use]
    pub fn subscribe_rooms(&self) -> watch::Receiver<Vec<RoomSummary>> {
        self.rooms.clone()
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
        room: u8,
        reply: oneshot::Sender<Result<RelaySession, BrokerError>>,
    },
    Rooms {
        reply: oneshot::Sender<Vec<RoomSummary>>,
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
    room: u8,
    events: mpsc::Sender<ServerEnvelope>,
    next_message_id: u32,
}

struct Broker {
    commands: mpsc::Receiver<BrokerCommand>,
    room_updates: watch::Sender<Vec<RoomSummary>>,
    clients: BTreeMap<ClientId, ConnectedClient>,
    next_client_id: u32,
}

impl Broker {
    fn new(
        commands: mpsc::Receiver<BrokerCommand>,
        room_updates: watch::Sender<Vec<RoomSummary>>,
    ) -> Self {
        Self {
            commands,
            room_updates,
            clients: BTreeMap::new(),
            next_client_id: 1,
        }
    }

    async fn run(mut self) {
        while let Some(command) = self.commands.recv().await {
            self.handle(command);
        }
    }

    fn handle(&mut self, command: BrokerCommand) {
        match command {
            BrokerCommand::Register { name, room, reply } => self.register(name, room, reply),
            BrokerCommand::Rooms { reply } => {
                let _ = reply.send(room_summaries(self.clients.iter()));
            }
            BrokerCommand::SendMessage { client_id, bitmap } => {
                self.send_message(client_id, bitmap);
            }
            BrokerCommand::Unregister { client_id } => self.unregister(client_id),
        }
    }

    fn register(
        &mut self,
        name: String,
        room: u8,
        reply: oneshot::Sender<Result<RelaySession, BrokerError>>,
    ) {
        let client_id = ClientId(self.next_client_id);
        self.next_client_id = self.next_client_id.wrapping_add(1).max(1);
        let (events, receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let joined = Participant {
            client_id: client_id.0,
            name: name.clone(),
        };
        self.clients.insert(
            client_id,
            ConnectedClient {
                name,
                room,
                events: events.clone(),
                next_message_id: 0,
            },
        );
        self.send_to(
            client_id,
            ServerWireEvent::SessionReady {
                client_id: client_id.0,
            },
        );
        self.broadcast_to_room(
            room,
            ServerWireEvent::ParticipantJoined {
                room,
                participant: joined,
            },
        );
        self.broadcast_participants(room);

        if reply
            .send(Ok(RelaySession {
                client_id,
                events: receiver,
            }))
            .is_err()
        {
            self.unregister(client_id);
        }
    }

    fn send_message(&mut self, client_id: ClientId, bitmap: Vec<u8>) {
        let Some(sender) = self.clients.get_mut(&client_id) else {
            return;
        };
        let message_id = sender.next_message_id;
        sender.next_message_id = sender.next_message_id.wrapping_add(1);
        let sender_name = sender.name.clone();
        let room = sender.room;
        let message = ServerEnvelope::new(ServerWireEvent::Message {
            station_id: client_id.0,
            name: sender_name,
            message_id,
            bitmap: STANDARD.encode(bitmap),
        });
        let previous_clients = self.clients.len();
        self.clients.retain(|_, client| {
            client.room != room || client.events.try_send(message.clone()).is_ok()
        });
        if self.clients.len() != previous_clients {
            self.publish_rooms();
        }
        self.send_to(client_id, ServerWireEvent::Delivered { message_id });
    }

    fn unregister(&mut self, client_id: ClientId) {
        let Some(client) = self.clients.remove(&client_id) else {
            return;
        };
        self.broadcast_to_room(
            client.room,
            ServerWireEvent::ParticipantLeft {
                room: client.room,
                participant: Participant {
                    client_id: client_id.0,
                    name: client.name,
                },
            },
        );
        self.broadcast_participants(client.room);
    }

    fn broadcast_to_room(&mut self, room: u8, event: ServerWireEvent) {
        let event = ServerEnvelope::new(event);
        self.clients.retain(|_, client| {
            client.room != room || client.events.try_send(event.clone()).is_ok()
        });
    }

    fn broadcast_participants(&mut self, room: u8) {
        let participants = self
            .clients
            .iter()
            .filter(|(_, client)| client.room == room)
            .map(participant)
            .collect::<Vec<_>>();
        let event = ServerEnvelope::new(ServerWireEvent::Participants { room, participants });
        self.clients.retain(|_, client| {
            client.room != room || client.events.try_send(event.clone()).is_ok()
        });
        self.publish_rooms();
    }

    fn send_to(&mut self, client_id: ClientId, event: ServerWireEvent) {
        let should_remove = self
            .clients
            .get(&client_id)
            .is_some_and(|client| client.events.try_send(ServerEnvelope::new(event)).is_err());
        if should_remove {
            self.clients.remove(&client_id);
            self.publish_rooms();
        }
    }

    fn publish_rooms(&self) {
        publish_rooms_if_changed(&self.room_updates, room_summaries(self.clients.iter()));
    }
}

fn participant((&client_id, client): (&ClientId, &ConnectedClient)) -> Participant {
    Participant {
        client_id: client_id.0,
        name: client.name.clone(),
    }
}

fn room_summaries<'a>(
    clients: impl Iterator<Item = (&'a ClientId, &'a ConnectedClient)>,
) -> Vec<RoomSummary> {
    let mut rooms = empty_room_summaries();
    for (client_id, client) in clients {
        let room = &mut rooms[usize::from(client.room)];
        room.participants.push(participant((client_id, client)));
        room.participant_count = room.participants.len();
    }
    rooms
}

fn empty_room_summaries() -> Vec<RoomSummary> {
    (0..ROOM_COUNT)
        .map(|room| RoomSummary {
            room,
            participant_count: 0,
            participants: Vec::new(),
        })
        .collect()
}

fn publish_rooms_if_changed(updates: &watch::Sender<Vec<RoomSummary>>, rooms: Vec<RoomSummary>) {
    let changed = *updates.borrow() != rooms;
    if changed {
        updates.send_replace(rooms);
    }
}

struct LiveBroker {
    config: LiveConfig,
    commands: mpsc::Receiver<BrokerCommand>,
    room_updates: watch::Sender<Vec<RoomSummary>>,
    hardware_events: mpsc::UnboundedReceiver<LiveEvent>,
    hardware_event_sender: mpsc::UnboundedSender<LiveEvent>,
    hardware_commands: Option<std_mpsc::Sender<LiveCommand>>,
    observer_shutdown: Option<std_mpsc::Sender<()>>,
    hardware_running: bool,
    passive_counts: [usize; ROOM_COUNT as usize],
    client: Option<(ClientId, ConnectedClient)>,
    participants: Vec<Participant>,
    roster_initialized: bool,
    next_message_id: u32,
}

impl LiveBroker {
    fn new(
        config: LiveConfig,
        commands: mpsc::Receiver<BrokerCommand>,
        room_updates: watch::Sender<Vec<RoomSummary>>,
    ) -> Self {
        let (hardware_event_sender, hardware_events) = mpsc::unbounded_channel();
        Self {
            config,
            commands,
            room_updates,
            hardware_events,
            hardware_event_sender,
            hardware_commands: None,
            observer_shutdown: None,
            hardware_running: false,
            passive_counts: [0; ROOM_COUNT as usize],
            client: None,
            participants: Vec::new(),
            roster_initialized: false,
            next_message_id: 0,
        }
    }

    async fn run(mut self) {
        self.start_room_observer();
        loop {
            tokio::select! {
                command = self.commands.recv() => {
                    let Some(command) = command else {
                        self.shutdown_hardware();
                        self.shutdown_room_observer();
                        break;
                    };
                    self.handle_command(command);
                }
                event = self.hardware_events.recv() => {
                    let Some(event) = event else {
                        break;
                    };
                    self.handle_hardware_event(event);
                }
            }
        }
    }

    fn start_room_observer(&mut self) {
        let (shutdown, shutdown_receiver) = std_mpsc::channel();
        self.observer_shutdown = Some(shutdown);
        let config = self.config.clone();
        let events = self.hardware_event_sender.clone();
        let spawn_result = thread::Builder::new()
            .name("picto-relay-room-observer".into())
            .spawn(move || {
                loop {
                    let emit = |event| {
                        let _ = events.send(event);
                    };
                    match run_live_room_observer(&config, &shutdown_receiver, &emit) {
                        Ok(()) => break,
                        Err(error) => emit(LiveEvent::RoomObserverError {
                            message: error.to_string(),
                        }),
                    }
                    match shutdown_receiver.recv_timeout(Duration::from_secs(1)) {
                        Ok(()) | Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            });
        if let Err(error) = spawn_result {
            self.observer_shutdown = None;
            warn!(%error, "failed to start passive room observer thread");
        }
    }

    fn shutdown_room_observer(&mut self) {
        if let Some(sender) = self.observer_shutdown.take() {
            let _ = sender.send(());
        }
    }

    fn handle_command(&mut self, command: BrokerCommand) {
        match command {
            BrokerCommand::Register { name, room, reply } => self.register(&name, room, reply),
            BrokerCommand::Rooms { reply } => {
                let _ = reply.send(self.room_summaries());
            }
            BrokerCommand::SendMessage { client_id, bitmap } => {
                self.send_message(client_id, bitmap);
            }
            BrokerCommand::Unregister { client_id } => self.unregister(client_id),
        }
    }

    fn register(
        &mut self,
        name: &str,
        room: u8,
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
                name: name.to_owned(),
                room,
                events,
                next_message_id: 0,
            },
        ));
        let (hardware_commands, hardware_receiver) = std_mpsc::channel();
        self.hardware_commands = Some(hardware_commands);
        self.hardware_running = true;
        self.next_message_id = 0;
        self.roster_initialized = false;
        let joined = Participant {
            client_id: station_id(self.config.station),
            name: name.to_owned(),
        };
        self.participants = vec![joined.clone()];
        let mut config = self.config.clone();
        config.room = Some(room);
        let hardware_events = self.hardware_event_sender.clone();
        let hardware_name = name.to_owned();
        let spawn_result = thread::Builder::new()
            .name("picto-relay-radio".into())
            .spawn(move || {
                let emit = |event| {
                    let _ = hardware_events.send(event);
                };
                if let Err(error) =
                    run_live_backend(&config, &hardware_name, &hardware_receiver, &emit)
                {
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
            self.participants.clear();
            let _ = reply.send(Err(BrokerError::Unavailable));
            warn!(%error, "failed to start physical radio thread");
            return;
        }
        self.send_to(ServerWireEvent::Participants {
            room,
            participants: self.participants.clone(),
        });
        self.publish_rooms();
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
        self.participants.clear();
        self.roster_initialized = false;
        self.shutdown_hardware();
        self.publish_rooms();
    }

    fn shutdown_hardware(&mut self) {
        if let Some(sender) = self.hardware_commands.take() {
            let _ = sender.send(LiveCommand::Shutdown);
        }
    }

    fn handle_hardware_event(&mut self, event: LiveEvent) {
        match &event {
            LiveEvent::RoomCounts { counts } => {
                self.passive_counts = (*counts).map(usize::from);
                self.publish_rooms();
                return;
            }
            LiveEvent::RoomObserverError { message } => {
                self.passive_counts = [0; ROOM_COUNT as usize];
                self.publish_rooms();
                warn!(%message, "passive room observer stopped; retrying");
                return;
            }
            _ => {}
        }
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
                let joined = self
                    .participants
                    .iter()
                    .find(|participant| participant.client_id == station_id(self.config.station))
                    .cloned();
                if let Some(participant) = joined {
                    self.send_to(ServerWireEvent::ParticipantJoined { room, participant });
                }
                let Some((client_id, _)) = &self.client else {
                    return;
                };
                ServerWireEvent::SessionReady {
                    client_id: client_id.0,
                }
            }
            LiveEvent::Participants { room, participants } => {
                let participants = participants
                    .into_iter()
                    .map(|participant| Participant {
                        client_id: station_id(participant.station),
                        name: participant.name,
                    })
                    .collect::<Vec<_>>();
                if self.roster_initialized {
                    self.send_participant_changes(room, &participants);
                } else {
                    self.roster_initialized = true;
                }
                self.participants = participants;
                ServerWireEvent::Participants {
                    room,
                    participants: self.participants.clone(),
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
                self.roster_initialized = false;
                let room = self.client.as_ref().map(|(_, client)| client.room);
                let participants = std::mem::take(&mut self.participants);
                if let Some(room) = room {
                    for participant in participants {
                        self.send_to(ServerWireEvent::ParticipantLeft { room, participant });
                    }
                    self.send_to(ServerWireEvent::Participants {
                        room,
                        participants: Vec::new(),
                    });
                }
                ServerWireEvent::BackendState {
                    state: "left",
                    detail: "physical station left the PictoChat room".into(),
                }
            }
            LiveEvent::RoomCounts { .. } | LiveEvent::RoomObserverError { .. } => unreachable!(),
        };
        self.send_to(wire_event);
        self.publish_rooms();
    }

    fn send_participant_changes(&mut self, room: u8, participants: &[Participant]) {
        let left = self
            .participants
            .iter()
            .filter(|current| {
                !participants
                    .iter()
                    .any(|next| next.client_id == current.client_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        let joined = participants
            .iter()
            .filter(|next| {
                !self
                    .participants
                    .iter()
                    .any(|current| current.client_id == next.client_id)
            })
            .cloned()
            .collect::<Vec<_>>();

        for participant in left {
            self.send_to(ServerWireEvent::ParticipantLeft { room, participant });
        }
        for participant in joined {
            self.send_to(ServerWireEvent::ParticipantJoined { room, participant });
        }
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

    fn room_summaries(&self) -> Vec<RoomSummary> {
        let mut rooms = empty_room_summaries();
        for (room, count) in rooms.iter_mut().zip(self.passive_counts) {
            room.participant_count = count;
        }
        if let Some((_, client)) = &self.client {
            let room = &mut rooms[usize::from(client.room)];
            room.participants.clone_from(&self.participants);
            room.participant_count = room.participant_count.max(room.participants.len());
        }
        rooms
    }

    fn publish_rooms(&self) {
        publish_rooms_if_changed(&self.room_updates, self.room_summaries());
    }
}

fn station_id(mac: picto_relay_codec::MacAddress) -> u32 {
    let octets = mac.octets();
    u32::from_be_bytes([octets[2], octets[3], octets[4], octets[5]])
}

#[cfg(test)]
mod tests {
    use picto_relay_codec::MacAddress;
    use picto_relay_join::{LiveConfig, LiveEvent, LiveParticipant};

    use super::{
        BITMAP_BYTES, BrokerHandle, CLIENT_EVENT_CAPACITY, ClientId, ConnectedClient, LiveBroker,
    };
    use crate::protocol::ServerWireEvent;

    #[tokio::test]
    async fn broker_should_deliver_message_to_two_virtual_clients() {
        let broker = BrokerHandle::spawn_memory();
        let mut alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");
        let mut bob = broker
            .register("Bob".into(), 0)
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

    #[tokio::test]
    async fn rooms_should_group_clients_by_selected_room() {
        let broker = BrokerHandle::spawn_memory();
        let _alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");
        let _bob = broker
            .register("Bob".into(), 2)
            .await
            .expect("Bob should register");

        let rooms = broker.rooms().await.expect("rooms should be available");

        assert_eq!(rooms[0].participants[0].name, "Alice");
        assert_eq!(rooms[2].participants[0].name, "Bob");
    }

    #[tokio::test]
    async fn rooms_should_count_memory_clients() {
        let broker = BrokerHandle::spawn_memory();
        let _alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");

        let rooms = broker.rooms().await.expect("rooms should be available");

        assert_eq!(rooms[0].participant_count, 1);
    }

    #[tokio::test]
    async fn room_subscription_should_publish_complete_snapshots() {
        let broker = BrokerHandle::spawn_memory();
        let mut rooms = broker.subscribe_rooms();

        let _alice = broker
            .register("Alice".into(), 1)
            .await
            .expect("Alice should register");
        rooms
            .changed()
            .await
            .expect("broker should remain available");

        assert_eq!(rooms.borrow()[1].participants[0].name, "Alice");
    }

    #[tokio::test]
    async fn register_should_publish_join_event_to_room_members() {
        let broker = BrokerHandle::spawn_memory();
        let mut alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");
        drain(&mut alice.events);

        let _bob = broker
            .register("Bob".into(), 0)
            .await
            .expect("Bob should register");
        let event = alice.events.recv().await.expect("join should be published");

        assert!(matches!(
            event.event,
            ServerWireEvent::ParticipantJoined { participant, .. }
                if participant.name == "Bob"
        ));
    }

    #[tokio::test]
    async fn unregister_should_publish_leave_event_to_room_members() {
        let broker = BrokerHandle::spawn_memory();
        let mut alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");
        let bob = broker
            .register("Bob".into(), 0)
            .await
            .expect("Bob should register");
        drain(&mut alice.events);

        broker.unregister(bob.client_id);
        let event = alice
            .events
            .recv()
            .await
            .expect("leave should be published");

        assert!(matches!(
            event.event,
            ServerWireEvent::ParticipantLeft { participant, .. }
                if participant.name == "Bob"
        ));
    }

    #[tokio::test]
    async fn broker_should_not_deliver_messages_across_rooms() {
        let broker = BrokerHandle::spawn_memory();
        let mut alice = broker
            .register("Alice".into(), 0)
            .await
            .expect("Alice should register");
        let mut bob = broker
            .register("Bob".into(), 1)
            .await
            .expect("Bob should register");
        drain(&mut alice.events);
        drain(&mut bob.events);

        broker
            .send_message(alice.client_id, vec![0x55; BITMAP_BYTES])
            .expect("bitmap should enqueue");
        let _alice_message = receive_message(&mut alice.events).await;

        assert!(bob.events.try_recv().is_err());
    }

    #[test]
    fn live_broker_should_publish_physical_join_names() {
        let (_commands, receiver) = tokio::sync::mpsc::channel(1);
        let (room_updates, _rooms) = tokio::sync::watch::channel(super::empty_room_summaries());
        let mut broker = LiveBroker::new(LiveConfig::default(), receiver, room_updates);
        let (events, mut event_receiver) = tokio::sync::mpsc::channel(CLIENT_EVENT_CAPACITY);
        broker.client = Some((
            ClientId(1),
            ConnectedClient {
                name: "Relay".into(),
                room: 0,
                events,
                next_message_id: 0,
            },
        ));
        broker.participants = vec![crate::protocol::Participant {
            client_id: super::station_id(LiveConfig::default().station),
            name: "Relay".into(),
        }];
        broker.roster_initialized = true;

        broker.handle_hardware_event(LiveEvent::Participants {
            room: 0,
            participants: vec![
                LiveParticipant {
                    station: LiveConfig::default().station,
                    name: "Relay".into(),
                },
                LiveParticipant {
                    station: MacAddress([0, 0x25, 0xa0, 0xdc, 0x81, 0xdf]),
                    name: "Rob".into(),
                },
            ],
        });

        let event = event_receiver
            .try_recv()
            .expect("physical join event should be published");
        assert!(matches!(
            event.event,
            ServerWireEvent::ParticipantJoined { participant, .. }
                if participant.name == "Rob"
        ));
    }

    #[test]
    fn live_broker_should_publish_physical_leave_names() {
        let (_commands, receiver) = tokio::sync::mpsc::channel(1);
        let (room_updates, _rooms) = tokio::sync::watch::channel(super::empty_room_summaries());
        let mut broker = LiveBroker::new(LiveConfig::default(), receiver, room_updates);
        let (events, mut event_receiver) = tokio::sync::mpsc::channel(CLIENT_EVENT_CAPACITY);
        broker.client = Some((
            ClientId(1),
            ConnectedClient {
                name: "Relay".into(),
                room: 0,
                events,
                next_message_id: 0,
            },
        ));
        broker.participants = vec![crate::protocol::Participant {
            client_id: 42,
            name: "Rob".into(),
        }];
        broker.roster_initialized = true;

        broker.handle_hardware_event(LiveEvent::Participants {
            room: 0,
            participants: Vec::new(),
        });

        let event = event_receiver
            .try_recv()
            .expect("physical leave event should be published");
        assert!(matches!(
            event.event,
            ServerWireEvent::ParticipantLeft { participant, .. }
                if participant.name == "Rob"
        ));
    }

    #[test]
    fn live_broker_should_publish_passive_room_counts_without_a_session() {
        let (_commands, receiver) = tokio::sync::mpsc::channel(1);
        let (room_updates, _rooms) = tokio::sync::watch::channel(super::empty_room_summaries());
        let mut broker = LiveBroker::new(LiveConfig::default(), receiver, room_updates);

        broker.handle_hardware_event(LiveEvent::RoomCounts {
            counts: [1, 0, 2, 0],
        });

        assert_eq!(broker.room_summaries()[2].participant_count, 2);
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
