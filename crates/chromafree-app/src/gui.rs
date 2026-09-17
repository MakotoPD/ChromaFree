use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result};
use chromafree_capture::{CameraDevice, CaptureFormat, MediaFoundation, camera_formats, list_cameras};
use chromafree_core::color::clamp_u8;
use chromafree_core::{ColorMatrix, ModelVariant, PipelineOutput, available_rvm_variants};
use chromafree_ipc::{ObjectNames, PixelFormat};
use slint::{
    CloseRequestResponse, ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::camera::MediaFoundationCameras;
use crate::config::{
    Config, DeviceConfig, EffectMode, FormatConfig, MethodConfig, format_color, parse_color, parse_quality,
};
use crate::desktop::{
    APP_AUTHOR, APP_VERSION, DISCORD_URL, PROJECT_URL, log_directory, open_in_shell, pick_background_image,
    watch_directory,
};
use crate::engine::{
    Engine, EngineCommand, EngineObserver, EngineOptions, EngineState, EngineStatus, VirtualCameraState,
};
use crate::i18n::{Language, tr};

slint::include_modules!();

const CAMERA_CLOSE_DELAY: Duration = Duration::from_secs(5);
const PREVIEW_CHECKER_SIZE: usize = 16;
const PREVIEW_CHECKER_LIGHT: u8 = 204;
const PREVIEW_CHECKER_DARK: u8 = 153;
const OUTPUT_PRESETS: [(u32, u32); 10] = [
    (640, 360),
    (960, 540),
    (1280, 720),
    (1600, 900),
    (1920, 1080),
    (1280, 800),
    (1920, 1200),
    (640, 480),
    (960, 720),
    (1440, 1080),
];
const FPS_PRESETS: [u32; 4] = [15, 24, 30, 60];
const EFFECT_MODES: [EffectMode; 5] = [
    EffectMode::Passthrough,
    EffectMode::Blur,
    EffectMode::Color,
    EffectMode::Image,
    EffectMode::Transparent,
];
fn auto_label() -> String {
    tr("Automatic", "Automatycznie").to_owned()
}

#[derive(Default)]
struct PreviewFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Default)]
struct Shared {
    status: Mutex<EngineStatus>,
    status_pending: AtomicBool,
    preview: Mutex<PreviewFrame>,
    preview_pending: AtomicBool,
}

struct GuiObserver(Arc<Shared>);

impl EngineObserver for GuiObserver {
    fn status_changed(&self, status: &EngineStatus) {
        *self.0.status.lock().unwrap_or_else(PoisonError::into_inner) = status.clone();
        if !self.0.status_pending.swap(true, Ordering::AcqRel) {
            let _ = slint::invoke_from_event_loop(|| with_app(App::show_status));
        }
    }

    fn preview(&self, output: &PipelineOutput<'_>, matrix: ColorMatrix) {
        if self.0.preview_pending.load(Ordering::Acquire) {
            return;
        }
        {
            let mut preview = self.0.preview.lock().unwrap_or_else(PoisonError::into_inner);
            render_preview(output, matrix, &mut preview);
        }
        self.0.preview_pending.store(true, Ordering::Release);
        let _ = slint::invoke_from_event_loop(|| with_app(App::show_preview));
    }
}

