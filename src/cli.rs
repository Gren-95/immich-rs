//! Command line definition. Option names, short flags, defaults and environment variables match
//! `immich` (commander.js), including commander's rules: a boolean option's environment variable
//! enables it whenever it is set (whatever its value), the command line beats the environment,
//! and conflicting options are reported with commander's wording.

use clap::builder::FalseyValueParser;
use clap::parser::ValueSource;
use clap::{ArgAction, ArgMatches, Args, CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::config::default_config_directory;

const HELP_TEMPLATE: &str = "{usage-heading} {usage}\n\n{about-with-newline}\n{all-args}{after-help}";
/// commander lists the options before the commands.
const ROOT_HELP_TEMPLATE: &str =
    "{usage-heading} {usage}\n\n{about-with-newline}\nOptions:\n{options}\n\nCommands:\n{subcommands}\n";
pub const VISIBILITY_CHOICES: [&str; 4] = ["archive", "timeline", "hidden", "locked"];

fn config_directory_help() -> String {
    format!(
        "Configuration directory where auth.yml will be stored (default: \"{}\", env: IMMICH_CONFIG_DIR)",
        default_config_directory()
    )
}

fn concurrency_help() -> String {
    format!(
        "Number of assets to upload at the same time (default: {}, env: IMMICH_UPLOAD_CONCURRENCY)",
        default_concurrency()
    )
}

/// `Math.max(1, os.cpus().length - 1)`.
pub fn default_concurrency() -> usize {
    logical_cpu_count().saturating_sub(1).max(1)
}

/// Counts CPUs like Node's `os.cpus()` (every `cpuN` line of `/proc/stat` on Linux).
fn logical_cpu_count() -> usize {
    let from_proc = std::fs::read_to_string("/proc/stat").ok().map(|stat| {
        stat.lines()
            .filter(|line| {
                line.strip_prefix("cpu")
                    .and_then(|rest| rest.chars().next())
                    .is_some_and(|c| c.is_ascii_digit())
            })
            .count()
    });
    match from_proc {
        Some(count) if count > 0 => count,
        _ => std::thread::available_parallelism().map(usize::from).unwrap_or(1),
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "immich-rs",
    about = "Command line interface for Immich",
    override_usage = "immich-rs [options] [command]",
    help_template = ROOT_HELP_TEMPLATE,
    subcommand_value_name = "command",
    subcommand_help_heading = "Commands",
    disable_version_flag = true,
    disable_help_flag = true,
    disable_help_subcommand = true,
    args_override_self = true
)]
pub struct Cli {
    #[arg(short = 'V', long = "version", action = ArgAction::SetTrue, help = "output the version number")]
    pub version: bool,

    #[arg(
        short = 'd',
        long = "config-directory",
        value_name = "directory",
        global = true,
        display_order = 100,
        env = "IMMICH_CONFIG_DIR",
        hide_env = true,
        help = config_directory_help()
    )]
    pub config_directory: Option<String>,

    #[arg(
        short = 'u',
        long = "url",
        value_name = "url",
        global = true,
        display_order = 101,
        num_args = 0..=1,
        default_missing_value = "",
        env = "IMMICH_INSTANCE_URL",
        hide_env = true,
        help = "Immich server URL (env: IMMICH_INSTANCE_URL)"
    )]
    pub url: Option<String>,

    #[arg(
        short = 'k',
        long = "key",
        value_name = "key",
        global = true,
        display_order = 102,
        num_args = 0..=1,
        default_missing_value = "",
        env = "IMMICH_API_KEY",
        hide_env = true,
        help = "Immich API key (env: IMMICH_API_KEY)"
    )]
    pub key: Option<String>,

    #[arg(short = 'h', long = "help", action = ArgAction::Help, global = true, display_order = 103,
        help = "display help for command")]
    pub help: Option<bool>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    #[command(
        visible_alias = "login-key",
        about = "Login using an API key",
        override_usage = "immich-rs login|login-key [options] <url> <key>",
        help_template = HELP_TEMPLATE,
        disable_help_flag = true
    )]
    Login {
        #[arg(value_name = "url", help = "Immich server URL")]
        server_url: String,
        #[arg(value_name = "key", help = "Immich API key")]
        api_key: String,
    },

    #[command(about = "Remove stored credentials", help_template = HELP_TEMPLATE, disable_help_flag = true)]
    Logout,

    #[command(
        name = "server-info",
        about = "Display server information",
        help_template = HELP_TEMPLATE,
        disable_help_flag = true
    )]
    ServerInfo,

    #[command(
        about = "Upload assets",
        override_usage = "immich-rs upload [paths...] [options]",
        help_template = HELP_TEMPLATE,
        disable_help_flag = true,
        args_override_self = true
    )]
    Upload(UploadArgs),

    #[command(
        about = "display help for command",
        override_usage = "immich-rs help [command]",
        help_template = HELP_TEMPLATE,
        disable_help_flag = true
    )]
    Help {
        #[arg(value_name = "command")]
        command: Option<String>,
    },
}

