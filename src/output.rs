//! Console formatting shared by the commands: sizes, plurals, progress bars and JSON output.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use serde::Serialize;

use crate::date::js_round;

/// The plural suffix used throughout the original CLI's messages.
pub fn s(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

const METRIC_UNITS: [(f64, f64, &str); 9] = [
    (0.0, 1e3, "B"),
    (1e3, 1e6, "kB"),
    (1e6, 1e9, "MB"),
    (1e9, 1e12, "GB"),
    (1e12, 1e15, "TB"),
    (1e15, 1e18, "PB"),
    (1e18, 1e21, "EB"),
    (1e21, 1e24, "ZB"),
    (1e24, 1e27, "YB"),
];

fn round_decimal(value: f64, precision: i32) -> f64 {
    let multiplier = 10f64.powi(precision);
    js_round(value * multiplier) / multiplier
}

/// Rounds a non-negative decimal string half away from zero to one fractional digit and
/// strips trailing zeros, like `Intl.NumberFormat('en-US', { maximumFractionDigits: 1 })`.
fn format_one_decimal(value: f64) -> String {
    // Rust's `Display` gives the shortest round-trip representation, which is what ICU rounds.
    let text = format!("{value}");
    let (integer, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let mut digits: Vec<u8> = integer.bytes().map(|b| b - b'0').collect();
    let first_fraction = fraction.bytes().next().map(|b| b - b'0');
    let round_up = fraction.as_bytes().get(1).is_some_and(|&b| b >= b'5');
    let mut tenth = first_fraction.unwrap_or(0);
    if round_up {
        if tenth == 9 {
            tenth = 0;
            let mut index = digits.len();
            loop {
                if index == 0 {
                    digits.insert(0, 1);
                    break;
                }
                index -= 1;
                if digits[index] == 9 {
                    digits[index] = 0;
                } else {
                    digits[index] += 1;
                    break;
                }
            }
        } else {
            tenth += 1;
        }
    }
    let integer_text: String = digits.iter().map(|d| char::from(b'0' + d)).collect();
    let grouped = group_thousands(&integer_text);
    if tenth == 0 {
        grouped
    } else {
        format!("{grouped}.{tenth}")
    }
}

fn group_thousands(digits: &str) -> String {
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(digit);
    }
    output
}

/// Human readable size, identical to the `byte-size` package with its defaults
/// (metric units, one decimal) as used by the original CLI.
pub fn byte_size(bytes: u64) -> String {
    let value = bytes as f64;
    let Some(&(from, _, _)) = METRIC_UNITS.iter().find(|(from, to, _)| value >= *from && value < *to) else {
        return format!("{bytes} ");
    };
    let rounded = if from == 0.0 {
        round_decimal(value, 1)
    } else {
        round_decimal(value / from, 1) * from
    };
    match METRIC_UNITS
        .iter()
        .find(|(from, to, _)| rounded >= *from && rounded < *to)
    {
        Some(&(0.0, _, unit)) => format!("{} {unit}", format_one_decimal(value)),
        Some(&(unit_from, _, unit)) => format!("{} {unit}", format_one_decimal(value / unit_from)),
        None => format!("{bytes} "),
    }
}

/// `cli-progress`'s ETA formatting (rounded to multiples of five seconds above ten seconds).
fn format_eta(seconds: u64) -> String {
    let round5 = |value: f64| (5.0 * js_round(value / 5.0)) as u64;
    let last_two = |value: u64| {
        let text = value.to_string();
        text[text.len().saturating_sub(2)..].to_string()
    };
    if seconds > 3600 {
        format!(
            "{}h{}m",
            last_two(seconds / 3600),
            last_two(round5((seconds % 3600) as f64 / 60.0))
        )
    } else if seconds > 60 {
        format!(
            "{}m{}s",
            last_two(seconds / 60),
            last_two(round5((seconds % 60) as f64))
        )
    } else if seconds > 10 {
        format!("{}s", last_two(round5(seconds as f64)))
    } else {
        format!("{seconds}s")
    }
}

fn percentage(state: &ProgressState) -> u64 {
    match state.len() {
        Some(0) | None => 100,
        Some(total) => (state.pos() as f64 / total as f64 * 100.0).floor() as u64,
    }
}

fn eta_seconds(state: &ProgressState) -> u64 {
    state.eta().as_secs_f64().ceil() as u64
}

/// Which values a progress bar shows after `value/total`.
#[derive(Clone)]
pub enum BarUnits {
    /// Sizes formatted with [`byte_size`].
    Bytes,
    /// Plain counts followed by a noun, e.g. `3/10 assets`.
    Count(&'static str),
}

fn style(template: &str, units: BarUnits, shown_value: Option<Arc<AtomicU64>>) -> ProgressStyle {
    let style = ProgressStyle::with_template(template).unwrap_or_else(|_| ProgressStyle::default_bar());
    style
        .progress_chars("█░")
        .with_key("pct", |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            let _ = write!(w, "{}", percentage(state));
        })
        .with_key("eta_formatted", |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            let _ = write!(w, "{}", format_eta(eta_seconds(state)));
        })
        .with_key("eta_seconds", |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            let _ = write!(w, "{}", eta_seconds(state));
        })
        .with_key("amounts", move |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            let total = state.len().unwrap_or(0);
            let value = shown_value
                .as_ref()
                .map_or(state.pos(), |shown| shown.load(Ordering::Relaxed));
            let _ = match &units {
                BarUnits::Bytes => write!(w, "{}/{}", byte_size(value), byte_size(total)),
                BarUnits::Count(noun) => write!(w, "{value}/{total} {noun}"),
            };
        })
}

