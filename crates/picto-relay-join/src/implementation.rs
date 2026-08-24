use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use picto_relay_codec::{
    DRAWING_HEIGHT, DRAWING_WIDTH, FcsMode, FrameType, MacAddress, MacFrame, ManagementBody,
    MessageReassembler, MultiplayerHeader, NintendoBeacon, NintendoFlow, PictoChatEnvelope,
    PictoChatFragment, PictoChatMessage, PictoChatProfile, PictoChatRoster, ReassemblyKey,
    SequenceControl, build_association_request, build_authentication_request,
    build_client_reply_frame, build_management_frame, decode_nintendo_data,
    encode_fragment_envelope, encode_message, fragment_payload_starting_at, parse_management,
};
use picto_relay_radio::{
    LinuxMonitorRadio, RadioBackend, RadioConfig, ReceivedFrame, TransmitFrame,
};

const AUTHENTICATION_SEQUENCE: u16 = 0;
const ASSOCIATION_SEQUENCE: u16 = 1;
const LEAVE_SEQUENCE: u16 = 8;
const FIRMWARE_DIAGNOSTIC_PREFIX_MASK: u8 = 0xf0;
const FIRMWARE_DIAGNOSTIC_PREFIX: u8 = 0xd0;
const FIRMWARE_DIAGNOSTIC_QUEUED: u8 = 0x01;
const FIRMWARE_DIAGNOSTIC_COMPLETED: u8 = 0x02;
const FIRMWARE_DIAGNOSTIC_TX_CLEAN: u8 = 0x04;
const FIRMWARE_DIAGNOSTIC_BUSY: u8 = 0x08;
const FIRMWARE_MESSAGE_CURSOR_PREFIX_MASK: u16 = 0xff00;
const FIRMWARE_MESSAGE_CURSOR_PREFIX: u16 = 0xa500;
const SIFS_REPLY_TARGET_US: u64 = 11;
const FIRMWARE_ARM_TEMPLATE_PREFIX: u8 = 0xa0;
const FIRMWARE_ARM_MESSAGE_PREFIX: u8 = 0xc0;
const FIRMWARE_ARM_SLOT_PREFIX: u8 = 0xb0;
const FIRMWARE_COMMIT_MARKER: u8 = 0xae;
const FIRMWARE_DISARM_MARKER: u8 = 0xaf;
const API_BITMAP_WIDTH: usize = 256;
const API_BITMAP_HEIGHT: usize = 96;
const API_BITMAP_BYTES: usize = API_BITMAP_WIDTH * API_BITMAP_HEIGHT / 8;
const LIVE_RECEIVE_SLICE: Duration = Duration::from_millis(20);
const ROOM_OBSERVER_RECEIVE_SLICE: Duration = Duration::from_millis(100);
const ROOM_OBSERVER_EXPIRY: Duration = Duration::from_secs(2);
const PICTOCHAT_ROOM_COUNT: usize = 4;
const LIVE_MESSAGE_TIMEOUT: Duration = Duration::from_secs(20);
const FIRMWARE_MESSAGE_WINDOW_SIZE: usize = 16;
const MAX_NATIVE_DRAWING_HEIGHT: usize = 80;
const LIVE_MESSAGE_QUEUE_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum FirmwareReplyTemplate {
    Idle = 0,
    Session = 1,
    InitialAnnouncement = 2,
    Empty = 3,
    InitialProfile = 4,
    InitialCompletion = 5,
    RepeatAnnouncement = 6,
    FinalProfile = 7,
    FinalCompletion = 8,
    MessageAnnouncement = 9,
    MessageFragment0 = 10,
    MessageFragment1 = 11,
    MessageFragment2 = 12,
    MessageFragment3 = 13,
    MessageFragment4 = 14,
    MessageFragment5 = 15,
    MessageFragment6 = 16,
    MessageFragment7 = 17,
    MessageFragment8 = 18,
    MessageFragment9 = 19,
    MessageFragment10 = 20,
    MessageFragment11 = 21,
    MessageFragment12 = 22,
    MessageFragment13 = 23,
    MessageCompletion = 24,
}

const FIRMWARE_MESSAGE_TEMPLATES: [FirmwareReplyTemplate; 16] = [
    FirmwareReplyTemplate::MessageAnnouncement,
    FirmwareReplyTemplate::MessageFragment0,
    FirmwareReplyTemplate::MessageFragment1,
    FirmwareReplyTemplate::MessageFragment2,
    FirmwareReplyTemplate::MessageFragment3,
    FirmwareReplyTemplate::MessageFragment4,
    FirmwareReplyTemplate::MessageFragment5,
    FirmwareReplyTemplate::MessageFragment6,
    FirmwareReplyTemplate::MessageFragment7,
    FirmwareReplyTemplate::MessageFragment8,
    FirmwareReplyTemplate::MessageFragment9,
    FirmwareReplyTemplate::MessageFragment10,
    FirmwareReplyTemplate::MessageFragment11,
    FirmwareReplyTemplate::MessageFragment12,
    FirmwareReplyTemplate::MessageFragment13,
    FirmwareReplyTemplate::MessageCompletion,
];

impl FirmwareReplyTemplate {
    const fn marker(self) -> u8 {
        let template = self as u8;
        if template < FirmwareReplyTemplate::MessageAnnouncement as u8 {
            FIRMWARE_ARM_TEMPLATE_PREFIX | template
        } else {
            FIRMWARE_ARM_MESSAGE_PREFIX
                | (template - FirmwareReplyTemplate::MessageAnnouncement as u8)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IdentityPass {
    Initial,
    Final,
}

#[derive(Debug, Parser)]
#[command(
    name = "picto-relay-join",
    about = "Make one Raspberry Pi virtual station appear in a DS-hosted PictoChat room"
)]
struct Cli {
    /// Dedicated AR9271 interface.
    #[arg(short, long, default_value = "wlan1")]
    interface: String,

    /// Put the interface into monitor mode and select the channel before scanning.
    #[arg(long)]
    prepare: bool,

    /// 2.4 GHz channel carrying the `PictoChat` room.
    #[arg(short, long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=14))]
    channel: u8,

    /// Room to join: A, B, C, or D. Omit to use the first discovered room.
    #[arg(short, long, value_parser = parse_room)]
    room: Option<u8>,

    /// Nintendo-OUI MAC address for the virtual DS. Keep it unique on the channel.
    #[arg(long, default_value = "00:09:bf:52:00:01", value_parser = parse_mac)]
    station: MacAddress,

    /// Nickname placed in the `PictoChat` profile (at most 10 UTF-16 units).
    #[arg(long, default_value = "PictoRelay", value_parser = parse_name)]
    name: String,

    /// Preload the identity handshake and idle replies for custom AR9271 firmware.
    #[arg(long)]
    firmware_assist: bool,

    /// Send a small native `PictoChat` dot after the identity exchange completes.
    #[arg(long, requires = "firmware_assist")]
    send_dot: bool,

    /// Maximum seconds to wait for a matching room beacon.
    #[arg(long, default_value_t = 60)]
    scan_timeout: u64,

    /// Milliseconds to wait for each authentication or association response.
    #[arg(long, default_value_t = 750)]
    response_timeout_ms: u64,

    /// Number of management-request attempts before failing.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..=10))]
    attempts: u8,

    /// Seconds to answer Nintendo slot polls after association.
    #[arg(long, default_value_t = 10)]
    hold_seconds: u64,

    /// Record this join attempt as a radiotap PCAP without starting another sniffer.
    #[arg(long)]
    capture: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
struct DiscoveredRoom {
    host: MacAddress,
    room: u8,
    users: u8,
    game_id: u32,
    stream_code: u16,
    rssi_dbm: Option<i8>,
}

#[derive(Clone, Copy, Debug)]
struct ObservedRoom {
    room: u8,
    users: u8,
    last_seen: Instant,
}

#[derive(Debug, Default)]
struct ObservedRooms {
    hosts: BTreeMap<MacAddress, ObservedRoom>,
}

impl ObservedRooms {
    fn observe(&mut self, room: DiscoveredRoom, now: Instant) {
        if usize::from(room.room) >= PICTOCHAT_ROOM_COUNT {
            return;
        }
        self.hosts.insert(
            room.host,
            ObservedRoom {
                room: room.room,
                users: room.users,
                last_seen: now,
            },
        );
    }