#[derive(Args, Debug)]
pub struct UploadArgs {
    #[arg(short = 'r', long = "recursive", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new(),
        env = "IMMICH_RECURSIVE", hide_env = true, help = "Recursive (default: false, env: IMMICH_RECURSIVE)")]
    pub recursive: bool,

    #[arg(
        short = 'i',
        long = "ignore",
        value_name = "pattern",
        env = "IMMICH_IGNORE_PATHS",
        hide_env = true,
        help = "Pattern to ignore (env: IMMICH_IGNORE_PATHS)"
    )]
    pub ignore: Option<String>,

    #[arg(long = "skip-hash", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new(),
        env = "IMMICH_SKIP_HASH", hide_env = true,
        help = "Don't hash files before upload (default: false, env: IMMICH_SKIP_HASH)")]
    pub skip_hash: bool,

    #[arg(short = 'H', long = "include-hidden", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new(),
        env = "IMMICH_INCLUDE_HIDDEN", hide_env = true,
        help = "Include hidden folders (default: false, env: IMMICH_INCLUDE_HIDDEN)")]
    pub include_hidden: bool,

    #[arg(short = 'a', long = "album", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new(),
        env = "IMMICH_AUTO_CREATE_ALBUM", hide_env = true,
        help = "Automatically create albums based on folder name (default: false, env: IMMICH_AUTO_CREATE_ALBUM)")]
    pub album: bool,

    #[arg(
        short = 'A',
        long = "album-name",
        value_name = "name",
        env = "IMMICH_ALBUM_NAME",
        hide_env = true,
        help = "Add all assets to specified album (env: IMMICH_ALBUM_NAME)"
    )]
    pub album_name: Option<String>,

    #[arg(
        long = "visibility",
        value_name = "visibility",
        env = "IMMICH_VISIBILITY",
        hide_env = true,
        help = "Set the visibility of uploaded assets (choices: \"archive\", \"timeline\", \"hidden\", \"locked\", env: IMMICH_VISIBILITY)"
    )]
    pub visibility: Option<String>,

    #[arg(short = 'n', long = "dry-run", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new(),
        env = "IMMICH_DRY_RUN", hide_env = true,
        help = "Don't perform any actions, just show what will be done (default: false, env: IMMICH_DRY_RUN)")]
    pub dry_run: bool,

    #[arg(short = 'c', long = "concurrency", value_name = "number", env = "IMMICH_UPLOAD_CONCURRENCY",
        hide_env = true, help = concurrency_help())]
    pub concurrency: Option<String>,

    #[arg(short = 'j', long = "json-output", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new(),
        env = "IMMICH_JSON_OUTPUT", hide_env = true,
        help = "Output detailed information in json format (default: false, env: IMMICH_JSON_OUTPUT)")]
    pub json_output: bool,

    #[arg(long = "delete", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new(),
        env = "IMMICH_DELETE_ASSETS", hide_env = true,
        help = "Delete local assets after upload (env: IMMICH_DELETE_ASSETS)")]
    pub delete: bool,

    #[arg(long = "delete-duplicates", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new(),
        env = "IMMICH_DELETE_DUPLICATES", hide_env = true,
        help = "Delete local assets that are duplicates (already exist on server) (env: IMMICH_DELETE_DUPLICATES)")]
    pub delete_duplicates: bool,

    #[arg(long = "no-progress", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new(),
        env = "IMMICH_PROGRESS_BAR", hide_env = true, help = "Hide progress bars (env: IMMICH_PROGRESS_BAR)")]
    pub no_progress: bool,

    #[arg(long = "watch", action = ArgAction::SetTrue, value_parser = FalseyValueParser::new(),
        env = "IMMICH_WATCH_CHANGES", hide_env = true,
        help = "Watch for changes and upload automatically (default: false, env: IMMICH_WATCH_CHANGES)")]
    pub watch: bool,

    #[arg(value_name = "paths", help = "One or more paths to assets to be uploaded")]
    pub paths: Vec<String>,
}

