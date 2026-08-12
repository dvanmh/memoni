use anyhow::{Result, anyhow, bail};
use egui::Modifiers;
use env_logger::TimestampPrecision;
use log::{LevelFilter, debug, info, warn};
use memoni::AppMode;
use memoni::config::Config;
use memoni::input::Input;
use memoni::keymap_action::{
    KeyAction, KeymapAction, PasteModifier, PointerAction, SimpleScrollAction,
};
use memoni::persistence::Persistence;
use memoni::selection::Selection;
use memoni::timerfd_source::TimerfdSource;
use memoni::ui::{Ui, UiFlow};
use memoni::x11_key_converter::X11KeyConverter;
use memoni::x11_window::X11Window;
use memoni::{opengl_context::OpenGLContext, selection::SelectionType};
use mio::unix::SourceFd;
use signal_hook::consts::TERM_SIGNALS;
use signal_hook_mio::v1_0::Signals;
use std::os::unix::net::{UnixListener, UnixStream};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    os::fd::{AsFd as _, AsRawFd as _},
    path::Path,
    time::Duration,
};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::Mapping;
use x11rb::xcb_ffi::XCBConnection;

const SOCKET_DIR: &str = "/tmp/memoni/";
const X11_TOKEN: mio::Token = mio::Token(0);
const SIGNAL_TOKEN: mio::Token = mio::Token(1);
const MEMONI_TOKEN: mio::Token = mio::Token(2);
const KEYBOARD_GRAB_RETRY_TOKEN: mio::Token = mio::Token(3);
const POINTER_GRAB_RETRY_TOKEN: mio::Token = mio::Token(4);

enum Args {
    Client(ClientArgs),
    Server(ServerArgs),
}

#[derive(Debug)]
struct ClientArgs {
    selection: SelectionType,
}

#[derive(Debug)]
struct ServerArgs {
    selection: SelectionType,
}

fn main() -> Result<()> {
    let (args, log_level) = parse_args()?;

    env_logger::Builder::new()
        .filter_level(log_level)
        .format_timestamp(Some(TimestampPrecision::Millis))
        .target(env_logger::Target::Stdout)
        .init();
    info!("logger initialized at level: {log_level}");

    let display_id = std::env::var("DISPLAY")
        .inspect_err(|e| warn!("failed to read DISPLAY environment variable: {}", e))
        .ok()
        .as_deref()
        .and_then(|s| s.strip_prefix(':'))
        .map(|s| s.to_string())
        .filter(|s| s != "0"); // only use if it's not the default display ":0"

    debug!("ensuring socket dir exists: {SOCKET_DIR}");
    let socket_dir = Path::new(SOCKET_DIR);
    fs::create_dir_all(socket_dir)?;

    match args {
        Args::Client(args) => {
            info!("starting client mode with selection: {}", args.selection);
            debug!("client args: {args:#?}");

            let socket_file_name = if let Some(id) = &display_id {
                format!("{}_{}.sock", args.selection, id)
            } else {
                format!("{}.sock", args.selection)
            };
            let socket_path = socket_dir.join(socket_file_name);
            client(args, &socket_path, display_id)?
        }
        Args::Server(args) => {
            info!("starting server mode with selection: {}", args.selection);
            debug!("server args: {args:#?}");

            let socket_file_name = if let Some(id) = &display_id {
                format!("{}_{}.sock", args.selection, id)
            } else {
                format!("{}.sock", args.selection)
            };
            let socket_path = socket_dir.join(socket_file_name);
            server(args, &socket_path, display_id)?
        }
    }

    Ok(())
}