    fn counts(&mut self, now: Instant) -> [u8; PICTOCHAT_ROOM_COUNT] {
        self.hosts.retain(|_, room| {
            now.saturating_duration_since(room.last_seen) <= ROOM_OBSERVER_EXPIRY
        });
        let mut counts = [0; PICTOCHAT_ROOM_COUNT];
        for room in self.hosts.values() {
            let count = &mut counts[usize::from(room.room)];
            *count = (*count).max(room.users);
        }
        counts
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManagementResponse {
    Authentication(u16),
    Association { status: u16, slot: u8 },
}

/// Runs one diagnostic join attempt from command-line arguments.
///
/// # Errors
///
/// Returns an error when arguments are invalid, the radio cannot be prepared,
/// room association fails, or capture finalization fails.
pub fn run_probe() -> Result<()> {
    let options = Cli::parse();
    println!("Picto Relay active join probe");
    println!("  station   {}", options.station);
    println!(
        "  interface {} · channel {}",
        options.interface, options.channel
    );
    let mut radio = open_radio(&options)?;
    let result = run_join(&options, &mut radio);
    if let Some(stats) = radio.finish_capture()? {
        let path = options
            .capture
            .as_deref()
            .context("capture stats require a capture path")?;
        println!(
            "capture  {} · {} packet(s) · {} bytes",
            path.display(),
            stats.packets,
            stats.bytes
        );
    }
    result
}

fn open_radio(options: &Cli) -> Result<LinuxMonitorRadio> {
    let radio_interface = if options.prepare {
        println!("  radio     preparing monitor + hardware-ACK interfaces");
        let monitor =
            prepare_radio_interfaces(&options.interface, options.channel, options.station)?;
        println!(
            "  radio     capture/inject on {monitor} · ACK as {} on {}",
            options.station, options.interface
        );
        monitor
    } else {
        options.interface.clone()
    };
    let mut radio = if let Some(capture_path) = &options.capture {
        println!("  capture   {}", capture_path.display());
        LinuxMonitorRadio::open_with_capture(&radio_interface, capture_path)?
    } else {
        LinuxMonitorRadio::open(&radio_interface)?
    };
    radio.configure(RadioConfig {
        channel: options.channel,
    })?;
    Ok(radio)
}

fn run_join(options: &Cli, radio: &mut impl RadioBackend) -> Result<()> {
    println!(
        "\n1/4 On a Nintendo DS, open PictoChat and enter {}.",
        options
            .room
            .map_or("the desired room".into(), |room| format!(
                "Room {}",
                room_name(room)
            ))
    );
    println!("    Scanning for a room beacon…");
    let discovered = discover_room(
        radio,
        options.room,
        Instant::now() + Duration::from_secs(options.scan_timeout),
    )?;
    println!(
        "    Found Room {} hosted by {} · {} user(s){} · stream 0x{:04x}",
        room_name(discovered.room),
        discovered.host,
        discovered.users,
        format_rssi(discovered.rssi_dbm),
        discovered.stream_code
    );
    if discovered.users >= 16 {
        bail!("Room {} is full", room_name(discovered.room));
    }

    let response_timeout = Duration::from_millis(options.response_timeout_ms);
    println!("\n2/4 Sending open-system authentication…");
    authenticate(
        radio,
        options.station,
        discovered.host,
        options.attempts,
        response_timeout,
    )?;
    println!("    Host authenticated the virtual station.");

    println!("\n3/4 Preparing Nintendo binary-session association…");
    let slot = associate_with_firmware_replies(
        radio,
        options.station,
        discovered,
        &options.name,
        options.firmware_assist,
        options.send_dot,
        options.attempts,
        response_timeout,
    )?;
    println!("    Host assigned participant slot {slot}.");

    println!("\n4/4 Answering Nintendo slot polls; watch the DS participant list…");
    let report = maintain_membership(
        radio,
        discovered,
        MembershipConfig {
            station: options.station,
            slot,
            name: &options.name,
            firmware_assist: options.firmware_assist,
            deadline: Instant::now() + Duration::from_secs(options.hold_seconds),
        },
    );
    let leave = leave_room(
        radio,
        discovered,
        options.station,
        slot,
        options.firmware_assist,
    );
    let report = report?;
    leave?;
    report_membership(options, slot, &report);
    if options.firmware_assist && !report.initialization.profile_finalized() {
        bail!("the host sustained slot {slot}, but did not complete both identity profile passes");
    }
    validate_membership_report(
        discovered,
        slot,
        discovered.users.saturating_add(1),
        &report,
    )?;
    Ok(())
}

fn arm_firmware_replies(
    radio: &mut impl RadioBackend,
    discovered: DiscoveredRoom,
    station: MacAddress,
    slot: u8,
    name: &str,
    firmware_assist: bool,
    send_dot: bool,
) -> Result<()> {
    if !firmware_assist {
        return Ok(());
    }
    for (sequence, (template, body)) in firmware_reply_templates(station, slot, name, send_dot)?
        .into_iter()
        .enumerate()
    {
        let sequence = u16::try_from(sequence).context("firmware template sequence overflow")?;
        let bytes = build_firmware_arm_frame(
            station,
            discovered.host,
            sequence.saturating_add(2),
            slot,
            template,
            &body,
        );
        radio.transmit(TransmitFrame { bytes })?;
    }
    println!(
        "    Preloaded slot {slot}'s session, profile, idle{} replies into the AR9271 firmware.",
        if send_dot {
            ", and one-dot message"
        } else {
            ""
        }
    );
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "keeps the radio operation's independent retry and firmware controls explicit"
)]
fn associate_with_firmware_replies(
    radio: &mut impl RadioBackend,
    station: MacAddress,
    discovered: DiscoveredRoom,
    name: &str,
    firmware_assist: bool,
    send_dot: bool,
    attempts: u8,
    response_timeout: Duration,
) -> Result<u8> {
    let predicted_slot = discovered.users;
    arm_firmware_replies(
        radio,
        discovered,
        station,
        predicted_slot,
        name,
        firmware_assist,
        send_dot,
    )?;

    let assigned_slot = associate(radio, station, discovered, attempts, response_timeout)?;
    if firmware_assist && assigned_slot != predicted_slot {
        println!(
            "    Host selected slot {assigned_slot} instead of anticipated slot {predicted_slot}; replacing the firmware templates."
        );
        arm_firmware_replies(
            radio,
            discovered,
            station,
            assigned_slot,
            name,
            firmware_assist,
            send_dot,
        )?;
    }
    Ok(assigned_slot)
}

fn firmware_reply_templates(
    station: MacAddress,
    slot: u8,
    name: &str,
    send_dot: bool,
) -> Result<Vec<(FirmwareReplyTemplate, Vec<u8>)>> {
    let mut templates = vec![
        (FirmwareReplyTemplate::Session, captured_session_response()),
        (
            FirmwareReplyTemplate::InitialAnnouncement,
            captured_profile_announcement(slot),
        ),
        (FirmwareReplyTemplate::Empty, Vec::new()),
        (
            FirmwareReplyTemplate::InitialProfile,
            build_virtual_profile(station, slot, name, IdentityPass::Initial)?,
        ),
        (
            FirmwareReplyTemplate::InitialCompletion,
            captured_profile_completion(slot, IdentityPass::Initial),
        ),
        (
            FirmwareReplyTemplate::RepeatAnnouncement,
            captured_repeat_announcement(slot),
        ),
        (
            FirmwareReplyTemplate::FinalProfile,
            build_virtual_profile(station, slot, name, IdentityPass::Final)?,
        ),
        (
            FirmwareReplyTemplate::FinalCompletion,
            captured_profile_completion(slot, IdentityPass::Final),
        ),
    ];
    if send_dot {
        templates.extend(
            FIRMWARE_MESSAGE_TEMPLATES
                .into_iter()
                .zip(build_dot_message_bodies(station, slot)?),
        );
    }
    templates.push((FirmwareReplyTemplate::Idle, vec![0, 0x80]));
    Ok(templates)
}

fn build_dot_message_bodies(station: MacAddress, slot: u8) -> Result<Vec<Vec<u8>>> {
    let mut pixels = vec![0; DRAWING_WIDTH * 16];
    for y in 7..=9 {
        for x in 127..=129 {
            pixels[y * DRAWING_WIDTH + x] = 1;
        }
    }
    build_message_bodies(station, slot, 3, 0, &pixels)
}

fn build_message_bodies(
    station: MacAddress,
    slot: u8,
    first_frame: u16,
    message_sequence: u16,
    pixels: &[u8],
) -> Result<Vec<Vec<u8>>> {
    let message = encode_message(station, pixels)?;
    let fragments = fragment_payload_starting_at(&message, [0, 0], 160, first_frame)?;

    let mut bodies = Vec::with_capacity(fragments.len() + 2);
    bodies.push(captured_message_announcement(
        slot,
        message.len(),
        message_sequence,
    )?);
    for fragment in &fragments {
        bodies.push(encode_fragment_envelope(
            NintendoFlow::ClientToHost,
            fragment,
        )?);
    }
    bodies.push(captured_message_completion(slot, message_sequence));
    Ok(bodies)
}

fn captured_message_announcement(
    slot: u8,
    message_bytes: usize,
    message_sequence: u16,
) -> Result<Vec<u8>> {
    let message_bytes = u32::try_from(message_bytes).context("message length exceeds 32 bits")?;
    let mut body = vec![0x0a, 0x8d, 0, 0, 0x14, 0, slot, 0, 0xff, 0xff];
    body.extend_from_slice(&message_bytes.to_le_bytes());
    let command_counter = 0x0c_u32.wrapping_add(u32::from(message_sequence).wrapping_mul(3));
    let nonce = 0x75f0_3cca_u32 ^ u32::from(message_sequence).wrapping_mul(0x9e37_79b9);
    let transfer_counter = 5_u16.wrapping_add(message_sequence.wrapping_mul(2));
    body.extend_from_slice(&command_counter.to_le_bytes());
    body.extend_from_slice(&nonce.to_le_bytes());
    body.extend_from_slice(&transfer_counter.to_le_bytes());
    Ok(body)
}

fn captured_message_completion(slot: u8, message_sequence: u16) -> Vec<u8> {
    let mut body = vec![
        0x0a, 0x8d, 3, 0, 0x14, 0, slot, 4, 0xff, 0xff, 0xdd, 0x93, 0x32, 2, 1, 0xff, 0xff, 0xff,
        0xbb, 0xc8, 0x36, 2,
    ];
    let transfer_counter = 6_u16.wrapping_add(message_sequence.wrapping_mul(2));
    body.extend_from_slice(&transfer_counter.to_le_bytes());
    body
}

fn build_firmware_arm_frame(
    station: MacAddress,
    host: MacAddress,
    sequence_number: u16,
    slot: u8,
    template: FirmwareReplyTemplate,
    body: &[u8],
) -> Vec<u8> {
    let mut frame = build_client_reply_frame(station, host, sequence_number, body);
    frame.extend_from_slice(&[template.marker(), FIRMWARE_ARM_SLOT_PREFIX | slot]);
    frame
}

fn build_firmware_arm_message_frame(
    station: MacAddress,
    host: MacAddress,
    sequence_number: u16,
    slot: u8,
    window_slot: usize,
    body: &[u8],
) -> Result<Vec<u8>> {
    if window_slot >= FIRMWARE_MESSAGE_WINDOW_SIZE {
        bail!("firmware message window slot {window_slot} is out of range");
    }
    let mut frame = build_client_reply_frame(station, host, sequence_number, body);
    let marker_slot = u8::try_from(window_slot).context("firmware window slot exceeds 8 bits")?;
    frame.extend_from_slice(&[
        FIRMWARE_ARM_MESSAGE_PREFIX | marker_slot,
        FIRMWARE_ARM_SLOT_PREFIX | slot,
    ]);
    Ok(frame)
}

fn build_firmware_disarm_frame(station: MacAddress, host: MacAddress, slot: u8) -> Vec<u8> {
    let mut frame = build_client_reply_frame(station, host, LEAVE_SEQUENCE, &[0, 0x80]);
    frame.extend_from_slice(&[FIRMWARE_DISARM_MARKER, FIRMWARE_ARM_SLOT_PREFIX | slot]);
    frame
}

fn build_firmware_commit_frame(
    station: MacAddress,
    host: MacAddress,
    sequence_number: u16,
    slot: u8,
    reply_count: u8,
) -> Vec<u8> {
    let mut frame = build_client_reply_frame(station, host, sequence_number, &[reply_count, 0x80]);
    frame.extend_from_slice(&[FIRMWARE_COMMIT_MARKER, FIRMWARE_ARM_SLOT_PREFIX | slot]);
    frame
}

fn leave_room(
    radio: &mut impl RadioBackend,
    discovered: DiscoveredRoom,
    station: MacAddress,
    slot: u8,
    firmware_assist: bool,
) -> Result<()> {
    let disarm_result = if firmware_assist {
        radio.transmit(TransmitFrame {
            bytes: build_firmware_disarm_frame(station, discovered.host, slot),
        })
    } else {
        Ok(())
    };
    let deauthentication_result = radio.transmit(TransmitFrame {
        bytes: build_management_frame(
            12,
            discovered.host,
            station,
            discovered.host,
            LEAVE_SEQUENCE.saturating_add(1),
            &[3, 0],
        ),
    });
    disarm_result.context("failed to disarm firmware replies")?;
    deauthentication_result.context("failed to notify the host that the station left")?;
    println!(
        "    Left Room {} and released slot {slot}.",
        room_name(discovered.room)
    );
    Ok(())
}

fn report_membership(options: &Cli, slot: u8, report: &MembershipReport) {
    if options.firmware_assist {
        println!(
            "\nheld    slot {slot} for {}s · {} targeted poll(s) · {} accepted · {} missed",
            options.hold_seconds,
            report.targeted_polls,
            report.accepted_acknowledgements,
            report.failed_acknowledgements
        );
    } else {
        println!(
            "\nheld    slot {slot} for {}s · {} injected reply/replies · {} accepted · {} missed",
            options.hold_seconds,
            report.replies,
            report.accepted_acknowledgements,
            report.failed_acknowledgements
        );
    }
    if options.firmware_assist {
        println!(
            "firmware {} observed · {} recognized · {} queued · {} completed · {} clean · {} busy",
            report.firmware.observed_polls,
            report.firmware.recognized_polls,
            report.firmware.queued_replies,
            report.firmware.completed_replies,
            report.firmware.clean_completions,
            report.firmware.busy_polls
        );
        if report.firmware.unarmed_polls != 0
            || report.firmware.host_mismatch_polls != 0
            || report.firmware.slot_mismatch_polls != 0
        {
            println!(
                "precheck {} unarmed · {} host mismatch · {} slot mismatch",
                report.firmware.unarmed_polls,
                report.firmware.host_mismatch_polls,
                report.firmware.slot_mismatch_polls
            );
            if let Some(code) = report.firmware.arm_failure_code {
                println!("arm      0x{code:02x} · {}", arm_failure_name(code));
            }
        }
        if let (Some(minimum), Some(average), Some(maximum)) = (
            report.round_trip.minimum_us,
            report.round_trip.average_us(),
            report.round_trip.maximum_us,
        ) {
            println!(
                "air      RX clear→host ACK {minimum}/{average}/{maximum} µs min/avg/max · {} sample(s)",
                report.round_trip.samples
            );
        }
        report_timing("target", "RX clear→reply", report.expected_reply_start);
        report_timing(
            "queued",
            "RX clear→TX queue",
            report.firmware.queue_start_delay,
        );
        report_timing(
            "tx start",
            "RX clear→TX start",
            report.firmware.tx_start_delay,
        );
        report_timing("queue", "queue→TX start", report.firmware.queue_wait);
        report_offset_timing(
            "lateness",
            "TX start versus SIFS",
            report.firmware.tx_lateness,
        );
        report_timing("accepted", "RX clear→host ACK", report.accepted_round_trip);
        report_timing("missed", "RX clear→host ACK", report.failed_round_trip);
        if report.firmware.recognized_polls == 0 {
            println!(
                "warning  no firmware diagnostic markers; reload the current Picto Relay firmware"
            );
        }
    }
    if options.firmware_assist {
        println!(
            "profile firmware-assisted · initial announcement {} · control synchronized {} · final identity {:?} {} · completion {}",
            yes_no(report.initialization.announcement_sent()),
            yes_no(report.initialization.control_synchronized()),
            options.name,
            if report.initialization.profile_sent() {
                "sent"
            } else {
                "not requested"
            },
            yes_no(report.initialization.profile_finalized())
        );
    } else {
        println!("profile disabled · use --firmware-assist for a timing-correct identity exchange");
    }
}

fn discover_room(
    radio: &mut impl RadioBackend,
    requested_room: Option<u8>,
    deadline: Instant,
) -> Result<DiscoveredRoom> {
    receive_until(radio, deadline, |received| {
        let Some(room) = room_from_frame(received)? else {
            return Ok(None);
        };
        Ok((requested_room.is_none() || requested_room == Some(room.room)).then_some(room))
    })?
    .context("no matching PictoChat room beacon was received before the scan timeout")
}

fn authenticate(
    radio: &mut impl RadioBackend,
    station: MacAddress,
    host: MacAddress,
    attempts: u8,
    response_timeout: Duration,
) -> Result<()> {
    for attempt in 0..attempts {
        let frame =
            build_authentication_request(station, host, AUTHENTICATION_SEQUENCE, attempt > 0);
        radio.transmit(TransmitFrame { bytes: frame })?;
        let (response, radio_acknowledged) = observe_management_exchange(
            radio,
            station,
            host,
            Instant::now() + response_timeout,
            |body| match body {
                ManagementBody::Authentication(authentication)
                    if authentication.algorithm == 0 && authentication.sequence == 2 =>
                {
                    Some(ManagementResponse::Authentication(authentication.status))
                }
                _ => None,
            },
        )?;
        match response {
            Some(ManagementResponse::Authentication(0)) => return Ok(()),
            Some(ManagementResponse::Authentication(status)) => {
                bail!("host rejected authentication with status {status}")
            }
            Some(ManagementResponse::Association { .. }) => unreachable!(),
            None => report_management_timeout(attempt, radio_acknowledged),
        }
    }
    bail!("host did not answer authentication after {attempts} attempt(s)")
}

fn associate(
    radio: &mut impl RadioBackend,
    station: MacAddress,
    mut room: DiscoveredRoom,
    attempts: u8,
    response_timeout: Duration,
) -> Result<u8> {
    for attempt in 0..attempts {
        let previous_stream = room.stream_code;
        room = refresh_room(radio, room, Instant::now() + response_timeout)?;
        println!(
            "    Attempt {} synchronized to host beacon · stream 0x{:04x}{}",
            attempt + 1,
            room.stream_code,
            if room.stream_code == previous_stream {
                ""
            } else {
                " (advanced)"
            }
        );
        let frame = build_association_request(
            station,
            room.host,
            room.game_id,
            room.stream_code,
            ASSOCIATION_SEQUENCE,
            attempt > 0,
        );
        radio.transmit(TransmitFrame { bytes: frame })?;
        let (response, radio_acknowledged) = observe_management_exchange(
            radio,
            station,
            room.host,
            Instant::now() + response_timeout,
            |body| match body {
                ManagementBody::AssociationResponse(association) => {
                    Some(ManagementResponse::Association {
                        status: association.status,
                        slot: association.participant_slot(),
                    })
                }
                _ => None,
            },
        )?;
        match response {
            Some(ManagementResponse::Association { status: 0, slot }) => return Ok(slot),
            Some(ManagementResponse::Association { status, .. }) => {
                bail!("host rejected association with status {status}")
            }
            Some(ManagementResponse::Authentication(_)) => unreachable!(),
            None => report_management_timeout(attempt, radio_acknowledged),
        }
    }
    bail!("host did not answer association after {attempts} attempt(s)")
}

fn refresh_room(
    radio: &mut impl RadioBackend,
    discovered: DiscoveredRoom,
    deadline: Instant,
) -> Result<DiscoveredRoom> {
    receive_until(radio, deadline, |received| {
        let Some(room) = room_from_frame(received)? else {
            return Ok(None);
        };
        Ok((room.host == discovered.host && room.room == discovered.room).then_some(room))
    })?
    .with_context(|| {
        format!(
            "Room {} host {} stopped advertising before association",
            room_name(discovered.room),
            discovered.host
        )
    })
}

fn observe_management_exchange(
    radio: &mut impl RadioBackend,
    station: MacAddress,
    host: MacAddress,
    deadline: Instant,
    mut classify: impl FnMut(ManagementBody) -> Option<ManagementResponse>,
) -> Result<(Option<ManagementResponse>, bool)> {
    let mut radio_acknowledged = false;
    while let Some(received) = radio.receive(deadline)? {
        let Ok(frame) = MacFrame::parse(&received.bytes, FcsMode::Absent) else {
            continue;
        };
        if frame.header.frame_control.frame_type() == picto_relay_codec::FrameType::Control
            && frame.header.frame_control.subtype() == 13
            && frame.header.address_1 == station
        {
            radio_acknowledged = true;
            continue;
        }
        if frame.header.address_1 != station || frame.header.address_2 != Some(host) {
            continue;
        }
        let Ok(body) = parse_management(&frame) else {
            continue;
        };
        if let Some(response) = classify(body) {
            return Ok((Some(response), radio_acknowledged));
        }
    }
    Ok((None, radio_acknowledged))
}

fn report_management_timeout(attempt: u8, radio_acknowledged: bool) {
    println!(
        "    Attempt {} timed out · radio ACK {} · retrying…",
        attempt + 1,
        if radio_acknowledged {
            "received"
        } else {
            "not observed"
        }
    );
}

#[derive(Clone, Copy, Debug, Default)]
struct MembershipReport {
    replies: u32,
    targeted_polls: u32,
    accepted_acknowledgements: u32,
    failed_acknowledgements: u32,
    advertised: bool,
    final_users: Option<u8>,
    initialization: InitializationState,
    firmware: FirmwareTransportReport,
    round_trip: RoundTripTiming,
    accepted_round_trip: RoundTripTiming,
    failed_round_trip: RoundTripTiming,
    expected_reply_start: RoundTripTiming,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FirmwareTransportReport {
    observed_polls: u32,
    unarmed_polls: u32,
    arm_failure_code: Option<u8>,
    host_mismatch_polls: u32,
    slot_mismatch_polls: u32,
    recognized_polls: u32,
    queued_replies: u32,
    completed_replies: u32,
    clean_completions: u32,
    busy_polls: u32,
    queue_start_delay: RoundTripTiming,
    queue_wait: RoundTripTiming,
    tx_start_delay: RoundTripTiming,
    tx_lateness: OffsetTiming,
    pending_expected_reply_start_us: Option<u64>,
}

impl FirmwareTransportReport {
    fn observe(
        &mut self,
        antenna: Option<u8>,
        fragment_number: Option<u8>,
        sequence_number: Option<u16>,
        duration: u16,
        expected_reply_start_us: u64,
    ) {
        let antenna_diagnostic = antenna
            .filter(|value| value & FIRMWARE_DIAGNOSTIC_PREFIX_MASK == FIRMWARE_DIAGNOSTIC_PREFIX)
            .map(|value| value & !FIRMWARE_DIAGNOSTIC_PREFIX_MASK);
        let Some(diagnostic) =
            antenna_diagnostic.or_else(|| fragment_number.filter(|&value| value != 0))
        else {
            return;
        };
        self.observed_polls += 1;
        match diagnostic {
            0x02 | 0x04 | 0x05 | 0x06 | 0x09 | 0x0f => {
                self.unarmed_polls += 1;
                self.arm_failure_code = Some(diagnostic);
                return;
            }
            0x0b => {
                self.host_mismatch_polls += 1;
                return;
            }
            0x0c => {
                self.slot_mismatch_polls += 1;
                return;
            }
            _ => {}
        }
        self.recognized_polls += 1;
        let queued = diagnostic & FIRMWARE_DIAGNOSTIC_QUEUED != 0;
        let completed = diagnostic & FIRMWARE_DIAGNOSTIC_COMPLETED != 0;
        self.queued_replies += u32::from(queued);
        self.completed_replies += u32::from(completed);
        self.clean_completions += u32::from(diagnostic & FIRMWARE_DIAGNOSTIC_TX_CLEAN != 0);
        self.busy_polls += u32::from(diagnostic & FIRMWARE_DIAGNOSTIC_BUSY != 0);
        if completed {
            let tx_start_delay_us = u64::from(duration);
            self.tx_start_delay.observe(tx_start_delay_us);
            if let Some(queue_start_delay_us) = sequence_number.map(u64::from) {
                self.queue_start_delay.observe(queue_start_delay_us);
                if let Some(queue_wait_us) = tx_start_delay_us.checked_sub(queue_start_delay_us) {
                    self.queue_wait.observe(queue_wait_us);
                }
            }
            if let Some(expected_us) = self.pending_expected_reply_start_us.take() {
                self.tx_lateness
                    .observe(i64::from(duration) - i64::try_from(expected_us).unwrap_or(i64::MAX));
            }
        }
        if queued {
            self.pending_expected_reply_start_us = Some(expected_reply_start_us);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct OffsetTiming {
    samples: u32,
    total_us: i64,
    minimum_us: Option<i64>,
    maximum_us: Option<i64>,
    within_50_us: u32,
}

impl OffsetTiming {
    fn observe(&mut self, offset_us: i64) {
        self.samples += 1;
        self.total_us = self.total_us.saturating_add(offset_us);
        self.minimum_us = Some(self.minimum_us.map_or(offset_us, |old| old.min(offset_us)));
        self.maximum_us = Some(self.maximum_us.map_or(offset_us, |old| old.max(offset_us)));
        self.within_50_us += u32::from(offset_us.unsigned_abs() <= 50);
    }

    fn average_us(self) -> Option<i64> {
        (self.samples != 0).then(|| self.total_us / i64::from(self.samples))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RoundTripTiming {
    samples: u32,
    total_us: u64,
    minimum_us: Option<u64>,
    maximum_us: Option<u64>,
}

impl RoundTripTiming {
    fn observe(&mut self, elapsed_us: u64) {
        self.samples += 1;
        self.total_us = self.total_us.saturating_add(elapsed_us);
        self.minimum_us = Some(
            self.minimum_us
                .map_or(elapsed_us, |old| old.min(elapsed_us)),
        );
        self.maximum_us = Some(
            self.maximum_us
                .map_or(elapsed_us, |old| old.max(elapsed_us)),
        );
    }

    fn average_us(self) -> Option<u64> {
        (self.samples != 0).then(|| self.total_us / u64::from(self.samples))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum InitializationPhase {
    #[default]
    InitialHostProfile,
    InitialAnnouncementEcho,
    InitialProfileRequest,
    InitialProfileEcho,
    InitialCompletionGate,
    InitialCompletion,
    RepeatAnnouncement,
    FinalHostProfile,
    FinalProfile,
    FinalProfileEcho,
    FinalCompletion,
    Complete,
}

#[derive(Clone, Copy, Debug, Default)]
struct InitializationState {
    phase: InitializationPhase,
}

impl InitializationState {
    const fn announcement_sent(self) -> bool {
        !matches!(self.phase, InitializationPhase::InitialHostProfile)
    }

    const fn control_synchronized(self) -> bool {
        !matches!(
            self.phase,
            InitializationPhase::InitialHostProfile | InitializationPhase::InitialAnnouncementEcho
        )
    }

    const fn profile_sent(self) -> bool {
        matches!(
            self.phase,
            InitializationPhase::FinalProfileEcho
                | InitializationPhase::FinalCompletion
                | InitializationPhase::Complete
        )
    }

    const fn profile_finalized(self) -> bool {
        matches!(self.phase, InitializationPhase::Complete)
    }
}

#[derive(Clone, Copy, Debug)]
struct MembershipConfig<'a> {
    station: MacAddress,
    slot: u8,
    name: &'a str,
    firmware_assist: bool,
    deadline: Instant,
}

#[derive(Clone, Copy, Debug)]
struct PendingPoll {
    tsft_us: Option<u64>,
}

fn maintain_membership(
    radio: &mut impl RadioBackend,
    discovered: DiscoveredRoom,
    config: MembershipConfig<'_>,
) -> Result<MembershipReport> {
    let expected_users = discovered.users.saturating_add(1);
    let participant_mask = 1_u16
        .checked_shl(u32::from(config.slot))
        .context("participant slot cannot be represented in a Nintendo poll mask")?;
    let mut sequence_number = 2_u16;
    let mut report = MembershipReport::default();
    let mut initialization = InitializationState::default();
    let mut pending_poll = None;
    while let Some(received) = radio.receive(config.deadline)? {
        if let Some(room) = room_from_frame(&received)? {
            if room.host == discovered.host && room.room == discovered.room {
                report.final_users = Some(room.users);
                report.advertised |= room.users >= expected_users;
            }
            continue;
        }
        let Ok(frame) = MacFrame::parse(&received.bytes, FcsMode::Absent) else {
            continue;
        };
        let Ok(data) = decode_nintendo_data(&frame) else {
            continue;
        };
        if data.source != discovered.host {
            continue;
        }
        match data.multiplayer {
            MultiplayerHeader::HostCommand { client_mask, .. }
                if data.flow == NintendoFlow::HostToClient
                    && client_mask & participant_mask != 0 =>
            {
                report.targeted_polls += 1;
                let expected_reply_start_us = SIFS_REPLY_TARGET_US;
                report.expected_reply_start.observe(expected_reply_start_us);
                if config.firmware_assist {
                    let sequence_control = frame.header.sequence_control;
                    report.firmware.observe(
                        received.antenna,
                        sequence_control.map(SequenceControl::fragment_number),
                        sequence_control.map(SequenceControl::sequence_number),
                        frame.header.duration,
                        expected_reply_start_us,
                    );
                }
                pending_poll = Some(PendingPoll {
                    tsft_us: received.tsft_us,
                });
                if config.firmware_assist {
                    let _ = initialization_reply(
                        &data.payload,
                        &mut initialization,
                        config.station,
                        config.slot,
                        config.name,
                    )?;
                }
                if !config.firmware_assist {
                    radio.transmit(TransmitFrame {
                        bytes: build_client_reply_frame(
                            config.station,
                            discovered.host,
                            sequence_number,
                            &[0, 0x80],
                        ),
                    })?;
                    sequence_number = sequence_number.wrapping_add(1);
                    report.replies += 1;
                }
            }
            MultiplayerHeader::HostAcknowledgement {
                failed_client_mask, ..
            } if pending_poll.is_some() => {
                let poll = pending_poll
                    .take()
                    .expect("matching acknowledgement requires a pending poll");
                record_host_acknowledgement(
                    &mut report,
                    failed_client_mask,
                    participant_mask,
                    poll.tsft_us,
                    received.tsft_us,
                );
            }
            MultiplayerHeader::ClientReply
            | MultiplayerHeader::HostCommand { .. }
            | MultiplayerHeader::HostAcknowledgement { .. } => {}
        }
    }
    report.initialization = initialization;
    Ok(report)
}

fn validate_membership_report(
    discovered: DiscoveredRoom,
    slot: u8,
    expected_users: u8,
    report: &MembershipReport,
) -> Result<()> {
    if !report.advertised {
        bail!(
            "association succeeded, but Room {} never advertised {expected_users} users",
            room_name(discovered.room)
        );
    }
    if report.targeted_polls == 0 {
        bail!("the host admitted the station but never invited slot {slot} to reply");
    }
    if report
        .final_users
        .is_some_and(|users| users < expected_users)
    {
        bail!(
            "the host briefly admitted slot {slot}, then removed it after {} targeted poll(s): {} accepted, {} missed",
            report.targeted_polls,
            report.accepted_acknowledgements,
            report.failed_acknowledgements
        );
    }
    Ok(())
}

fn record_host_acknowledgement(
    report: &mut MembershipReport,
    failed_client_mask: u16,
    participant_mask: u16,
    poll_tsft: Option<u64>,
    acknowledgement_tsft: Option<u64>,
) {
    let accepted = failed_client_mask & participant_mask == 0;
    let elapsed_us = poll_tsft
        .zip(acknowledgement_tsft)
        .and_then(|(poll, acknowledgement)| acknowledgement.checked_sub(poll));
    if let Some(elapsed_us) = elapsed_us {
        report.round_trip.observe(elapsed_us);
        if accepted {
            report.accepted_round_trip.observe(elapsed_us);
        } else {
            report.failed_round_trip.observe(elapsed_us);
        }
    }
    if accepted {
        report.accepted_acknowledgements += 1;
    } else {
        report.failed_acknowledgements += 1;
    }
}

fn report_timing(label: &str, relationship: &str, timing: RoundTripTiming) {
    if let (Some(minimum), Some(average), Some(maximum)) =
        (timing.minimum_us, timing.average_us(), timing.maximum_us)
    {
        println!(
            "{label:<8} {relationship} {minimum}/{average}/{maximum} µs · {} sample(s)",
            timing.samples
        );
    }
}

fn report_offset_timing(label: &str, relationship: &str, timing: OffsetTiming) {
    if let (Some(minimum), Some(average), Some(maximum)) =
        (timing.minimum_us, timing.average_us(), timing.maximum_us)
    {
        println!(
            "{label:<8} {relationship} {minimum:+}/{average:+}/{maximum:+} µs min/avg/max · {}/{} within ±50 µs",
            timing.within_50_us, timing.samples
        );
    }
}

fn initialization_reply(
    host_payload: &[u8],
    state: &mut InitializationState,
    station: MacAddress,
    slot: u8,
    name: &str,
) -> Result<Vec<u8>> {
    let Ok(envelope) = PictoChatEnvelope::parse(NintendoFlow::HostToClient, host_payload) else {
        return Ok(vec![0, 0x80]);
    };
    let normalized_id = envelope.packet_id & 0x7fff;
    let identity_subtype = envelope
        .is_message_fragment()
        .then(|| PictoChatFragment::parse(&envelope.payload, NintendoFlow::HostToClient))
        .and_then(Result::ok)
        .filter(|fragment| fragment.data.len() == 84)
        .map(|fragment| fragment.data[1]);

    match state.phase {
        InitializationPhase::InitialHostProfile if identity_subtype == Some(0) => {
            state.phase = InitializationPhase::InitialAnnouncementEcho;
            Ok(captured_profile_announcement(slot))
        }
        InitializationPhase::InitialAnnouncementEcho if normalized_id == 0x1d0a => {
            state.phase = InitializationPhase::InitialProfileRequest;
            Ok(Vec::new())
        }
        InitializationPhase::InitialProfileRequest if normalized_id == 0x1d0a => {
            state.phase = InitializationPhase::InitialProfileEcho;
            build_virtual_profile(station, slot, name, IdentityPass::Initial)
        }
        InitializationPhase::InitialProfileEcho if identity_subtype == Some(0) => {
            state.phase = InitializationPhase::InitialCompletionGate;
            Ok(vec![0, 0x80])
        }
        InitializationPhase::InitialCompletionGate if normalized_id == 0x1c34 => {
            state.phase = InitializationPhase::InitialCompletion;
            Ok(Vec::new())
        }
        InitializationPhase::InitialCompletion if normalized_id == 0x1c34 => {
            state.phase = InitializationPhase::RepeatAnnouncement;
            Ok(captured_profile_completion(slot, IdentityPass::Initial))
        }
        InitializationPhase::RepeatAnnouncement if normalized_id == 0x1d0a => {
            state.phase = InitializationPhase::FinalHostProfile;
            Ok(captured_repeat_announcement(slot))
        }
        InitializationPhase::FinalHostProfile if identity_subtype == Some(1) => {
            state.phase = InitializationPhase::FinalProfile;
            Ok(Vec::new())
        }
        InitializationPhase::FinalProfile if normalized_id == 0x1c34 => {
            state.phase = InitializationPhase::FinalProfileEcho;
            build_virtual_profile(station, slot, name, IdentityPass::Final)
        }
        InitializationPhase::FinalProfileEcho if identity_subtype == Some(1) => {
            state.phase = InitializationPhase::FinalCompletion;
            Ok(vec![0, 0x80])
        }
        InitializationPhase::FinalCompletion if normalized_id == 0x1c34 => {
            state.phase = InitializationPhase::Complete;
            Ok(captured_profile_completion(slot, IdentityPass::Final))
        }
        InitializationPhase::InitialHostProfile
        | InitializationPhase::InitialAnnouncementEcho
        | InitializationPhase::InitialProfileRequest
        | InitializationPhase::InitialProfileEcho
        | InitializationPhase::InitialCompletionGate
        | InitializationPhase::InitialCompletion
        | InitializationPhase::RepeatAnnouncement
        | InitializationPhase::FinalHostProfile
        | InitializationPhase::FinalProfile
        | InitializationPhase::FinalProfileEcho
        | InitializationPhase::FinalCompletion
        | InitializationPhase::Complete => Ok(vec![0, 0x80]),
    }
}

fn captured_session_response() -> Vec<u8> {
    let mut body = Vec::with_capacity(108);
    body.extend_from_slice(&[0x34, 0x8c, 0x06, 0, 0x68, 0]);
    body.extend_from_slice(&[0x11, 0x19, 0x4e, 0xbb]);
    body.resize(106, 0);
    body.extend_from_slice(&[1, 0]);
    body
}

fn captured_profile_announcement(slot: u8) -> Vec<u8> {
    vec![
        0x0a, 0x8d, 0, 0, 0x14, 0, slot, 0xd0, 0xff, 0xff, 0x54, 0, 0, 0, 0, 0, 0, 0, 0xd8, 0x26,
        0x61, 0x45, 1, 0,
    ]
}

fn captured_profile_completion(slot: u8, pass: IdentityPass) -> Vec<u8> {
    let (packet_id_high, footer) = match pass {
        IdentityPass::Initial => (0x8d, 2),
        IdentityPass::Final => (0x0d, 4),
    };
    vec![
        0x0a,
        packet_id_high,
        3,
        0,
        0x14,
        0,
        slot,
        4,
        0xff,
        0xff,
        0xdd,
        0x93,
        0x32,
        2,
        1,
        0xff,
        0xff,
        0xff,
        0xbb,
        0xc8,
        0x36,
        2,
        footer,
        0,
    ]
}

fn captured_repeat_announcement(slot: u8) -> Vec<u8> {
    vec![
        0x0a, 0x8d, 0, 0, 0x14, 0, slot, 0, 0xff, 0xff, 0x54, 0, 0, 0, 0x0c, 0, 0, 0, 0xbb, 0x93,
        0x33, 0xd1, 3, 0,
    ]
}

fn build_virtual_profile(
    station: MacAddress,
    slot: u8,
    name: &str,
    pass: IdentityPass,
) -> Result<Vec<u8>> {
    let mut profile = [0_u8; 84];
    let (packet_id_high, subtype, footer) = match pass {
        IdentityPass::Initial => (0x8e, 0, 1),
        IdentityPass::Final => (0x0e, 1, 2),
    };
    profile[..2].copy_from_slice(&[3, subtype]);
    for (word, octets) in profile[2..8]
        .chunks_exact_mut(2)
        .zip(station.0.chunks_exact(2))
    {
        word.copy_from_slice(&[octets[1], octets[0]]);
    }
    for (target, code_unit) in profile[8..28].chunks_exact_mut(2).zip(name.encode_utf16()) {
        target.copy_from_slice(&code_unit.to_le_bytes());
    }
    profile[82..].copy_from_slice(&[3, 2]);

    let mut body = Vec::with_capacity(100);
    body.extend_from_slice(&[0x30, packet_id_high, 2, 0, 0x60, 0]);
    body.extend_from_slice(&[slot, 0x69, 0x54, 1, 0, 0, 0, 0]);
    body.extend_from_slice(&profile);
    body.extend_from_slice(&[footer, 0]);
    if body.len() != 100 {
        bail!("virtual profile encoded to an unexpected size");
    }
    Ok(body)
}

fn receive_until<T>(
    radio: &mut impl RadioBackend,
    deadline: Instant,
    mut inspect: impl FnMut(&ReceivedFrame) -> Result<Option<T>>,
) -> Result<Option<T>> {
    while let Some(received) = radio.receive(deadline)? {
        if let Some(value) = inspect(&received)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn room_from_frame(received: &ReceivedFrame) -> Result<Option<DiscoveredRoom>> {
    let Ok(frame) = MacFrame::parse(&received.bytes, FcsMode::Absent) else {
        return Ok(None);
    };
    let Ok(ManagementBody::Beacon(beacon)) = parse_management(&frame) else {
        return Ok(None);
    };
    let Ok(nintendo) = NintendoBeacon::from_beacon(&beacon) else {
        return Ok(None);
    };
    let Some(pictochat) = nintendo.pictochat()? else {
        return Ok(None);
    };
    let Some(host) = frame.header.address_2 else {
        return Ok(None);
    };
    Ok(Some(DiscoveredRoom {
        host,
        room: pictochat.room,
        users: pictochat.user_count,
        game_id: nintendo.game_id,
        stream_code: nintendo.stream_code,
        rssi_dbm: received.rssi_dbm,
    }))
}

fn prepare_radio_interfaces(interface: &str, channel: u8, station: MacAddress) -> Result<String> {
    validate_radio_interface(interface)?;
    let monitor_interface = format!("{interface}mon");
    if monitor_interface.len() > 15 {
        bail!("derived monitor interface name {monitor_interface:?} is too long");
    }
    if Path::new("/usr/bin/nmcli").exists() {
        run_command(
            "/usr/bin/nmcli",
            &["device", "set", interface, "managed", "no"],
        )?;
    }
    if Path::new("/sys/class/net")
        .join(&monitor_interface)
        .exists()
    {
        run_command("/usr/sbin/iw", &["dev", &monitor_interface, "del"])?;
    }
    run_command("/usr/sbin/ip", &["link", "set", "dev", interface, "down"])?;
    if let Err(error) = run_command(
        "/usr/sbin/iw",
        &["dev", interface, "set", "type", "managed"],
    ) {
        let _ = run_command("/usr/sbin/ip", &["link", "set", "dev", interface, "up"]);
        return Err(error);
    }
    if let Err(error) = run_command(
        "/usr/sbin/ip",
        &[
            "link",
            "set",
            "dev",
            interface,
            "address",
            &station.to_string(),
        ],
    ) {
        let _ = run_command("/usr/sbin/ip", &["link", "set", "dev", interface, "up"]);
        return Err(error);
    }
    run_command(
        "/usr/sbin/iw",
        &[
            "dev",
            interface,
            "interface",
            "add",
            &monitor_interface,
            "type",
            "monitor",
        ],
    )?;
    run_command(
        "/usr/sbin/ip",
        &["link", "set", "dev", &monitor_interface, "up"],
    )?;
    run_command(
        "/usr/sbin/iw",
        &[
            "dev",
            &monitor_interface,
            "set",
            "channel",
            &channel.to_string(),
        ],
    )?;
    run_command("/usr/sbin/ip", &["link", "set", "dev", interface, "up"])?;
    Ok(monitor_interface)
}

fn validate_radio_interface(interface: &str) -> Result<()> {
    let driver_path = fs::read_link(format!("/sys/class/net/{interface}/device/driver"))
        .with_context(|| format!("cannot identify the driver for interface {interface}"))?;
    let driver = driver_path.file_name().and_then(|name| name.to_str());
    if driver != Some("ath9k_htc") {
        bail!(
            "refusing to prepare {interface}: expected the dedicated ath9k_htc adapter, found {}",
            driver.unwrap_or("an unknown driver")
        );
    }

    let default_route = command_output(
        "/usr/sbin/ip",
        &["route", "show", "default", "dev", interface],
    )?;
    if !default_route.trim().is_empty() {
        bail!("refusing to prepare {interface}: it currently carries a default route");
    }

    let Some(peer) = ssh_peer(env::var("SSH_CONNECTION").ok().as_deref()).map(str::to_owned) else {
        return Ok(());
    };
    let peer_route = command_output("/usr/sbin/ip", &["route", "get", &peer])?;
    if route_uses_interface(&peer_route, interface) {
        bail!("refusing to prepare {interface}: the active SSH peer is routed through it");
    }
    Ok(())
}

fn command_output(program: &str, arguments: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .with_context(|| format!("could not execute {program}"))?;
    if !output.status.success() {
        bail!(
            "{} {} failed: {}",
            program,
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn ssh_peer(connection: Option<&str>) -> Option<&str> {
    connection?.split_whitespace().next()
}

fn route_uses_interface(route: &str, interface: &str) -> bool {
    let mut fields = route.split_whitespace();
    while let Some(field) = fields.next() {
        if field == "dev" && fields.next() == Some(interface) {
            return true;
        }
    }
    false
}

fn run_command(program: &str, arguments: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .with_context(|| format!("failed to execute {program}"))?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{} {} failed: {}",
        program,
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

fn parse_room(value: &str) -> std::result::Result<u8, String> {
    match value.trim().to_ascii_uppercase().as_str() {
        "A" => Ok(0),
        "B" => Ok(1),
        "C" => Ok(2),
        "D" => Ok(3),
        _ => Err("room must be A, B, C, or D".into()),
    }
}

fn parse_mac(value: &str) -> std::result::Result<MacAddress, String> {
    let octets = value
        .split(':')
        .map(|octet| u8::from_str_radix(octet, 16).map_err(|_| "invalid MAC address".to_owned()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let octets = <[u8; 6]>::try_from(octets)
        .map_err(|_| "MAC address must contain six octets".to_owned())?;
    if octets[0] & 1 != 0 {
        return Err("station MAC must be unicast".into());
    }
    Ok(MacAddress(octets))
}

fn parse_name(value: &str) -> std::result::Result<String, String> {
    if value.is_empty() {
        return Err("name must not be empty".into());
    }
    if value.encode_utf16().count() > 10 {
        return Err("name must contain at most 10 UTF-16 code units".into());
    }
    if value.chars().any(char::is_control) {
        return Err("name must not contain control characters".into());
    }
    Ok(value.to_owned())
}

const fn yes_no(value: bool) -> &'static str {
    if value { "sent" } else { "not sent" }
}

const fn arm_failure_name(code: u8) -> &'static str {
    match code {
        0x02 => "TX arm function was not called",
        0x04 => "template was shorter than expected",
        0x05 => "template was not a data frame",
        0x06 => "template logical destination did not match",
        0x09 => "private payload marker did not arrive",
        0x0f => "template matched but its TX buffer was not retained",
        _ => "unknown arm failure",
    }
}

const fn room_name(room: u8) -> char {
    match room {
        0 => 'A',
        1 => 'B',
        2 => 'C',
        3 => 'D',
        _ => '?',
    }
}

fn format_rssi(rssi: Option<i8>) -> String {
    rssi.map_or_else(String::new, |rssi| format!(" · {rssi} dBm"))
}

/// Configuration for one long-lived physical `PictoChat` station.
#[derive(Clone, Debug)]
pub struct LiveConfig {
    /// Dedicated `ath9k_htc` interface used for hardware acknowledgements.
    pub interface: String,
    /// Prepare managed and monitor interfaces before opening the radio.
    pub prepare: bool,
    /// 2.4 GHz channel carrying the room.
    pub channel: u8,
    /// Requested room number (`0` through `3`), or the first room found.
    pub room: Option<u8>,
    /// Nintendo-OUI station address advertised by the virtual client.
    pub station: MacAddress,
    /// Maximum time to wait for the room beacon.
    pub scan_timeout: Duration,
    /// Time allowed for each management response.
    pub response_timeout: Duration,
    /// Number of authentication and association attempts.
    pub attempts: u8,
    /// Optional radiotap capture path.
    pub capture: Option<PathBuf>,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            interface: "wlan1".into(),
            prepare: false,
            channel: 1,
            room: Some(0),
            station: MacAddress([0x00, 0x09, 0xbf, 0x52, 0x00, 0x01]),
            scan_timeout: Duration::from_secs(60),
            response_timeout: Duration::from_millis(750),
            attempts: 4,
            capture: None,
        }
    }
}

/// Commands accepted by a live physical station worker.
#[derive(Debug)]
pub enum LiveCommand {
    /// Queue one API-format 256x96 packed monochrome drawing.
    SendMessage { message_id: u32, bitmap: Vec<u8> },
    /// Leave the room and stop the worker.
    Shutdown,
}

/// One station advertised by the active physical `PictoChat` room.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveParticipant {
    /// MAC address carried by the host's member/session roster.
    pub station: MacAddress,
    /// Profile nickname when observed, otherwise a stable MAC-derived label.
    pub name: String,
}

/// Events emitted by a live physical station worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveEvent {
    /// Replaces passive room counts learned from Nintendo beacon advertisements.
    RoomCounts {
        /// A-through-D user counts advertised by the strongest active host per room.
        counts: [u8; PICTOCHAT_ROOM_COUNT],
    },
    /// Reports that the passive room observer stopped and will be retried.
    RoomObserverError {
        /// Radio error that stopped the current observer attempt.
        message: String,
    },
    /// Human-readable progress for the API client and logs.
    State { state: &'static str, detail: String },
    /// The profile handshake completed and the virtual nickname is live.
    Ready {
        host: MacAddress,
        room: u8,
        slot: u8,
    },
    /// Replaces the known physical participant roster for the active room.
    Participants {
        room: u8,
        participants: Vec<LiveParticipant>,
    },
    /// A drawing was rebroadcast by the physical host.
    Message {
        station_id: u32,
        name: String,
        message_id: u32,
        bitmap: Vec<u8>,
    },
    /// The physical host rebroadcast the submitted drawing.
    Delivered { message_id: u32 },
    /// The physical host did not rebroadcast the drawing before the deadline.
    DeliveryTimedOut { message_id: u32 },
    /// A recoverable live-backend command failed.
    Error { code: &'static str, message: String },
    /// The station deauthenticated and released its participant slot.
    Left,
}

/// Prepares the dedicated adapter once and rewrites the configuration to use
/// the resulting monitor interface without repeating interface setup.
///
/// # Errors
///
/// Returns an error when the adapter is not an `ath9k_htc` device or Linux
/// cannot create and configure the monitor interface.
pub fn prepare_live_config(mut config: LiveConfig) -> Result<LiveConfig> {
    if config.prepare {
        config.interface =
            prepare_radio_interfaces(&config.interface, config.channel, config.station)?;
        config.prepare = false;
    }
    Ok(config)
}

/// Observes `PictoChat` room beacons without authenticating or transmitting.
///
/// This blocking function is intended for a dedicated operating-system thread.
/// Sending any value through `shutdown` stops the observer cleanly.
///
/// # Errors
///
/// Returns an error when interface preparation or raw-radio reception fails.
pub fn run_live_room_observer(
    config: &LiveConfig,
    shutdown: &mpsc::Receiver<()>,
    events: &impl Fn(LiveEvent),
) -> Result<()> {
    let radio_interface = if config.prepare {
        prepare_radio_interfaces(&config.interface, config.channel, config.station)?
    } else {
        config.interface.clone()
    };
    let mut radio = LinuxMonitorRadio::open(&radio_interface)?;
    radio.configure(RadioConfig {
        channel: config.channel,
    })?;

    let mut rooms = ObservedRooms::default();
    let mut previous_counts = [0; PICTOCHAT_ROOM_COUNT];
    events(LiveEvent::RoomCounts {
        counts: previous_counts,
    });

    loop {
        match shutdown.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let deadline = Instant::now() + ROOM_OBSERVER_RECEIVE_SLICE;
        if let Some(received) = radio.receive(deadline)?
            && let Some(room) = room_from_frame(&received)?
        {
            rooms.observe(room, received.received_at);
        }
        let counts = rooms.counts(Instant::now());
        if counts != previous_counts {
            previous_counts = counts;
            events(LiveEvent::RoomCounts { counts });
        }
    }
}

#[derive(Debug)]
struct PendingLiveMessage {
    message_id: u32,
    bitmap: Vec<u8>,
}

#[derive(Debug)]
struct ActiveLiveMessage {
    message_id: u32,
    deadline: Instant,
    announcement_prefix: [u8; 16],
    announcement_accepted: bool,
    total_replies: u8,
    accepted_replies: u8,
    queued_attempts: u16,
    failed_attempts: u16,
    awaiting_ack: bool,
    remaining_bodies: VecDeque<Vec<u8>>,
    next_arm_sequence: u16,
    fragment_count: u16,
}

#[derive(Debug, Eq, PartialEq)]
struct MessageWindowRefill {
    window_slot: usize,
    body: Vec<u8>,
}

#[derive(Debug, Default)]
struct IncomingMessages {
    reassembler: MessageReassembler,
    completed: BTreeSet<(MacAddress, [u8; 2], u16)>,
    completed_order: VecDeque<(MacAddress, [u8; 2], u16)>,
}

impl ActiveLiveMessage {
    fn accept_through(&mut self, cursor: u8) -> Vec<MessageWindowRefill> {
        let cursor = cursor.min(self.total_replies);
        let mut refills = Vec::new();
        while self.accepted_replies < cursor {
            let window_slot = usize::from(self.accepted_replies) % FIRMWARE_MESSAGE_WINDOW_SIZE;
            self.accepted_replies = self.accepted_replies.saturating_add(1);
            if let Some(body) = self.remaining_bodies.pop_front() {
                refills.push(MessageWindowRefill { window_slot, body });
            }
        }
        refills
    }

    const fn delivery_confirmed(&self) -> bool {
        self.announcement_accepted && self.accepted_replies >= self.total_replies
    }
}

#[derive(Debug)]
struct LiveRoster {
    members: BTreeMap<u8, MacAddress>,
    names: BTreeMap<MacAddress, String>,
}

impl LiveRoster {
    fn new(host: MacAddress, station: MacAddress, station_slot: u8, station_name: &str) -> Self {
        let mut members = BTreeMap::from([(0, host)]);
        members.insert(station_slot, station);
        Self {
            members,
            names: BTreeMap::from([(station, station_name.to_owned())]),
        }
    }

    fn observe_host_payload(&mut self, host_payload: &[u8]) -> bool {
        let Ok(envelope) = PictoChatEnvelope::parse(NintendoFlow::HostToClient, host_payload)
        else {
            return false;
        };
        match envelope.packet_id & 0x7fff {
            0x1c34 => {
                let Ok(roster) = PictoChatRoster::parse(&envelope.payload) else {
                    return false;
                };
                let members = roster
                    .members
                    .into_iter()
                    .enumerate()
                    .filter_map(|(slot, station)| u8::try_from(slot).ok().zip(station))
                    .collect::<BTreeMap<_, _>>();
                if members == self.members {
                    false
                } else {
                    self.members = members;
                    true
                }
            }
            0x1e30 => {
                let Ok(fragment) =
                    PictoChatFragment::parse(&envelope.payload, NintendoFlow::HostToClient)
                else {
                    return false;
                };
                if !fragment.final_fragment || fragment.data_offset != 0 {
                    return false;
                }
                let Ok(profile) = PictoChatProfile::parse(&fragment.data) else {
                    return false;
                };
                let visible = self
                    .members
                    .values()
                    .any(|&station| station == profile.station);
                let changed = self.names.get(&profile.station) != Some(&profile.nickname);
                self.names.insert(profile.station, profile.nickname);
                visible && changed
            }
            _ => false,
        }
    }

    fn remove(&mut self, station: MacAddress) -> bool {
        let previous_len = self.members.len();
        self.members.retain(|_, member| *member != station);
        self.names.remove(&station);
        self.members.len() != previous_len
    }

    fn participants(&self) -> Vec<LiveParticipant> {
        self.members
            .values()
            .map(|&station| LiveParticipant {
                station,
                name: self.display_name(station),
            })
            .collect()
    }

    fn display_name(&self, station: MacAddress) -> String {
        self.names
            .get(&station)
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or_else(|| format!("DS {station}"))
    }
}

fn departed_station(frame: &MacFrame, host: MacAddress) -> Option<MacAddress> {
    let control = frame.header.frame_control;
    if control.frame_type() != FrameType::Management
        || control.subtype() != 12
        || frame.header.address_3 != Some(host)
        || !frame.body().starts_with(&3_u16.to_le_bytes())
    {
        return None;
    }
    frame.header.address_2.filter(|&station| station != host)
}

fn emit_live_roster(room: u8, roster: &LiveRoster, events: &impl Fn(LiveEvent)) {
    events(LiveEvent::Participants {
        room,
        participants: roster.participants(),
    });
}

/// Runs a blocking AR9271-backed `PictoChat` station until shutdown.
///
/// This function owns the monitor socket and is intended to run on a dedicated
/// operating-system thread, outside an async executor.
///
/// # Errors
///
/// Returns an error when interface preparation, room association, or raw-radio
/// I/O fails. Recoverable per-message failures are emitted as [`LiveEvent`]s.
pub fn run_live_backend(
    config: &LiveConfig,
    name: &str,
    commands: &mpsc::Receiver<LiveCommand>,
    events: &impl Fn(LiveEvent),
) -> Result<()> {
    parse_name(name).map_err(anyhow::Error::msg)?;
    let radio_interface = if config.prepare {
        events(LiveEvent::State {
            state: "preparing_radio",
            detail: format!(
                "preparing {} and its monitor interface on channel {}",
                config.interface, config.channel
            ),
        });
        prepare_radio_interfaces(&config.interface, config.channel, config.station)?
    } else {
        config.interface.clone()
    };
    let mut radio = if let Some(path) = &config.capture {
        LinuxMonitorRadio::open_with_capture(&radio_interface, path)?
    } else {
        LinuxMonitorRadio::open(&radio_interface)?
    };
    radio.configure(RadioConfig {
        channel: config.channel,
    })?;

    events(LiveEvent::State {
        state: "scanning",
        detail: config.room.map_or_else(
            || "scanning for a PictoChat room".into(),
            |room| format!("scanning for Room {}", room_name(room)),
        ),
    });
    let discovered = discover_room(
        &mut radio,
        config.room,
        Instant::now() + config.scan_timeout,
    )?;
    if discovered.users >= 16 {
        bail!("Room {} is full", room_name(discovered.room));
    }
    events(LiveEvent::State {
        state: "authenticating",
        detail: format!(
            "found Room {} hosted by {} with {} user(s)",
            room_name(discovered.room),
            discovered.host,
            discovered.users
        ),
    });
    authenticate(
        &mut radio,
        config.station,
        discovered.host,
        config.attempts,
        config.response_timeout,
    )?;
    events(LiveEvent::State {
        state: "associating",
        detail: "authentication accepted; loading firmware profile templates".into(),
    });
    let slot = associate_with_firmware_replies(
        &mut radio,
        config.station,
        discovered,
        name,
        true,
        false,
        config.attempts,
        config.response_timeout,
    )?;
    events(LiveEvent::State {
        state: "initializing_profile",
        detail: format!("host assigned participant slot {slot}"),
    });

    let live_result = maintain_live_backend(
        &mut radio,
        discovered,
        config.station,
        slot,
        name,
        commands,
        events,
    );
    let leave_result = leave_room(&mut radio, discovered, config.station, slot, true);
    let capture_result = radio.finish_capture();
    events(LiveEvent::Left);
    live_result?;
    leave_result?;
    capture_result?;
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "the blocking radio loop keeps command, timeout, and frame ordering together"
)]
fn maintain_live_backend(
    radio: &mut impl RadioBackend,
    discovered: DiscoveredRoom,
    station: MacAddress,
    slot: u8,
    name: &str,
    commands: &mpsc::Receiver<LiveCommand>,
    events: &impl Fn(LiveEvent),
) -> Result<()> {
    let participant_mask = 1_u16
        .checked_shl(u32::from(slot))
        .context("participant slot cannot be represented in a Nintendo poll mask")?;
    let mut initialization = InitializationState::default();
    let mut ready_reported = false;
    let mut queued_messages = VecDeque::new();
    let mut active_message = None;
    let mut next_message_frame = 3;
    let mut next_message_sequence = 0;
    let mut incoming_messages = IncomingMessages::default();
    let mut roster = LiveRoster::new(discovered.host, station, slot, name);

    loop {
        loop {
            match commands.try_recv() {
                Ok(LiveCommand::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                Err(mpsc::TryRecvError::Empty) => break,
                Ok(LiveCommand::SendMessage { message_id, bitmap }) => {
                    if queued_messages.len() >= LIVE_MESSAGE_QUEUE_CAPACITY {
                        events(LiveEvent::Error {
                            code: "message_queue_full",
                            message: format!(
                                "the physical station already has {LIVE_MESSAGE_QUEUE_CAPACITY} queued drawings"
                            ),
                        });
                    } else if bitmap.len() != API_BITMAP_BYTES {
                        events(LiveEvent::Error {
                            code: "invalid_bitmap_size",
                            message: format!(
                                "bitmap must contain exactly {API_BITMAP_BYTES} bytes, received {}",
                                bitmap.len()
                            ),
                        });
                    } else {
                        queued_messages.push_back(PendingLiveMessage { message_id, bitmap });
                    }
                }
            }
        }

        let pending = if initialization.profile_finalized() && active_message.is_none() {
            queued_messages.pop_front()
        } else {
            None
        };
        if let Some(pending) = pending {
            match arm_live_message(
                radio,
                discovered.host,
                station,
                slot,
                next_message_frame,
                next_message_sequence,
                &pending.bitmap,
            ) {
                Ok(mut message) => {
                    next_message_frame = next_message_frame.wrapping_add(message.fragment_count);
                    next_message_sequence = next_message_sequence.wrapping_add(1);
                    events(LiveEvent::State {
                        state: "sending",
                        detail: format!(
                            "loaded the first {} of {} drawing replies into the AR9271",
                            FIRMWARE_MESSAGE_WINDOW_SIZE.min(usize::from(message.total_replies)),
                            message.total_replies
                        ),
                    });
                    message.message_id = pending.message_id;
                    message.deadline = Instant::now() + LIVE_MESSAGE_TIMEOUT;
                    active_message = Some(message);
                }
                Err(error) => events(LiveEvent::Error {
                    code: "message_rejected",
                    message: error.to_string(),
                }),
            }
        }

        if active_message
            .as_ref()
            .is_some_and(|message| Instant::now() >= message.deadline)
        {
            let message = active_message
                .take()
                .context("active message disappeared before timeout reporting")?;
            events(LiveEvent::DeliveryTimedOut {
                message_id: message.message_id,
            });
        }

        let Some(received) = radio.receive(Instant::now() + LIVE_RECEIVE_SLICE)? else {
            continue;
        };
        let Ok(frame) = MacFrame::parse(&received.bytes, FcsMode::Absent) else {
            continue;
        };
        if let Some(departed) = departed_station(&frame, discovered.host) {
            if roster.remove(departed) {
                emit_live_roster(discovered.room, &roster, events);
            }
            continue;
        }
        let Ok(data) = decode_nintendo_data(&frame) else {
            continue;
        };
        if data.source != discovered.host {
            continue;
        }

        if data.flow == NintendoFlow::HostToClient {
            if roster.observe_host_payload(&data.payload) {
                emit_live_roster(discovered.room, &roster, events);
            }
            observe_live_announcement(&data.payload, &mut active_message);
            receive_live_message(
                &data.payload,
                &mut incoming_messages,
                station,
                &mut active_message,
                &roster,
                events,
            )?;
        }

        match data.multiplayer {
            MultiplayerHeader::HostCommand { client_mask, .. }
                if data.flow == NintendoFlow::HostToClient
                    && client_mask & participant_mask != 0 =>
            {
                let was_ready = initialization.profile_finalized();
                let _ =
                    initialization_reply(&data.payload, &mut initialization, station, slot, name)?;
                if !was_ready && initialization.profile_finalized() && !ready_reported {
                    ready_reported = true;
                    events(LiveEvent::Ready {
                        host: discovered.host,
                        room: discovered.room,
                        slot,
                    });
                }
                if let (Some(cursor), Some(message)) =
                    (firmware_message_cursor(&frame), &mut active_message)
                {
                    for refill in message.accept_through(cursor) {
                        radio.transmit(TransmitFrame {
                            bytes: build_firmware_arm_message_frame(
                                station,
                                discovered.host,
                                message.next_arm_sequence,
                                slot,
                                refill.window_slot,
                                &refill.body,
                            )?,
                        })?;
                        message.next_arm_sequence = message.next_arm_sequence.wrapping_add(1);
                    }
                }
                match (
                    firmware_message_reply_queued(&received, &frame),
                    &mut active_message,
                ) {
                    (true, Some(message)) if message.accepted_replies < message.total_replies => {
                        message.queued_attempts = message.queued_attempts.saturating_add(1);
                        message.awaiting_ack = true;
                    }
                    _ => {}
                }
            }
            MultiplayerHeader::HostAcknowledgement {
                failed_client_mask, ..
            } => {
                if let Some(message) = active_message
                    .as_mut()
                    .filter(|message| message.awaiting_ack)
                {
                    message.awaiting_ack = false;
                    if failed_client_mask & participant_mask != 0 {
                        message.failed_attempts = message.failed_attempts.saturating_add(1);
                    }
                }
            }
            MultiplayerHeader::ClientReply | MultiplayerHeader::HostCommand { .. } => {}
        }

        let delivery_confirmed = active_message
            .as_ref()
            .is_some_and(ActiveLiveMessage::delivery_confirmed);
        if let Some(message) = delivery_confirmed.then(|| active_message.take()).flatten() {
            events(LiveEvent::Delivered {
                message_id: message.message_id,
            });
        }
    }
}

fn arm_live_message(
    radio: &mut impl RadioBackend,
    host: MacAddress,
    station: MacAddress,
    slot: u8,
    first_frame: u16,
    message_sequence: u16,
    bitmap: &[u8],
) -> Result<ActiveLiveMessage> {
    let pixels = api_bitmap_to_native(bitmap)?;
    let mut bodies = VecDeque::from(build_message_bodies(
        station,
        slot,
        first_frame,
        message_sequence,
        &pixels,
    )?);
    let total_replies =
        u8::try_from(bodies.len()).context("message needs more than 255 replies")?;
    let fragment_count = u16::from(total_replies)
        .checked_sub(2)
        .context("message reply stream omitted its framing replies")?;
    let announcement_prefix = bodies
        .front()
        .and_then(|body| body.get(6..22))
        .and_then(|prefix| <[u8; 16]>::try_from(prefix).ok())
        .context("message announcement omitted its correlation prefix")?;
    let initial_count = bodies.len().min(FIRMWARE_MESSAGE_WINDOW_SIZE);
    for window_slot in 0..initial_count {
        let body = bodies
            .pop_front()
            .context("message reply window became unexpectedly empty")?;
        radio.transmit(TransmitFrame {
            bytes: build_firmware_arm_message_frame(
                station,
                host,
                u16::try_from(window_slot).context("message window sequence overflow")? + 0x100,
                slot,
                window_slot,
                &body,
            )?,
        })?;
    }
    radio.transmit(TransmitFrame {
        bytes: build_firmware_commit_frame(
            station,
            host,
            0x100 + u16::try_from(initial_count).context("message window length overflow")?,
            slot,
            total_replies,
        ),
    })?;
    Ok(ActiveLiveMessage {
        message_id: 0,
        deadline: Instant::now(),
        announcement_prefix,
        announcement_accepted: false,
        total_replies,
        accepted_replies: 0,
        queued_attempts: 0,
        failed_attempts: 0,
        awaiting_ack: false,
        remaining_bodies: bodies,
        next_arm_sequence: 0x101
            + u16::try_from(initial_count).context("message window length overflow")?,
        fragment_count,
    })
}

fn observe_live_announcement(host_payload: &[u8], active_message: &mut Option<ActiveLiveMessage>) {
    let Ok(envelope) = PictoChatEnvelope::parse(NintendoFlow::HostToClient, host_payload) else {
        return;
    };
    if envelope.packet_id & 0x7fff != 0x1d0a {
        return;
    }
    let Some(active) = active_message else {
        return;
    };
    let matches_active = envelope
        .payload
        .get(..active.announcement_prefix.len())
        .is_some_and(|prefix| prefix == active.announcement_prefix);
    active.announcement_accepted |= matches_active;
}

fn api_bitmap_to_native(bitmap: &[u8]) -> Result<Vec<u8>> {
    if bitmap.len() != API_BITMAP_BYTES {
        bail!(
            "bitmap must contain exactly {API_BITMAP_BYTES} bytes, received {}",
            bitmap.len()
        );
    }
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for y in 0..API_BITMAP_HEIGHT {
        for x in 0..API_BITMAP_WIDTH {
            if !api_pixel(bitmap, x, y) {
                continue;
            }
            bounds = Some(match bounds {
                Some((min_x, min_y, max_x, max_y)) => {
                    (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                }
                None => (x, y, x, y),
            });
        }
    }
    let Some((min_x, min_y, max_x, max_y)) = bounds else {
        bail!("PictoChat does not support a completely blank drawing");
    };
    if max_y < MAX_NATIVE_DRAWING_HEIGHT {
        let encoded_height = (max_y + 16) / 16 * 16;
        let mut pixels = vec![0; DRAWING_WIDTH * encoded_height];
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                if api_pixel(bitmap, x, y) {
                    pixels[y * DRAWING_WIDTH + x] = 1;
                }
            }
        }
        return Ok(pixels);
    }

    let source_width = max_x - min_x + 1;
    let source_height = max_y - min_y + 1;
    let target_width =
        (source_width * MAX_NATIVE_DRAWING_HEIGHT / source_height).clamp(1, DRAWING_WIDTH);
    let offset_x = (DRAWING_WIDTH - target_width) / 2;
    let mut pixels = vec![0; DRAWING_WIDTH * MAX_NATIVE_DRAWING_HEIGHT];
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if api_pixel(bitmap, x, y) {
                let target_x =
                    offset_x + ((x - min_x) * target_width / source_width).min(target_width - 1);
                let target_y = ((y - min_y) * MAX_NATIVE_DRAWING_HEIGHT / source_height)
                    .min(MAX_NATIVE_DRAWING_HEIGHT - 1);
                pixels[target_y * DRAWING_WIDTH + target_x] = 1;
            }
        }
    }
    Ok(pixels)
}

fn api_pixel(bitmap: &[u8], x: usize, y: usize) -> bool {
    let byte = y * (API_BITMAP_WIDTH / 8) + x / 8;
    bitmap[byte] & (1 << (7 - x % 8)) != 0
}

fn native_message_to_api_bitmap(message: &PictoChatMessage) -> Vec<u8> {
    let mut bitmap = vec![0; API_BITMAP_BYTES];
    for y in 0..DRAWING_HEIGHT {
        for x in 0..DRAWING_WIDTH {
            if message.pixel(x, y).is_some_and(|pixel| pixel != 0) {
                let byte = y * (API_BITMAP_WIDTH / 8) + x / 8;
                bitmap[byte] |= 1 << (7 - x % 8);
            }
        }
    }
    bitmap
}

fn receive_live_message(
    host_payload: &[u8],
    incoming: &mut IncomingMessages,
    station: MacAddress,
    active_message: &mut Option<ActiveLiveMessage>,
    roster: &LiveRoster,
    events: &impl Fn(LiveEvent),
) -> Result<()> {
    let Ok(envelope) = PictoChatEnvelope::parse(NintendoFlow::HostToClient, host_payload) else {
        return Ok(());
    };
    if !envelope.is_message_fragment() {
        return Ok(());
    }
    let Ok(fragment) = PictoChatFragment::parse(&envelope.payload, NintendoFlow::HostToClient)
    else {
        return Ok(());
    };
    let key = ReassemblyKey {
        source: station,
        opaque_id: fragment.opaque_id,
    };
    let Some(payload) = incoming.reassembler.push(key, &fragment)? else {
        return Ok(());
    };
    let Ok(message) = PictoChatMessage::parse(&payload) else {
        return Ok(());
    };
    let sender = message.sender();
    let completion_key = (sender, fragment.opaque_id, fragment.frame_number);
    if !incoming.completed.insert(completion_key) {
        return Ok(());
    }
    incoming.completed_order.push_back(completion_key);
    if let Some(oldest) = (incoming.completed_order.len() > 256)
        .then(|| incoming.completed_order.pop_front())
        .flatten()
    {
        incoming.completed.remove(&oldest);
    }
    let transfer_tag = u32::from(u16::from_le_bytes(fragment.opaque_id));
    let message_id = active_message
        .as_ref()
        .filter(|_| sender == station)
        .map_or(transfer_tag, |active| active.message_id);
    let bitmap = native_message_to_api_bitmap(&message);
    events(LiveEvent::Message {
        station_id: station_id(sender),
        name: roster.display_name(sender),
        message_id,
        bitmap,
    });
    let own_delivery = sender == station
        && active_message
            .as_ref()
            .is_some_and(|active| active.announcement_accepted);
    if let Some(active) = own_delivery.then(|| active_message.take()).flatten() {
        events(LiveEvent::Delivered {
            message_id: active.message_id,
        });
    }
    Ok(())
}

fn firmware_reply_queued(received: &ReceivedFrame, frame: &MacFrame) -> bool {
    firmware_diagnostic(received, frame)
        .is_some_and(|diagnostic| diagnostic & FIRMWARE_DIAGNOSTIC_QUEUED != 0)
}

fn firmware_message_reply_queued(received: &ReceivedFrame, frame: &MacFrame) -> bool {
    firmware_reply_queued(received, frame)
        && firmware_diagnostic(received, frame)
            .is_some_and(|diagnostic| diagnostic & FIRMWARE_DIAGNOSTIC_BUSY != 0)
}

fn firmware_diagnostic(received: &ReceivedFrame, frame: &MacFrame) -> Option<u8> {
    let antenna_diagnostic = received
        .antenna
        .filter(|value| value & FIRMWARE_DIAGNOSTIC_PREFIX_MASK == FIRMWARE_DIAGNOSTIC_PREFIX)
        .map(|value| value & !FIRMWARE_DIAGNOSTIC_PREFIX_MASK);
    let fallback = frame
        .header
        .sequence_control
        .map(SequenceControl::fragment_number)
        .filter(|&value| value != 0);
    antenna_diagnostic.or(fallback)
}

const fn firmware_message_cursor(frame: &MacFrame) -> Option<u8> {
    if frame.header.duration & FIRMWARE_MESSAGE_CURSOR_PREFIX_MASK != FIRMWARE_MESSAGE_CURSOR_PREFIX
    {
        return None;
    }
    Some((frame.header.duration & !FIRMWARE_MESSAGE_CURSOR_PREFIX_MASK) as u8)
}

const fn station_id(mac: MacAddress) -> u32 {
    u32::from_be_bytes([mac.0[2], mac.0[3], mac.0[4], mac.0[5]])
}

#[cfg(test)]
#[path = "../../picto-relay-codec/tests/support/mod.rs"]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use picto_relay_codec::{
        HOST_TO_CLIENT, build_ack_frame, build_data_frame, build_management_frame,
    };
    use picto_relay_radio::RadioError;

    use super::test_support::decode_hex;

    #[derive(Debug)]
    enum TestRadioStep {
        Receive { at: Instant, frame: ReceivedFrame },
        Transmit(Vec<u8>),
    }

    #[derive(Debug)]
    struct TestRadio {
        steps: VecDeque<TestRadioStep>,
    }

    impl TestRadio {
        fn new(steps: impl IntoIterator<Item = TestRadioStep>) -> Self {
            Self {
                steps: steps.into_iter().collect(),
            }
        }

        fn finish(self) {
            assert!(
                self.steps.is_empty(),
                "test radio has {} unconsumed step(s)",
                self.steps.len()
            );
        }
    }

    impl RadioBackend for TestRadio {
        fn configure(&mut self, config: RadioConfig) -> Result<(), RadioError> {
            if !(1..=14).contains(&config.channel) {
                return Err(RadioError::InvalidChannel(config.channel));
            }
            Ok(())
        }

        fn receive(&mut self, deadline: Instant) -> Result<Option<ReceivedFrame>, RadioError> {
            let Some(TestRadioStep::Receive { at, .. }) = self.steps.front() else {
                return Ok(None);
            };
            if *at > deadline {
                return Ok(None);
            }
            let Some(TestRadioStep::Receive { frame, .. }) = self.steps.pop_front() else {
                return Ok(None);
            };
            Ok(Some(frame))
        }

        fn transmit(&mut self, frame: TransmitFrame) -> Result<(), RadioError> {
            let Some(step) = self.steps.pop_front() else {
                return Err(RadioError::Transmit(format!(
                    "unexpected test transmission at end of steps: {:?}",
                    frame.bytes
                )));
            };
            let TestRadioStep::Transmit(expected) = step else {
                return Err(RadioError::Transmit(format!(
                    "unexpected test transmission while receive was pending: {:?}",
                    frame.bytes
                )));
            };
            if frame.bytes == expected {
                Ok(())
            } else {
                Err(RadioError::Transmit(format!(
                    "test transmission mismatch: expected {expected:?}, received {:?}",
                    frame.bytes
                )))
            }
        }
    }

    fn host_envelope(packet_id: u16, payload: &[u8]) -> Vec<u8> {
        let mut body = Vec::with_capacity(6 + payload.len());
        body.extend_from_slice(&packet_id.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(
            &u16::try_from(payload.len())
                .expect("test payload should fit")
                .to_le_bytes(),
        );
        body.extend_from_slice(payload);
        body
    }

    fn host_identity_envelope(subtype: u8) -> Vec<u8> {
        let mut payload = vec![0, 0xff, 84, 1, 0, 0, 0, 0];
        payload.resize(92, 0);
        payload[9] = subtype;
        payload.extend_from_slice(&[1, 0, 2, 0]);
        host_envelope(0x1e30, &payload)
    }

    fn host_roster_envelope(members: &[(u8, MacAddress)]) -> Vec<u8> {
        let mut payload = vec![0; 104];
        for &(slot, station) in members {
            let offset = 4 + usize::from(slot) * 6;
            for (word, octets) in payload[offset..offset + 6]
                .chunks_exact_mut(2)
                .zip(station.0.chunks_exact(2))
            {
                word.copy_from_slice(&[octets[1], octets[0]]);
            }
        }
        host_envelope(0x1c34, &payload)
    }

    fn host_named_profile_envelope(station: MacAddress, name: &str) -> Vec<u8> {
        let mut record = [0_u8; 84];
        record[0] = 3;
        for (word, octets) in record[2..8]
            .chunks_exact_mut(2)
            .zip(station.0.chunks_exact(2))
        {
            word.copy_from_slice(&[octets[1], octets[0]]);
        }
        for (target, unit) in record[8..28].chunks_exact_mut(2).zip(name.encode_utf16()) {
            target.copy_from_slice(&unit.to_le_bytes());
        }
        let mut payload = vec![0, 0xff, 84, 1, 0, 0, 0, 0];
        payload.extend_from_slice(&record);
        payload.extend_from_slice(&[1, 0, 2, 0]);
        host_envelope(0x1e30, &payload)
    }

    #[test]
    fn room_parser_should_accept_case_insensitively() {
        assert_eq!(parse_room("c"), Ok(2));
    }

    #[test]
    fn mac_parser_should_reject_multicast_station() {
        assert!(parse_mac("03:09:bf:00:00:10").is_err());
    }

    #[test]
    fn ssh_peer_should_extract_the_remote_address() {
        assert_eq!(
            ssh_peer(Some("10.0.0.135 53144 10.0.0.231 22")),
            Some("10.0.0.135")
        );
    }

    #[test]
    fn route_should_identify_the_selected_device() {
        assert!(route_uses_interface(
            "10.0.0.135 dev wlan1 src 10.0.0.231 uid 1000",
            "wlan1"
        ));
    }

    #[test]
    fn route_should_not_match_a_device_name_prefix() {
        assert!(!route_uses_interface(
            "10.0.0.135 dev wlan10 src 10.0.0.231 uid 1000",
            "wlan1"
        ));
    }

    #[test]
    fn room_beacon_should_supply_the_runtime_host_bssid() {
        let host = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let mut bytes = decode_hex(include_str!(
            "../../picto-relay-codec/tests/fixtures/room-refresh-a.hex"
        ));
        bytes[10..16].copy_from_slice(&host.octets());
        bytes[16..22].copy_from_slice(&host.octets());
        let received = ReceivedFrame {
            received_at: Instant::now(),
            tsft_us: None,
            bytes,
            rssi_dbm: Some(-40),
            antenna: None,
        };

        let room = room_from_frame(&received)
            .expect("room beacon should decode")
            .expect("fixture should contain a room");

        assert_eq!(room.host, host);
    }

    #[test]
    fn firmware_diagnostics_should_decode_transport_outcomes() {
        let mut report = FirmwareTransportReport::default();
        for (antenna, fragment_number, duration) in [
            (Some(0xd1), None, 0),
            (Some(0xd7), None, 675),
            (Some(0xd8), None, 0),
            (Some(1), Some(1), 0),
            (Some(1), Some(0x0f), 0),
            (Some(1), Some(0x0b), 0),
            (Some(1), Some(0x0c), 0),
            (None, None, 0),
        ] {
            report.observe(antenna, fragment_number, Some(600), duration, 675);
        }
        assert_eq!(
            report,
            FirmwareTransportReport {
                observed_polls: 7,
                unarmed_polls: 1,
                arm_failure_code: Some(0x0f),
                host_mismatch_polls: 1,
                slot_mismatch_polls: 1,
                recognized_polls: 4,
                queued_replies: 3,
                completed_replies: 1,
                clean_completions: 1,
                busy_polls: 1,
                queue_start_delay: RoundTripTiming {
                    samples: 1,
                    total_us: 600,
                    minimum_us: Some(600),
                    maximum_us: Some(600),
                },
                queue_wait: RoundTripTiming {
                    samples: 1,
                    total_us: 75,
                    minimum_us: Some(75),
                    maximum_us: Some(75),
                },
                tx_start_delay: RoundTripTiming {
                    samples: 1,
                    total_us: 675,
                    minimum_us: Some(675),
                    maximum_us: Some(675),
                },
                tx_lateness: OffsetTiming {
                    samples: 1,
                    total_us: 0,
                    minimum_us: Some(0),
                    maximum_us: Some(0),
                    within_50_us: 1,
                },
                pending_expected_reply_start_us: Some(675),
            }
        );
    }

    #[test]
    fn firmware_message_queue_marker_should_exclude_idle_replies() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
        let bytes = build_client_reply_frame(station, host, 1, &[0, 0x80]);
        let frame = MacFrame::parse(&bytes, FcsMode::Absent).expect("frame should parse");
        let mut received = ReceivedFrame {
            received_at: Instant::now(),
            tsft_us: None,
            bytes,
            rssi_dbm: None,
            antenna: Some(0xd1),
        };

        assert!(firmware_reply_queued(&received, &frame));
        assert!(!firmware_message_reply_queued(&received, &frame));
        received.antenna = Some(0xd9);
        assert!(firmware_message_reply_queued(&received, &frame));
    }

    #[test]
    fn firmware_message_cursor_should_decode_cumulative_acceptance() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
        let mut bytes = build_client_reply_frame(station, host, 1, &[0, 0x80]);
        bytes[2..4].copy_from_slice(&0xa52a_u16.to_le_bytes());
        let frame = MacFrame::parse(&bytes, FcsMode::Absent).expect("frame should parse");

        assert_eq!(firmware_message_cursor(&frame), Some(42));
    }

    #[test]
    fn message_window_should_catch_up_after_monitor_frame_loss() {
        let mut message = ActiveLiveMessage {
            message_id: 1,
            deadline: Instant::now(),
            announcement_prefix: [0; 16],
            announcement_accepted: false,
            total_replies: 20,
            accepted_replies: 0,
            queued_attempts: 0,
            failed_attempts: 0,
            awaiting_ack: false,
            remaining_bodies: VecDeque::from([vec![16], vec![17], vec![18], vec![19]]),
            next_arm_sequence: 0,
            fragment_count: 18,
        };

        let refills = message.accept_through(18);

        assert_eq!(
            refills,
            vec![
                MessageWindowRefill {
                    window_slot: 0,
                    body: vec![16],
                },
                MessageWindowRefill {
                    window_slot: 1,
                    body: vec![17],
                },
                MessageWindowRefill {
                    window_slot: 2,
                    body: vec![18],
                },
                MessageWindowRefill {
                    window_slot: 3,
                    body: vec![19],
                },
            ]
        );
    }

    #[test]
    fn message_delivery_should_require_announcement_and_all_acceptances() {
        let mut message = ActiveLiveMessage {
            message_id: 1,
            deadline: Instant::now(),
            announcement_prefix: [0; 16],
            announcement_accepted: true,
            total_replies: 16,
            accepted_replies: 15,
            queued_attempts: 0,
            failed_attempts: 0,
            awaiting_ack: false,
            remaining_bodies: VecDeque::new(),
            next_arm_sequence: 0,
            fragment_count: 14,
        };
        message.accept_through(16);

        assert!(message.delivery_confirmed());
    }

    #[test]
    fn firmware_timing_should_measure_sifs_from_rx_clear() {
        let mut report = FirmwareTransportReport::default();
        report.observe(Some(0xd1), None, Some(0), 0, SIFS_REPLY_TARGET_US);
        report.observe(Some(0xd7), None, Some(15), 20, SIFS_REPLY_TARGET_US);

        assert_eq!(report.tx_lateness.average_us(), Some(9));
    }

    #[test]
    fn firmware_arm_frame_should_encode_template_and_slot_after_wire_payload() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
        let frame =
            build_firmware_arm_frame(station, host, 2, 3, FirmwareReplyTemplate::Idle, &[0, 0x80]);
        assert_eq!(&frame[24..], &[0, 0x80, 0xa0, 0xb3]);
        assert_eq!(u16::from_le_bytes([frame[2], frame[3]]), 1_022);
    }

    #[test]
    fn firmware_disarm_frame_should_encode_private_disarm_marker() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
        let frame = build_firmware_disarm_frame(station, host, 1);

        assert_eq!(&frame[24..], &[0, 0x80, 0xaf, 0xb1]);
        assert_eq!(u16::from_le_bytes([frame[2], frame[3]]), 1_022);
    }

    #[test]
    fn firmware_commit_frame_should_follow_staged_message_templates() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
        let frame = build_firmware_commit_frame(station, host, 0x110, 1, 16);

        assert_eq!(&frame[24..], &[16, 0x80, 0xae, 0xb1]);
    }

    #[test]
    fn live_message_bodies_should_use_zero_tag_and_supplied_frame_counter() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let pixels = vec![0; DRAWING_WIDTH * 16];
        let bodies =
            build_message_bodies(station, 1, 17, 1, &pixels).expect("message bodies should encode");
        let envelope = PictoChatEnvelope::parse(NintendoFlow::ClientToHost, &bodies[1])
            .expect("first fragment envelope should parse");
        let fragment = PictoChatFragment::parse(&envelope.payload, NintendoFlow::ClientToHost)
            .expect("first fragment should parse");

        assert_eq!((fragment.opaque_id, fragment.frame_number), ([0, 0], 17));
    }

