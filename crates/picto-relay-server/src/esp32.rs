//! Blocking serial worker isolated from the async HTTP broker.
use crate::bitmap::{api_bitmap_to_native, native_message_to_api_bitmap};
use crate::broker::station_id;
use crate::capture::Capture;
use crate::worker::{LiveCommand, LiveEvent, LiveParticipant};
use anyhow::{Context, Result, bail};
use picto_relay_codec::{
    MAC_ADDRESS_LEN, MacAddress, MessageReassembler, NintendoFlow, PictoChatEnvelope,
    PictoChatFragment, PictoChatMessage, PictoChatProfile, PictoChatRoster, ReassemblyKey,
    encode_message,
};
use picto_relay_link as link;
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
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
/// How long a command may go unanswered, or stay busy, before the session is reset.
const REQUEST_DEADLINE: Duration = Duration::from_secs(3);
/// How often an unanswered or busy command is resent.
const REQUEST_RETRY_INTERVAL: Duration = Duration::from_millis(150);

// ---- Room discovery --------------------------------------------------------

/// Member counts for rooms A through D, [`link::NO_BEACON`] when unknown.
type RoomCounts = [u8; link::ROOM_COUNT];
/// Longest a full channel sweep by the firmware may take.
const SCAN_TIMEOUT: Duration = Duration::from_secs(6);

// ---- Session ---------------------------------------------------------------

/// Pause before reopening the device after a failed session.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// How often the firmware is told the host is still alive.
const PING_INTERVAL: Duration = Duration::from_secs(1);
/// Longest finding the room and exchanging profiles may take before rejoining.
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest gap between firmware status reports before reconnecting.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest a drawing may take from upload to DS delivery.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(40);
/// Drawings the firmware holds at once: one being sent and one waiting.
const MAX_UPLOADED_DRAWINGS: usize = 2;
/// Drawings that may wait on the host behind those.
const MAX_QUEUED_DRAWINGS: usize = 4;
/// Recently completed incoming drawings remembered to drop host repeats.
const MAX_COMPLETED_DRAWINGS: usize = 128;
/// Fragment sizes a DS host may use when rebroadcasting drawings, in bytes.
const HOST_FRAGMENT_SIZES: [usize; 3] = [80, 120, 160];
/// Colors in the `PictoChat` palette.
const PALETTE_COLORS: usize = 16;
/// Scale of a percentage.
const PERCENT: usize = 100;

// ---- PictoChat application packets -----------------------------------------

/// Mask removing the flag bit whose meaning is not established from a packet ID.
const PACKET_ID_MASK: u16 = 0x7fff;
/// Packet ID of the host's participant roster.
const ROSTER_PACKET_ID: u16 = 0x1c34;
/// Length of the opaque transfer tag that identifies a host rebroadcast.
const TRANSFER_TAG_LEN: usize = 2;

