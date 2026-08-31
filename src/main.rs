//! Desktop entry point.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use fastsapp::{app, backend, paths, settings, single_instance, util};

use clap::Parser;

/// A fast, native WhatsApp client.
#[derive(Debug, Parser)]
#[command(name = "fastsapp", version, about)]
struct Cli {
    /// Log more from the WhatsApp library.
    #[arg(short, long)]
    verbose: bool,

    /// Start with sample chats and no WhatsApp connection (for screenshots).
    #[cfg(feature = "demo")]
    #[arg(long)]
    demo: bool,

    /// What to show in demo mode: `chat`, `empty`, `settings`, `login`,
    /// `pair`, `shortcuts`, `about`, `info`, `light`, or a comma-separated
    /// mix such as `chat,light`.
    #[cfg(feature = "demo")]
    #[arg(long)]
    demo_page: Option<String>,

    /// Write a PNG of the demo window to this path and exit. Implies
    /// `--demo`.
    #[cfg(feature = "demo")]
    #[arg(long, value_name = "PATH")]
    demo_shot: Option<std::path::PathBuf>,

    /// How long to wait before the shot is taken, so fonts and icons are in.
    #[cfg(feature = "demo")]
    #[arg(long, value_name = "MS", default_value_t = 1500)]
    demo_shot_delay: u64,
}

fn main() -> eframe::Result<()> {
    let cli = Cli::parse();
    let default_filter = if cli.verbose {
        "info,fastsapp=debug,whatsapp_rust=debug,wacore=debug"
    } else {
        "warn,fastsapp=info"
    };
    let dirs = paths::AppDirs::discover();
    let dirs_ready = dirs.ensure();
    let mut logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_filter));
    // Launched from a desktop, stderr goes nowhere; keep the run's log where
    // a bug report can find it. A demo run keeps to stderr: it must not
    // replace the log of a real session that may be running alongside.
    #[cfg(feature = "demo")]
    let demo_run = cli.demo || cli.demo_shot.is_some();
    #[cfg(not(feature = "demo"))]
    let demo_run = false;
    if !demo_run {
        match std::fs::File::create(dirs.log_file()) {
            Ok(file) => {
                logger.target(env_logger::Target::Pipe(Box::new(Tee(file))));
            }
            Err(error) => eprintln!("not keeping a log file: {error}"),
        }
    }
    logger.init();
    if let Err(error) = dirs_ready {
        log::warn!("unable to create the application directories: {error}");
    }
    log_panics(dirs.panic_log());
    let settings = settings::Settings::load(&dirs.settings_file());

    let waker = backend::Waker::default();
    #[cfg(feature = "demo")]
    let demo = cli.demo || cli.demo_shot.is_some();
    #[cfg(not(feature = "demo"))]
    let demo = false;
    // A second launch surfaces the instance already running instead of
    // fighting it for the link. Held for the lifetime of the process; a
    // demo run is a throwaway next to the real one and stays out of it.
    let instance = if demo {
        None
    } else {
        match single_instance::acquire(&waker) {
            single_instance::Outcome::Only(guard) => Some(guard),
            single_instance::Outcome::Surfaced => {
                log::info!("FastsApp is already running; asked it to show its window");
                return Ok(());
            }
        }
    };
    #[allow(unused_mut)]
    let mut app = if demo {
        app::App::headless(dirs, settings).0
    } else {
        app::App::new(&waker, dirs, settings, app::AppOptions { tray: true })
    };
    if let Some(guard) = &instance {
        app.set_remote_control(guard);
    }
    #[cfg(feature = "demo")]
    if demo {
        fastsapp::demo::populate(&mut app);
        fastsapp::demo::apply_flags(&mut app, cli.demo_page.as_deref());
    }
    #[cfg(feature = "demo")]
    let shot = cli.demo_shot.clone().map(|path| Shot {
        path,
        due: std::time::Instant::now() + std::time::Duration::from_millis(cli.demo_shot_delay),
        asked: false,
    });
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(app)));

    // The application (the link, the archive, the tray) outlives any
    // window. Closing to the tray destroys the window, and this loop
    // creates a new one when the tray, a notification, or another launch
    // asks for it. Plain window lifecycle, portable across desktops.
    loop {
        let creator_slot = std::sync::Arc::clone(&slot);
        let creator_waker = waker.clone();
        #[cfg(feature = "demo")]
        let creator_shot = shot.clone();
        eframe::run_native(
            "FastsApp",
            native_options(),
            Box::new(move |cc| {
                creator_waker.attach(&cc.egui_ctx);
                let mut app = creator_slot
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                    .expect("application state present");
                app.attach(&cc.egui_ctx);
                Ok(Box::new(Shell {
                    app: Some(app),
                    slot: std::sync::Arc::clone(&creator_slot),
                    #[cfg(feature = "demo")]
                    shot: creator_shot,
                }))
            }),
        )?;
        waker.detach();

        let hide = {
            let guard = slot.lock().unwrap_or_else(|p| p.into_inner());
            let app = guard.as_ref().expect("application state present");
            !app.quit_requested && app.hide_intent
        };
        if !hide {
            break;
        }

        // Tray life: no window, but the link, the archive, and the tray
        // keep going until Show or Quit.
        let headless = egui::Context::default();
        slot.lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
            .expect("application state present")
            .window_gone();
        loop {
            {
                let mut guard = slot.lock().unwrap_or_else(|p| p.into_inner());
                let app = guard.as_mut().expect("application state present");
                app.background_frame(&headless);
                if app.quit_requested || app.wants_show {
                    break;
                }
            }
            fastsapp::tray::idle(std::time::Duration::from_millis(150));
        }
        let quit = slot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .expect("application state present")
            .quit_requested;
        if quit {
            break;
        }
    }

    if let Some(mut app) = slot.lock().unwrap_or_else(|p| p.into_inner()).take() {
        app.shutdown();
    }
    drop(instance);
    Ok(())
}