    #[test]
    fn five_band_message_should_produce_sixty_seven_reply_bodies() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let mut pixels = vec![0; DRAWING_WIDTH * MAX_NATIVE_DRAWING_HEIGHT];
        pixels[(MAX_NATIVE_DRAWING_HEIGHT - 1) * DRAWING_WIDTH + 200] = 1;

        let bodies =
            build_message_bodies(station, 1, 17, 1, &pixels).expect("message should encode");

        assert_eq!(bodies.len(), 67);
    }

    #[test]
    fn message_controls_should_advance_after_known_good_first_values() {
        let first =
            captured_message_announcement(1, 2_084, 0).expect("first announcement should encode");
        let second =
            captured_message_announcement(1, 2_084, 1).expect("second announcement should encode");
        let second_completion = captured_message_completion(1, 1);

        assert_eq!(
            (&first[14..], &second[14..], &second_completion[22..]),
            (
                &[0x0c, 0, 0, 0, 0xca, 0x3c, 0xf0, 0x75, 5, 0][..],
                &[0x0f, 0, 0, 0, 0x73, 0x45, 0xc7, 0xeb, 7, 0][..],
                &[8, 0][..],
            )
        );
    }

    #[test]
    fn host_announcement_echo_should_mark_active_message_accepted() {
        let mut host_echo =
            captured_message_announcement(1, 2_084, 1).expect("announcement should encode");
        host_echo[1] = 0x1d;
        host_echo.extend_from_slice(&[2, 0]);
        let announcement_prefix = <[u8; 16]>::try_from(&host_echo[6..22])
            .expect("fixture should contain announcement prefix");
        let mut active = Some(ActiveLiveMessage {
            message_id: 1,
            deadline: Instant::now(),
            announcement_prefix,
            announcement_accepted: false,
            total_replies: 16,
            accepted_replies: 0,
            queued_attempts: 0,
            failed_attempts: 0,
            awaiting_ack: false,
            remaining_bodies: VecDeque::new(),
            next_arm_sequence: 0,
            fragment_count: 14,
        });

        observe_live_announcement(&host_echo, &mut active);

        assert!(active.is_some_and(|message| message.announcement_accepted));
    }

