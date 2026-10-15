// Nyx — a Windows terminal built for latency.
//
// No console window on a release build: Nyx *is* the terminal, and a second one flashing up
// behind it on every launch is the first thing anyone would notice.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod event;
mod expand;
mod layout;
mod render;
mod term;
mod theme;
mod ui;
mod util;
mod win;

use app::NyxApp;

/// Where `log::warn!` actually ends up.
///
/// A release build sets `windows_subsystem = "windows"` so that a console does not flash up
/// behind the terminal — which also means it has **no stderr**. env_logger's default target is
/// stderr, so every diagnostic in the program was written to a handle that does not exist:
/// thirty-one `warn!` and `error!` calls, including the ones `win/` exists to produce, saying why
/// the hotkey would not register or the registry key would not write. `config::paths::log_dir`
/// had been sitting unused the whole time, which is what gave it away.
///
/// A debug build keeps stderr, because there the console is real and immediate.
///
/// Nothing here is allowed to stop Nyx starting. A log that cannot be opened falls back to
/// stderr, which in release is the same as silence — no worse than before, and never a terminal
/// that refuses to launch because of its own logging.
fn init_logging() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"));

    #[cfg(not(debug_assertions))]
    if let Some(file) = open_log() {
        builder.target(env_logger::Target::Pipe(Box::new(file)));
    }

    builder.init();
}

/// Append, with a size ceiling rather than rotation.
///
/// Appending is what makes `--new-window` safe: two processes can hold this file at once, and on
/// Windows an append is atomic, so their lines interleave instead of overwriting each other.
/// Truncating per launch would have meant the second process erasing the first one's log. The
/// ceiling is what keeps "append forever" from being a problem someone finds in a year.
#[cfg(not(debug_assertions))]
fn open_log() -> Option<std::fs::File> {
    use std::io::Write;

    const CEILING: u64 = 1024 * 1024;

    let dir = config::paths::log_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("nyx.log");

    let oversized = std::fs::metadata(&path)
        .map(|m| m.len() > CEILING)
        .unwrap_or(false);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(!oversized)
        .write(oversized)
        .truncate(oversized)
        .open(&path)
        .ok()?;

    // A banner per launch, written directly rather than through `log!` so it appears at the
    // default `warn` filter. Without it an appended file is a pile of warnings with no way to
    // tell which run produced which — and it is also the one line that proves logging works at
    // all on a run that had nothing to complain about.
    let _ = writeln!(file, "--- nyx {} starting ---", env!("CARGO_PKG_VERSION"));
    Some(file)
}