fn render_preview(output: &PipelineOutput<'_>, matrix: ColorMatrix, preview: &mut PreviewFrame) {
    let size = match output {
        PipelineOutput::Nv12(frame) => frame.size(),
        PipelineOutput::Bgra(frame) => frame.size(),
    };
    if preview.rgba.len() != size.luma_len() * 4 {
        preview.rgba.resize(size.luma_len() * 4, 0);
    }
    preview.width = size.width();
    preview.height = size.height();
    let width = size.width() as usize;
    match output {
        PipelineOutput::Nv12(frame) => {
            let k = matrix.inverse();
            let chroma_stride = size.chroma_width() as usize * 2;
            for (y, (row, luma_row)) in preview
                .rgba
                .chunks_exact_mut(width * 4)
                .zip(frame.luma().chunks_exact(width))
                .enumerate()
            {
                let chroma_row = &frame.chroma()[y / 2 * chroma_stride..(y / 2 + 1) * chroma_stride];
                for (x, (pixel, &luma)) in row.as_chunks_mut::<4>().0.iter_mut().zip(luma_row).enumerate() {
                    let c = 298 * (i32::from(luma) - 16);
                    let d = i32::from(chroma_row[x & !1]) - 128;
                    let e = i32::from(chroma_row[(x & !1) + 1]) - 128;
                    pixel[0] = clamp_u8((c + k.red_v * e + 128) >> 8);
                    pixel[1] = clamp_u8((c + k.green_u * d + k.green_v * e + 128) >> 8);
                    pixel[2] = clamp_u8((c + k.blue_u * d + 128) >> 8);
                    pixel[3] = 255;
                }
            }
        }
        PipelineOutput::Bgra(frame) => {
            for (y, (row, source_row)) in preview
                .rgba
                .chunks_exact_mut(width * 4)
                .zip(frame.data().chunks_exact(width * 4))
                .enumerate()
            {
                for (x, (pixel, source)) in row
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(source_row.as_chunks::<4>().0.iter())
                    .enumerate()
                {
                    let checker = if (x / PREVIEW_CHECKER_SIZE + y / PREVIEW_CHECKER_SIZE).is_multiple_of(2) {
                        PREVIEW_CHECKER_LIGHT
                    } else {
                        PREVIEW_CHECKER_DARK
                    };
                    let alpha = u32::from(source[3]);
                    let blend = |channel: u8| {
                        ((u32::from(channel) * alpha + u32::from(checker) * (255 - alpha) + 127) / 255) as u8
                    };
                    pixel[0] = blend(source[2]);
                    pixel[1] = blend(source[1]);
                    pixel[2] = blend(source[0]);
                    pixel[3] = 255;
                }
            }
        }
    }
}

fn fps_label(fps: impl std::fmt::Display, language: Language) -> String {
    language.pick(format!("{fps} fps"), format!("{fps} kl./s"))
}

pub fn status_lines(status: &EngineStatus, language: Language) -> (String, String, String, bool) {
    let pick = |english: &str, polish: &str| language.pick(english, polish).to_owned();
    let headline = match &status.state {
        EngineState::Idle => pick(
            "Waiting for an app to use the ChromaFree camera",
            "Czeka na aplikację, która użyje kamery ChromaFree",
        ),
        EngineState::Running => {
            let fps = fps_label(format!("{:.0}", status.fps), language);
            let processing = status.processing_ms;
            match status.latency_ms {
                Some(latency) => language.pick(
                    format!("Running: {fps}, {processing:.1} ms per frame, latency {latency:.0} ms"),
                    format!("Działa: {fps}, {processing:.1} ms na klatkę, opóźnienie {latency:.0} ms"),
                ),
                None => language.pick(
                    format!("Running: {fps}, {processing:.1} ms per frame"),
                    format!("Działa: {fps}, {processing:.1} ms na klatkę"),
                ),
            }
        }
        EngineState::NoCamera => pick("No camera detected", "Nie wykryto żadnej kamery"),
        EngineState::CameraMissing(name) => language.pick(
            format!("The camera \"{name}\" is not connected. Choose another one on the Camera tab."),
            format!("Kamera „{name}” nie jest podłączona. Wybierz inną w zakładce Kamera."),
        ),
        EngineState::CameraBusy => pick(
            "The camera is used by another app or there is not enough USB bandwidth. Close the program that uses the camera directly and ChromaFree will try again.",
            "Kamera jest zajęta przez inną aplikację albo brakuje przepustowości USB. Zamknij program, który używa kamery bezpośrednio, a ChromaFree spróbuje ponownie.",
        ),
        EngineState::Error(error) => language.pick(format!("Error: {error}"), format!("Błąd: {error}")),
    };
    let mut details = Vec::new();
    if let Some((name, format)) = &status.camera {
        details.push(language.pick(format!("Camera: {name}, {format}"), format!("Kamera: {name}, {format}")));
    }
    if let Some(backend) = &status.backend {
        details.push(backend.clone());
    }
    if let Some(model) = &status.model {
        details.push(format!("Model: {model}"));
    }
    match status.consumer_format {
        Some(PixelFormat::Nv12) => details.push(pick("Consumer: NV12", "Odbiorca: NV12")),
        Some(PixelFormat::Bgra) => details.push(pick(
            "Consumer: ARGB32 with transparency",
            "Odbiorca: ARGB32 z przezroczystością",
        )),
        None => {}
    }
    if let Some(warning) = &status.warning {
        details.push(language.pick(format!("Warning: {warning}"), format!("Uwaga: {warning}")));
    }
    let virtual_camera = match &status.virtual_camera {
        VirtualCameraState::Disabled => String::new(),
        VirtualCameraState::Registered => pick(
            "The \"ChromaFree\" virtual camera is available in other apps.",
            "Wirtualna kamera „ChromaFree” jest dostępna w innych aplikacjach.",
        ),
        VirtualCameraState::Unavailable(error) => language.pick(
            format!("The virtual camera is unavailable. Reinstall ChromaFree to register vcam-source.dll. ({error})"),
            format!("Wirtualna kamera jest niedostępna. Zainstaluj ChromaFree ponownie, aby zarejestrować vcam-source.dll. ({error})"),
        ),
    };
    let problem = !matches!(status.state, EngineState::Idle | EngineState::Running)
        || matches!(status.virtual_camera, VirtualCameraState::Unavailable(_));
    (headline, details.join(" • "), virtual_camera, problem)
}