struct Port {
    file: File,
    decoder: link::Decoder,
    next_id: u32,
    events: VecDeque<link::Packet>,
    /// Where captured radio frames go, when the firmware is asked for them.
    capture: Option<Capture>,
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
            capture: None,
        })
    }
    fn read(&mut self) -> Result<()> {
        let mut bytes = [0; READ_CHUNK_BYTES];
        let n = self.file.read(&mut bytes)?;
        for &b in &bytes[..n] {
            if let Some(p) = self.decoder.push(b) {
                // Captured frames are numerous: write them out as they arrive
                // instead of queueing them behind the session's events.
                if p.kind == link::FRAME {
                    self.record(&p)?;
                    continue;
                }
                if self.events.len() >= MAX_PENDING_EVENTS {
                    bail!("USB event backlog exceeded limit");
                }
                self.events.push_back(p);
            }
        }
        Ok(())
    }
    fn record(&mut self, frame: &link::Packet) -> Result<()> {
        let (Some(capture), Some(data)) = (
            &mut self.capture,
            frame.payload.get(link::FRAME_DATA_OFFSET..),
        ) else {
            return Ok(());
        };
        let time = &frame.payload[link::FRAME_TIME_OFFSET..link::FRAME_FLAGS_OFFSET];
        let length = &frame.payload[link::FRAME_LENGTH_OFFSET..link::FRAME_DATA_OFFSET];
        capture.write(
            u32::from_le_bytes(time.try_into()?),
            u16::from_le_bytes(length.try_into()?),
            data,
        )
    }
    /// Sends a command until the firmware gives a final answer. Returns its
    /// status and response data; a busy device or a lost packet is retried.
    fn exchange(&mut self, kind: u8, data: &[u8]) -> Result<(u8, Vec<u8>)> {
        let id = self.next_id;
        tracing::debug!(kind, id, "USB request");
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("USB command ID exhausted")?;
        let wire = link::encode(kind, id, data).context("USB command too large")?;
        let deadline = Instant::now() + REQUEST_DEADLINE;
        let mut next_send = Instant::now();
        let mut attempts = 0;
        while Instant::now() < deadline {
            if Instant::now() >= next_send {
                self.file.write_all(&wire)?;
                attempts += 1;
                next_send = Instant::now() + REQUEST_RETRY_INTERVAL;
            }
            self.read()?;
            let Some(index) = self
                .events
                .iter()
                .position(|p| p.kind == link::RESPONSE && p.id == id)
            else {
                continue;
            };
            let reply = self.events.remove(index).context("missing USB response")?;
            match reply.payload.split_first() {
                Some((&link::STATUS_BUSY, _)) | None => {}
                Some((&status, data)) => {
                    tracing::debug!(kind, id, attempts, status, "USB answered");
                    return Ok((status, data.to_vec()));
                }
            }
        }
        bail!("ESP32 command {kind} timed out or remained busy")
    }
    /// Sends a command that is expected to succeed and returns its response data.
    fn request(&mut self, kind: u8, data: &[u8]) -> Result<Vec<u8>> {
        match self.exchange(kind, data)? {
            (link::STATUS_OK, data) => Ok(data),
            (status, _) => bail!("ESP32 refused command {kind}: {}", status_text(status)),
        }
    }
    /// Attaches to the firmware and returns its station address.
    fn hello(&mut self) -> Result<MacAddress> {
        let (status, data) = self.exchange(link::HELLO, &[link::PROTOCOL_VERSION])?;
        let version = data.get(link::HELLO_VERSION_OFFSET).copied();
        let build = String::from_utf8_lossy(data.get(link::HELLO_BUILD_OFFSET..).unwrap_or(&[]))
            .into_owned();
        if status == link::STATUS_VERSION_MISMATCH {
            bail!(
                "ESP32 firmware (build {build}) speaks protocol {} but this server speaks {}; \
                 flash matching firmware with `just firmware-upload`",
                version.map_or_else(|| "?".into(), |v| v.to_string()),
                link::PROTOCOL_VERSION
            );
        }
        if status != link::STATUS_OK {
            bail!("ESP32 refused to attach: {}", status_text(status));
        }
        tracing::info!(build, "ESP32 firmware attached");
        let mac = data
            .get(link::HELLO_MAC_OFFSET..link::HELLO_MAC_OFFSET + link::MAC_LEN)
            .context("invalid ESP32 hello")?;
        Ok(MacAddress(mac.try_into()?))
    }
}

fn status_text(status: u8) -> &'static str {
    match status {
        link::STATUS_OK => "ok",
        link::STATUS_BUSY => "busy",
        link::STATUS_INVALID => "malformed command",
        link::STATUS_WRONG_STATE => "not possible in the current session state",
        link::STATUS_VERSION_MISMATCH => "protocol version mismatch",
        link::STATUS_NOT_ATTACHED => "no HELLO was sent",
        _ => "unknown status",
    }
}

