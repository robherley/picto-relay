use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::{
    decode::{DecodeSummary, Decoder},
    output::PrettyPrinter,
    pcap::PcapReader,
    radiotap::Radiotap,
};

pub(crate) struct ReplayResult {
    pub(crate) pcap_path: PathBuf,
    pub(crate) packets: u64,
    pub(crate) bytes: u64,
    pub(crate) decoded: DecodeSummary,
}

pub(crate) fn run(path: &Path, verbose: bool, no_color: bool) -> Result<ReplayResult> {
    let printer = PrettyPrinter::new(no_color);
    printer.banner("replaying", &path.display().to_string());
    println!();
    let mut reader = PcapReader::open(path)?;
    let mut decoder = Decoder::new(printer, verbose);
    let mut packets = 0_u64;
    let mut bytes = 0_u64;
    let mut first_timestamp = None;
    while let Some(packet) = reader.read_packet()? {
        packets += 1;
        bytes += u64::try_from(packet.bytes.len())?;
        let started = *first_timestamp.get_or_insert(packet.timestamp_seconds);
        let elapsed = (packet.timestamp_seconds - started).max(0.0);
        match Radiotap::parse(&packet.bytes) {
            Ok(radiotap) => decoder.inspect(
                elapsed,
                &packet.bytes[radiotap.header_length..],
                radiotap.fcs_mode,
            ),
            Err(_) => decoder.malformed_radiotap(),
        }
    }
    Ok(ReplayResult {
        pcap_path: path.to_owned(),
        packets,
        bytes,
        decoded: decoder.finish(),
    })
}