    #[test]
    fn mismatched_host_announcement_should_not_mark_active_message_accepted() {
        let mut host_echo =
            captured_message_announcement(1, 2_084, 1).expect("announcement should encode");
        host_echo[1] = 0x1d;
        host_echo.extend_from_slice(&[2, 0]);
        let mut announcement_prefix = <[u8; 16]>::try_from(&host_echo[6..22])
            .expect("fixture should contain announcement prefix");
        announcement_prefix[8] ^= 0xff;
        let mut active = Some(ActiveLiveMessage {
            message_id: 1,
            deadline: Instant::now(),
            announcement_prefix,
            announcement_accepted: false,
            total_replies: 16,
            accepted_replies: 0,
            queued_attempts: 0,
            failed_attempts: 0,
            awaiting_ack: false,
            remaining_bodies: VecDeque::new(),
            next_arm_sequence: 0,
            fragment_count: 14,
        });

        observe_live_announcement(&host_echo, &mut active);

        assert!(active.is_some_and(|message| !message.announcement_accepted));
    }

    #[test]
    fn unrelated_host_announcement_should_not_revoke_acceptance() {
        let mut host_echo =
            captured_message_announcement(1, 2_084, 1).expect("announcement should encode");
        host_echo[1] = 0x1d;
        host_echo.extend_from_slice(&[2, 0]);
        let announcement_prefix = <[u8; 16]>::try_from(&host_echo[6..22])
            .expect("fixture should contain announcement prefix");
        let mut active = Some(ActiveLiveMessage {
            message_id: 1,
            deadline: Instant::now(),
            announcement_prefix,
            announcement_accepted: true,
            total_replies: 16,
            accepted_replies: 0,
            queued_attempts: 0,
            failed_attempts: 0,
            awaiting_ack: false,
            remaining_bodies: VecDeque::new(),
            next_arm_sequence: 0,
            fragment_count: 14,
        });
        host_echo[14] ^= 0xff;

        observe_live_announcement(&host_echo, &mut active);

        assert!(active.is_some_and(|message| message.announcement_accepted));
    }