/// Options shared by every command.
#[derive(Debug, Clone)]
pub struct GlobalOptions {
    pub config_directory: String,
    /// `None` when unset or empty (the original treats an empty value as missing).
    pub url: Option<String>,
    pub key: Option<String>,
}

/// Fully resolved `upload` options.
#[derive(Debug, Clone)]
pub struct UploadOptions {
    pub paths: Vec<String>,
    pub recursive: bool,
    pub ignore: Option<String>,
    pub skip_hash: bool,
    pub include_hidden: bool,
    pub album: bool,
    pub album_name: Option<String>,
    pub visibility: Option<String>,
    pub dry_run: bool,
    pub concurrency: usize,
    pub json_output: bool,
    pub delete: bool,
    pub delete_duplicates: bool,
    pub progress: bool,
    pub watch: bool,
}

pub enum Invocation {
    Version,
    /// Print the help of the root command (`None`) or of a subcommand, to stdout.
    Help(Option<String>),
    /// No command: print help to stderr and exit 1, like commander.
    MissingCommand,
    Login {
        url: String,
        key: String,
        global: GlobalOptions,
    },
    Logout(GlobalOptions),
    ServerInfo(GlobalOptions),
    Upload(GlobalOptions, Box<UploadOptions>),
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.is_empty())
}

/// Where a value came from, for commander-style conflict messages.
fn from_env(matches: &ArgMatches, id: &str) -> bool {
    matches.value_source(id) == Some(ValueSource::EnvVariable)
}

fn is_explicit(matches: &ArgMatches, id: &str) -> bool {
    matches!(
        matches.value_source(id),
        Some(ValueSource::CommandLine | ValueSource::EnvVariable)
    )
}

/// How commander names an option in error messages, depending on where its value came from.
fn describe(matches: &ArgMatches, id: &str, flags: &str, env: &str) -> String {
    if from_env(matches, id) {
        format!("environment variable '{env}'")
    } else {
        format!("option '{flags}'")
    }
}

/// commander's message for an option value it rejects, naming the env var when it came from one.
fn invalid_value(matches: &ArgMatches, id: &str, flags: &str, env: &str, value: &str, reason: &str) -> String {
    let source = if from_env(matches, id) {
        format!("value '{value}' from env '{env}'")
    } else {
        format!("argument '{value}'")
    };
    format!("error: option '{flags}' {source} is invalid. {reason}")
}

fn check_visibility(matches: &ArgMatches, visibility: Option<&str>) -> Result<(), String> {
    match visibility {
        Some(value) if !VISIBILITY_CHOICES.contains(&value) => Err(invalid_value(
            matches,
            "visibility",
            "--visibility <visibility>",
            "IMMICH_VISIBILITY",
            value,
            &format!("Allowed choices are {}.", VISIBILITY_CHOICES.join(", ")),
        )),
        _ => Ok(()),
    }
}