/// Explains a LEFT event to the user.
fn left_text(payload: &[u8]) -> String {
    let reason = payload.get(link::LEFT_REASON_OFFSET).copied();
    let code = payload
        .get(link::LEFT_CODE_OFFSET..link::LEFT_LEN)
        .and_then(|code| code.try_into().ok())
        .map_or(0, u16::from_le_bytes);
    match reason {
        Some(link::LEFT_REQUESTED) => "the relay left the room".into(),
        Some(link::LEFT_NO_HOST) => {
            "no DS is hosting that room nearby; open the room on a DS first".into()
        }
        Some(link::LEFT_JOIN_TIMEOUT) => "the DS did not answer the join request".into(),
        Some(link::LEFT_REJECTED) => format!("the DS refused the join (status {code})"),
        Some(link::LEFT_KICKED) => format!("the DS closed the session (reason {code})"),
        Some(link::LEFT_HOST_LOST) => "the DS stopped responding".into(),
        Some(link::LEFT_RADIO_STALLED) => "the ESP32 radio stalled".into(),
        _ => "the session ended for an unknown reason".into(),
    }
}

/// Asks the idle firmware to sweep every channel and reports the rooms it saw.
/// Called only when no session worker owns the serial port.
pub(crate) fn discover(path: &str) -> Result<RoomCounts> {
    let mut port = Port::open(path)?;
    port.hello()?;
    port.request(link::LEAVE, &[])?;
    port.request(link::SCAN, &[])?;
    let deadline = Instant::now() + SCAN_TIMEOUT;
    while Instant::now() < deadline {
        port.read()?;
        while let Some(packet) = port.events.pop_front() {
            if packet.kind == link::ROOMS {
                return packet
                    .payload
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("invalid room scan result"));
            }
        }
    }
    bail!("ESP32 room scan timed out")
}

