//! Blocking serial worker isolated from the async HTTP broker.
use crate::bitmap::{api_bitmap_to_native, native_message_to_api_bitmap};
use crate::broker::station_id;
use crate::worker::{LiveCommand, LiveEvent, LiveParticipant};
use anyhow::{Context, Result, bail};
use picto_relay_codec::{
    MAC_ADDRESS_LEN, MacAddress, MessageReassembler, NintendoFlow, PictoChatEnvelope,
    PictoChatFragment, PictoChatMessage, PictoChatProfile, PictoChatRoster, ReassemblyKey,
    encode_fragment_envelope, encode_message, fragment_payload_starting_at,
};
use picto_relay_link as link;
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{File, OpenOptions},
    io::{Read, Write},
    process::Command,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Duration, Instant},
};

// ---- Serial port -----------------------------------------------------------

/// `stty` settings: raw 8-bit bytes, no echo, ignore modem control lines, keep
/// DTR asserted on close (dropping it resets the ESP32), and return from a read
/// after 0.1 s even when no byte arrived.
const STTY_SETTINGS: [&str; 8] = ["raw", "-echo", "clocal", "-hupcl", "min", "0", "time", "1"];
/// Bytes read from the serial port at a time.
const READ_CHUNK_BYTES: usize = 512;
/// Undelivered firmware packets tolerated before the session is reset.
const MAX_PENDING_EVENTS: usize = 256;
/// Request IDs start here and increase for the life of a connection.
const FIRST_REQUEST_ID: u32 = 1;
/// How long a command may go unanswered before the session is reset.
const REQUEST_DEADLINE: Duration = Duration::from_secs(3);
/// How often an unanswered command is resent.
const REQUEST_RETRY_INTERVAL: Duration = Duration::from_millis(150);

// ---- Room discovery --------------------------------------------------------

/// Member counts for rooms A through D, [`link::NO_BEACON`] when unknown.
type RoomCounts = [u8; link::ROOM_COUNT];
/// Time spent listening for beacons on each channel.
const CHANNEL_DWELL: Duration = Duration::from_millis(250);
/// Channels a DS host normally picks, scanned before the rest.
const USUAL_HOST_CHANNELS: [u8; 3] = [1, 7, 13];
/// Lowest 2.4 GHz channel scanned.
const FIRST_CHANNEL: u8 = 1;
/// Highest 2.4 GHz channel scanned.
const LAST_CHANNEL: u8 = 13;

// ---- Session ---------------------------------------------------------------

/// Pause before reopening the device after a failed session.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// How often the firmware is told the host is still alive.
const PING_INTERVAL: Duration = Duration::from_secs(1);
/// Longest the DS profile exchange may take before rejoining.
const PROFILE_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest gap between firmware status reports before reconnecting.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest a drawing may take from upload to DS delivery.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(40);
/// Drawings that may wait behind the one being sent.
const MAX_QUEUED_DRAWINGS: usize = 4;
/// Recently completed incoming drawings remembered to drop host repeats.
const MAX_COMPLETED_DRAWINGS: usize = 128;
/// Fragment sizes a DS host may use when rebroadcasting drawings, in bytes.
const HOST_FRAGMENT_SIZES: [usize; 3] = [80, 120, 160];
/// Colors in the `PictoChat` palette.
const PALETTE_COLORS: usize = 16;
/// Upload progress is reported in this many steps.
const UPLOAD_PROGRESS_STEPS: usize = 10;
/// Scale of a percentage.
const PERCENT: usize = 100;

// ---- PictoChat application packets -----------------------------------------