/// `--concurrency`, which must be a whole number of at least 1.
fn parse_concurrency(matches: &ArgMatches, value: Option<&str>) -> Result<usize, String> {
    let Some(text) = value else {
        return Ok(default_concurrency());
    };
    match text.trim().parse::<usize>() {
        Ok(number) if number >= 1 => Ok(number),
        _ => Err(invalid_value(
            matches,
            "concurrency",
            "-c, --concurrency <number>",
            "IMMICH_UPLOAD_CONCURRENCY",
            text,
            "Expected a whole number of at least 1.",
        )),
    }
}

/// commander's `.conflicts()`: both options set (by flag or environment) is an error.
fn check_conflict(matches: &ArgMatches, first: (&str, &str, &str), second: (&str, &str, &str)) -> Result<(), String> {
    let (first_id, first_flags, first_env) = first;
    let (second_id, second_flags, second_env) = second;
    if is_explicit(matches, first_id) && is_explicit(matches, second_id) {
        return Err(format!(
            "error: {} cannot be used with {}",
            describe(matches, first_id, first_flags, first_env),
            describe(matches, second_id, second_flags, second_env)
        ));
    }
    Ok(())
}

fn resolve_upload(matches: &ArgMatches, args: UploadArgs) -> Result<UploadOptions, String> {
    // A boolean option's environment variable turns it on whenever it is set.
    let flag = |id: &str, parsed: bool| parsed || from_env(matches, id);

    check_visibility(matches, args.visibility.as_deref())?;
    check_conflict(
        matches,
        ("album_name", "-A, --album-name <name>", "IMMICH_ALBUM_NAME"),
        ("album", "-a, --album", "IMMICH_AUTO_CREATE_ALBUM"),
    )?;
    check_conflict(
        matches,
        ("dry_run", "-n, --dry-run", "IMMICH_DRY_RUN"),
        ("skip_hash", "--skip-hash", "IMMICH_SKIP_HASH"),
    )?;
    let concurrency = parse_concurrency(matches, args.concurrency.as_deref())?;
    let watch = flag("watch", args.watch);

    Ok(UploadOptions {
        paths: args.paths,
        recursive: flag("recursive", args.recursive),
        ignore: args.ignore,
        skip_hash: flag("skip_hash", args.skip_hash),
        include_hidden: flag("include_hidden", args.include_hidden),
        album: flag("album", args.album),
        album_name: args.album_name,
        visibility: args.visibility,
        dry_run: flag("dry_run", args.dry_run),
        concurrency,
        json_output: flag("json_output", args.json_output),
        delete: flag("delete", args.delete),
        delete_duplicates: flag("delete_duplicates", args.delete_duplicates),
        // `--watch` implies `--no-progress`.
        progress: !flag("no_progress", args.no_progress) && !watch,
        watch,
    })
}

/// Parses the process arguments. `Err` carries a ready-to-print clap error.
pub fn parse() -> Result<Invocation, ParseError> {
    parse_from(std::env::args_os())
}

pub enum ParseError {
    Clap(clap::Error),
    Message(String),
}

pub fn parse_from<I, T>(args: I) -> Result<Invocation, ParseError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let matches = Cli::command().try_get_matches_from(args).map_err(ParseError::Clap)?;
    let cli = Cli::from_arg_matches(&matches).map_err(ParseError::Clap)?;
    if cli.version {
        return Ok(Invocation::Version);
    }
    let global = GlobalOptions {
        config_directory: cli.config_directory.unwrap_or_else(default_config_directory),
        url: non_empty(cli.url),
        key: non_empty(cli.key),
    };
    Ok(match cli.command {
        None => Invocation::MissingCommand,
        Some(Command::Help { command }) => Invocation::Help(command),
        Some(Command::Login { server_url, api_key }) => Invocation::Login {
            url: server_url,
            key: api_key,
            global,
        },
        Some(Command::Logout) => Invocation::Logout(global),
        Some(Command::ServerInfo) => Invocation::ServerInfo(global),
        Some(Command::Upload(args)) => {
            let Some(upload_matches) = matches.subcommand_matches("upload") else {
                return Err(ParseError::Message("error: failed to read upload options".to_string()));
            };
            let options = resolve_upload(upload_matches, args).map_err(ParseError::Message)?;
            Invocation::Upload(global, Box::new(options))
        }
    })
}

