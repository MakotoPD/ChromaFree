<p align="center">
  <img src="crates/chromafree-app/assets/chromafree-256.png" width="128" alt="ChromaFree icon">
</p>

# ChromaFree

A lightweight virtual camera for Windows 11 that removes, replaces or blurs the background of your webcam on the GPU.
It shows up as a regular camera called **ChromaFree** in Discord, browsers, Meld Studio, OBS and other apps.

Free and open source, no telemetry, no ads, no OBS or Python needed at runtime.

## Features

- **Background effects:** blur with an adjustable strength, solid color with green screen and blue screen presets or
  any color, your own image (PNG or JPEG), or real transparency (alpha channel) for apps that support ARGB32 such as
  Meld Studio. Apps without alpha support see the chosen color instead.
- **Segmentation on the GPU** with DirectML, so it works on any DirectX 12 GPU (AMD, NVIDIA, Intel):
  - Robust Video Matting — accurate, temporally stable edges (default),
  - MediaPipe Selfie Segmentation — very light.
- **Mask quality** picked automatically for your camera's aspect ratio and resolution, or chosen manually; edge
  softness and temporal smoothing can be tuned.
- **Input camera, resolution and frame rate** selection, remembered between runs.
- **Output resolution and frame rate** of the virtual camera, rotation and mirroring.
- **Low resource usage:** the camera is opened only while an app uses ChromaFree and released a few seconds later.
- Optional **live preview** of exactly what the virtual camera outputs.

Measured on an AMD Radeon RX 6950 XT with a 1080p camera, 1280×720 at 30 FPS with background blur:

| State | CPU | Memory |
|---|---|---|
| Idle (no app uses the camera) | 0% | ~15–30 MB |
| Active | ~3% of one core | ~80–95 MB |
| Camera to output latency | 11–16 ms | |

## Requirements

- Windows 11 (build 22000 or newer) — the virtual camera uses `MFCreateVirtualCamera`.
- A GPU with DirectX 12 support. Without one, segmentation can run on the CPU at a much higher CPU cost.

## Installation

Download `ChromaFree-Setup-<version>.exe` from the [releases page](https://github.com/MakotoPD/ChromaFree/releases) and run it. The installer needs administrator rights
once, because the virtual camera source (`vcam-source.dll`) is loaded by the Windows Frame Server service and must be
registered for all users.

The installer adds the **ChromaFree** camera to Windows for all users. It stays available even when the app is not
running and then shows an offline image. Start ChromaFree from the Start menu (or let it start when you sign in); it
lives in the tray and closing the window keeps processing available. Pick **ChromaFree** as the camera in the app you
want to use.

## Usage tips

- **Transparent background in Meld Studio:** choose *Remove background: transparency*. Meld Studio keeps the alpha channel; Discord, browsers and OBS receive the fallback color.
- **Chroma key workflows:** choose the color effect with the green screen preset and key it in your streaming app.
- **Interface language:** English, or Polish when Windows uses Polish. Set `CHROMAFREE_LANGUAGE=en` or `pl` to override. Translations live in `crates/chromafree-app/translations/<lang>/LC_MESSAGES/chromafree-app.po`.
- **Changing the output resolution** applies when an app opens the camera again; apps that keep it open continue at their current resolution.
- The configuration is stored in `%APPDATA%\ChromaFree\config.toml` and reloaded when the file changes.
- Logs are written to `%LOCALAPPDATA%\ChromaFree\chromafree.log`. The virtual camera source running inside Windows
  Frame Server logs activations and stream starts to `%ProgramData%\ChromaFree\vcam-source.log`.

## Troubleshooting

- **The status says the virtual camera is unavailable:** the DLL is not registered. Reinstall ChromaFree, or when
  building from source run `vcam-source\install.ps1` from an elevated PowerShell.
- **The camera does not appear in an app:** close and reopen that app after starting ChromaFree. Some apps only
  enumerate cameras when they start.
- **The physical camera is in use:** only one process can open a camera at a time. Close other apps that use it
  directly, or let them use ChromaFree instead.
- **Uninstall leaves `vcam-source.dll` behind:** the Frame Server service may still hold it. It is removed after a
  restart, or immediately after `Restart-Service FrameServer -Force` from an elevated PowerShell.

## Building from source

Prerequisites:

- [Rust](https://rustup.rs) (stable, MSVC toolchain),
- Visual Studio 2026 Build Tools with the C++ workload (includes CMake),
- [uv](https://docs.astral.sh/uv/) to prepare the models (Python is only used at build time),
- [Inno Setup 6](https://jrsoftware.org/isinfo.php) to build the installer.

```powershell
powershell -ExecutionPolicy Bypass -File models/download.ps1
cargo build --release
cmake -S vcam-source -B build/vcam -A x64
cmake --build build/vcam --config Release
cargo test --workspace
```

Run CMake from a Developer PowerShell. `models/download.ps1` downloads the original models from their official
sources, verifies SHA-256 checksums, converts MediaPipe to ONNX and generates the static RVM variants.

To try the virtual camera without the installer, register the DLL from an elevated PowerShell and start the app:

```powershell
powershell -ExecutionPolicy Bypass -File vcam-source/install.ps1
cargo run --release -p chromafree-app
```

To build the installer:

```powershell
powershell -ExecutionPolicy Bypass -File installer/build.ps1
```

The installer is written to `build/installer`.

## Architecture

Windows loads virtual camera sources into the Frame Server service rather than into the application, so ChromaFree
is split into two processes connected by shared memory:

```
physical camera → chromafree.exe (Rust: capture, D3D12 + DirectML pipeline, Slint window, tray)
                      ↓ shared memory (seqlock), format chosen by the consuming app
                  vcam-source.dll (C++ Media Foundation source inside Frame Server)
                      ↓
                  Discord / browsers / Meld Studio / OBS
```

| Directory | Contents |
|---|---|
| `crates/chromafree-core` | Frame processing pipeline and CPU fallback, independent of Windows camera APIs |
| `crates/chromafree-gpu` | D3D12 compute pipeline sharing the device with DirectML |
| `crates/chromafree-ipc` | Shared memory protocol and the generated C header |
| `crates/chromafree-capture` | Camera capture through the Media Foundation Source Reader |
| `crates/chromafree-app` | Application: engine, configuration, virtual camera registration, window and tray |
| `vcam-source` | Virtual camera media source DLL |
| `tools/chromafree-probe` | Shared memory producer and reader for testing without a registered camera |
| `models` | Model download and conversion scripts (model files are not stored in the repository) |
| `installer` | Inno Setup script and packaging script |

## Community and contributing

- Join the [Discord server](https://discord.gg/pv52eNr6uc) for questions and help.
- Report bugs and suggest features through the [issue forms](https://github.com/MakotoPD/ChromaFree/issues/new/choose).
- Read the [contributing guide](.github/CONTRIBUTING.md) before opening a pull request, and report security issues
  as described in the [security policy](.github/SECURITY.md).

## License

ChromaFree is licensed under the [GNU General Public License v3.0 or later](LICENSE). The Robust Video Matting models
are GPL-3.0 licensed. See [THIRD_PARTY.md](THIRD_PARTY.md) for bundled components and their licenses.