/// Mask removing the flag bit whose meaning is not established from a packet ID.
const PACKET_ID_MASK: u16 = 0x7fff;
/// Packet ID of the host's participant roster.
const ROSTER_PACKET_ID: u16 = 0x1c34;
/// Packet ID of a client transfer announcement or completion (`d0a`).
const CLIENT_CONTROL_PACKET_ID: u16 = 0x8d0a;
/// Declared length of a client `d0a` envelope: its 18-byte payload plus two.
const CONTROL_DECLARED_LENGTH: u16 = 20;
/// Envelope sequence of a transfer announcement.
const ANNOUNCE_PACKET_SEQUENCE: u16 = 0;
/// Envelope sequence of a transfer completion.
const COMPLETION_PACKET_SEQUENCE: u16 = 3;
/// Byte after the slot in an announcement; observed constant.
const ANNOUNCE_MARKER: u8 = 0;
/// Byte after the slot in a completion; observed constant.
const COMPLETION_MARKER: u8 = 4;
/// Two bytes after the marker in both controls; observed constant.
const CONTROL_FILLER: u16 = 0xffff;
/// Command counter of a session's first announcement.
const FIRST_COMMAND_COUNTER: u32 = 12;
/// Command counter increase between drawings, as captured from a real DS.
const COMMAND_COUNTER_STEP: u32 = 3;
/// Per-transfer nonce of the first drawing. The DS ignores a repeated value,
/// so each drawing must differ; how a real DS generates it is unknown.
const NONCE_SEED: u32 = 0x75f0_3cca;
/// Multiplied by the drawing number and combined with the seed by XOR to vary the nonce.
const NONCE_MULTIPLIER: u32 = 0x9e37_79b9;
/// Transfer counter of a session's first announcement. A completion uses its
/// announcement's value plus one.
const FIRST_TRANSFER_COUNTER: u16 = 5;
/// Transfer counter increase between drawings.
const TRANSFER_COUNTER_STEP: u16 = 2;
/// Number of u32 fields in a `d0a` control payload.
const CONTROL_FIELDS: usize = 3;
/// The u32 fields of a completion, replayed from a capture; meaning unknown.
const COMPLETION_OPAQUE_FIELDS: [u32; CONTROL_FIELDS] = [0x0232_93dd, 0xffff_ff01, 0x0236_c8bb];
/// Opaque transfer tag carried by every fragment; a real DS client sends zeros.
const TRANSFER_TAG: [u8; 2] = [0, 0];
/// Frame number of a session's first drawing fragment, after the profile exchange.
const FIRST_FRAGMENT_NUMBER: u16 = 3;

struct Port {
    file: File,
    decoder: link::Decoder,
    next_id: u32,
    events: VecDeque<link::Packet>,
}
impl Port {
    fn open(path: &str) -> Result<Self> {
        let baud = link::BAUD.to_string();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("open {path}"))?;
        // Hold the port open while configuring: macOS restores terminal settings
        // when the last descriptor closes.
        // stty avoids a second platform-specific unsafe serial implementation.
        let flag = if cfg!(target_os = "macos") {
            "-f"
        } else {
            "-F"
        };
        let status = Command::new("stty")
            .args([flag, path, &baud])
            .args(STTY_SETTINGS)
            .status()
            .context("failed to run stty")?;
        if !status.success() {
            bail!("could not configure USB serial port {path}");
        }
        tracing::info!(baud, "ESP32 serial configured");
        Ok(Self {
            file,
            decoder: link::Decoder::new(),
            next_id: FIRST_REQUEST_ID,
            events: VecDeque::new(),
        })
    }
    fn read(&mut self) -> Result<()> {
        let mut bytes = [0; READ_CHUNK_BYTES];
        let n = self.file.read(&mut bytes)?;
        for &b in &bytes[..n] {
            if let Some(p) = self.decoder.push(b) {
                if self.events.len() >= MAX_PENDING_EVENTS {
                    bail!("USB event backlog exceeded limit");
                }
                self.events.push_back(p);
            }
        }
        Ok(())
    }
    fn request(&mut self, kind: u8, data: &[u8]) -> Result<Vec<u8>> {
        let id = self.next_id;
        tracing::debug!(kind, id, "USB request");
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("USB command ID exhausted")?;
        let p = link::Packet::new(kind, id, data).context("USB command too large")?;
        let mut bytes = [0; link::MAX_ENCODED];
        let n = link::encode(&p, &mut bytes);
        let deadline = Instant::now() + REQUEST_DEADLINE;
        let mut next_send = Instant::now();
        let mut attempts = 0;
        while Instant::now() < deadline {
            if Instant::now() >= next_send {
                self.file.write_all(&bytes[..n])?;
                attempts += 1;
                next_send = Instant::now() + REQUEST_RETRY_INTERVAL;
            }
            self.read()?;
            if let Some(index) = self
                .events
                .iter()
                .position(|p| p.kind == link::RESPONSE && p.id == id)
            {
                let reply = self.events.remove(index).context("missing USB response")?;
                if let Some((&link::RESPONSE_OK, data)) = reply.data().split_first() {
                    tracing::debug!(kind, id, attempts, "USB accepted");
                    return Ok(data.to_vec());
                }
            }
        }
        bail!("ESP32 command {kind} timed out or remained busy")
    }
}

/// Poll passive discovery only when no session worker owns the serial port.
pub(crate) fn discover(path: &str, channel: u8) -> Result<RoomCounts> {
    let mut port = Port::open(path)?;
    port.request(link::HELLO, &[])?;
    port.request(link::LEAVE, &[])?;
    Ok(scan_channels(channel, None, |channel| observe_channel(&mut port, channel))?.0)
}