    #[test]
    fn leave_should_disarm_firmware_before_deauthenticating() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
        let room = DiscoveredRoom {
            host,
            room: 0,
            users: 1,
            game_id: 0x0040_000a,
            stream_code: 3,
            rssi_dbm: Some(-40),
        };
        let mut radio = TestRadio::new([
            TestRadioStep::Transmit(build_firmware_disarm_frame(station, host, 1)),
            TestRadioStep::Transmit(build_management_frame(
                12,
                host,
                station,
                host,
                LEAVE_SEQUENCE.saturating_add(1),
                &[3, 0],
            )),
        ]);

        leave_room(&mut radio, room, station, 1, true).expect("leave should complete");
        radio.finish();
    }

    #[test]
    fn round_trip_timing_should_summarize_air_samples() {
        let mut timing = RoundTripTiming::default();
        timing.observe(998);
        timing.observe(1_002);
        assert_eq!(timing.samples, 2);
        assert_eq!(timing.minimum_us, Some(998));
        assert_eq!(timing.average_us(), Some(1_000));
        assert_eq!(timing.maximum_us, Some(1_002));
    }

    #[test]
    fn host_acknowledgement_should_split_accepted_and_failed_timing() {
        let mut report = MembershipReport::default();
        record_host_acknowledgement(&mut report, 0, 0x0002, Some(1_000), Some(2_200));
        record_host_acknowledgement(&mut report, 0x0002, 0x0002, Some(3_000), Some(4_800));
        assert_eq!(report.accepted_round_trip.average_us(), Some(1_200));
        assert_eq!(report.failed_round_trip.average_us(), Some(1_800));
    }