fn parse_args() -> Result<(Args, LevelFilter)> {
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_env();
    let is_server_mode = parser.try_raw_args().is_some_and(|mut raw_args| {
        let is_server_mode = raw_args.peek().is_some_and(|a| a.eq(OsStr::new("server")));
        if is_server_mode {
            raw_args.next();
        }
        is_server_mode
    });

    let mut selection_type = SelectionType::CLIPBOARD;
    let mut log_level = LevelFilter::Warn;
    let mut shows_help = false;
    let mut shows_version = false;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('s') | Long("selection") => {
                let selection_str: String = parser.value()?.parse()?;
                selection_type = match selection_str.as_str() {
                    "PRIMARY" => SelectionType::PRIMARY,
                    "CLIPBOARD" => SelectionType::CLIPBOARD,
                    _ => bail!("invalid selection type \"{selection_str}\""),
                };
            }
            Short('l') | Long("log-level") => {
                log_level = parser.value()?.parse().map_err(|err| match err {
                    lexopt::Error::ParsingFailed { value, .. } => {
                        anyhow!("invalid log level \"{value}\"")
                    }
                    _ => err.into(),
                })?;
            }
            Short('v') | Long("version") if !is_server_mode => {
                shows_version = true;
            }
            Short('h') | Long("help") => {
                shows_help = true;
            }
            _ => return Err(arg.unexpected().into()),
        }
    }

    if shows_help {
        if is_server_mode {
            println!(
                        "\
Start memoni server.

USAGE:
  memoni server [OPTIONS]

OPTIONS:
  -s, --selection TYPE    Sets selection type [possible values: CLIPBOARD, PRIMARY] [default: CLIPBOARD]
  -l, --log-level LEVEL   Sets log level [possible values: off, error, warn, info, debug, trace] [default: warn]
  -h, --help              Prints help information"
                    );
        } else {
            println!(
                        "\
Show memoni window if memoni server is running.
To run in server mode, use: memoni server [OPTIONS]

USAGE:
  memoni [OPTIONS]

OPTIONS:
  -s, --selection TYPE    Sets selection type [possible values: CLIPBOARD, PRIMARY] [default: CLIPBOARD]
  -l, --log-level LEVEL   Sets log level [possible values: off, error, warn, info, debug, trace] [default: warn]
  -v, --version           Prints memoni version
  -h, --help              Prints help information"
                    );
        }
        std::process::exit(0);
    }

    if shows_version {
        println!("v{}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    Ok((
        if is_server_mode {
            Args::Server(ServerArgs {
                selection: selection_type,
            })
        } else {
            Args::Client(ClientArgs {
                selection: selection_type,
            })
        },
        log_level,
    ))
}

fn client(args: ClientArgs, socket_path: &Path, display_id: Option<String>) -> Result<()> {
    if !fs::exists(socket_path)? {
        eprintln!(
            "Error: memoni server for selection \"{}\"{} is not running",
            args.selection,
            display_id
                .map(|id| format!(" on display {:?}", id))
                .unwrap_or_default()
        );
        std::process::exit(1);
    }

    debug!("connecting to socket: {socket_path:?}");
    let mut stream = UnixStream::connect(socket_path)?;

    info!("sending 'show_win' to server");
    stream.write_all(b"show_win")?;

    Ok(())
}