/// One of the hashing/duplicate-check bars: `{message} | {bar} | {percentage}% | ETA: .. | {value}/{total}`.
pub fn bytes_bar(message: &str, total: u64) -> ProgressBar {
    let template = "{msg} | {bar:40} | {pct}% | ETA: {eta_formatted} | {amounts}";
    ProgressBar::new(total)
        .with_style(style(template, BarUnits::Bytes, None))
        .with_message(message.to_string())
}

/// The upload bar. Like the original, its fill tracks newly uploaded bytes while the label
/// shows uploaded plus duplicate bytes (`shown`).
pub fn upload_bar(total: u64, shown: Arc<AtomicU64>) -> ProgressBar {
    let template = "Uploading assets | {bar:40} | {pct}% | ETA: {eta_formatted} | {amounts}";
    ProgressBar::new(total).with_style(style(template, BarUnits::Bytes, Some(shown)))
}

/// The album and deletion bars: `{label} | {bar} | {percentage}% | ETA: {eta}s | {value}/{total} {noun}`.
pub fn count_bar(label: &str, noun: &'static str, total: u64) -> ProgressBar {
    let template = format!("{label} | {{bar:40}} | {{pct}}% | ETA: {{eta_seconds}}s | {{amounts}}");
    ProgressBar::new(total).with_style(style(&template, BarUnits::Count(noun), None))
}

/// Lists failed tasks like the original: `Failed to upload 2 assets:` then `- <path> - <error>`.
pub fn print_failures(heading: &str, noun: &str, failures: &[(String, String)]) {
    if failures.is_empty() {
        return;
    }
    println!("{heading} {} {noun}{}:", failures.len(), s(failures.len()));
    for (path, error) in failures {
        println!("- {path} - {error}");
    }
}