    #[test]
    fn firmware_replies_should_be_preloaded_before_association() {
        let now = Instant::now();
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
        let room_beacon = decode_hex(include_str!(
            "../../picto-relay-codec/tests/fixtures/room-refresh-a.hex"
        ));
        let fresh_room_beacon = decode_hex(include_str!(
            "../../picto-relay-codec/tests/fixtures/room-refresh-b.hex"
        ));
        let received = |at, bytes| ReceivedFrame {
            received_at: at,
            tsft_us: None,
            bytes,
            rssi_dbm: Some(-40),
            antenna: None,
        };
        let room = room_from_frame(&received(now, room_beacon))
            .expect("room beacon should decode")
            .expect("fixture should contain a room");

        let mut script = firmware_reply_templates(station, 1, "PictoRelay", false)
            .expect("templates should encode")
            .into_iter()
            .enumerate()
            .map(|(sequence, (template, body))| {
                TestRadioStep::Transmit(build_firmware_arm_frame(
                    station,
                    host,
                    u16::try_from(sequence).expect("template sequence should fit") + 2,
                    1,
                    template,
                    &body,
                ))
            })
            .collect::<Vec<_>>();
        script.extend([
            TestRadioStep::Receive {
                at: now + Duration::from_millis(1),
                frame: received(now + Duration::from_millis(1), fresh_room_beacon),
            },
            TestRadioStep::Transmit(build_association_request(
                station,
                host,
                0,
                0x19,
                ASSOCIATION_SEQUENCE,
                false,
            )),
            TestRadioStep::Receive {
                at: now + Duration::from_millis(2),
                frame: received(
                    now + Duration::from_millis(2),
                    build_management_frame(1, station, host, host, 3, &[0x21, 0, 0, 0, 1, 0xc0]),
                ),
            },
        ]);
        let mut radio = TestRadio::new(script);

        let slot = associate_with_firmware_replies(
            &mut radio,
            station,
            room,
            "PictoRelay",
            true,
            false,
            1,
            Duration::from_secs(1),
        )
        .expect("association should complete");

        assert_eq!(slot, 1);
        radio.finish();
    }

