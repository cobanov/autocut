# Contributing to AutoCut

Thanks for helping. AutoCut does one job: remove silence from a video, fast and
locally. Changes that keep it simple and offline are the easiest to merge.

## Before you start

- **Open an issue first** for a new feature or anything that touches the export
  path. A short note on what you want to add and why saves a rewrite later.
  Small fixes and typos can go straight to a pull request.
- **One feature per pull request.** If you have several ideas, send them as
  separate PRs that each stand on their own. Each one can then be reviewed,
  merged or reverted without holding up the rest.
- **Keep it local.** AutoCut needs no account, no upload and no network to edit
  a video. Anything that calls an outside service (AI models, cloud APIs) must be
  optional, off by default, and the app must work exactly as before when it is
  off. Never ship an API key or require one.

## Setup

You need Node 22.13+, pnpm 11 and a stable Rust toolchain. On Linux, also install
the Tauri system libraries (see the `rust` job in
[`.github/workflows/ci.yml`](.github/workflows/ci.yml) for the exact list).

```sh
pnpm install
pnpm tauri:dev        # the first build fetches ~200MB of ffmpeg
```

To run the Rust tests without downloading ffmpeg:

```sh
cd src-tauri && AUTOCUT_STUB_SIDECARS=1 cargo test --lib
```

Never use `AUTOCUT_STUB_SIDECARS=1` for a real build: the result cannot process
video.

## Where things live

| Path | What it holds |
| --- | --- |
| `src/` | Svelte 5 + TypeScript UI |
| `src/lib/cuts.ts` | Cut list logic (keeps, merges, edits), tested in `cuts.test.ts` |
| `src-tauri/src/` | Rust backend: probing, silence detection (`vad.rs`), MP4 and FCPXML export |
| `src-tauri/build.rs` | Fetches the pinned ffmpeg/ffprobe sidecars |
| `site/` | The landing page at autocut.cobanov.dev |

## Before you open a pull request

Run what CI runs. A PR that fails any of these will not be merged:

```sh
pnpm run check                                   # svelte-check, types
pnpm test                                        # vitest
pnpm run build
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
AUTOCUT_STUB_SIDECARS=1 cargo test --lib
```

Also:

- **Add tests** for new logic. Cut list changes go in `src/lib/cuts.test.ts`,
  Rust logic gets a `#[cfg(test)]` module next to the code.
- **Try it in the app.** Run `pnpm tauri:dev`, cut a real video and export it as
  both MP4 and FCPXML. Say in the PR which OS you tested on.
- **Keep the defaults.** A new option should not change what an existing user
  gets when they open the app and press export.
- **No new heavy dependencies** without asking in the issue first. The app ships
  as a small bundle, and every crate or npm package adds to it.
- **Do not bump the version.** Releases are cut by the maintainer.

## Pull request description

Say what the change does, why, and how you tested it. Add a screenshot or a
short clip for anything visible in the UI.

## Commit messages

Short, imperative, with a [Conventional Commits](https://www.conventionalcommits.org)
prefix, as in the existing history:

```
feat: normalize export loudness to -14 LUFS
fix(export): keep audio in sync after a trimmed keep
docs: explain the padding slider
```

## Reporting bugs

Open an issue with the bug report template: your OS, the AutoCut version, what
you did, what you expected and what happened. If a specific video triggers it,
the output of `ffprobe <file>` helps a lot.

## License

AutoCut is MIT licensed. By opening a pull request you agree that your
contribution is released under the same [license](LICENSE).