/// Re-indents valid JSON text like `JSON.stringify(JSON.parse(text), undefined, indent)`,
/// keeping the original key order (serde_json's map would sort keys).
pub fn pretty_json(text: &str, indent: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len() * 2);
    let mut level = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0;
    let newline = |output: &mut String, level: usize| {
        output.push('\n');
        output.push_str(&indent.repeat(level));
    };
    while index < chars.len() {
        let c = chars[index];
        index += 1;
        if in_string {
            output.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                output.push(c);
            }
            '{' | '[' => {
                let close = if c == '{' { '}' } else { ']' };
                let next = chars[index..]
                    .iter()
                    .position(|n| !n.is_whitespace())
                    .map(|offset| index + offset);
                if let Some(next) = next.filter(|&next| chars[next] == close) {
                    output.push(c);
                    output.push(close);
                    index = next + 1;
                } else {
                    output.push(c);
                    level += 1;
                    newline(&mut output, level);
                }
            }
            '}' | ']' => {
                level = level.saturating_sub(1);
                newline(&mut output, level);
                output.push(c);
            }
            ',' => {
                output.push(',');
                newline(&mut output, level);
            }
            ':' => output.push_str(": "),
            c if c.is_whitespace() => {}
            c => output.push(c),
        }
    }
    output
}

/// An asset in the JSON output and in album/delete bookkeeping. `id` is omitted from JSON
/// when the server did not return one, as `JSON.stringify` drops `undefined`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Asset {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub filepath: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonOutput<'a> {
    new_files: &'a [String],
    duplicates: &'a [Asset],
    new_assets: &'a [Asset],
}

/// `JSON.stringify({ newFiles, duplicates, newAssets }, undefined, 4)`.
pub fn json_output(new_files: &[String], duplicates: &[Asset], new_assets: &[Asset]) -> String {
    let output = JsonOutput {
        new_files,
        duplicates,
        new_assets,
    };
    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    if output.serialize(&mut serializer).is_err() {
        return String::from("{}");
    }
    String::from_utf8(buffer).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_size_matches_the_byte_size_package() {
        assert_eq!(byte_size(0), "0 B");
        assert_eq!(byte_size(999), "999 B");
        assert_eq!(byte_size(1000), "1 kB");
        assert_eq!(byte_size(1500), "1.5 kB");
        assert_eq!(byte_size(1_234_567), "1.2 MB");
        assert_eq!(byte_size(999_950), "1 MB");
        assert_eq!(byte_size(1_250_000), "1.3 MB");
        assert_eq!(byte_size(1_150_000), "1.2 MB");
        assert_eq!(byte_size(52_349_812_345), "52.3 GB");
        assert_eq!(byte_size(1_000_000_000_000), "1 TB");
    }

    #[test]
    fn plural_suffix() {
        assert_eq!(s(0), "s");
        assert_eq!(s(1), "");
        assert_eq!(s(2), "s");
    }

    #[test]
    fn eta_is_formatted_like_cli_progress() {
        assert_eq!(format_eta(7), "7s");
        assert_eq!(format_eta(23), "25s");
        assert_eq!(format_eta(125), "2m5s");
        assert_eq!(format_eta(7322), "2h0m");
        assert_eq!(format_eta(9000), "2h30m");
    }

    #[test]
    fn pretty_json_keeps_key_order() {
        let text = r#"{ "z": 1, "a": [ ], "m": {"k": "a,b:{c}\\\"", "e": {}}, "l": [1, 2] }"#;
        let expected = "{\n  \"z\": 1,\n  \"a\": [],\n  \"m\": {\n    \"k\": \"a,b:{c}\\\\\\\"\",\n    \"e\": {}\n  },\n  \"l\": [\n    1,\n    2\n  ]\n}";
        assert_eq!(pretty_json(text, "  "), expected);
        assert_eq!(pretty_json("\"text\"", "  "), "\"text\"");
    }

    #[test]
    fn json_output_has_the_original_shape() {
        let new_files = vec!["/p/a.jpg".to_string()];
        let duplicates = vec![
            Asset {
                id: Some("uuid-1".to_string()),
                filepath: "/p/b.jpg".to_string(),
            },
            Asset {
                id: None,
                filepath: "/p/c.jpg".to_string(),
            },
        ];
        let expected = r#"{
    "newFiles": [
        "/p/a.jpg"
    ],
    "duplicates": [
        {
            "id": "uuid-1",
            "filepath": "/p/b.jpg"
        },
        {
            "filepath": "/p/c.jpg"
        }
    ],
    "newAssets": []
}"#;
        assert_eq!(json_output(&new_files, &duplicates, &[]), expected);
    }
}
