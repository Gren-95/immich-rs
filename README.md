# immich-rs

[![CI](https://github.com/Gren-95/immich-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/Gren-95/immich-rs/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Gren-95/immich-rs)](https://github.com/Gren-95/immich-rs/releases/latest)
[![Matches @immich/cli](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2FGren-95%2Fimmich-rs%2Fmain%2FCargo.toml&query=%24.package.version&label=matches%20%40immich%2Fcli&color=4250af)](https://www.npmjs.com/package/@immich/cli)
[![License: AGPL-3.0](https://img.shields.io/github/license/Gren-95/immich-rs)](LICENSE)

Upload photos and videos to your [Immich](https://github.com/immich-app/immich) server from the
command line. immich-rs is a Rust rewrite of the official Immich CLI (`immich`, the npm package
`@immich/cli`) and a drop-in replacement for it:

- **Same interface.** The same commands, options, environment variables, `auth.yml` file and
  console output, so existing scripts and logins keep working.
- **One static binary.** No Node.js or npm: download one file and run it.
- **Kept current.** A monthly checkup compares it with the newest `@immich/cli` and lists the
  upstream changes to port.

This is a tribute rewrite, not an official Immich project. It follows the original CLI's logic
closely and, like Immich, is licensed under the GNU Affero General Public License v3.0. All
credit for Immich and its CLI goes to the Immich team.

## Install

### Download a release

Static builds for Linux on x86_64 and aarch64 (any distribution):

```bash
arch=$(uname -m)
base=https://github.com/Gren-95/immich-rs/releases/latest/download
curl -fsSLO "$base/immich-rs-$arch-linux.tar.gz"
curl -fsSLO "$base/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "immich-rs-$arch-linux.tar.gz" immich-rs
install -Dm755 immich-rs ~/.local/bin/immich-rs
```

### Build from source

With a recent stable Rust toolchain:

```bash
cargo install --locked --git https://github.com/Gren-95/immich-rs
```

This installs `immich-rs` into `~/.cargo/bin`. Only Linux is tested.

### Use it as `immich`

To run it where scripts call `immich`, link it under that name somewhere on your `PATH`:

```bash
ln -s ~/.local/bin/immich-rs ~/.local/bin/immich
```

## Quick start

```bash
immich-rs login https://immich.example.com/api YOUR_API_KEY
immich-rs upload --recursive ~/Pictures
```

Create the API key in Immich under Account Settings, API Keys.

## Usage

```
Usage: immich-rs [options] [command]

Options:
  -V, --version                       output the version number
  -d, --config-directory <directory>  Configuration directory where auth.yml will be stored (default: "~/.config/immich/", env: IMMICH_CONFIG_DIR)
  -u, --url [<url>]                   Immich server URL (env: IMMICH_INSTANCE_URL)
  -k, --key [<key>]                   Immich API key (env: IMMICH_API_KEY)
  -h, --help                          display help for command

Commands:
  login        Login using an API key [alias: login-key]
  logout       Remove stored credentials
  server-info  Display server information
  upload       Upload assets
  help         display help for command
```

### Log in

```bash
immich-rs login https://immich.example.com/api YOUR_API_KEY
```

The URL and key are stored in `~/.config/immich/auth.yml` (mode 0600). If the server advertises
its API location through `/.well-known/immich`, the discovered URL is used and saved on the next
command. Alternatively pass `--url` and `--key` (or set `IMMICH_INSTANCE_URL` and
`IMMICH_API_KEY`) on every command; both must be given for them to be used.

```bash
immich-rs server-info
immich-rs logout
```

### Upload

```
Usage: immich-rs upload [paths...] [options]

Options:
  -r, --recursive             Recursive (default: false, env: IMMICH_RECURSIVE)
  -i, --ignore <pattern>      Pattern to ignore (env: IMMICH_IGNORE_PATHS)
      --skip-hash             Don't hash files before upload (default: false, env: IMMICH_SKIP_HASH)
  -H, --include-hidden        Include hidden folders (default: false, env: IMMICH_INCLUDE_HIDDEN)
  -a, --album                 Automatically create albums based on folder name (default: false, env: IMMICH_AUTO_CREATE_ALBUM)
  -A, --album-name <name>     Add all assets to specified album (env: IMMICH_ALBUM_NAME)
      --visibility <visibility>  Set the visibility of uploaded assets (choices: "archive", "timeline", "hidden", "locked", env: IMMICH_VISIBILITY)
  -n, --dry-run               Don't perform any actions, just show what will be done (default: false, env: IMMICH_DRY_RUN)
  -c, --concurrency <number>  Number of assets to upload at the same time (default: CPU count - 1, env: IMMICH_UPLOAD_CONCURRENCY)
  -j, --json-output           Output detailed information in json format (default: false, env: IMMICH_JSON_OUTPUT)
      --delete                Delete local assets after upload (env: IMMICH_DELETE_ASSETS)
      --delete-duplicates     Delete local assets that are duplicates (already exist on server) (env: IMMICH_DELETE_DUPLICATES)
      --no-progress           Hide progress bars (env: IMMICH_PROGRESS_BAR)
      --watch                 Watch for changes and upload automatically (default: false, env: IMMICH_WATCH_CHANGES)
```

Examples:

```bash
# Upload single files
immich-rs upload photo1.jpg photo2.jpg

# Upload a folder and everything below it
immich-rs upload --recursive ~/Pictures

# One album per folder name, or everything into one album
immich-rs upload --recursive --album ~/Pictures
immich-rs upload --recursive --album-name "Summer 2024" ~/Pictures/Summer

# Skip folders named ocr or Screenshots, and show what would happen without uploading
immich-rs upload --dry-run --recursive ~/Pictures --ignore '**/{ocr,Screenshots}/**'

# Keep uploading new files as they appear
immich-rs upload --watch --recursive ~/Pictures
```

How upload works, as in the original:

- Files given directly are uploaded as they are. Folders are searched for files whose extension
  the server supports (case-insensitive), going into subfolders only with `--recursive`. Files
  and folders whose name starts with `.` are skipped unless `--include-hidden`.
- `--ignore` is a glob matched against absolute paths as `**/<pattern>`, case-insensitively.
  Use `'**/Raw/**'` to skip a folder, or `'*.tif'` to skip a file type.
- Files are SHA-1 hashed and checked against the server in batches of 5000, so only new files
  are uploaded. `--skip-hash` skips this step and lets the server reject duplicates.
- An XMP sidecar next to a file (`photo.jpg.xmp` or `photo.xmp`) is uploaded with it.
- `--watch` waits until a file has stopped growing for two seconds, then uploads in batches of
  100 files, or 10 seconds after the last change. Ctrl-C prints `Exiting...` and exits with 0.
- Failed uploads are retried up to three times and listed at the end. Individual failures do not
  change the exit code. Setup errors (no auth file, connection failure, missing API key
  permissions, invalid options) exit with 1.

Boolean environment variables follow commander.js rules: the option is enabled whenever the
variable is set, whatever its value (`IMMICH_RECURSIVE=false` still enables `--recursive`), and
`IMMICH_PROGRESS_BAR` set to anything hides the progress bars.

## Differences from the original

Console messages, exit codes, requests sent to the server and the `auth.yml` format were compared
against `immich` 3.4.2 and match. The known differences:

- **Ordering.** The original lists files in JSON output, and adds them to albums, in the order
  hashing and uploads happened to finish. immich-rs uses sorted path order, so output is
  deterministic.
- **Crashes become messages.** Where the original stops with a Node.js stack trace (server
  unreachable, invalid URL, unreadable folder, a failing album request), immich-rs prints one
  line such as `Failed to connect to server http://… - TypeError: fetch failed (…)`. The exit
  code is still 1. An unreachable server is reported right away, before any other work.
- **Folders with parentheses.** The original finds nothing in a folder given on the command line
  whose name contains `(…)` (fast-glob reads it as a pattern group). immich-rs searches it.
- **Folders named `undefined`.** Without `--ignore`, the original passes the pattern
  `**/undefined` to fast-glob and skips any folder of that name. immich-rs does not.
- **Glob arguments.** A path that does not exist is treated as a glob, as in the original. `*`,
  `?`, `[…]`, `{a,b}` and `**` are supported. Extglobs (`@(…)`) and ranges (`{1..3}`) are not.
- **Symlink loops.** A symlink pointing back to a folder already being searched is not followed.
  The original follows such a loop about 40 levels deep, until the OS refuses, and finds the same
  files again under every looped path.
- **`--concurrency`** is checked up front (a whole number of at least 1). The original fails
  later with a fastq error.
- **`-u`/`-k` without a value** count as not given. The original sets them to `true` and crashes.
- **Usage errors** (unknown option, missing argument) use clap's wording. Conflicting options and
  invalid `--visibility` values use commander's exact messages. Help text follows commander's
  layout but is rendered by clap.
- **Progress bars** have the same layout but are drawn by indicatif and hidden when stderr is
  not a terminal. The original prints a final, partly unformatted bar to stderr even when it is
  not a terminal. The deletion bar counts every file. The original's only shows the last chunk.
- **Watch mode** uploads batches one after another. The original can run overlapping batches.
  `--ignore` in watch mode matches the path and its parent folders. The original uses
  micromatch's substring-style matching.
- **TLS** uses rustls with the bundled Mozilla root certificates, like Node's built-in store.
  There is no equivalent of `NODE_EXTRA_CA_CERTS` for a private CA. As with Node's `fetch`, proxy
  environment variables are not used.
- A file that changes size while being uploaded fails with `file changed size while uploading`
  instead of sending a body that disagrees with its length.

## Keeping up with upstream

The version number is the `@immich/cli` version whose behaviour immich-rs matches: 3.4.2 means
the same commands and output as `@immich/cli` 3.4.2.

On the 1st of every month, the [checkup workflow](.github/workflows/checkup.yml) runs:

- formatting, clippy and the tests on the newest stable Rust
- `cargo audit` for security advisories and yanked or unmaintained crates
- the dependency updates Cargo.toml allows, tried against the tests, and new major versions
- a check for a newer `@immich/cli`, with the upstream commits under `packages/cli` since the
  matched version

Anything that needs a look becomes an issue labelled `checkup`. To backport a new upstream
version, port the listed commits, set `version` in Cargo.toml to the new version and tag it (see
below). Run the same checks locally with `.github/scripts/checkup.sh` (needs `cargo-audit`,
`jq` and a logged-in `gh`).

## Development

```bash
cargo fmt
cargo clippy --all-targets
cargo test
cargo build --release
```

To release, push a tag that matches the version in Cargo.toml:

```bash
git tag v3.4.2
git push origin v3.4.2
```

The [release workflow](.github/workflows/release.yml) builds the static binaries, checks them
and publishes them with their SHA-256 checksums.

## License

GNU Affero General Public License v3.0, the same as Immich. See [LICENSE](LICENSE).
