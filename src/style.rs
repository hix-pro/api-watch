use std::io::IsTerminal;
use std::sync::OnceLock;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if std::env::var("CLICOLOR").ok().as_deref() == Some("0") {
            return false;
        }
        if std::env::var("TERM").ok().as_deref() == Some("dumb") {
            return false;
        }
        std::io::stdout().is_terminal()
    })
}

fn paint(text: &str, code: &str) -> String {
    if enabled() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn bold(text: &str) -> String {
    paint(text, "1")
}

pub fn dim(text: &str) -> String {
    paint(text, "2")
}

pub fn red(text: &str) -> String {
    paint(text, "31")
}

pub fn green(text: &str) -> String {
    paint(text, "32")
}

pub fn yellow(text: &str) -> String {
    paint(text, "33")
}

pub fn cyan(text: &str) -> String {
    paint(text, "36")
}

pub fn magenta(text: &str) -> String {
    paint(text, "35")
}
