//! Interactive terminal client for the Picto Relay JSON API.

use std::{io::Write as _, path::Path, time::Duration};

use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::Parser;
use picto_relay_server::protocol::{BITMAP_BYTES, CANVAS_HEIGHT, CANVAS_WIDTH};
use reqwest::{Client, Response};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    time::{MissedTickBehavior, interval},
};

const DEFAULT_URL: &str = "http://127.0.0.1:3001/api/v1";
const DEFAULT_POLL_INTERVAL_MS: u64 = 500;
const NATIVE_DRAWING_HEIGHT: usize = 80;
const NAMETAG_BOTTOM_Y: usize = 16;
const NAMETAG_SAFE_MIN_X: usize = 88;
const FULL_SAFE_MIN_X: usize = 28;
const VISIBLE_TEXT_MAX_X: usize = 244;

#[derive(Clone, Copy, Debug)]
struct TextLayout {
    scale: usize,
    origin_y: usize,
    line_advance: usize,
    max_lines: usize,
}

const LARGE_TEXT: TextLayout = TextLayout {
    scale: 2,
    origin_y: 1,
    line_advance: 16,
    max_lines: 5,
};
const COMPACT_TEXT: TextLayout = TextLayout {
    scale: 1,
    origin_y: 1,
    line_advance: 8,
    max_lines: 10,
};

#[derive(Debug, Parser)]
#[command(
    name = "picto-relay-repl",
    about = "Send and receive PictoChat drawings through the Picto Relay JSON API"
)]
struct Cli {
    /// JSON API base URL.
    #[arg(long, default_value = DEFAULT_URL)]
    url: String,

    /// Virtual station display name.
    #[arg(long, default_value = "Terminal")]
    name: String,

    /// Milliseconds between automatic receive polls.
    #[arg(long, default_value_t = DEFAULT_POLL_INTERVAL_MS, value_parser = clap::value_parser!(u64).range(100..))]
    poll_ms: u64,
}

#[derive(Debug)]
struct ApiClient {
    http: Client,
    base_url: String,
    session_id: u32,
}

impl ApiClient {
    async fn connect(base_url: &str, name: &str) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .context("failed to build HTTP client")?;
        let base_url = base_url.trim_end_matches('/').to_owned();
        let response = http
            .post(format!("{base_url}/sessions"))
            .json(&json!({"name": name}))
            .send()
            .await
            .with_context(|| format!("could not reach {base_url}"))?;
        let body = json_response(response).await?;
        let session_id = body["session_id"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .context("create-session response omitted a valid session_id")?;
        Ok(Self {
            http,
            base_url,
            session_id,
        })
    }

    async fn send_bitmap(&self, bitmap: &[u8]) -> Result<()> {
        if bitmap.len() != BITMAP_BYTES {
            bail!(
                "bitmap must contain exactly {BITMAP_BYTES} bytes, received {}",
                bitmap.len()
            );
        }
        let response = self
            .http
            .post(format!(
                "{}/sessions/{}/messages",
                self.base_url, self.session_id
            ))
            .json(&json!({"bitmap": STANDARD.encode(bitmap)}))
            .send()
            .await
            .context("failed to submit drawing")?;
        successful_response(response).await?;
        Ok(())
    }

    async fn poll_events(&self) -> Result<Vec<Value>> {
        let response = self
            .http
            .get(format!(
                "{}/sessions/{}/events",
                self.base_url, self.session_id
            ))
            .send()
            .await
            .context("failed to poll events")?;
        let body = json_response(response).await?;
        Ok(body["events"].as_array().cloned().unwrap_or_default())
    }

    async fn status(&self) -> Result<Value> {
        let response = self
            .http
            .get(format!("{}/status", self.base_url))
            .send()
            .await
            .context("failed to query gateway status")?;
        json_response(response).await
    }

    async fn disconnect(&self) -> Result<()> {
        let response = self
            .http
            .delete(format!("{}/sessions/{}", self.base_url, self.session_id))
            .send()
            .await
            .context("failed to delete relay session")?;
        successful_response(response).await?;
        Ok(())
    }
}

async fn successful_response(response: Response) -> Result<Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("API returned {status}: {body}")
}