fn observe_channel(port: &mut Port, channel: u8) -> Result<RoomCounts> {
    // Tuning clears firmware observations. Allow multiple beacons before reading.
    port.request(link::DISCOVER, &[channel])?;
    std::thread::sleep(CHANNEL_DWELL);
    port.request(link::DISCOVER, &[channel])?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid discovery response"))
}

fn scan_channels(
    preferred: u8,
    target: Option<u8>,
    mut observe: impl FnMut(u8) -> Result<RoomCounts>,
) -> Result<(RoomCounts, Option<u8>)> {
    let mut counts = [link::NO_BEACON; link::ROOM_COUNT];
    let mut channels = vec![preferred];
    for channel in USUAL_HOST_CHANNELS
        .into_iter()
        .chain(FIRST_CHANNEL..=LAST_CHANNEL)
    {
        if !channels.contains(&channel) {
            channels.push(channel);
        }
    }
    for channel in channels {
        let observed = observe(channel)?;
        for (room, &count) in observed.iter().enumerate() {
            if count != link::NO_BEACON {
                counts[room] = count;
                tracing::debug!(room, channel, count, "room beacon found");
            }
        }
        if target.is_some_and(|room| observed[usize::from(room)] != link::NO_BEACON) {
            return Ok((counts, Some(channel)));
        }
    }
    Ok((counts, None))
}