fn main() -> eframe::Result<()> {
    init_logging();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().is_some_and(|a| a == "--launcher") {
        let modifiers = raw.get(1).and_then(|a| a.parse().ok()).unwrap_or(0x4006);
        let vk = raw.get(2).and_then(|a| a.parse().ok()).unwrap_or(0x20);
        if let Err(err) = win::launcher::run(modifiers, vk) {
            log::warn!("{err}");
        }
        return Ok(());
    }
    if raw.first().is_some_and(|a| a == "--stop-launcher") {
        win::launcher::stop();
        return Ok(());
    }
    if raw.first().is_some_and(|a| a == "--remove-launcher") {
        let _ = win::launcher::configure(&config::schema::LauncherConfig::default());
        return Ok(());
    }
    let focus = raw.iter().any(|a| a == "--focus");
    let mut args = win::ipc::Args::parse(std::env::args().skip(1));
    args.request.focus = focus;
    if args.help {
        win::console::print(win::ipc::HELP);
        return Ok(());
    }
    // The two flags the installer script uses. They do their work and exit rather than opening a
    // window, so `install.ps1` can register the context menu without a terminal appearing.
    if args.install_menu || args.remove_menu {
        std::process::exit(run_menu_command(args.install_menu));
    }

    // Loaded before the window so the saved size is applied on the first frame rather than as a
    // visible resize after it.
    // A first load that does not parse is the one case where the defaults are the right answer:
    // there is no running config for it to keep. Every later load goes through the watcher,
    // which keeps what is running instead — see `config::load::Loaded`.
    let (config, config_problems) = config::load::load().or_defaults();
    if raw.first().is_some_and(|a| a == "--refresh-launcher") {
        if config.launcher.enabled {
            if let Err(err) = win::launcher::configure(&config.launcher) {
                log::warn!("{err}");
            }
        }
        return Ok(());
    }
    if let Err(err) = win::launcher::ensure(&config.launcher) {
        log::warn!("{err}");
    }
    let window = config.window.clone();

    // Single instance, decided before anything expensive happens.
    //
    // `--attach` is what Explorer's context menu passes, and it asks for this launch *only*;
    // `single_instance` asks for it always. `--new-window` overrules both, which is the escape
    // hatch for anyone who does want a second window.
    //
    // The claim is held for the life of the process. Testing it and letting go would let two
    // launches a microsecond apart both decide they were first.
    let attach = (focus || args.attach || config.window.single_instance) && !args.new_window;
    let claim = win::ipc::claim();
    if claim.is_none() && attach && win::ipc::send(&args.request) {
        return Ok(());
    }
    // Falling through with no claim means either a deliberate second window, or an instance that
    // holds the mutex but is not listening — a second window is a far better answer than nothing.
    let startup = win::ipc::Startup {
        request: args.request,
        primary: claim.is_some(),
    };
    let _claim = claim;

    // DX12 only, unless WGPU_BACKEND says otherwise.
    //
    // Letting wgpu enumerate everything costs ~46 MB of committed memory and a failed Vulkan
    // loader probe on every launch (measured: 398 MB private bytes vs 352 MB), for a backend we
    // will never select on Windows 11. `Backends::from_env` keeps the override available for
    // debugging a driver problem.
    let backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::DX12);
    let mut wgpu_options = egui_wgpu::WgpuConfiguration {
        // Vsync on, with the shallowest frame queue the driver will accept (`LOW_LATENCY` is
        // AutoVsync + a maximum frame latency of 1). A deeper queue trades keystroke latency for
        // throughput we do not need: output rate is already decoupled from frame rate by the
        // repaint coalescer.
        surface: egui_wgpu::SurfaceConfig::LOW_LATENCY,
        ..Default::default()
    };
    if let egui_wgpu::WgpuSetup::CreateNew(create) = &mut wgpu_options.wgpu_setup {
        create.instance_descriptor.backends = backends;
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_inner_size([window.width, window.height])
            .with_min_inner_size([320.0, 160.0])
            .with_title("Nyx"),
        wgpu_options,
        ..Default::default()
    };

    // NYX_BARE=1 opens the same eframe/wgpu window with no terminal in it at all. It exists to
    // answer "is this memory ours or the graphics stack's" with a measurement instead of an
    // argument, and it costs one branch at startup.
    if std::env::var_os("NYX_BARE").is_some() {
        return eframe::run_native(
            "Nyx (bare)",
            options,
            Box::new(|_cc| Ok(Box::new(Bare) as Box<dyn eframe::App>)),
        );
    }

    let mut once = Some((config, config_problems, startup));
    eframe::run_native(
        "Nyx",
        options,
        Box::new(move |cc| {
            let (config, problems, startup) = once.take().expect("app factory runs once");
            match NyxApp::new(cc, config, problems, startup) {
                Ok(app) => Ok(Box::new(app) as Box<dyn eframe::App>),
                Err(err) => Err(err.into()),
            }
        }),
    )
}

/// `--install-context-menu` / `--remove-context-menu`. Returns a process exit code.
///
/// An exit code and not just a message, because the caller is a script: `install.ps1` has to be
/// able to tell whether the menu entry is actually there without parsing text that may have gone
/// nowhere at all.
fn run_menu_command(install: bool) -> i32 {
    let exe = win::registry::executable();
    let result = if install {
        win::registry::install("Open Nyx here", &exe)
    } else {
        win::registry::remove()
    };
    match result {
        Ok(()) => {
            win::console::line(if install {
                "Added \"Open Nyx here\" to Explorer's context menu."
            } else {
                "Removed \"Open Nyx here\" from Explorer's context menu."
            });
            0
        }
        Err(err) => {
            win::console::line(&format!("failed: {err}"));
            1
        }
    }
}

struct Bare;

impl eframe::App for Bare {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.label("bare eframe + wgpu, no terminal");
    }
}