/// Renders the help text of the root command or of the named subcommand.
pub fn render_help(command: Option<&str>) -> Option<String> {
    let mut root = Cli::command();
    match command {
        None => Some(root.render_help().to_string()),
        Some(name) => {
            root.build();
            let sub = root
                .get_subcommands_mut()
                .find(|sub| sub.get_name() == name || sub.get_all_aliases().any(|a| a == name))?;
            Some(sub.render_help().to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(args: &[&str]) -> Result<UploadOptions, String> {
        let mut argv = vec!["immich-rs", "upload"];
        argv.extend_from_slice(args);
        match parse_from(argv) {
            Ok(Invocation::Upload(_, options)) => Ok(*options),
            Ok(_) => Err("not an upload".to_string()),
            Err(ParseError::Message(message)) => Err(message),
            Err(ParseError::Clap(error)) => Err(error.to_string()),
        }
    }

    #[test]
    fn verifies_the_clap_definition() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_upload_flags_and_defaults() {
        let options = upload(&["-r", "-i", "**/ocr/**", "-n", "--visibility", "archive", "a", "b"]).expect("parse");
        assert!(options.recursive && options.dry_run && options.progress && !options.watch);
        assert_eq!(options.ignore.as_deref(), Some("**/ocr/**"));
        assert_eq!(options.visibility.as_deref(), Some("archive"));
        assert_eq!(options.paths, vec!["a", "b"]);
        assert_eq!(options.concurrency, default_concurrency());
    }

    #[test]
    fn watch_implies_no_progress() {
        let options = upload(&["--watch", "-c", "4", "dir"]).expect("parse");
        assert!(options.watch && !options.progress);
        assert_eq!(options.concurrency, 4);
    }

    #[test]
    fn reports_conflicts_with_commander_wording() {
        assert_eq!(
            upload(&["-a", "-A", "x"]).err().as_deref(),
            Some("error: option '-A, --album-name <name>' cannot be used with option '-a, --album'")
        );
        assert_eq!(
            upload(&["--skip-hash", "-n"]).err().as_deref(),
            Some("error: option '-n, --dry-run' cannot be used with option '--skip-hash'")
        );
        assert_eq!(
            upload(&["--visibility", "public"]).err().as_deref(),
            Some(
                "error: option '--visibility <visibility>' argument 'public' is invalid. Allowed choices are archive, timeline, hidden, locked."
            )
        );
    }

    #[test]
    fn global_options_work_after_the_subcommand() {
        match parse_from([
            "immich-rs",
            "upload",
            "-u",
            "http://h/api",
            "-k",
            "secret",
            "-d",
            "/tmp/c",
            "x",
        ]) {
            Ok(Invocation::Upload(global, _)) => {
                assert_eq!(global.url.as_deref(), Some("http://h/api"));
                assert_eq!(global.key.as_deref(), Some("secret"));
                assert_eq!(global.config_directory, "/tmp/c");
            }
            _ => panic!("expected an upload invocation"),
        }
    }

    #[test]
    fn login_alias_and_help_rendering() {
        assert!(matches!(
            parse_from(["immich-rs", "login-key", "http://h", "k"]),
            Ok(Invocation::Login { ref url, ref key, .. }) if url == "http://h" && key == "k"
        ));
        let help = render_help(Some("upload")).unwrap_or_default();
        assert!(help.contains("IMMICH_UPLOAD_CONCURRENCY") && help.contains("--delete-duplicates"));
        assert!(render_help(Some("nope")).is_none());
    }
}