/// Runs one persistent API session, reconnecting the device until shutdown.
/// # Errors
/// Returns on a command-channel failure during shutdown.
pub fn run(
    port: &str,
    capture: Option<&Path>,
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
        let mut joined = false;
        let device = Device {
            path: port,
            capture,
        };
        match connected(&device, room, name, profile, commands, emit, &mut joined) {
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
                // A session that was up and dropped is usually back within a
                // second with the same people in the room, so the member list
                // is kept and corrected by the next roster. If the attempt
                // never got in, the room is gone: nobody is there any more.
                if !joined {
                    emit(LiveEvent::Participants {
                        room,
                        participants: Vec::new(),
                    });
                }
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
/// A drawing the firmware holds, in the order it will send them.
struct Sending {
    id: u32,
    bitmap: Vec<u8>,
    deadline: Instant,
    started: Instant,
    progress: Option<(u8, u8, usize)>,
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
    completed: VecDeque<(MacAddress, u16, [u8; TRANSFER_TAG_LEN])>,
    ready: bool,
    sending: VecDeque<Sending>,
    queue: VecDeque<(u32, Vec<u8>)>,
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
        let waiting = self.queue.len() + self.sending.len().saturating_sub(1);
        let Some(drawing) = self.sending.iter_mut().find(|drawing| drawing.id == p.id) else {
            return;
        };
        let sent = p.payload[link::STATUS_SENT_OFFSET];
        let total = p.payload[link::STATUS_TOTAL_OFFSET];
        let echoed = p.payload[link::STATUS_ECHOED_OFFSET];
        let current = (sent, echoed, waiting);
        if drawing.progress == Some(current) {
            return;
        }
        drawing.progress = Some(current);
        emit(LiveEvent::State {
            state: "sending",
            detail: format!(
                "Drawing {}: {sent}/{total} fragments sent, {echoed} confirmed · {waiting} waiting",
                drawing.id
            ),
        });
    }
    fn packet(&mut self, p: &link::Packet, emit: &impl Fn(LiveEvent)) -> Result<()> {
        match p.kind {
            link::RX => self.receive(&p.payload, emit),
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
                    .payload
                    .chunks_exact(size_of::<u32>())
                    .filter_map(|counter| counter.try_into().ok())
                    .map(u32::from_le_bytes)
                    .collect();
                tracing::debug!(?counters, "ESP32 radio diagnostics");
            }
            link::READY if p.payload.len() == link::READY_LEN => {
                self.host = MacAddress(
                    p.payload[link::READY_HOST_OFFSET..link::READY_HOST_OFFSET + link::MAC_LEN]
                        .try_into()?,
                );
                self.slot = p.payload[link::READY_SLOT_OFFSET];
                self.ready = true;
                tracing::info!(
                    host = %self.host,
                    slot = self.slot,
                    channel = p.payload[link::READY_CHANNEL_OFFSET],
                    "joined the DS room"
                );
                emit(LiveEvent::Ready {
                    host: self.host,
                    room: self.room,
                    slot: self.slot,
                });
            }
            link::DELIVERED => {
                let Some(index) = self.sending.iter().position(|drawing| drawing.id == p.id) else {
                    return Ok(());
                };
                let drawing = self.sending.remove(index).context("missing drawing")?;
                tracing::info!(
                    message_id = drawing.id,
                    elapsed_ms = drawing.started.elapsed().as_millis(),
                    "ESP32 drawing delivered"
                );
                emit(LiveEvent::Message {
                    station_id: station_id(self.station),
                    name: self.name.into(),
                    message_id: drawing.id,
                    bitmap: drawing.bitmap,
                });
                emit(LiveEvent::Delivered {
                    message_id: drawing.id,
                });
            }
            link::LEFT => bail!("{}; rejoining", left_text(&p.payload)),
            link::STATUS if p.payload.len() == link::STATUS_LEN => {
                tracing::debug!(transfer = p.id, status = ?p.payload, "ESP32 status");
                self.progress(p, emit);
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

/// Where the ESP32 is and where its radio capture goes, if anywhere.
struct Device<'a> {
    path: &'a str,
    capture: Option<&'a Path>,
}

fn connected(
    device: &Device<'_>,
    room: u8,
    name: &str,
    profile: &crate::worker::ProfileOptions,
    commands: &Receiver<LiveCommand>,
    emit: &impl Fn(LiveEvent),
    joined: &mut bool,
) -> Result<()> {
    let mut port = Port::open(device.path)?;
    let station = port.hello()?;
    emit(LiveEvent::Station { station });
    // Release any previous owner (e.g. a server process that restarted).
    port.request(link::LEAVE, &[])?;
    if let Some(path) = device.capture {
        port.capture = Some(Capture::open(path)?);
        port.request(link::TRACE, &[1])?;
        tracing::info!(file = %path.display(), "recording radio frames");
    }
    if profile.message.encode_utf16().count() * size_of::<u16>() > link::PROFILE_MESSAGE_LEN
        || profile.message.contains('\0')
        || profile.color > link::MAX_PROFILE_COLOR
    {
        bail!("invalid profile message or color");
    }
    emit(LiveEvent::State {
        state: "connecting",
        detail: format!(
            "searching Wi-Fi channels for Room {}",
            char::from(b'A' + room)
        ),
    });
    // Field order follows the link::JOIN_*_OFFSET layout.
    let mut join = vec![room];
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
        sending: VecDeque::new(),
        queue: VecDeque::new(),
    };
    // The member list is published when the host's first roster arrives.
    let result = worker(&mut port, &mut session, commands, emit);
    *joined = session.ready;
    let unfinished = session.sending.iter().map(|drawing| drawing.id);
    for message_id in unfinished.chain(session.queue.iter().map(|(id, _)| *id)) {
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
        if !session.ready && joined_at.elapsed() > JOIN_TIMEOUT {
            bail!("joining the DS room stalled; rejoining");
        }
        if last_status.elapsed() > STATUS_TIMEOUT {
            bail!("ESP32 status stream stopped");
        }
        if session
            .sending
            .iter()
            .any(|drawing| Instant::now() >= drawing.deadline)
        {
            bail!("drawing delivery timed out; resetting the physical session");
        }
        // The firmware accepts the next drawing while it is still sending one.
        if session.ready
            && session.sending.len() < MAX_UPLOADED_DRAWINGS
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
    let native = match native_drawing(session.station, &bitmap) {
        Ok(native) => native,
        Err(error) => {
            emit(LiveEvent::Error {
                code: "invalid_drawing",
                message: error.to_string(),
            });
            emit(LiveEvent::DeliveryTimedOut { message_id: id });
            return Ok(());
        }
    };
    let commands = draw_commands(id, &native)?;
    for (index, command) in commands.iter().enumerate() {
        port.request(link::DRAW, command)?;
        emit(LiveEvent::State {
            state: "uploading",
            detail: format!(
                "Drawing {id}: {}% uploaded · {} waiting",
                (index + 1) * PERCENT / commands.len(),
                session.queue.len()
            ),
        });
    }
    session.sending.push_back(Sending {
        id,
        bitmap,
        deadline: Instant::now() + DELIVERY_TIMEOUT,
        started,
        progress: None,
    });
    tracing::info!(
        message_id = id,
        upload_ms = started.elapsed().as_millis(),
        bytes = native.len(),
        commands = commands.len(),
        "ESP32 upload complete"
    );
    Ok(())
}

/// Encodes an API bitmap as the native drawing the DS displays. The firmware
/// fragments it and wraps it for the radio.
fn native_drawing(station: MacAddress, bitmap: &[u8]) -> Result<Vec<u8>> {
    Ok(encode_message(station, &api_bitmap_to_native(bitmap)?)?)
}

/// Splits a native drawing into DRAW payloads following the `link::DRAW_*` layout.
fn draw_commands(id: u32, native: &[u8]) -> Result<Vec<Vec<u8>>> {
    let total = u16::try_from(native.len()).context("drawing too large")?;
    native
        .chunks(link::MAX_DRAW_CHUNK)
        .enumerate()
        .map(|(index, chunk)| {
            let position = u16::try_from(index * link::MAX_DRAW_CHUNK)?;
            let mut command = id.to_le_bytes().to_vec();
            command.extend_from_slice(&position.to_le_bytes());
            command.extend_from_slice(&total.to_le_bytes());
            command.extend_from_slice(chunk);
            Ok(command)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use picto_relay_codec::{encode_fragment_envelope, fragment_payload_starting_at};
    use std::cell::RefCell;

    #[test]
    fn draw_commands_cover_the_drawing_in_order() {
        let native: Vec<u8> = (0..=u8::MAX)
            .cycle()
            .take(link::MAX_DRAWING_BYTES)
            .collect();
        let commands = draw_commands(7, &native).unwrap();
        assert_eq!(commands.len(), native.len().div_ceil(link::MAX_DRAW_CHUNK));
        let mut rebuilt = Vec::new();
        for command in &commands {
            assert!(command.len() <= link::MAX_COMMAND_PAYLOAD);
            assert_eq!(
                command[link::DRAW_ID_OFFSET..link::DRAW_POSITION_OFFSET],
                7u32.to_le_bytes()
            );
            let position = u16::from_le_bytes(
                command[link::DRAW_POSITION_OFFSET..link::DRAW_TOTAL_OFFSET]
                    .try_into()
                    .unwrap(),
            );
            let total = u16::from_le_bytes(
                command[link::DRAW_TOTAL_OFFSET..link::DRAW_DATA_OFFSET]
                    .try_into()
                    .unwrap(),
            );
            assert_eq!(usize::from(position), rebuilt.len());
            assert_eq!(usize::from(total), native.len());
            rebuilt.extend_from_slice(&command[link::DRAW_DATA_OFFSET..]);
        }
        assert_eq!(rebuilt, native);
    }

    #[test]
    fn native_drawings_fit_the_firmware_limits_and_reject_blank_input() {
        let station = MacAddress([2, 1, 2, 3, 4, 5]);
        let mut bitmap = vec![0; 3072];
        bitmap[32 * 79 + 30] = 0xa5;
        let tall = native_drawing(station, &bitmap).unwrap();
        assert_eq!(tall.len(), link::MAX_DRAWING_BYTES);
        let mut bitmap = vec![0; 3072];
        bitmap[30] = 0xa5;
        let short = native_drawing(station, &bitmap).unwrap();
        assert_eq!(short.len(), link::MIN_DRAWING_BYTES);
        let decoded = PictoChatMessage::parse(&short).unwrap();
        assert_eq!(native_message_to_api_bitmap(&decoded), bitmap);
        assert!(native_drawing(station, &[0; 3072]).is_err());
    }

    /// What a DS client transmits for one drawing, built with the codec: the
    /// announcement, the fragments and the completion. The firmware implements
    /// this in C (`client_drawing_body`); the fixture below keeps them equal.
    fn reference_bodies(
        slot: u8,
        drawing_number: u16,
        first_frame: u16,
        native: &[u8],
    ) -> Vec<Vec<u8>> {
        const FIRMWARE_FRAGMENT_BYTES: usize = 120;
        let n = u32::from(drawing_number);
        let transfer_counter = 5 + drawing_number * 2;
        // Envelope: packet ID 0x8d0a, sequence 0, declared length 20.
        let mut announce = vec![0x0a, 0x8d, 0, 0, 0x14, 0, slot, 0, 0xff, 0xff];
        announce.extend_from_slice(&u32::try_from(native.len()).unwrap().to_le_bytes());
        announce.extend_from_slice(&(12 + n * 3).to_le_bytes());
        announce.extend_from_slice(&(0x75f0_3cca ^ n.wrapping_mul(0x9e37_79b9)).to_le_bytes());
        announce.extend_from_slice(&transfer_counter.to_le_bytes());
        let mut bodies = vec![announce];
        for mut fragment in
            fragment_payload_starting_at(native, [0, 0], FIRMWARE_FRAGMENT_BYTES, 0).unwrap()
        {
            fragment.frame_number = first_frame.wrapping_add(fragment.frame_number);
            let mut body = encode_fragment_envelope(NintendoFlow::ClientToHost, &fragment).unwrap();
            // The first fragment-header byte is the sender's slot; the codec's
            // captured value is from a client in slot 1.
            body[6] = slot;
            bodies.push(body);
        }
        // Envelope sequence 3, then fields replayed from a capture.
        let mut done = vec![
            0x0a, 0x8d, 3, 0, 0x14, 0, slot, 4, 0xff, 0xff, 0xdd, 0x93, 0x32, 2, 1, 0xff, 0xff,
            0xff, 0xbb, 0xc8, 0x36, 2,
        ];
        done.extend_from_slice(&(transfer_counter + 1).to_le_bytes());
        bodies.push(done);
        bodies
    }

    /// Writes or checks the fixtures that `firmware/esp32/test/test.c` replays:
    /// a native drawing, and the bodies a client must send for it twice in one
    /// session. Regenerate with `just protocol-header`.
    #[test]
    fn firmware_drawing_fixture_matches_the_codec() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../firmware/esp32/test/fixtures");
        let station = MacAddress([0xac, 0x15, 0x18, 0xd6, 0x94, 0x34]);
        let slot = 1;
        let mut bitmap = vec![0; 3072];
        for row in 0..80 {
            bitmap[32 * row + 4 + row % 24] = 0x5a;
        }
        let native = native_drawing(station, &bitmap).unwrap();
        assert_eq!(native.len(), link::MAX_DRAWING_BYTES);

        let first = reference_bodies(slot, 0, 3, &native);
        let fragments = u16::try_from(first.len() - 2).unwrap();
        let second = reference_bodies(slot, 1, 3 + fragments, &native);
        assert_ne!(
            first[0], second[0],
            "the DS ignores repeated control values"
        );
        let mut bodies = Vec::new();
        for body in first.iter().chain(&second) {
            bodies.push(u8::try_from(body.len()).unwrap());
            bodies.extend_from_slice(body);
        }
        for (name, expected) in [("drawing.bin", &native), ("drawing-bodies.bin", &bodies)] {
            let path = fixtures.join(name);
            if std::env::var_os("PICTO_WRITE_FIXTURES").is_some() {
                std::fs::write(&path, expected).unwrap();
            }
            assert!(
                &std::fs::read(&path).unwrap() == expected,
                "{name} is out of date; run `just protocol-header`"
            );
        }
    }

    #[test]
    fn left_reasons_are_explained() {
        assert!(left_text(&[link::LEFT_NO_HOST, 0, 0]).contains("open the room"));
        assert!(left_text(&[link::LEFT_KICKED, 3, 0]).contains("reason 3"));
        assert!(left_text(&[]).contains("unknown"));
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
                    assemblers: host_reassemblers(),
                    completed: VecDeque::new(),
                    ready: true,
                    sending: VecDeque::new(),
                    queue: VecDeque::new(),
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
