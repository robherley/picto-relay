use std::{
    collections::BTreeMap,
    path::PathBuf,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "picto-relay-listen",
    about = "Record and decode a tagged PictoChat scenario from a monitor interface"
)]
pub(crate) struct Cli {
    /// Monitor-mode wireless interface carrying radiotap frames.
    #[arg(short, long, default_value = "wlan1")]
    interface: String,

    /// Put the interface into monitor mode and select --channel before capture.
    #[arg(long)]
    prepare: bool,

    /// 2.4 GHz channel used with --prepare.
    #[arg(short, long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=14))]
    channel: u8,

    /// Short human-readable scenario name, such as "client-one-pixel".
    #[arg(short, long, required_unless_present = "read")]
    scenario: Option<String>,

    /// Replay and decode an existing classic radiotap PCAP instead of capturing live.
    #[arg(long, value_name = "PCAP", conflicts_with_all = ["prepare", "duration"])]
    read: Option<PathBuf>,

    /// Scenario metadata in KEY=VALUE form; repeat for multiple tags.
    #[arg(short, long, value_name = "KEY=VALUE")]
    tag: Vec<Tag>,

    /// Free-form scenario notes stored in the metadata manifest.
    #[arg(long)]
    notes: Option<String>,

    /// Directory receiving the PCAP and JSON manifest.
    #[arg(short, long, default_value = "/home/rob/picto-relay-captures")]
    output: PathBuf,

    /// Stop automatically after this many seconds; otherwise use Ctrl-C.
    #[arg(short, long, value_name = "SECONDS")]
    duration: Option<u64>,

    /// Print every valid application envelope instead of only significant events.
    #[arg(short, long)]
    verbose: bool,

    /// Disable ANSI styling even when stdout is a terminal.
    #[arg(long)]
    no_color: bool,
}

#[derive(Clone, Debug)]
struct Tag {
    key: String,
    value: String,
}

impl FromStr for Tag {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((key, tag_value)) = value.split_once('=') else {
            return Err("tag must use KEY=VALUE syntax".into());
        };
        let key = key.trim();
        let tag_value = tag_value.trim();
        if key.is_empty() || tag_value.is_empty() {
            return Err("tag key and value must both be non-empty".into());
        }
        if !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("tag key may contain only letters, numbers, '-' and '_'".into());
        }
        Ok(Self {
            key: key.into(),
            value: tag_value.into(),
        })
    }
}

#[derive(Debug)]
pub(crate) struct CaptureOptions {
    pub(crate) read: Option<PathBuf>,
    pub(crate) interface: String,
    pub(crate) prepare: bool,
    pub(crate) channel: u8,
    pub(crate) scenario: String,
    pub(crate) slug: String,
    pub(crate) tags: BTreeMap<String, String>,
    pub(crate) notes: Option<String>,
    pub(crate) output: PathBuf,
    pub(crate) duration: Option<Duration>,
    pub(crate) verbose: bool,
    pub(crate) no_color: bool,
    pub(crate) started_unix_ms: u64,
}

impl Cli {
    pub(crate) fn into_options(self) -> Result<CaptureOptions> {
        let scenario = self.scenario.as_deref().unwrap_or("offline-replay").trim();
        if scenario.is_empty() {
            bail!("scenario name cannot be empty");
        }
        let slug = scenario_slug(scenario);
        if slug.is_empty() {
            bail!("scenario name must contain at least one letter or number");
        }
        let tags = self
            .tag
            .into_iter()
            .map(|tag| (tag.key, tag.value))
            .collect();
        let started_unix_ms = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("system clock is before the Unix epoch")?
                .as_millis(),
        )
        .context("system timestamp does not fit in 64 bits")?;
        Ok(CaptureOptions {
            read: self.read,
            interface: self.interface,
            prepare: self.prepare,
            channel: self.channel,
            scenario: scenario.into(),
            slug,
            tags,
            notes: self.notes,
            output: self.output,
            duration: self.duration.map(Duration::from_secs),
            verbose: self.verbose,
            no_color: self.no_color,
            started_unix_ms,
        })
    }
}

fn scenario_slug(scenario: &str) -> String {
    scenario
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .fold(String::new(), |mut slug, character| {
            if character != '-' || !slug.ends_with('-') {
                slug.push(character);
            }
            slug
        })
        .trim_matches('-')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_slug_should_normalize_for_file_name() {
        assert_eq!(scenario_slug(" Client: one pixel! "), "client-one-pixel");
    }

    #[test]
    fn tag_should_reject_path_like_key() {
        assert!("../kind=value".parse::<Tag>().is_err());
    }
}