/// Every log line goes to stderr and to the log file.
struct Tee(std::fs::File);

impl std::io::Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        self.0.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        self.0.flush()
    }
}

/// Records every panic in `path` before the process dies of it.
fn log_panics(path: std::path::PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        let thread = std::thread::current();
        let entry = format!(
            "{} fastsapp {} on thread {:?}: {info}\n",
            jiff::Timestamp::now(),
            env!("CARGO_PKG_VERSION"),
            thread.name().unwrap_or("unnamed"),
        );
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path);
        if let Ok(mut file) = file {
            use std::io::Write;
            let _ = file.write_all(entry.as_bytes());
        }
    }));
}

fn native_options() -> eframe::NativeOptions {
    let viewport = egui::ViewportBuilder::default()
        .with_title("FastsApp")
        .with_app_id("fastsapp")
        .with_inner_size([1180.0, 780.0])
        .with_min_inner_size([720.0, 480.0])
        .with_icon(app_icon())
        // macOS: no title bar strip above the app. The content runs to the
        // top edge and the traffic lights float over it; the interface
        // leaves room for them with `theme::titlebar_inset`. Nothing
        // changes elsewhere.
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false);
    eframe::NativeOptions {
        viewport,
        // A Wayland compositor stops sending frame callbacks to a window on
        // a hidden workspace; waiting for vsync there would block the event
        // loop, leave the compositor's pings unanswered, and Hyprland then
        // offers to terminate the app. Repaints are event-driven, so
        // nothing spins without vsync.
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The eframe adapter around the long-lived [`app::App`]: delegates frames
/// and, when the window goes away, hands the state back for the next one.
struct Shell {
    app: Option<app::App>,
    slot: std::sync::Arc<std::sync::Mutex<Option<app::App>>>,
    #[cfg(feature = "demo")]
    shot: Option<Shot>,
}

impl Drop for Shell {
    fn drop(&mut self) {
        *self.slot.lock().unwrap_or_else(|p| p.into_inner()) = self.app.take();
    }
}

/// A screenshot the window still owes us.
#[cfg(feature = "demo")]
#[derive(Clone)]
struct Shot {
    path: std::path::PathBuf,
    due: std::time::Instant,
    asked: bool,
}

#[cfg(feature = "demo")]
impl Shell {
    fn drive_shot(&mut self, ctx: &egui::Context) {
        let Some(shot) = self.shot.as_mut() else {
            return;
        };
        ctx.request_repaint();
        if !shot.asked && std::time::Instant::now() >= shot.due {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            shot.asked = true;
        }
        let image = ctx.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else {
            return;
        };
        let [width, height] = [image.size[0] as u32, image.size[1] as u32];
        let pixels: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_srgba_unmultiplied())
            .collect();
        match image::RgbaImage::from_raw(width, height, pixels) {
            Some(buffer) => match buffer.save(&shot.path) {
                Ok(()) => log::info!("wrote {}x{} to {}", width, height, shot.path.display()),
                Err(error) => log::error!("could not write {}: {error}", shot.path.display()),
            },
            None => log::error!("the frame buffer did not match {width}x{height}"),
        }
        self.shot = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for Shell {
    /// egui's memory (scroll positions, pending scroll animations, text
    /// cursors) is not worth keeping across runs, and a pending animation
    /// restored from a previous run's clock would hold a chat short of its
    /// end until that clock is reached. The window's size and place are
    /// kept regardless.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(app) = self.app.as_mut() {
            app.background_frame(ctx);
        }
        #[cfg(feature = "demo")]
        self.drive_shot(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(app) = self.app.as_mut() {
            app.frame_ui(ui);
        }
    }

    /// The window is closing; the app may live on in the tray, so only what
    /// must not be lost is written now.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(app) = self.app.as_mut() {
            app.save_state();
        }
    }
}

fn app_icon() -> egui::IconData {
    const SIZE: usize = 128;
    egui::IconData {
        rgba: util::app_icon_rgba(SIZE),
        width: SIZE as u32,
        height: SIZE as u32,
    }
}