fn strings(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items.into_iter().map(SharedString::from).collect::<Vec<_>>(),
    ))
}

fn with_media_foundation<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    std::thread::spawn(move || {
        let _media_foundation = MediaFoundation::startup().ok()?;
        Some(work())
    })
    .join()
    .ok()
    .flatten()
}

fn aspect_label(variant: ModelVariant) -> &'static str {
    let (w, h) = (variant.width.max(variant.height), variant.width.min(variant.height));
    let portrait = variant.height > variant.width;
    match (w * 9 == h * 16, w * 10 == h * 16, w * 3 == h * 4, portrait) {
        (true, _, _, false) => "16:9",
        (true, _, _, true) => "9:16",
        (_, true, _, false) => "16:10",
        (_, true, _, true) => "10:16",
        (_, _, true, false) => "4:3",
        (_, _, true, true) => "3:4",
        _ => "",
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app(action: impl FnOnce(&mut App)) {
    APP.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut()
            && let Some(app) = guard.as_mut()
        {
            action(app);
        }
    });
}

struct App {
    config: Config,
    config_path: PathBuf,
    shared: Arc<Shared>,
    engine: Engine,
    window: Option<MainWindow>,
    cameras: Vec<CameraDevice>,
    formats: Vec<CaptureFormat>,
    variants: Vec<ModelVariant>,
    show_item: MenuId,
    quit_item: MenuId,
    _tray: TrayIcon,
}

pub fn run(config: Config, config_path: PathBuf, models_dir: PathBuf, show_window: bool) -> Result<()> {
    let shared = Arc::new(Shared::default());
    let engine = Engine::start(
        EngineOptions {
            names: ObjectNames::local(),
            models_dir: models_dir.clone(),
            register_virtual_camera: true,
            camera_close_delay: CAMERA_CLOSE_DELAY,
        },
        config.clone(),
        MediaFoundationCameras,
        GuiObserver(Arc::clone(&shared)),
    )?;

    let show = MenuItem::new(tr("Show window", "Pokaż okno"), true, None);
    let quit = MenuItem::new(tr("Quit", "Zakończ"), true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &quit])?;
    let tray = TrayIconBuilder::new()
        .with_tooltip("ChromaFree")
        .with_icon(Icon::from_resource(1, Some((32, 32)))?)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .context("creating the tray icon")?;
    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        let _ = slint::invoke_from_event_loop(move || with_app(|app| app.menu_clicked(&event.id)));
    }));
    TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = slint::invoke_from_event_loop(|| with_app(App::open_window));
        }
    }));

    if let Some(directory) = config_path.parent() {
        std::fs::create_dir_all(directory)?;
        watch_directory(directory, || {
            let _ = slint::invoke_from_event_loop(|| with_app(App::reload_config));
        })?;
    }

    let app = App {
        config,
        config_path,
        shared,
        engine,
        window: None,
        cameras: Vec::new(),
        formats: Vec::new(),
        variants: available_rvm_variants(&models_dir),
        show_item: show.id().clone(),
        quit_item: quit.id().clone(),
        _tray: tray,
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    if show_window {
        with_app(App::open_window);
    }
    slint::run_event_loop_until_quit()?;
    APP.with(|cell| cell.borrow_mut().take());
    Ok(())
}