async fn json_response(response: Response) -> Result<Value> {
    let response = successful_response(response).await?;
    response
        .json()
        .await
        .context("API response did not contain valid JSON")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command<'a> {
    Text(&'a str),
    Dot { x: usize, y: usize },
    File(&'a str),
    Poll,
    Status,
    Help,
    Quit,
    Empty,
}

fn parse_command(line: &str) -> Result<Command<'_>> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(Command::Empty);
    }
    if !line.starts_with('/') {
        return Ok(Command::Text(line));
    }
    let mut words = line.split_whitespace();
    let command = words.next().unwrap_or_default();
    match command {
        "/dot" => {
            let x = words
                .next()
                .map(str::parse)
                .transpose()?
                .unwrap_or(CANVAS_WIDTH / 2);
            let y = words
                .next()
                .map(str::parse)
                .transpose()?
                .unwrap_or(CANVAS_HEIGHT / 2);
            if words.next().is_some() {
                bail!("usage: /dot [x y]");
            }
            Ok(Command::Dot { x, y })
        }
        "/file" => {
            let path = line[command.len()..].trim();
            if path.is_empty() {
                bail!("usage: /file PATH");
            }
            Ok(Command::File(path))
        }
        "/poll" => Ok(Command::Poll),
        "/status" => Ok(Command::Status),
        "/help" | "/?" => Ok(Command::Help),
        "/quit" | "/exit" => Ok(Command::Quit),
        _ => bail!("unknown command {command}; use /help"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Cli::parse();
    let api = ApiClient::connect(&options.url, &options.name).await?;
    println!(
        "Picto Relay REPL · session {} · {}\nType a message or /help.",
        api.session_id, options.name
    );

    let run_result = run_repl(&api, Duration::from_millis(options.poll_ms)).await;
    let disconnect_result = api.disconnect().await;
    run_result?;
    disconnect_result.context("REPL stopped but could not delete its relay session")
}

async fn run_repl(api: &ApiClient, poll_interval: Duration) -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut polls = interval(poll_interval);
    polls.set_missed_tick_behavior(MissedTickBehavior::Skip);
    prompt()?;

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line.context("failed to read terminal input")? else {
                    break;
                };
                match parse_command(&line) {
                    Ok(Command::Text(text)) => {
                        match render_text(text) {
                            Ok(bitmap) => {
                                api.send_bitmap(&bitmap).await?;
                                println!("queued text drawing");
                            }
                            Err(error) => eprintln!("error: {error}"),
                        }
                    }
                    Ok(Command::Dot { x, y }) => {
                        api.send_bitmap(&render_dot(x, y)?).await?;
                        println!("queued dot at {x},{y}");
                    }
                    Ok(Command::File(path)) => {
                        let bitmap = tokio::fs::read(Path::new(path))
                            .await
                            .with_context(|| format!("failed to read {path}"))?;
                        api.send_bitmap(&bitmap).await?;
                        println!("queued {}-byte drawing", bitmap.len());
                    }
                    Ok(Command::Poll) => print_events(api.poll_events().await?),
                    Ok(Command::Status) => print_status(&api.status().await?),
                    Ok(Command::Help) => print_help(),
                    Ok(Command::Quit) => break,
                    Ok(Command::Empty) => {}
                    Err(error) => eprintln!("error: {error}"),
                }
                prompt()?;
            }
            _ = polls.tick() => {
                let events = api.poll_events().await?;
                if !events.is_empty() {
                    println!();
                    print_events(events);
                    prompt()?;
                }
            }
            signal = tokio::signal::ctrl_c() => {
                signal.context("failed to listen for Ctrl-C")?;
                println!();
                break;
            }
        }
    }
    Ok(())
}

fn prompt() -> Result<()> {
    print!("> ");
    std::io::stdout().flush().context("failed to flush prompt")
}

fn print_help() {
    println!(
        "Commands:\n  TEXT             render and send typed text\n  /dot [x y]       send a 3x3 dot (default: center)\n  /file PATH       send a raw {BITMAP_BYTES}-byte packed bitmap\n  /poll            receive queued events now\n  /status          show gateway/backend status\n  /help            show this help\n  /quit            delete the session and exit"
    );
}

fn print_status(status: &Value) {
    println!(
        "gateway v{} · backend {} · {}x{} · {} bytes",
        status["version"].as_u64().unwrap_or_default(),
        status["backend"].as_str().unwrap_or("unknown"),
        status["bitmap_width"].as_u64().unwrap_or_default(),
        status["bitmap_height"].as_u64().unwrap_or_default(),
        status["bitmap_bytes"].as_u64().unwrap_or_default()
    );
}

