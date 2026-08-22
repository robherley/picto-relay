use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use pnet::datalink::{self, Channel};
use serde::Serialize;

use crate::{
    cli::CaptureOptions,
    decode::{DecodeSummary, Decoder},
    output::PrettyPrinter,
    pcap::PcapWriter,
    radiotap::Radiotap,
};

const READ_TIMEOUT: Duration = Duration::from_millis(250);
const READ_BUFFER_SIZE: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct CaptureResult {
    pub(crate) pcap_path: PathBuf,
    pub(crate) metadata_path: PathBuf,
    pub(crate) packets: u64,
    pub(crate) bytes: u64,
    pub(crate) decoded: DecodeSummary,
}

#[derive(Serialize)]
struct ScenarioManifest<'a> {
    format_version: u8,
    status: &'a str,
    scenario: &'a str,
    interface: &'a str,
    monitor_prepared: bool,
    channel: Option<u8>,
    tags: &'a std::collections::BTreeMap<String, String>,
    notes: &'a Option<String>,
    started_unix_ms: u64,
    ended_unix_ms: u64,
    pcap_file: String,
    packets: u64,
    captured_bytes: u64,
    decoded: &'a DecodeSummary,
    error: Option<&'a str>,
}

#[expect(
    clippy::too_many_lines,
    reason = "the capture lifecycle is kept linear so every exit flushes PCAP and metadata"
)]
pub(crate) fn run(options: &CaptureOptions, running: &AtomicBool) -> Result<CaptureResult> {
    fs::create_dir_all(&options.output).with_context(|| {
        format!(
            "failed to create capture directory {}",
            options.output.display()
        )
    })?;
    let base_name = format!("{}-{}", options.started_unix_ms, options.slug);
    let pcap_path = options.output.join(format!("{base_name}.pcap"));
    let metadata_path = options.output.join(format!("{base_name}.json"));
    let printer = PrettyPrinter::new(options.no_color);
    printer.banner("scenario", &options.scenario);
    printer.banner("interface", &options.interface);
    if options.prepare {
        printer.banner(
            "radio",
            &format!("prepare monitor mode · channel {}", options.channel),
        );
    }
    printer.banner("recording", &pcap_path.display().to_string());
    if !options.tags.is_empty() {
        let tags = options
            .tags
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" · ");
        printer.banner("tags", &tags);
    }
    println!();

    if options.prepare {
        prepare_monitor_interface(&options.interface, options.channel)?;
    }

    let interface = datalink::interfaces()
        .into_iter()
        .find(|interface| interface.name == options.interface)
        .with_context(|| {
            let names = datalink::interfaces()
                .into_iter()
                .map(|interface| interface.name)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "interface '{}' was not found; available interfaces: {names}",
                options.interface
            )
        })?;
    let config = datalink::Config {
        read_timeout: Some(READ_TIMEOUT),
        read_buffer_size: READ_BUFFER_SIZE,
        promiscuous: true,
        ..datalink::Config::default()
    };
    let Channel::Ethernet(_sender, mut receiver) = datalink::channel(&interface, config)
        .with_context(|| format!("failed to open raw capture on {}", options.interface))?
    else {
        bail!(
            "interface {} returned an unsupported channel type",
            options.interface
        );
    };

    let mut writer = PcapWriter::create(&pcap_path)?;
    let mut decoder = Decoder::new(printer, options.verbose);
    let started = Instant::now();
    let mut capture_error = None;
    while running.load(Ordering::Relaxed) {
        match receiver.next() {
            Ok(packet) => {
                let captured_at = SystemTime::now();
                writer.write_packet(captured_at, packet)?;
                let elapsed = started.elapsed().as_secs_f64();
                match Radiotap::parse(packet) {
                    Ok(radiotap) => decoder.inspect(
                        elapsed,
                        &packet[radiotap.header_length..],
                        radiotap.fcs_mode,
                    ),
                    Err(_) => decoder.malformed_radiotap(),
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(error) => {
                capture_error = Some(format!("capture receive failed: {error}"));
                break;
            }
        }
    }

    let (packets, bytes) = writer.finish()?;
    let decode_summary = decoder.finish();
    let ended_unix_ms = unix_time_ms()?;
    let pcap_file = pcap_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("capture file name is not valid UTF-8")?
        .to_owned();
    let manifest = ScenarioManifest {
        format_version: 1,
        status: if capture_error.is_some() {
            "failed"
        } else {
            "complete"
        },
        scenario: &options.scenario,
        interface: &options.interface,
        monitor_prepared: options.prepare,
        channel: options.prepare.then_some(options.channel),
        tags: &options.tags,
        notes: &options.notes,
        started_unix_ms: options.started_unix_ms,
        ended_unix_ms,
        pcap_file,
        packets,
        captured_bytes: bytes,
        decoded: &decode_summary,
        error: capture_error.as_deref(),
    };
    let metadata = serde_json::to_vec_pretty(&manifest)?;
    fs::write(&metadata_path, metadata)
        .with_context(|| format!("failed to write metadata {}", metadata_path.display()))?;

    if let Some(error) = capture_error {
        return Err(anyhow!(error));
    }
    Ok(CaptureResult {
        pcap_path,
        metadata_path,
        packets,
        bytes,
        decoded: decode_summary,
    })
}

fn unix_time_ms() -> Result<u64> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the Unix epoch")?
            .as_millis(),
    )
    .context("system timestamp does not fit in 64 bits")
}

fn prepare_monitor_interface(interface: &str, channel: u8) -> Result<()> {
    run_command("/usr/sbin/ip", &["link", "set", "dev", interface, "down"])?;
    if let Err(error) = run_command(
        "/usr/sbin/iw",
        &["dev", interface, "set", "type", "monitor"],
    ) {
        let _ = run_command("/usr/sbin/ip", &["link", "set", "dev", interface, "up"]);
        return Err(error);
    }
    run_command("/usr/sbin/ip", &["link", "set", "dev", interface, "up"])?;
    run_command(
        "/usr/sbin/iw",
        &["dev", interface, "set", "channel", &channel.to_string()],
    )
}

fn run_command(program: &str, arguments: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .with_context(|| format!("failed to execute {program}"))?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    bail!(
        "{} {} failed: {}",
        program,
        arguments.join(" "),
        message.trim()
    );
}