/// Runs one persistent API session, reconnecting the device until shutdown.
/// # Errors
/// Returns on a command-channel failure during shutdown.
pub fn run(
    port: &str,
    channel: u8,
    room: u8,
    name: &str,
    profile: &crate::worker::ProfileOptions,
    commands: &Receiver<LiveCommand>,
    emit: &impl Fn(LiveEvent),
) -> Result<()> {
    loop {
        emit(LiveEvent::State {
            state: "connecting",
            detail: format!("opening ESP32 on {port}"),
        });
        match connected(port, channel, room, name, profile, commands, emit) {
            Ok(()) => {
                emit(LiveEvent::Left);
                return Ok(());
            }
            Err(error) => {
                tracing::warn!(%error, "ESP32 session interrupted");
                emit(LiveEvent::State {
                    state: "reconnecting",
                    detail: error.to_string(),
                });
            }
        }
        match commands.recv_timeout(RECONNECT_DELAY) {
            Ok(LiveCommand::Shutdown) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                emit(LiveEvent::Left);
                return Ok(());
            }
            Ok(LiveCommand::SendMessage { message_id, .. }) => {
                emit(LiveEvent::DeliveryTimedOut { message_id });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}
struct Active {
    id: u32,
    bitmap: Vec<u8>,
    deadline: Instant,
    started: Instant,
    progress: Option<(u8, u32, usize)>,
}
struct Session<'a> {
    station: MacAddress,
    host: MacAddress,
    room: u8,
    slot: u8,
    name: &'a str,
    names: BTreeMap<MacAddress, String>,
    profiles: BTreeMap<MacAddress, PictoChatProfile>,
    members: Vec<MacAddress>,
    assemblers: [MessageReassembler; HOST_FRAGMENT_SIZES.len()],
    completed: VecDeque<(MacAddress, u16, [u8; TRANSFER_TAG.len()])>,
    ready: bool,
    active: Option<Active>,
    queue: VecDeque<(u32, Vec<u8>)>,
    sequence: u16,
    next_fragment: u16,
}
impl Session<'_> {
    fn participants(&self, emit: &impl Fn(LiveEvent)) {
        emit(LiveEvent::Participants {
            room: self.room,
            participants: self
                .members
                .iter()
                .map(|mac| LiveParticipant {
                    station: *mac,
                    name: self
                        .names
                        .get(mac)
                        .cloned()
                        .unwrap_or_else(|| mac.to_string()),
                })
                .collect(),
        });
    }
    fn receive(&mut self, body: &[u8], emit: &impl Fn(LiveEvent)) {
        let Ok(env) = PictoChatEnvelope::parse(NintendoFlow::HostToClient, body) else {
            return;
        };
        if env.packet_id & PACKET_ID_MASK == ROSTER_PACKET_ID {
            if let Ok(roster) = PictoChatRoster::parse(&env.payload) {
                let members: Vec<_> = roster.members.into_iter().flatten().collect();
                if members != self.members {
                    self.members = members;
                    self.participants(emit);
                }
            }
            return;
        }
        if !env.is_message_fragment() {
            return;
        }
        let Ok(fragment) = PictoChatFragment::parse(&env.payload, NintendoFlow::HostToClient)
        else {
            return;
        };
        if let Ok(profile) = PictoChatProfile::parse(&fragment.data) {
            tracing::debug!(station = %profile.station, nickname = %profile.nickname, profile_message = %profile.message, profile_color = profile.color, "DS profile received");
            let changed = self.profiles.get(&profile.station).is_none_or(|old| {
                old.nickname != profile.nickname
                    || old.message != profile.message
                    || old.color != profile.color
            });
            if changed {
                self.profiles.insert(profile.station, profile.clone());
                emit(LiveEvent::Profile(profile.clone()));
            }
            if self.names.get(&profile.station) != Some(&profile.nickname) {
                self.names.insert(profile.station, profile.nickname);
                self.participants(emit);
            }
            return;
        }
        for assembler in &mut self.assemblers {
            let key = ReassemblyKey {
                source: self.host,
                opaque_id: fragment.opaque_id,
            };
            let Ok(Some(bytes)) = assembler.push(key, &fragment) else {
                continue;
            };
            let Ok(message) = PictoChatMessage::parse(&bytes) else {
                continue;
            };
            let sender = message.sender();
            let key = (sender, fragment.frame_number, fragment.opaque_id);
            if self.completed.contains(&key) {
                continue;
            }
            self.completed.push_back(key);
            if self.completed.len() > MAX_COMPLETED_DRAWINGS {
                self.completed.pop_front();
            }
            // The device's exact echo tracking publishes our own message once.
            if sender == self.station {
                continue;
            }
            let mut palette_counts = [0usize; PALETTE_COLORS];
            for &pixel in message.pixels() {
                palette_counts[usize::from(pixel)] += 1;
            }
            tracing::debug!(%sender, ?palette_counts, "DS drawing received");
            emit(LiveEvent::Message {
                station_id: station_id(sender),
                name: self
                    .names
                    .get(&sender)
                    .cloned()
                    .unwrap_or_else(|| sender.to_string()),
                message_id: u32::from(fragment.frame_number),
                bitmap: native_message_to_api_bitmap(&message),
            });
        }
    }
    fn progress(&mut self, p: &link::Packet, emit: &impl Fn(LiveEvent)) {
        let Some(a) = self.active.as_mut().filter(|a| a.id == p.id) else {
            return;
        };
        let echoed = status_counter(p, link::STATUS_ECHOED_COUNTER);
        let bodies_sent = p.payload[link::STATUS_SENT_OFFSET];
        let current = (bodies_sent, echoed, self.queue.len());
        if a.progress == Some(current) {
            return;
        }
        a.progress = Some(current);
        // Fragments exclude the announcement (sent first) and the completion.
        let total = p.payload[link::STATUS_COUNT_OFFSET].saturating_sub(link::CONTROL_BODIES);
        let sent = bodies_sent.saturating_sub(1).min(total);
        emit(LiveEvent::State {
            state: "sending",
            detail: format!(
                "Drawing {}: {sent}/{total} fragments sent, {echoed} confirmed · {} waiting",
                a.id,
                self.queue.len()
            ),
        });
    }
    fn packet(&mut self, p: &link::Packet, emit: &impl Fn(LiveEvent)) -> Result<()> {
        match p.kind {
            link::RX => self.receive(p.data(), emit),
            link::GAP => {
                tracing::warn!("ESP32 USB receive gap; discarding incomplete incoming drawings");
                self.assemblers = host_reassemblers();
                emit(LiveEvent::Error {
                    code: "usb_rx_gap",
                    message:
                        "USB receive queue overflowed; incomplete incoming drawings were discarded"
                            .into(),
                });
            }
            link::DIAG => {
                let counters: Vec<u32> = p
                    .data()
                    .chunks_exact(size_of::<u32>())
                    .filter_map(|counter| counter.try_into().ok())
                    .map(u32::from_le_bytes)
                    .collect();
                tracing::debug!(?counters, "ESP32 radio diagnostics");
            }
            link::STATUS if p.len == link::STATUS_LEN => {
                tracing::debug!(transfer = p.id, status = ?p.data(), "ESP32 status");
                let state = p.payload[link::STATUS_STATE_OFFSET];
                self.slot = p.payload[link::STATUS_SLOT_OFFSET];
                self.host = MacAddress(
                    p.payload[link::STATUS_HOST_OFFSET..link::STATUS_HOST_OFFSET + MAC_ADDRESS_LEN]
                        .try_into()?,
                );
                let ready = state == link::STATE_CONNECTED
                    && p.payload[link::STATUS_PHASE_OFFSET] == link::PHASE_READY;
                if ready && !self.ready {
                    emit(LiveEvent::Ready {
                        host: self.host,
                        room: self.room,
                        slot: self.slot,
                    });
                }
                self.ready = ready;
                self.progress(p, emit);
                let delivered = p.payload[link::STATUS_DELIVERED_OFFSET] != 0;
                if delivered && self.active.as_ref().is_some_and(|a| a.id == p.id) {
                    let a = self.active.take().context("missing active message")?;
                    tracing::info!(
                        message_id = a.id,
                        elapsed_ms = a.started.elapsed().as_millis(),
                        "ESP32 drawing delivered"
                    );
                    emit(LiveEvent::Message {
                        station_id: station_id(self.station),
                        name: self.name.into(),
                        message_id: a.id,
                        bitmap: a.bitmap,
                    });
                    emit(LiveEvent::Delivered { message_id: a.id });
                }
                if state == link::STATE_OFF {
                    bail!("ESP32 left the room; rejoining");
                }
            }
            _ => {}
        }
        Ok(())
    }
}
/// One reassembler per fragment size a DS host may use.
fn host_reassemblers() -> [MessageReassembler; HOST_FRAGMENT_SIZES.len()] {
    HOST_FRAGMENT_SIZES.map(MessageReassembler::new)
}