fn server(args: ServerArgs, socket_path: &Path, display_id: Option<String>) -> Result<()> {
    let config = Config::load(args.selection)?;

    let window = X11Window::new(&config, args.selection)?;
    let mut gl_context = OpenGLContext::new(&window, &config)?;
    let key_converter = X11KeyConverter::new(&window.conn)?;
    let mut input = Input::new(&window, &key_converter)?;
    let mut keymap_action = KeymapAction::new()?;

    let mut persistence = Persistence::new(args.selection, &display_id)?;
    let mut selection = Selection::new(
        persistence.load_selection_data()?,
        &window,
        &key_converter,
        args.selection,
        &config,
        // XFixes sends a SelectionNotify for each change while the user drags the mouse to adjust selection.
        // Debounce to merge consecutive items with similar text.
        args.selection == SelectionType::PRIMARY,
    )?;
    let mut ui = Ui::new(&config)?;
    for (_, item) in &selection.items {
        ui.build_button_widget(item)?;
    }

    let (mut poll, socket_listener, mut signals, keyboard_grab_timer, pointer_grab_timer) =
        match create_poll(&window.conn, socket_path) {
            Ok(res) => res,
            Err(err) => {
                if let Some(io_err) = err.downcast_ref::<io::Error>()
                    && io_err.kind() == io::ErrorKind::AddrInUse
                {
                    eprintln!(
                        "Error: another server for selection \"{}\"{} is already running",
                        args.selection,
                        display_id
                            .map(|id| format!(" on display {:?}", id))
                            .unwrap_or_default()
                    );
                    std::process::exit(1);
                } else {
                    return Err(err);
                }
            }
        };
    let mut poll_events = mio::Events::with_capacity(8);

    let main_loop_result = (|| -> Result<()> {
        let mut window_shown = false;
        let mut pointer_button_press_count = 0;
        let mut active_id = selection
            .items
            .get_by_index(0)
            .map(|(id, _)| *id)
            .unwrap_or(0);
        let mut mode = AppMode::Normal;
        let mut first_loop = true;

        info!("starting main event loop");
        'main_loop: loop {
            let mut will_show_window = false;
            let mut will_hide_window = false;
            let mut paste_item_id = None;
            let mut paste_modifier = PasteModifier::default();

            // non-blocking when window is visible, blocking otherwise
            let poll_timeout = if window_shown {
                Some(Duration::ZERO)
            } else {
                None
            };
            poll.poll(&mut poll_events, poll_timeout).or_else(|e| {
                if e.kind() == io::ErrorKind::Interrupted {
                    // We get interrupt when a signal happens inside poll. That's non-fatal, just
                    // retry.
                    poll_events.clear();
                    Ok(())
                } else {
                    Err(e)
                }
            })?;
            for event in &poll_events {
                match event.token() {
                    X11_TOKEN => {} // handled below
                    SIGNAL_TOKEN => {
                        if let Some(raw_signal) = signals.pending().next()
                            && let Some(signal) =
                                rustix::process::Signal::from_named_raw(raw_signal)
                        {
                            info!("received {signal:?}, stopping main event loop");
                            break 'main_loop;
                        }
                    }
                    MEMONI_TOKEN => {
                        info!("accepting client connection");
                        let (mut stream, _) = socket_listener.accept()?;

                        let mut buf = [0u8; 1024];
                        match stream.read(&mut buf) {
                            Ok(0) => {
                                warn!("client closed without sending command");
                            }
                            Ok(n) => {
                                let command = String::from_utf8_lossy(&buf[..n]);
                                match command.as_ref() {
                                    "show_win" => {
                                        info!("received client command: {command}, showing window");
                                        will_show_window = true;
                                    }
                                    _ => {
                                        warn!("unknown client command: {command}");
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("failed to read client command: {e:?}");
                            }
                        }
                    }
                    KEYBOARD_GRAB_RETRY_TOKEN => {
                        keyboard_grab_timer.clear_event()?;
                        window.grab_keyboard(&keyboard_grab_timer)?;
                    }
                    POINTER_GRAB_RETRY_TOKEN => {
                        pointer_grab_timer.clear_event()?;
                        window.grab_pointer(&pointer_grab_timer)?;
                    }
                    _ => unreachable!(),
                }
            }

            let mut items_updated = false;
            while let Some(event) = window.conn.poll_for_event()? {
                if let Event::Error(err) = event {
                    warn!("received X11 error: {err:?}");
                    continue;
                }

                if let Event::DestroyNotify(ev) = event
                    && ev.window == window.win_id.get()
                {
                    warn!("main window {} got destroyed", ev.window);
                    window.recreate_main_window()?;
                    gl_context.recreate_painter()?;

                    input = Input::new(&window, &key_converter)?;
                    ui.reset_context();
                    for (_, item) in &selection.items {
                        ui.build_button_widget(item)?;
                    }

                    continue;
                }

                if let Event::MappingNotify(ev) = event
                    && (ev.request == Mapping::KEYBOARD || ev.request == Mapping::MODIFIER)
                {
                    key_converter.update_mapping()?;
                    continue;
                }

                if let Event::ButtonPress(_) = event {
                    pointer_button_press_count += 1;
                }

                // When clicking outside of the window, only a release event is sent
                if let Event::ButtonRelease(ev) = event {
                    if pointer_button_press_count == 0 {
                        if ev.event != window.win_id.get() {
                            info!("pointer released outside window, hiding window");
                            will_hide_window = true;
                        }
                    } else {
                        pointer_button_press_count -= 1;
                    }
                }

                input.handle_event(&event);
                if let Some((new_selection_item, removed_selection_items)) =
                    selection.handle_event(&event)?
                {
                    ui.remove_button_widgets(removed_selection_items);
                    if let Some(new_item) = new_selection_item {
                        ui.build_button_widget(new_item)?;
                    }

                    persistence.save_selection_data(&selection.items, &selection.metadata)?;
                    items_updated = true;
                }
            }

            if will_show_window && window_shown {
                debug!("window already shown, ignoring show request");
                will_show_window = false;
            }

            if will_hide_window && !window_shown {
                debug!("window already hidden, ignoring hide request");
                will_hide_window = false;
            }

            if will_show_window {
                mode = AppMode::Normal;
                window.update_window_pos()?;
                input.update_pointer_pos()?;
                ui.reset();
                active_id = selection
                    .items
                    .get_by_index(selection.metadata.pinned_count)
                    .map(|(id, _)| *id)
                    .unwrap_or(0);
            }

            if first_loop || items_updated || window_shown || will_show_window {
                let (key_actions, pointer_actions) =
                    keymap_action.process_input(&mut input.egui_input, mode);
                let mut scroll_actions = vec![];
                for action in key_actions {
                    match action {
                        KeyAction::Paste(modifier) => {
                            info!("paste item {active_id} selected by key action, hiding window");
                            will_hide_window = true;
                            paste_item_id = Some(active_id);
                            paste_modifier = modifier;
                        }
                        KeyAction::Scroll(scroll_action) => scroll_actions.push(scroll_action),
                        KeyAction::Remove => {
                            let is_pinned = selection
                                .items
                                .iter()
                                .take(selection.metadata.pinned_count)
                                .any(|(&id, _)| id == active_id);
                            if is_pinned {
                                info!("ignoring remove action on pinned item {active_id}");
                                ui.show_error("Cannot remove pinned item");
                            } else {
                                let removed_item = selection.items.remove(&active_id);
                                if let Some(item) = removed_item {
                                    ui.remove_button_widgets(std::iter::once(item));
                                }
                                info!("selection item {active_id} removed");
                                persistence
                                    .save_selection_data(&selection.items, &selection.metadata)?;
                            }
                        }
                        KeyAction::Pin => {
                            let is_pinned = selection.toggle_pin(active_id)?;
                            if is_pinned {
                                info!("selection item {active_id} pinned");
                            } else {
                                info!("selection item {active_id} unpinned");
                            }
                            persistence
                                .save_selection_data(&selection.items, &selection.metadata)?;
                        }
                        KeyAction::QuickPaste(index) => {
                            if let Some((&id, _)) = selection.items.get_by_index(index) {
                                info!(
                                    "quickpaste item {id} (index {index}) selected by key action, hiding window"
                                );
                                will_hide_window = true;
                                paste_item_id = Some(id);
                            }
                        }

                        KeyAction::ShowHelp => {
                            info!("switching to Help mode");
                            mode = AppMode::Help;
                        }
                        KeyAction::SimpleScroll(direction) => {
                            let key = match direction {
                                SimpleScrollAction::Up => egui::Key::ArrowUp,
                                SimpleScrollAction::Down => egui::Key::ArrowDown,
                            };
                            input.egui_input.events.push(egui::Event::Key {
                                key,
                                physical_key: None,
                                pressed: true,
                                repeat: false,
                                modifiers: Modifiers::NONE,
                            });
                            input.egui_input.events.push(egui::Event::Key {
                                key,
                                physical_key: None,
                                pressed: false,
                                repeat: false,
                                modifiers: Modifiers::NONE,
                            });
                        }

                        KeyAction::Close => match mode {
                            AppMode::Normal => {
                                info!("received Close action in Normal mode, hiding window");
                                will_hide_window = true;
                            }
                            AppMode::Help => {
                                info!("switching to Normal mode");
                                mode = AppMode::Normal;
                            }
                        },
                    }
                }

                let ui_flow = if window.is_win_placed_above_pointer() {
                    UiFlow::BottomToTop
                } else {
                    UiFlow::TopToBottom
                };
                let (full_output, clicked_item) = ui.run(
                    input.egui_input.take(),
                    &mut active_id,
                    &selection.items,
                    &selection.metadata,
                    ui_flow,
                    &scroll_actions,
                    &mut keymap_action.pending_keys,
                    mode == AppMode::Help,
                )?;

                if let Some(clicked_id) = clicked_item {
                    for action in pointer_actions {
                        match action {
                            PointerAction::Paste(modifier) => {
                                info!("paste item {clicked_id} selected by pointer, hiding window");
                                will_hide_window = true;
                                paste_item_id = Some(clicked_id);
                                paste_modifier = modifier;
                            }
                        }
                    }
                } else if !pointer_actions.is_empty() {
                    debug!("pointer actions received when no items getting clicked");
                }

                gl_context.render(&ui.egui_ctx, full_output)?;
            }

            if will_show_window {
                window.show_window()?;
                window.grab_keyboard(&keyboard_grab_timer)?;
                window.grab_pointer(&pointer_grab_timer)?;
                window.enable_events()?;
                window.conn.flush()?;
                window_shown = true;
                info!("window shown");
            }

            if will_hide_window {
                window.hide_window()?;
                window.cancel_grab_retries(&keyboard_grab_timer, &pointer_grab_timer)?;
                window.ungrab_input()?;
                window.disable_events()?;
                window.conn.flush()?;
                window_shown = false;
                input.egui_input.modifiers = Modifiers::NONE;
                info!("window hidden");
            }

            if let Some(id) = paste_item_id {
                selection.paste(id, window.win_opened_pointer_pos.get(), paste_modifier)?;
            }

            first_loop = false;
        }
        Ok(())
    })();

    info!("cleaning up");
    window.ungrab_input()?;
    gl_context.destroy();
    debug!("removing socket file");
    fs::remove_file(socket_path)?;

    main_loop_result
}

fn create_poll<P: AsRef<Path> + std::fmt::Debug>(
    conn: &XCBConnection,
    socket_path: P,
) -> Result<(
    mio::Poll,
    UnixListener,
    Signals,
    TimerfdSource,
    TimerfdSource,
)> {
    let poll = mio::Poll::new()?;

    debug!("registering X11 events polling source");
    let conn_fd = conn.as_fd().as_raw_fd();
    poll.registry()
        .register(&mut SourceFd(&conn_fd), X11_TOKEN, mio::Interest::READABLE)?;

    debug!(
        "registering signals polling source: {:?}",
        TERM_SIGNALS
            .iter()
            .map(|s| rustix::process::Signal::from_named_raw(*s).unwrap())
            .collect::<Vec<_>>()
    );
    let mut signals = Signals::new(TERM_SIGNALS)?;
    poll.registry()
        .register(&mut signals, SIGNAL_TOKEN, mio::Interest::READABLE)?;

    debug!("registering socket source: {socket_path:?}");
    if fs::exists(&socket_path)?
        && let Err(err) = UnixStream::connect(&socket_path)
        && err.kind() == io::ErrorKind::ConnectionRefused
    {
        debug!("socket file exists but isn't in use, removing it");
        fs::remove_file(&socket_path)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    listener.set_nonblocking(true)?;
    poll.registry().register(
        &mut SourceFd(&listener.as_raw_fd()),
        MEMONI_TOKEN,
        mio::Interest::READABLE,
    )?;

    debug!("registering keyboard grab retry timer source");
    let keyboard_grab_timer =
        TimerfdSource::new().map_err(|e| anyhow!("failed to create keyboard grab timerfd: {e}"))?;
    poll.registry().register(
        &mut SourceFd(&keyboard_grab_timer.as_fd().as_raw_fd()),
        KEYBOARD_GRAB_RETRY_TOKEN,
        mio::Interest::READABLE,
    )?;

    debug!("registering pointer grab retry timer source");
    let pointer_grab_timer =
        TimerfdSource::new().map_err(|e| anyhow!("failed to create pointer grab timerfd: {e}"))?;
    poll.registry().register(
        &mut SourceFd(&pointer_grab_timer.as_fd().as_raw_fd()),
        POINTER_GRAB_RETRY_TOKEN,
        mio::Interest::READABLE,
    )?;

    Ok((
        poll,
        listener,
        signals,
        keyboard_grab_timer,
        pointer_grab_timer,
    ))
}