impl App {
    fn menu_clicked(&mut self, id: &MenuId) {
        if *id == self.show_item {
            self.open_window();
        } else if *id == self.quit_item {
            self.window = None;
            let _ = slint::quit_event_loop();
        }
    }

    fn open_window(&mut self) {
        if let Some(window) = &self.window {
            let _ = window.show();
            return;
        }
        let window = match MainWindow::new() {
            Ok(window) => window,
            Err(error) => {
                tracing::error!(%error, "creating the window failed");
                return;
            }
        };
        let language = Language::current();
        if language != Language::English
            && let Err(error) = slint::select_bundled_translation(language.code())
        {
            tracing::warn!(%error, "selecting the interface language failed");
        }
        window.set_output_names(strings(OUTPUT_PRESETS.iter().map(|(w, h)| format!("{w}×{h}"))));
        window.set_fps_names(strings(
            FPS_PRESETS.iter().map(|fps| fps_label(fps, Language::current())),
        ));
        window.set_quality_names(strings(
            std::iter::once(auto_label()).chain(
                self.variants
                    .iter()
                    .map(|v| format!("{}×{} {}", v.width, v.height, aspect_label(*v))),
            ),
        ));

        window.on_settings_edited(|| with_app(App::settings_edited));
        window.on_camera_selected(|| with_app(App::camera_selected));
        window.on_refresh_cameras(|| with_app(App::refresh_cameras));
        window.on_choose_image(|| with_app(App::choose_image));
        window.on_reset_mask(|| with_app(App::reset_mask));
        window.on_color_preset(|color| with_app(move |app| app.color_preset(&color)));
        window.on_preview_toggled(|enabled| with_app(move |app| app.set_preview(enabled)));
        window.set_app_version(APP_VERSION.into());
        window.set_app_author(APP_AUTHOR.into());
        window.on_open_project(|| open_in_shell(PROJECT_URL));
        window.on_open_discord(|| open_in_shell(DISCORD_URL));
        window.on_open_logs(|| {
            if let Some(directory) = log_directory() {
                open_in_shell(&directory.display().to_string());
            }
        });
        window.window().on_close_requested(|| {
            let _ = slint::invoke_from_event_loop(|| with_app(App::close_window));
            CloseRequestResponse::HideWindow
        });

        self.window = Some(window);
        self.refresh_cameras();
        self.show_config();
        self.shared.status_pending.store(false, Ordering::Release);
        self.show_status();
        if let Some(window) = &self.window
            && let Err(error) = window.show()
        {
            tracing::error!(%error, "showing the window failed");
        }
    }

    fn close_window(&mut self) {
        self.set_preview(false);
        self.window = None;
    }

    fn set_preview(&mut self, enabled: bool) {
        self.engine.send(EngineCommand::SetPreview(enabled));
        self.shared.preview_pending.store(false, Ordering::Release);
        if let Some(window) = &self.window {
            window.set_preview_enabled(enabled);
            window.set_has_preview(false);
        }
    }

    fn show_status(&mut self) {
        self.shared.status_pending.store(false, Ordering::Release);
        let Some(window) = &self.window else {
            return;
        };
        let status = self
            .shared
            .status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let (headline, details, virtual_camera, problem) = status_lines(&status, Language::current());
        window.set_status_text(headline.into());
        window.set_detail_text(details.into());
        window.set_virtual_camera_text(virtual_camera.into());
        window.set_status_problem(problem);
        if status.state != EngineState::Running {
            window.set_has_preview(false);
        }
    }