fn print_events(events: Vec<Value>) {
    for event in events {
        match event["type"].as_str().unwrap_or("unknown") {
            "backend_state" => println!(
                "{}: {}",
                event["state"].as_str().unwrap_or("backend"),
                event["detail"].as_str().unwrap_or_default()
            ),
            "session_ready" => println!(
                "joined as session {}",
                event["client_id"].as_u64().unwrap_or_default()
            ),
            "participants" => {
                let names = event["participants"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|participant| participant["name"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("participants: {names}");
            }
            "message" => print_message_event(&event),
            "delivered" => println!(
                "delivered message {}",
                event["message_id"].as_u64().unwrap_or_default()
            ),
            "delivery_timed_out" => println!(
                "delivery timed out for message {}",
                event["message_id"].as_u64().unwrap_or_default()
            ),
            "error" => eprintln!(
                "{}: {}",
                event["code"].as_str().unwrap_or("error"),
                event["message"].as_str().unwrap_or("unknown API error")
            ),
            kind => println!("event {kind}: {event}"),
        }
    }
}

fn print_message_event(event: &Value) {
    let name = event["name"].as_str().unwrap_or("unknown");
    let message_id = event["message_id"].as_u64().unwrap_or_default();
    let Some(encoded) = event["bitmap"].as_str() else {
        println!("message {message_id} from {name} has no bitmap");
        return;
    };
    match STANDARD.decode(encoded) {
        Ok(bitmap) if bitmap.len() == BITMAP_BYTES => {
            println!("message {message_id} from {name}:");
            println!("{}", ascii_preview(&bitmap));
        }
        Ok(bitmap) => println!(
            "message {message_id} from {name}: {} bitmap bytes",
            bitmap.len()
        ),
        Err(error) => println!("message {message_id} from {name}: invalid bitmap ({error})"),
    }
}

fn render_dot(x: usize, y: usize) -> Result<Vec<u8>> {
    if x >= CANVAS_WIDTH || y >= CANVAS_HEIGHT {
        bail!("dot coordinate must be inside 0..{CANVAS_WIDTH}, 0..{CANVAS_HEIGHT}");
    }
    let mut bitmap = vec![0; BITMAP_BYTES];
    for pixel_y in y.saturating_sub(1)..=(y + 1).min(CANVAS_HEIGHT - 1) {
        for pixel_x in x.saturating_sub(1)..=(x + 1).min(CANVAS_WIDTH - 1) {
            set_pixel(&mut bitmap, pixel_x, pixel_y);
        }
    }
    Ok(bitmap)
}

fn render_text(text: &str) -> Result<Vec<u8>> {
    render_text_with_layout(text, LARGE_TEXT)
        .or_else(|_| render_text_with_layout(text, COMPACT_TEXT))
}

fn render_text_with_layout(text: &str, layout: TextLayout) -> Result<Vec<u8>> {
    let last_glyph_row =
        layout.origin_y + (layout.max_lines - 1) * layout.line_advance + 7 * layout.scale;
    if last_glyph_row > NATIVE_DRAWING_HEIGHT {
        bail!("text layout exceeds the native 80-row PictoChat drawing limit");
    }
    let mut bitmap = vec![0; BITMAP_BYTES];
    let glyph_width = 5 * layout.scale;
    let glyph_advance = 6 * layout.scale;
    let mut line = 0;
    let mut x = text_line_origin_x(layout, line);
    for character in text.chars() {
        if character == '\n' || x + glyph_width > VISIBLE_TEXT_MAX_X {
            line += 1;
            x = text_line_origin_x(layout, line);
            if character == '\n' {
                if line >= layout.max_lines {
                    bail!("text exceeds the native 80-row PictoChat drawing limit");
                }
                continue;
            }
        }
        if line >= layout.max_lines {
            bail!("text exceeds the native 80-row PictoChat drawing limit");
        }
        let y = layout.origin_y + line * layout.line_advance;
        draw_glyph(&mut bitmap, x, y, layout.scale, glyph(character));
        x += glyph_advance;
    }
    Ok(bitmap)
}

fn text_line_origin_x(layout: TextLayout, line: usize) -> usize {
    let y = layout.origin_y + line * layout.line_advance;
    if y <= NAMETAG_BOTTOM_Y {
        NAMETAG_SAFE_MIN_X
    } else {
        FULL_SAFE_MIN_X
    }
}

#[cfg(test)]
fn text_line_capacity(layout: TextLayout, line: usize) -> usize {
    let glyph_width = 5 * layout.scale;
    let glyph_advance = 6 * layout.scale;
    (VISIBLE_TEXT_MAX_X - text_line_origin_x(layout, line) - glyph_width) / glyph_advance + 1
}

#[cfg(test)]
fn text_capacity(layout: TextLayout) -> usize {
    (0..layout.max_lines)
        .map(|line| text_line_capacity(layout, line))
        .sum()
}

fn draw_glyph(bitmap: &mut [u8], origin_x: usize, origin_y: usize, scale: usize, rows: [u8; 7]) {
    for (row, bits) in rows.into_iter().enumerate() {
        for column in 0..5 {
            if bits & (1 << (4 - column)) == 0 {
                continue;
            }
            for offset_y in 0..scale {
                for offset_x in 0..scale {
                    set_pixel(
                        bitmap,
                        origin_x + column * scale + offset_x,
                        origin_y + row * scale + offset_y,
                    );
                }
            }
        }
    }
}

fn set_pixel(bitmap: &mut [u8], x: usize, y: usize) {
    let byte = y * (CANVAS_WIDTH / 8) + x / 8;
    bitmap[byte] |= 1 << (7 - x % 8);
}

fn pixel(bitmap: &[u8], x: usize, y: usize) -> bool {
    let byte = y * (CANVAS_WIDTH / 8) + x / 8;
    bitmap[byte] & (1 << (7 - x % 8)) != 0
}

fn ascii_preview(bitmap: &[u8]) -> String {
    const BLOCK: usize = 4;
    let mut rows = Vec::new();
    for block_y in 0..CANVAS_HEIGHT / BLOCK {
        let mut row = String::with_capacity(CANVAS_WIDTH / BLOCK);
        for block_x in 0..CANVAS_WIDTH / BLOCK {
            let ink = (0..BLOCK).any(|offset_y| {
                (0..BLOCK).any(|offset_x| {
                    pixel(
                        bitmap,
                        block_x * BLOCK + offset_x,
                        block_y * BLOCK + offset_y,
                    )
                })
            });
            row.push(if ink { '█' } else { ' ' });
        }
        rows.push(row.trim_end().to_owned());
    }
    while rows.first().is_some_and(String::is_empty) {
        rows.remove(0);
    }
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    if rows.is_empty() {
        "(blank drawing)".into()
    } else {
        rows.join("\n")
    }
}

#[rustfmt::skip]
fn glyph(character: char) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        'A' => [0b01110,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001],
        'B' => [0b11110,0b10001,0b10001,0b11110,0b10001,0b10001,0b11110],
        'C' => [0b01111,0b10000,0b10000,0b10000,0b10000,0b10000,0b01111],
        'D' => [0b11110,0b10001,0b10001,0b10001,0b10001,0b10001,0b11110],
        'E' => [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b11111],
        'F' => [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b10000],
        'G' => [0b01111,0b10000,0b10000,0b10111,0b10001,0b10001,0b01111],
        'H' => [0b10001,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001],
        'I' => [0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b11111],
        'J' => [0b00111,0b00010,0b00010,0b00010,0b10010,0b10010,0b01100],
        'K' => [0b10001,0b10010,0b10100,0b11000,0b10100,0b10010,0b10001],
        'L' => [0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b11111],
        'M' => [0b10001,0b11011,0b10101,0b10101,0b10001,0b10001,0b10001],
        'N' => [0b10001,0b11001,0b10101,0b10011,0b10001,0b10001,0b10001],
        'O' => [0b01110,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
        'P' => [0b11110,0b10001,0b10001,0b11110,0b10000,0b10000,0b10000],
        'Q' => [0b01110,0b10001,0b10001,0b10001,0b10101,0b10010,0b01101],
        'R' => [0b11110,0b10001,0b10001,0b11110,0b10100,0b10010,0b10001],
        'S' => [0b01111,0b10000,0b10000,0b01110,0b00001,0b00001,0b11110],
        'T' => [0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100],
        'U' => [0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110],
        'V' => [0b10001,0b10001,0b10001,0b10001,0b10001,0b01010,0b00100],
        'W' => [0b10001,0b10001,0b10001,0b10101,0b10101,0b10101,0b01010],
        'X' => [0b10001,0b10001,0b01010,0b00100,0b01010,0b10001,0b10001],
        'Y' => [0b10001,0b10001,0b01010,0b00100,0b00100,0b00100,0b00100],
        'Z' => [0b11111,0b00001,0b00010,0b00100,0b01000,0b10000,0b11111],
        '0' => [0b01110,0b10001,0b10011,0b10101,0b11001,0b10001,0b01110],
        '1' => [0b00100,0b01100,0b00100,0b00100,0b00100,0b00100,0b01110],
        '2' => [0b01110,0b10001,0b00001,0b00010,0b00100,0b01000,0b11111],
        '3' => [0b11110,0b00001,0b00001,0b01110,0b00001,0b00001,0b11110],
        '4' => [0b00010,0b00110,0b01010,0b10010,0b11111,0b00010,0b00010],
        '5' => [0b11111,0b10000,0b10000,0b11110,0b00001,0b00001,0b11110],
        '6' => [0b01110,0b10000,0b10000,0b11110,0b10001,0b10001,0b01110],
        '7' => [0b11111,0b00001,0b00010,0b00100,0b01000,0b01000,0b01000],
        '8' => [0b01110,0b10001,0b10001,0b01110,0b10001,0b10001,0b01110],
        '9' => [0b01110,0b10001,0b10001,0b01111,0b00001,0b00001,0b01110],
        ' ' => [0; 7],
        '.' => [0,0,0,0,0,0b01100,0b01100],
        ',' => [0,0,0,0,0b01100,0b01100,0b01000],
        '!' => [0b00100,0b00100,0b00100,0b00100,0b00100,0,0b00100],
        '?' => [0b01110,0b10001,0b00001,0b00010,0b00100,0,0b00100],
        '-' => [0,0,0,0b11111,0,0,0],
        '_' => [0,0,0,0,0,0,0b11111],
        ':' => [0,0b00100,0b00100,0,0b00100,0b00100,0],
        '/' => [0b00001,0b00010,0b00010,0b00100,0b01000,0b01000,0b10000],
        '@' => [0b01110,0b10001,0b10111,0b10101,0b10111,0b10000,0b01111],
        '#' => [0b01010,0b01010,0b11111,0b01010,0b11111,0b01010,0b01010],
        '\'' => [0b00100,0b00100,0b00010,0,0,0,0],
        '(' => [0b00010,0b00100,0b01000,0b01000,0b01000,0b00100,0b00010],
        ')' => [0b01000,0b00100,0b00010,0b00010,0b00010,0b00100,0b01000],
        '+' => [0,0b00100,0b00100,0b11111,0b00100,0b00100,0],
        '=' => [0,0,0b11111,0,0b11111,0,0],
        _ => [0b11111,0b10001,0b10101,0b10101,0b10101,0b10001,0b11111],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_input_should_become_a_text_message() {
        assert_eq!(
            parse_command("hello").expect("command should parse"),
            Command::Text("hello")
        );
    }

    #[test]
    fn dot_should_default_to_the_canvas_center() {
        assert_eq!(
            parse_command("/dot").expect("command should parse"),
            Command::Dot {
                x: CANVAS_WIDTH / 2,
                y: CANVAS_HEIGHT / 2,
            }
        );
    }

    #[test]
    fn rendered_text_should_use_msb_first_bitmap_packing() {
        let bitmap = render_text("L").expect("text should render");
        assert!(pixel(&bitmap, 88, 1));
        assert!(pixel(&bitmap, 88, 14));
        assert!(pixel(&bitmap, 96, 14));
        assert!(!pixel(&bitmap, 98, 1));
    }

    #[test]
    fn long_text_should_stay_inside_calibrated_multiband_safe_area() {
        let bitmap = render_text("ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890")
            .expect("multiband text should render");
        let has_ink_outside_safe_area = (0..CANVAS_HEIGHT).any(|y| {
            (0..CANVAS_WIDTH).any(|x| {
                pixel(&bitmap, x, y)
                    && (y >= NATIVE_DRAWING_HEIGHT
                        || !(FULL_SAFE_MIN_X..VISIBLE_TEXT_MAX_X).contains(&x)
                        || (y <= NAMETAG_BOTTOM_Y && x < NAMETAG_SAFE_MIN_X))
            })
        });

        assert!(!has_ink_outside_safe_area);
    }

    #[test]
    fn long_text_should_use_rows_below_the_first_drawing_band() {
        let bitmap = render_text("ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890")
            .expect("multiband text should render");

        assert!(
            (17..NATIVE_DRAWING_HEIGHT).any(|y| (0..CANVAS_WIDTH).any(|x| pixel(&bitmap, x, y)))
        );
    }

    #[test]
    fn oversized_text_should_return_an_error_instead_of_truncating() {
        let compact_capacity = text_capacity(COMPACT_TEXT);
        let text = "A".repeat(compact_capacity + 1);

        assert!(render_text(&text).is_err());
    }

    #[test]
    fn dot_should_reject_coordinates_outside_the_canvas() {
        assert!(render_dot(CANVAS_WIDTH, 0).is_err());
    }

    #[test]
    fn blank_preview_should_be_compact() {
        assert_eq!(ascii_preview(&vec![0; BITMAP_BYTES]), "(blank drawing)");
    }
}
