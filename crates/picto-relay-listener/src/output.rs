use std::io::{self, IsTerminal};

#[derive(Clone, Copy)]
pub(crate) struct PrettyPrinter {
    color: bool,
}

impl PrettyPrinter {
    pub(crate) fn new(no_color: bool) -> Self {
        Self {
            color: !no_color && io::stdout().is_terminal(),
        }
    }

    pub(crate) fn banner(self, label: &str, value: &str) {
        println!("{}  {value}", self.paint(label, "1;36"));
    }

    pub(crate) fn event(self, elapsed: f64, kind: &str, detail: &str) {
        println!(
            "{}  {}  {detail}",
            self.paint(&format!("{elapsed:>8.3}s"), "2"),
            self.paint(&format!("{kind:<10}"), event_color(kind)),
        );
    }

    pub(crate) fn warning(self, elapsed: f64, detail: &str) {
        self.event(elapsed, "WARNING", detail);
    }

    pub(crate) fn finish(self, detail: &str) {
        println!("{}  {detail}", self.paint("complete", "1;32"));
    }

    fn paint(self, text: &str, style: &str) -> String {
        if self.color {
            format!("\x1b[{style}m{text}\x1b[0m")
        } else {
            text.into()
        }
    }
}

fn event_color(kind: &str) -> &'static str {
    match kind {
        "ROOM" => "1;35",
        "JOIN" | "LEAVE" => "1;34",
        "FRAGMENT" => "1;36",
        "MESSAGE" => "1;32",
        "FAIL" | "WARNING" => "1;31",
        _ => "1",
    }
}