    fn show_preview(&mut self) {
        if let Some(window) = &self.window {
            let preview = self.shared.preview.lock().unwrap_or_else(PoisonError::into_inner);
            if !preview.rgba.is_empty() {
                let buffer =
                    SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&preview.rgba, preview.width, preview.height);
                window.set_preview(Image::from_rgba8(buffer));
                window.set_has_preview(true);
            }
        }
        self.shared.preview_pending.store(false, Ordering::Release);
    }

    fn refresh_cameras(&mut self) {
        self.cameras = with_media_foundation(list_cameras)
            .and_then(Result::ok)
            .unwrap_or_default();
        let Some(window) = &self.window else {
            return;
        };
        let mut names: Vec<String> = self.cameras.iter().map(|camera| camera.name.clone()).collect();
        let configured = self.config.camera.symbolic_link.as_ref();
        let index = match configured {
            None => 0,
            Some(link) => match self.cameras.iter().position(|camera| &camera.symbolic_link == link) {
                Some(index) => index,
                None => {
                    let name = self.config.camera.name.clone().unwrap_or_else(|| link.clone());
                    names.push(format!("{name} ({})", tr("not connected", "niepodłączona")));
                    names.len() - 1
                }
            },
        };
        let message = match (
            self.cameras.is_empty(),
            configured.is_some() && index == self.cameras.len(),
        ) {
            (true, _) => tr(
                "No camera detected. Connect a camera and click Refresh.",
                "Nie wykryto żadnej kamery. Podłącz kamerę i kliknij Odśwież.",
            )
            .to_owned(),
            (false, true) => tr(
                "The saved camera is not connected. Connect it or choose another one.",
                "Zapamiętana kamera nie jest podłączona. Podłącz ją albo wybierz inną.",
            )
            .to_owned(),
            _ => String::new(),
        };
        window.set_camera_names(strings(names));
        window.set_camera_index(index as i32);
        window.set_camera_message(message.into());
        self.load_formats();
    }

    fn load_formats(&mut self) {
        let link = match self.config.camera.symbolic_link.clone() {
            Some(link) => Some(link),
            None => self.cameras.first().map(|camera| camera.symbolic_link.clone()),
        };
        self.formats = link
            .filter(|link| self.cameras.iter().any(|camera| &camera.symbolic_link == link))
            .and_then(|link| with_media_foundation(move || camera_formats(&link)))
            .and_then(Result::ok)
            .unwrap_or_default();
        let Some(window) = &self.window else {
            return;
        };
        let index = self
            .config
            .camera
            .format
            .and_then(|format| self.formats.iter().position(|f| *f == format.to_capture()))
            .map_or(0, |index| index + 1);
        window.set_format_names(strings(
            std::iter::once(auto_label()).chain(self.formats.iter().map(ToString::to_string)),
        ));
        window.set_format_index(index as i32);
    }

    fn show_config(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let config = &self.config;
        let output = OUTPUT_PRESETS
            .iter()
            .position(|preset| *preset == (config.output.width, config.output.height))
            .unwrap_or(2);
        window.set_output_index(output as i32);
        window.set_fps_index(
            FPS_PRESETS
                .iter()
                .position(|fps| *fps == config.output.fps)
                .unwrap_or(2) as i32,
        );
        window.set_rotation_index(i32::from(config.orientation.rotation % 360 / 90));
        window.set_mirror_horizontal(config.orientation.mirror_horizontal);
        window.set_mirror_vertical(config.orientation.mirror_vertical);
        window.set_effect_index(
            EFFECT_MODES
                .iter()
                .position(|mode| *mode == config.effect.mode)
                .unwrap_or(1) as i32,
        );
        window.set_blur_strength(config.effect.blur_strength * 100.0);
        window.set_color_text(config.effect.color.clone().into());
        if let Ok(color) = parse_color(&config.effect.color) {
            window.set_color_swatch(slint::Color::from_rgb_u8(color.r, color.g, color.b));
        }
        window.set_image_path(
            config
                .effect
                .image_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
                .into(),
        );
        window.set_method_index(match config.segmentation.method {
            MethodConfig::Rvm => 0,
            MethodConfig::MediaPipe => 1,
        });
        let quality = match parse_quality(&config.segmentation.quality) {
            Ok(chromafree_core::VariantPreference::Fixed(variant)) => self
                .variants
                .iter()
                .position(|v| *v == variant)
                .map_or(0, |index| index + 1),
            _ => 0,
        };
        window.set_quality_index(quality as i32);
        window.set_device_index(match config.segmentation.device {
            DeviceConfig::Gpu => 0,
            DeviceConfig::Cpu => 1,
        });
        let mask = config
            .mask_params()
            .unwrap_or_else(|| Config::default_mask_params(config.segmentation.method));
        window.set_edge_low(mask.edge_low * 100.0);
        window.set_edge_high(mask.edge_high * 100.0);
        window.set_smoothing(mask.temporal_smoothing * 100.0);
    }

    fn config_from_window(&self, window: &MainWindow) -> Config {
        let mut config = self.config.clone();
        let camera_index = window.get_camera_index().max(0) as usize;
        if let Some(camera) = self.cameras.get(camera_index) {
            config.camera.symbolic_link = Some(camera.symbolic_link.clone());
            config.camera.name = Some(camera.name.clone());
        }
        config.camera.format = (window.get_format_index() as usize)
            .checked_sub(1)
            .and_then(|index| self.formats.get(index))
            .and_then(FormatConfig::from_capture);
        if let Some((width, height)) = OUTPUT_PRESETS.get(window.get_output_index().max(0) as usize) {
            config.output.width = *width;
            config.output.height = *height;
        }
        if let Some(fps) = FPS_PRESETS.get(window.get_fps_index().max(0) as usize) {
            config.output.fps = *fps;
        }
        config.orientation.rotation = (window.get_rotation_index().clamp(0, 3) * 90) as u16;
        config.orientation.mirror_horizontal = window.get_mirror_horizontal();
        config.orientation.mirror_vertical = window.get_mirror_vertical();
        if let Some(mode) = EFFECT_MODES.get(window.get_effect_index().max(0) as usize) {
            config.effect.mode = *mode;
        }
        config.effect.blur_strength = (window.get_blur_strength() / 100.0).clamp(0.0, 1.0);
        if let Ok(color) = parse_color(&window.get_color_text()) {
            config.effect.color = format_color(color);
        }
        let method = if window.get_method_index() == 1 {
            MethodConfig::MediaPipe
        } else {
            MethodConfig::Rvm
        };
        config.segmentation.quality = (window.get_quality_index() as usize)
            .checked_sub(1)
            .and_then(|index| self.variants.get(index))
            .map_or_else(|| "auto".to_owned(), |v| format!("{}x{}", v.width, v.height));
        config.segmentation.device = if window.get_device_index() == 1 {
            DeviceConfig::Cpu
        } else {
            DeviceConfig::Gpu
        };
        if method != config.segmentation.method {
            config.segmentation.method = method;
            config.segmentation.edge_low = None;
            config.segmentation.edge_high = None;
            config.segmentation.temporal_smoothing = None;
        } else {
            let current = config
                .mask_params()
                .unwrap_or_else(|| Config::default_mask_params(method));
            let low = window.get_edge_low() / 100.0;
            let high = window.get_edge_high() / 100.0;
            let smoothing = window.get_smoothing() / 100.0;
            let changed = |a: f32, b: f32| (a - b).abs() > 0.005;
            if changed(low, current.edge_low)
                || changed(high, current.edge_high)
                || changed(smoothing, current.temporal_smoothing)
            {
                config.segmentation.edge_low = Some(low);
                config.segmentation.edge_high = Some(high.max(low + 0.01).min(1.0));
                config.segmentation.temporal_smoothing = Some(smoothing);
            }
        }
        config
    }

    fn settings_edited(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let config = self.config_from_window(window);
        self.commit(config);
    }

    fn commit(&mut self, config: Config) {
        if config == self.config {
            self.show_config();
            return;
        }
        let camera_changed = config.camera != self.config.camera;
        self.config = config;
        if let Err(error) = self.config.save(&self.config_path) {
            tracing::error!(error = format!("{error:#}"), "saving the configuration failed");
        }
        self.engine.send(EngineCommand::Apply(Box::new(self.config.clone())));
        if camera_changed && self.window.is_some() {
            self.refresh_cameras();
        }
        self.show_config();
    }

    fn camera_selected(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let index = window.get_camera_index().max(0) as usize;
        let Some(camera) = self.cameras.get(index).cloned() else {
            return;
        };
        let mut config = self.config.clone();
        config.camera.symbolic_link = Some(camera.symbolic_link);
        config.camera.name = Some(camera.name);
        config.camera.format = None;
        self.commit(config);
    }

    fn color_preset(&mut self, color: &str) {
        let mut config = self.config.clone();
        config.effect.color = color.to_owned();
        self.commit(config);
    }

    fn choose_image(&mut self) {
        match pick_background_image() {
            Ok(Some(path)) => {
                let mut config = self.config.clone();
                config.effect.image_path = Some(path);
                config.effect.mode = EffectMode::Image;
                self.commit(config);
            }
            Ok(None) => {}
            Err(error) => tracing::error!(error = format!("{error:#}"), "choosing an image failed"),
        }
    }

    fn reset_mask(&mut self) {
        let mut config = self.config.clone();
        config.segmentation.edge_low = None;
        config.segmentation.edge_high = None;
        config.segmentation.temporal_smoothing = None;
        self.commit(config);
    }

    fn reload_config(&mut self) {
        match Config::load(&self.config_path) {
            Ok(config) if config != self.config => {
                tracing::info!("configuration changed on disk");
                self.config = config;
                self.engine.send(EngineCommand::Apply(Box::new(self.config.clone())));
                if self.window.is_some() {
                    self.refresh_cameras();
                    self.show_config();
                }
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(error = format!("{error:#}"), "ignoring invalid configuration file"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chromafree_core::{BgraFrame, FrameSize};

    #[test]
    fn status_lines_describe_problems_in_each_language() {
        let mut status = EngineStatus {
            state: EngineState::CameraMissing("OBSBOT".to_owned()),
            virtual_camera: VirtualCameraState::Registered,
            ..EngineStatus::default()
        };
        let (headline, _, virtual_camera, problem) = status_lines(&status, Language::Polish);
        assert!(headline.contains("OBSBOT"));
        assert!(virtual_camera.contains("dostępna"));
        assert!(problem);
        let (_, _, virtual_camera, _) = status_lines(&status, Language::English);
        assert!(virtual_camera.contains("available"));

        status.state = EngineState::Running;
        status.fps = 30.0;
        status.consumer_format = Some(PixelFormat::Bgra);
        let (headline, details, _, problem) = status_lines(&status, Language::Polish);
        assert!(headline.contains("30 kl./s"));
        assert!(details.contains("ARGB32"));
        assert!(!problem);
    }

    #[test]
    fn preview_keeps_output_resolution_and_shows_transparency_on_a_checkerboard() {
        let size = FrameSize::new(1280, 720).unwrap();
        let mut frame = BgraFrame::new(size);
        for pixel in frame.data_mut().as_chunks_mut::<4>().0.iter_mut() {
            pixel.copy_from_slice(&[10, 20, 30, 255]);
        }
        frame.data_mut()[3] = 0;
        let mut preview = PreviewFrame::default();
        render_preview(&PipelineOutput::Bgra(&frame), ColorMatrix::Bt709, &mut preview);
        assert_eq!((preview.width, preview.height), (1280, 720));
        let light = PREVIEW_CHECKER_LIGHT;
        assert_eq!(&preview.rgba[..4], &[light, light, light, 255]);
        assert_eq!(&preview.rgba[4..8], &[30, 20, 10, 255]);

        let green = ColorMatrix::Bt709.to_yuv(chromafree_core::Rgb::GREEN_SCREEN);
        let nv12 = chromafree_core::Nv12Frame::filled(size, green);
        render_preview(&PipelineOutput::Nv12(&nv12), ColorMatrix::Bt709, &mut preview);
        let expected = ColorMatrix::Bt709.to_rgb(green);
        assert_eq!(
            &preview.rgba[preview.rgba.len() - 4..],
            &[expected.r, expected.g, expected.b, 255]
        );
        assert_eq!(aspect_label(ModelVariant::new(1280, 720)), "16:9");
        assert_eq!(aspect_label(ModelVariant::new(1080, 1440)), "3:4");
    }
}