    #[test]
    fn probe_should_discover_authenticate_associate_and_confirm() {
        let now = Instant::now();
        let station = MacAddress([0x02, 0x09, 0, 0, 0, 1]);
        let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
        let room_beacon = decode_hex(include_str!(
            "../../picto-relay-codec/tests/fixtures/room-refresh-a.hex"
        ));
        let fresh_room_beacon = decode_hex(include_str!(
            "../../picto-relay-codec/tests/fixtures/room-refresh-b.hex"
        ));
        let joined_beacon = decode_hex(include_str!(
            "../../picto-relay-codec/tests/fixtures/joined-room-a-beacon.hex"
        ));
        let authentication_response =
            build_management_frame(11, station, host, host, 2, &[0, 0, 2, 0, 0, 0]);
        let association_response =
            build_management_frame(1, station, host, host, 3, &[0x21, 0, 0, 0, 1, 0xc0]);
        let host_poll = build_data_frame(
            2,
            HOST_TO_CLIENT,
            host,
            host,
            4,
            false,
            &[0xe6, 0x03, 0x02, 0, 0, 0],
        );
        let host_acknowledgement = build_ack_frame(host, host, 2);
        let expected_authentication =
            build_authentication_request(station, host, AUTHENTICATION_SEQUENCE, false);
        let expected_association =
            build_association_request(station, host, 0, 0x19, ASSOCIATION_SEQUENCE, false);
        let expected_idle_reply = build_client_reply_frame(station, host, 2, &[0, 0x80]);
        let received = |at, bytes| ReceivedFrame {
            received_at: at,
            tsft_us: None,
            bytes,
            rssi_dbm: Some(-40),
            antenna: None,
        };
        let mut radio = TestRadio::new([
            TestRadioStep::Receive {
                at: now,
                frame: received(now, room_beacon),
            },
            TestRadioStep::Transmit(expected_authentication),
            TestRadioStep::Receive {
                at: now + Duration::from_millis(1),
                frame: received(now + Duration::from_millis(1), authentication_response),
            },
            TestRadioStep::Receive {
                at: now + Duration::from_millis(2),
                frame: received(now + Duration::from_millis(2), fresh_room_beacon),
            },
            TestRadioStep::Transmit(expected_association),
            TestRadioStep::Receive {
                at: now + Duration::from_millis(3),
                frame: received(now + Duration::from_millis(3), association_response),
            },
            TestRadioStep::Receive {
                at: now + Duration::from_millis(4),
                frame: received(now + Duration::from_millis(4), host_poll),
            },
            TestRadioStep::Transmit(expected_idle_reply),
            TestRadioStep::Receive {
                at: now + Duration::from_millis(5),
                frame: received(now + Duration::from_millis(4), joined_beacon),
            },
            TestRadioStep::Receive {
                at: now + Duration::from_millis(6),
                frame: received(now + Duration::from_millis(6), host_acknowledgement),
            },
        ]);
        radio
            .configure(RadioConfig { channel: 1 })
            .expect("test radio should configure");
        let room = discover_room(&mut radio, Some(0), now + Duration::from_secs(1))
            .expect("room should be discovered");
        authenticate(&mut radio, station, host, 1, Duration::from_secs(1))
            .expect("authentication should complete");
        let slot = associate(&mut radio, station, room, 1, Duration::from_secs(1))
            .expect("association should complete");
        let report = maintain_membership(
            &mut radio,
            room,
            MembershipConfig {
                station,
                slot,
                name: "PictoRelay",
                firmware_assist: false,
                deadline: now + Duration::from_secs(1),
            },
        )
        .expect("membership should be maintained");
        assert_eq!(slot, 1);
        assert_eq!(report.replies, 1);
        assert_eq!(report.accepted_acknowledgements, 1);
        radio.finish();
    }

