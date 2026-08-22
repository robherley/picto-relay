mod capture;
mod cli;
mod decode;
mod output;
mod pcap;
mod radiotap;
mod replay;

use std::{
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result};
use clap::Parser;

use crate::{cli::Cli, output::PrettyPrinter};

#[tokio::main]
async fn main() -> Result<()> {
    let options = Cli::parse().into_options()?;
    if let Some(path) = &options.read {
        let replayed = replay::run(path, options.verbose, options.no_color)?;
        let printer = PrettyPrinter::new(options.no_color);
        println!();
        printer.finish(&format!(
            "{} packets · {} bytes · {} Nintendo frames · {} completed message(s)",
            replayed.packets,
            replayed.bytes,
            replayed.decoded.nintendo_frames,
            replayed.decoded.completed_messages
        ));
        printer.banner("pcap", &replayed.pcap_path.display().to_string());
        return Ok(());
    }
    let duration = options.duration;
    let printer = PrettyPrinter::new(options.no_color);
    let running = Arc::new(AtomicBool::new(true));
    let capture_running = Arc::clone(&running);
    let mut task =
        tokio::task::spawn_blocking(move || capture::run(&options, capture_running.as_ref()));

    tokio::select! {
        result = &mut task => return report_result(printer, result),
        signal = tokio::signal::ctrl_c() => {
            signal.context("failed to install Ctrl-C handler")?;
            printer.banner("stopping", "Ctrl-C received; flushing capture");
        }
        () = async {
            if let Some(duration) = duration {
                tokio::time::sleep(duration).await;
            } else {
                pending::<()>().await;
            }
        } => {
            printer.banner("stopping", "duration reached; flushing capture");
        }
    }
    running.store(false, Ordering::Relaxed);
    report_result(printer, task.await)
}

fn report_result(
    printer: PrettyPrinter,
    result: std::result::Result<anyhow::Result<capture::CaptureResult>, tokio::task::JoinError>,
) -> Result<()> {
    let captured = result.context("capture worker panicked")??;
    println!();
    printer.finish(&format!(
        "{} packets · {} bytes · {} Nintendo frames · {} completed message(s)",
        captured.packets,
        captured.bytes,
        captured.decoded.nintendo_frames,
        captured.decoded.completed_messages
    ));
    printer.banner("pcap", &captured.pcap_path.display().to_string());
    printer.banner("metadata", &captured.metadata_path.display().to_string());
    Ok(())
}