/// Reads one of the little-endian u32 counters at the end of a STATUS payload.
fn status_counter(status: &link::Packet, index: usize) -> u32 {
    let offset = link::STATUS_COUNTERS_OFFSET + index * size_of::<u32>();
    status
        .data()
        .get(offset..offset + size_of::<u32>())
        .and_then(|counter| counter.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

/// Appends `text` as UTF-16LE, stopping before a character that would not fit
/// in `max_bytes`, then zero-pads the field to exactly `max_bytes`.
fn push_utf16_field(output: &mut Vec<u8>, text: &str, max_bytes: usize) {
    let end = output.len() + max_bytes;
    for unit in text.encode_utf16() {
        let bytes = unit.to_le_bytes();
        if output.len() + bytes.len() > end {
            break;
        }
        output.extend_from_slice(&bytes);
    }
    output.resize(end, 0);
}

fn connected(
    path: &str,
    channel: u8,
    room: u8,
    name: &str,
    profile: &crate::worker::ProfileOptions,
    commands: &Receiver<LiveCommand>,
    emit: &impl Fn(LiveEvent),
) -> Result<()> {
    let mut port = Port::open(path)?;
    let mac = port.request(link::HELLO, &[])?;
    let station = MacAddress(
        mac.get(..MAC_ADDRESS_LEN)
            .context("invalid ESP32 hello")?
            .try_into()?,
    );
    emit(LiveEvent::Station { station });
    // Release any previous owner (e.g. a server process that restarted).
    port.request(link::LEAVE, &[])?;
    emit(LiveEvent::State {
        state: "connecting",
        detail: format!(
            "searching Wi-Fi channels for Room {}",
            char::from(b'A' + room)
        ),
    });
    let channel = scan_channels(channel, Some(room), |channel| {
        observe_channel(&mut port, channel)
    })?
    .1
    .context("No host beacon found for the selected room; keep a DS in that room nearby")?;
    tracing::info!(room, channel, "joining discovered room");
    if profile.message.encode_utf16().count() * size_of::<u16>() > link::PROFILE_MESSAGE_LEN
        || profile.message.contains('\0')
        || profile.color > link::MAX_PROFILE_COLOR
    {
        bail!("invalid profile message or color");
    }
    // Field order follows the link::JOIN_*_OFFSET layout.
    let mut join = vec![room, channel];
    push_utf16_field(&mut join, name, link::NICKNAME_LEN);
    push_utf16_field(&mut join, &profile.message, link::PROFILE_MESSAGE_LEN);
    join.push(profile.color);
    port.request(link::JOIN, &join)?;
    port.events.clear();
    let mut session = Session {
        station,
        host: MacAddress([0; MAC_ADDRESS_LEN]),
        room,
        slot: 0,
        name,
        names: BTreeMap::from([(station, name.into())]),
        profiles: BTreeMap::new(),
        members: vec![station],
        assemblers: host_reassemblers(),
        completed: VecDeque::new(),
        ready: false,
        active: None,
        queue: VecDeque::new(),
        sequence: 0,
        next_fragment: FIRST_FRAGMENT_NUMBER,
    };
    session.participants(emit);
    let result = worker(&mut port, &mut session, commands, emit);
    if let Some(a) = session.active.take() {
        emit(LiveEvent::DeliveryTimedOut { message_id: a.id });
    }
    for (message_id, _) in session.queue {
        emit(LiveEvent::DeliveryTimedOut { message_id });
    }
    result
}
fn worker(
    port: &mut Port,
    session: &mut Session<'_>,
    commands: &Receiver<LiveCommand>,
    emit: &impl Fn(LiveEvent),
) -> Result<()> {
    let mut ping = Instant::now();
    let mut last_status = Instant::now();
    let joined_at = Instant::now();
    loop {
        loop {
            match commands.try_recv() {
                Ok(LiveCommand::Shutdown) | Err(TryRecvError::Disconnected) => {
                    let _ = port.request(link::LEAVE, &[]);
                    return Ok(());
                }
                Ok(LiveCommand::SendMessage { message_id, bitmap }) => {
                    if session.queue.len() < MAX_QUEUED_DRAWINGS {
                        session.queue.push_back((message_id, bitmap));
                    } else {
                        emit(LiveEvent::Error {
                            code: "queue_full",
                            message: "ESP32 message queue is full".into(),
                        });
                        emit(LiveEvent::DeliveryTimedOut { message_id });
                    }
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if Instant::now() >= ping {
            port.request(link::PING, &[])?;
            ping = Instant::now() + PING_INTERVAL;
        }
        port.read()?;
        while let Some(packet) = port.events.pop_front() {
            if packet.kind == link::STATUS {
                last_status = Instant::now();
            }
            session.packet(&packet, emit)?;
        }
        if !session.ready && joined_at.elapsed() > PROFILE_EXCHANGE_TIMEOUT {
            bail!("DS profile exchange stalled; rejoining");
        }
        if last_status.elapsed() > STATUS_TIMEOUT {
            bail!("ESP32 status stream stopped");
        }
        if session
            .active
            .as_ref()
            .is_some_and(|a| Instant::now() >= a.deadline)
        {
            bail!("drawing delivery timed out; resetting the physical session");
        }
        if session.ready
            && session.active.is_none()
            && let Some((id, bitmap)) = session.queue.pop_front()
        {
            upload_message(port, session, id, bitmap, emit)?;
        }
    }
}
fn upload_message(
    port: &mut Port,
    session: &mut Session<'_>,
    id: u32,
    bitmap: Vec<u8>,
    emit: &impl Fn(LiveEvent),
) -> Result<()> {
    let started = Instant::now();
    emit(LiveEvent::State {
        state: "uploading",
        detail: format!("Drawing {id}: preparing · {} waiting", session.queue.len()),
    });
    let bodies = match message_bodies(
        session.station,
        session.slot,
        session.sequence,
        session.next_fragment,
        &bitmap,
    ) {
        Ok(bodies) => bodies,
        Err(error) => {
            emit(LiveEvent::Error {
                code: "invalid_drawing",
                message: error.to_string(),
            });
            emit(LiveEvent::DeliveryTimedOut { message_id: id });
            return Ok(());
        }
    };
    session.sequence = session.sequence.wrapping_add(1);
    session.next_fragment = session
        .next_fragment
        .wrapping_add(u16::try_from(bodies.len())? - u16::from(link::CONTROL_BODIES));
    session.active = Some(Active {
        id,
        bitmap,
        deadline: Instant::now() + DELIVERY_TIMEOUT,
        started: Instant::now(),
        progress: None,
    });
    let mut begin = id.to_le_bytes().to_vec();
    begin.push(u8::try_from(bodies.len())?);
    port.request(link::BEGIN, &begin)?;
    let uploads = upload_commands(id, &bodies)?;
    let mut last_step = usize::MAX;
    for upload in &uploads {
        port.request(link::BODIES, &upload.data)?;
        let step = upload.completed * UPLOAD_PROGRESS_STEPS / bodies.len();
        if step != last_step {
            emit(LiveEvent::State {
                state: "uploading",
                detail: format!(
                    "Drawing {id}: {}% uploaded · {} waiting",
                    upload.completed * PERCENT / bodies.len(),
                    session.queue.len()
                ),
            });
            last_step = step;
        }
    }
    port.request(link::COMMIT, &id.to_le_bytes())?;
    tracing::info!(
        message_id = id,
        upload_ms = started.elapsed().as_millis(),
        bodies = bodies.len(),
        commands = uploads.len(),
        "ESP32 upload complete"
    );
    Ok(())
}
struct UploadCommand {
    data: Vec<u8>,
    completed: usize,
}
/// Packs consecutive bodies into as few BODIES commands as fit the payload limit.
fn upload_commands(id: u32, bodies: &[Vec<u8>]) -> Result<Vec<UploadCommand>> {
    let mut commands = Vec::new();
    let mut i = 0;
    while i < bodies.len() {
        let mut data = id.to_le_bytes().to_vec();
        data.push(u8::try_from(i)?);
        let mut record = [0; link::MAX_RECORD];
        while i < bodies.len() {
            let n = link::pack_body(&bodies[i], &mut record).context("invalid application body")?;
            if data.len() + n > link::MAX_PAYLOAD {
                break;
            }
            data.extend_from_slice(&record[..n]);
            i += 1;
        }
        commands.push(UploadCommand { data, completed: i });
    }
    Ok(commands)
}

fn message_bodies(
    station: MacAddress,
    slot: u8,
    sequence: u16,
    first_fragment: u16,
    bitmap: &[u8],
) -> Result<Vec<Vec<u8>>> {
    let native = encode_message(station, &api_bitmap_to_native(bitmap)?)?;
    let drawing = u32::from(sequence);
    let transfer_counter =
        FIRST_TRANSFER_COUNTER.wrapping_add(sequence.wrapping_mul(TRANSFER_COUNTER_STEP));

    let announce = control_body(
        ANNOUNCE_PACKET_SEQUENCE,
        slot,
        ANNOUNCE_MARKER,
        [
            u32::try_from(native.len())?,
            FIRST_COMMAND_COUNTER + drawing * COMMAND_COUNTER_STEP,
            NONCE_SEED ^ drawing.wrapping_mul(NONCE_MULTIPLIER),
        ],
        transfer_counter,
    );
    let mut bodies = vec![announce];
    let first_frame_number = 0;
    for mut fragment in fragment_payload_starting_at(
        &native,
        TRANSFER_TAG,
        link::FRAGMENT_BYTES,
        first_frame_number,
    )? {
        // The DS remembers fragment counters across drawings in one session.
        fragment.frame_number = first_fragment.wrapping_add(fragment.frame_number);
        bodies.push(encode_fragment_envelope(
            NintendoFlow::ClientToHost,
            &fragment,
        )?);
    }
    bodies.push(control_body(
        COMPLETION_PACKET_SEQUENCE,
        slot,
        COMPLETION_MARKER,
        COMPLETION_OPAQUE_FIELDS,
        transfer_counter.wrapping_add(1),
    ));
    Ok(bodies)
}

/// Builds a client transfer-control (`d0a`) body: the envelope header, then
/// slot, marker, filler, three u32 fields and the transfer counter. For an
/// announcement the fields are drawing length, command counter and nonce.
fn control_body(
    packet_sequence: u16,
    slot: u8,
    marker: u8,
    fields: [u32; CONTROL_FIELDS],
    transfer_counter: u16,
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&CLIENT_CONTROL_PACKET_ID.to_le_bytes());
    body.extend_from_slice(&packet_sequence.to_le_bytes());
    body.extend_from_slice(&CONTROL_DECLARED_LENGTH.to_le_bytes());
    body.extend_from_slice(&[slot, marker]);
    body.extend_from_slice(&CONTROL_FILLER.to_le_bytes());
    for field in fields {
        body.extend_from_slice(&field.to_le_bytes());
    }
    body.extend_from_slice(&transfer_counter.to_le_bytes());
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn channel_scan_finds_target_off_preferred_channel() {
        let mut visited = Vec::new();
        let (counts, channel) = scan_channels(1, Some(2), |channel| {
            visited.push(channel);
            Ok(match channel {
                1 => [1, 255, 255, 255],
                7 => [255, 255, 2, 255],
                _ => [255; 4],
            })
        })
        .unwrap();
        assert_eq!(channel, Some(7));
        assert_eq!(counts, [1, 255, 2, 255]);
        assert_eq!(visited, [1, 7]);
    }

    #[test]
    fn lobby_scan_merges_channels_and_keeps_missing_rooms_unknown() {
        let mut visited = Vec::new();
        let (counts, channel) = scan_channels(7, None, |channel| {
            assert!(!visited.contains(&channel));
            visited.push(channel);
            Ok(match channel {
                1 => [1, 255, 255, 255],
                13 => [255, 2, 255, 3],
                _ => [255; 4],
            })
        })
        .unwrap();
        assert_eq!(counts, [1, 2, 255, 3]);
        assert_eq!(channel, None);
        assert_eq!(visited.len(), 13);
        assert_eq!(visited[0], 7);
    }

    #[test]
    fn packed_uploads_preserve_bodies_and_batch_sparse_full_height_traffic() {
        let mut bitmap = vec![0; 3072];
        bitmap[32 * 79 + 30] = 0xa5;
        let bodies = message_bodies(MacAddress([2, 1, 2, 3, 4, 5]), 1, 0, 3, &bitmap).unwrap();
        let packed = upload_commands(42, &bodies).unwrap();
        assert!(packed.len() < bodies.len() / 4);
        let mut actual = Vec::new();
        for p in &packed {
            assert!(p.data.len() <= link::MAX_PAYLOAD);
            assert_eq!(usize::from(p.data[4]), actual.len());
            let mut bytes = &p.data[5..];
            while !bytes.is_empty() {
                let n = link::packed_record_len(bytes).unwrap();
                let mut body = [0; link::MAX_BODY];
                let len = link::unpack_body(&bytes[..n], &mut body).unwrap();
                actual.push(body[..len].to_vec());
                bytes = &bytes[n..];
            }
            assert_eq!(p.completed, actual.len());
        }
        assert_eq!(actual, bodies);
        let dense = upload_commands(1, &vec![vec![255; 108]; 5]).unwrap();
        assert_eq!(dense.len(), 3);
    }
    #[test]
    fn uploads_preserve_full_height_pixels_and_distinguish_repeated_messages() {
        let station = MacAddress([2, 1, 2, 3, 4, 5]);
        let mut bitmap = vec![0; 3072];
        bitmap[32 * 79 + 30] = 0xa5;
        let bodies = message_bodies(station, 1, 0, 3, &bitmap).unwrap();
        assert_eq!(bodies.len(), 88);
        let repeated = message_bodies(station, 1, 1, 89, &bitmap).unwrap();
        assert_ne!(bodies[0], repeated[0]);
        assert_ne!(bodies.last(), repeated.last());
        assert_ne!(
            bodies[1], repeated[1],
            "DS deduplicates counters across messages"
        );
        let wrap = message_bodies(station, 1, 2, u16::MAX, &bitmap).unwrap();
        assert_eq!(&wrap[1][134..136], &u16::MAX.to_le_bytes());
        assert_eq!(&wrap[2][134..136], &[0, 0]);
        let mut bytes = Vec::new();
        for body in &bodies[1..bodies.len() - 1] {
            assert!(body.len() <= link::MAX_BODY);
            let env = PictoChatEnvelope::parse(NintendoFlow::ClientToHost, body).unwrap();
            let f = PictoChatFragment::parse(&env.payload, NintendoFlow::ClientToHost).unwrap();
            bytes.extend_from_slice(&f.data);
        }
        let decoded = PictoChatMessage::parse(&bytes).unwrap();
        assert_eq!(native_message_to_api_bitmap(&decoded), bitmap);
        assert!(message_bodies(station, 1, 0, 3, &[0; 3072]).is_err());
    }

    #[test]
    fn incoming_fragments_reassemble_once_for_both_host_fragment_sizes() {
        let sender = MacAddress([0, 2, 3, 4, 5, 6]);
        let station = MacAddress([2, 1, 2, 3, 4, 5]);
        let mut bitmap = vec![0; 3072];
        bitmap[32 * 50 + 20] = 0x81;
        let mut palette = vec![0; crate::protocol::PALETTE_BITMAP_BYTES];
        palette[128 * 50 + 20] = 0xf3;
        for bitmap in [bitmap, palette] {
            let native = encode_message(sender, &api_bitmap_to_native(&bitmap).unwrap()).unwrap();
            for size in [80, 120, 160] {
                let mut session = Session {
                    station,
                    host: sender,
                    room: 0,
                    slot: 1,
                    name: "Relay",
                    names: BTreeMap::from([(sender, "DS".into())]),
                    profiles: BTreeMap::new(),
                    members: vec![sender, station],
                    assemblers: [
                        MessageReassembler::new(80),
                        MessageReassembler::new(120),
                        MessageReassembler::new(160),
                    ],
                    completed: VecDeque::new(),
                    ready: true,
                    active: None,
                    queue: VecDeque::new(),
                    sequence: 0,
                    next_fragment: 3,
                };
                let events = RefCell::new(Vec::new());
                let emit = |event| events.borrow_mut().push(event);
                for _ in 0..2 {
                    for fragment in fragment_payload_starting_at(&native, [1, 2], size, 50).unwrap()
                    {
                        let body = encode_fragment_envelope(NintendoFlow::HostToClient, &fragment)
                            .unwrap();
                        session.receive(&body, &emit);
                    }
                }
                let events = events.borrow();
                assert_eq!(events.len(), 1);
                let LiveEvent::Message {
                    bitmap: actual,
                    name,
                    ..
                } = &events[0]
                else {
                    panic!("expected drawing")
                };
                assert_eq!(actual, &bitmap);
                assert_eq!(name, "DS");
            }
        }
    }
}
