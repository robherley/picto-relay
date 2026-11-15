//! Opt-in physical delivery benchmark. Stop the server before running this test.
use anyhow::{Context, Result, bail, ensure};
use picto_relay_server::{
    broker::{BrokerHandle, RelaySession},
    protocol::SessionEvent,
};
use std::time::{Duration, Instant};

fn setting<T: std::str::FromStr>(name: &str, default: T) -> Result<T> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid {name}: {value}")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

fn drawing(dense: bool) -> Vec<u8> {
    let mut bitmap = vec![0; 3072];
    for y in 0..80 {
        for x in 0..256 {
            let ink = if dense {
                (x + y) % 2 == 0
            } else {
                x == 0 || x == 255 || y == 0 || y == 79 || x == y * 3 || x == 255 - y * 3
            };
            if ink {
                bitmap[y * 32 + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    bitmap
}

async fn wait_for(session: &mut RelaySession, ready: bool) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(if ready { 30 } else { 45 }), async {
        while let Some(envelope) = session.events.recv().await {
            match envelope {
                SessionEvent::SessionReady { .. } if ready => return Ok(()),
                SessionEvent::Delivered { message_id } if !ready => {
                    println!("delivered message={message_id}");
                    return Ok(());
                }
                SessionEvent::DeliveryTimedOut { message_id } => {
                    bail!("delivery timed out: {message_id}")
                }
                SessionEvent::Error { code, message } => bail!("{code}: {message}"),
                SessionEvent::BackendState {
                    state: "reconnecting",
                    detail,
                } => bail!("session interrupted: {detail}"),
                _ => {}
            }
        }
        bail!("session event stream closed")
    })
    .await
    .context("hardware benchmark deadline exceeded")?
}

#[tokio::test]
#[ignore = "requires ESP32 and DS; stop server, then run just benchmark"]
async fn hardware_drawing_delivery() -> Result<()> {
    let port = std::env::var("PICTO_SERIAL_PORT")
        .context("set PICTO_SERIAL_PORT explicitly (or use just benchmark)")?;
    let room = setting("PICTO_BENCH_ROOM", 0u8)?;
    let trials = setting("PICTO_BENCH_TRIALS", 3u32)?;
    let pattern = std::env::var("PICTO_BENCH_PATTERN").unwrap_or_else(|_| "cross".into());
    ensure!(room < 4 && trials > 0, "invalid room or trial count");
    ensure!(
        matches!(pattern.as_str(), "cross" | "dense"),
        "pattern must be cross or dense"
    );
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "picto_relay_server=info".into()),
        )
        .with_test_writer()
        .try_init();
    let broker = BrokerHandle::spawn_esp32(port, None);
    let mut session = broker.register("Benchmark".into(), room).await?;
    let result: Result<()> = async {
        wait_for(&mut session, true).await?;
        let bitmap = drawing(pattern == "dense");
        for trial in 1..=trials {
            let start = Instant::now();
            broker.send_message(session.client_id, bitmap.clone())?;
            wait_for(&mut session, false).await?;
            println!(
                "trial={trial} pattern={pattern} submission_to_delivery_ms={}",
                start.elapsed().as_millis()
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(())
    }
    .await;
    // Release the station on both success and benchmark errors.
    broker.unregister(session.client_id);
    drop(broker);
    tokio::time::timeout(Duration::from_secs(10), async {
        while session.events.recv().await.is_some() {}
    })
    .await
    .context("session cleanup timed out")?;
    result
}