    #[test]
    fn virtual_profile_should_encode_station_and_name() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let body = build_virtual_profile(station, 3, "PictoRelay", IdentityPass::Initial)
            .expect("profile should encode");
        assert_eq!(body.len(), 100);
        assert_eq!(body[14], 3);
        assert_eq!(&body[16..22], &[0x09, 0, 0x52, 0xbf, 1, 0]);
        assert_eq!(
            &body[22..42],
            &"PictoRelay"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn live_roster_should_join_c34_slots_to_e30_nicknames() {
        let host = MacAddress([0, 0x24, 0xf3, 0x67, 0x41, 0x87]);
        let physical = MacAddress([0, 0x25, 0xa0, 0xdc, 0x81, 0xdf]);
        let relay = MacAddress([0, 9, 0xbf, 0x52, 0, 1]);
        let mut roster = LiveRoster::new(host, relay, 2, "Relay");
        roster.observe_host_payload(&host_roster_envelope(&[
            (0, host),
            (1, physical),
            (2, relay),
        ]));

        let changed = roster.observe_host_payload(&host_named_profile_envelope(physical, "Rob"));

        assert!(changed);
        assert_eq!(roster.display_name(physical), "Rob");
    }

    #[test]
    fn live_roster_should_remove_a_deauthenticated_station() {
        let host = MacAddress([0, 0x24, 0xf3, 0x67, 0x41, 0x87]);
        let physical = MacAddress([0, 0x25, 0xa0, 0xdc, 0x81, 0xdf]);
        let bytes = build_management_frame(12, host, physical, host, 8, &3_u16.to_le_bytes());
        let frame =
            MacFrame::parse(&bytes, FcsMode::Absent).expect("deauthentication should parse");

        let departed = departed_station(&frame, host);

        assert_eq!(departed, Some(physical));
    }

    #[test]
    fn final_virtual_profile_should_encode_subtype_one() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let body = build_virtual_profile(station, 3, "PictoRelay", IdentityPass::Final)
            .expect("profile should encode");

        assert_eq!(
            (&body[..2], body[15], &body[98..]),
            (&[0x30, 0x0e][..], 1, &[2, 0][..])
        );
    }

    #[test]
    fn firmware_templates_should_end_with_idle_activation_template() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let templates = firmware_reply_templates(station, 3, "PictoRelay", false)
            .expect("templates should encode");

        assert_eq!(templates.len(), 9);
        assert_eq!(templates[0].0, FirmwareReplyTemplate::Session);
        assert_eq!(templates[1].0, FirmwareReplyTemplate::InitialAnnouncement);
        assert_eq!(templates[1].1[6], 3);
        assert_eq!(templates[2], (FirmwareReplyTemplate::Empty, Vec::new()));
        assert_eq!(templates[3].0, FirmwareReplyTemplate::InitialProfile);
        assert_eq!(templates[3].1[14], 3);
        assert_eq!(templates[4].0, FirmwareReplyTemplate::InitialCompletion);
        assert_eq!(templates[5].0, FirmwareReplyTemplate::RepeatAnnouncement);
        assert_eq!(templates[6].0, FirmwareReplyTemplate::FinalProfile);
        assert_eq!(templates[6].1[15], 1);
        assert_eq!(templates[7].0, FirmwareReplyTemplate::FinalCompletion);
        assert_eq!(templates[8], (FirmwareReplyTemplate::Idle, vec![0, 0x80]));
    }

    #[test]
    fn dot_message_should_preload_sixteen_frames_before_idle_activation() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let templates = firmware_reply_templates(station, 2, "PictoRelay", true)
            .expect("templates should encode");

        assert_eq!(templates.len(), 25);
        assert_eq!(templates[8].0, FirmwareReplyTemplate::MessageAnnouncement);
        assert_eq!(templates[8].1[6], 2);
        assert_eq!(&templates[8].1[10..14], &2084_u32.to_le_bytes());
        assert_eq!(templates[9].0, FirmwareReplyTemplate::MessageFragment0);
        assert_eq!(templates[22].0, FirmwareReplyTemplate::MessageFragment13);
        assert_eq!(templates[23].0, FirmwareReplyTemplate::MessageCompletion);
        assert_eq!(templates[24], (FirmwareReplyTemplate::Idle, vec![0, 0x80]));
        assert_eq!(FirmwareReplyTemplate::MessageAnnouncement.marker(), 0xc0);
        assert_eq!(FirmwareReplyTemplate::MessageCompletion.marker(), 0xcf);
    }

    #[test]
    fn initialization_should_complete_both_identity_profile_passes() {
        let station = MacAddress([0x00, 0x09, 0xbf, 0x52, 0, 1]);
        let mut state = InitializationState::default();
        let c34 = host_envelope(0x1c34, &[0; 104]);
        let d0a = host_envelope(0x1d0a, &[0; 20]);
        let initial_identity = host_identity_envelope(0);
        let final_identity = host_identity_envelope(1);

        for host_payload in [
            &c34,
            &c34,
            &d0a,
            &initial_identity,
            &d0a,
            &d0a,
            &initial_identity,
            &c34,
            &c34,
            &d0a,
            &final_identity,
            &final_identity,
            &d0a,
            &c34,
            &final_identity,
            &c34,
        ] {
            initialization_reply(host_payload, &mut state, station, 1, "PictoRelay")
                .expect("identity reply should encode");
        }

        assert_eq!(state.phase, InitializationPhase::Complete);
        assert!(state.profile_finalized());
    }

    #[test]
    fn live_bitmap_should_reject_a_blank_drawing() {
        let bitmap = vec![0; API_BITMAP_BYTES];

        assert!(api_bitmap_to_native(&bitmap).is_err());
    }

    #[test]
    fn live_bitmap_should_preserve_horizontal_position_for_native_height_ink() {
        let mut bitmap = vec![0; API_BITMAP_BYTES];
        let (x, y) = (88, 8);
        let byte = y * (API_BITMAP_WIDTH / 8) + x / 8;
        bitmap[byte] |= 1 << (7 - x % 8);

        let pixels = api_bitmap_to_native(&bitmap).expect("drawing should fit");

        assert_eq!(pixels[y * DRAWING_WIDTH + x], 1);
    }

    #[test]
    fn live_bitmap_should_fit_tall_ink_into_five_native_bands() {
        let mut bitmap = vec![0; API_BITMAP_BYTES];
        for y in 0..API_BITMAP_HEIGHT {
            let x = y * 2;
            let byte = y * (API_BITMAP_WIDTH / 8) + x / 8;
            bitmap[byte] |= 1 << (7 - x % 8);
        }

        let pixels = api_bitmap_to_native(&bitmap).expect("drawing should fit");

        assert_eq!(pixels.len(), DRAWING_WIDTH * MAX_NATIVE_DRAWING_HEIGHT);
        assert!(pixels.iter().any(|&pixel| pixel != 0));
        assert!(
            pixels[((MAX_NATIVE_DRAWING_HEIGHT - 1) * DRAWING_WIDTH)..]
                .iter()
                .any(|&pixel| pixel != 0)
        );
    }

    #[test]
    fn observed_rooms_should_report_the_largest_host_count_per_room() {
        let now = Instant::now();
        let mut rooms = ObservedRooms::default();
        for (host_suffix, users) in [(1, 1), (2, 3)] {
            rooms.observe(
                DiscoveredRoom {
                    host: MacAddress([0, 9, 0xbf, 0x52, 0, host_suffix]),
                    room: 0,
                    users,
                    game_id: 0,
                    stream_code: 0,
                    rssi_dbm: None,
                },
                now,
            );
        }

        assert_eq!(rooms.counts(now), [3, 0, 0, 0]);
    }

    #[test]
    fn observed_rooms_should_expire_stale_beacons() {
        let now = Instant::now();
        let mut rooms = ObservedRooms::default();
        rooms.observe(
            DiscoveredRoom {
                host: MacAddress([0, 9, 0xbf, 0x52, 0, 1]),
                room: 2,
                users: 1,
                game_id: 0,
                stream_code: 0,
                rssi_dbm: None,
            },
            now,
        );

        assert_eq!(
            rooms.counts(now + ROOM_OBSERVER_EXPIRY + Duration::from_millis(1)),
            [0; PICTOCHAT_ROOM_COUNT]
        );
    }
}
