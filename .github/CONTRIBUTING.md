# Contributing to ChromaFree

Thanks for your interest in improving ChromaFree. Bug reports, ideas, translations, testing with different cameras,
graphics cards and apps, and code are all welcome.

## Ways to help

- **Report bugs** with the [bug report form](https://github.com/MakotoPD/ChromaFree/issues/new?template=bug_report.yml).
  Include logs from `%LOCALAPPDATA%\ChromaFree\chromafree.log` and, for virtual camera problems,
  `%ProgramData%\ChromaFree\vcam-source.log`.
- **Suggest features** with the [feature request form](https://github.com/MakotoPD/ChromaFree/issues/new?template=feature_request.yml).
- **Translate** the interface, see [Translations](#translations).
- **Test compatibility** with apps that use cameras and tell us what works.
- **Talk with other users** on the [Discord server](https://discord.gg/pv52eNr6uc).

## Development setup

Follow [Building from source](../README.md#building-from-source) in the README. You need Windows 11, a GPU with
DirectX 12, Rust stable with the MSVC toolchain, Visual Studio Build Tools with the C++ workload and uv for preparing
the models.

## Project principles

- **Low resource usage comes first.** The app must use no processor time while no app uses the camera. Changes to the
  capture or processing path should include measurements (CPU, memory, latency) before and after.
- **Work on the GPU where possible.** Scaling, masking, blur and compositing run as D3D12 compute shaders next to
  DirectML. The CPU pipeline is a fallback and the reference for tests.
- **Keep the virtual camera DLL minimal.** It runs inside the Windows Frame Server service and only copies the latest
  frame from shared memory.
- **The shared memory layout has one source of truth** in `crates/chromafree-ipc`. The C header is generated. Every
  layout change bumps the protocol version.
- **No telemetry, no ads, no network access** beyond opening links the user clicks.

## Code style

- Run `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` before committing.
- No `unwrap()` or `expect()` in application runtime paths. Use `thiserror` in libraries and `anyhow` in the binary.
- No allocations per frame in the hot path. Allocate buffers at startup or when the resolution changes.
- Log with `tracing`. Nothing is logged per frame above the `trace` level.
- Code does not contain comments. Prefer clear names, small functions and types that describe intent, and explain
  non-obvious decisions in the pull request description.
- C++ uses C++20, CMake and WIL, and builds with warnings treated as errors.

## Tests

- `cargo test --workspace` runs tests that need no GPU or camera.
- Tests that need a GPU, DirectML, a real camera or a registered virtual camera are marked `#[ignore]`. Run them with
  `cargo test --workspace -- --ignored` when your change touches those areas.
- `tools/chromafree-probe` simulates a producer and a reader of the shared memory for testing without the app.

## Translations

The interface is written in English. Slint texts use `@tr()` in `crates/chromafree-app/ui/main.slint`, and their
translations are gettext files in `crates/chromafree-app/translations/<language>/LC_MESSAGES/chromafree-app.po`
without translation contexts. Texts created in Rust (status bar, tray menu, dialogs) use `i18n::tr` and
`Language::pick` in `crates/chromafree-app/src`. Adding a language means adding a `.po` file, a `Language` variant and
detection of its Windows language ID.

## Pull requests

1. Open an issue first for larger changes so the approach can be agreed on.
2. Create a branch from `main` and keep each pull request focused on one change.
3. Write commit titles in English in the imperative mood, for example `Add blur strength presets`.
4. Fill in the pull request template, including how you tested the change.

By contributing you agree that your contributions are licensed under the
[GNU General Public License v3.0 or later](../LICENSE).
