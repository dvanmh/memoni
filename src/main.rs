use anyhow::{Result, anyhow, bail};
use egui::Modifiers;
use env_logger::TimestampPrecision;
use log::{LevelFilter, debug, info, warn};
use memoni::config::{Config, PauseDuration, WindowPositionMode};
use memoni::item_list::ItemList;
use memoni::keymap_action::KeymapAction;
use memoni::keymap_spec::{KeyAction, PasteModifier, PointerAction, SimpleScrollAction};
use memoni::paste::PasteServer;
use memoni::persistence::Persistence;
use memoni::search::Search;
use memoni::selection::{EventHandlingResult, Selection};
use memoni::timerfd_source::TimerfdSource;
use memoni::ui::{Ui, UiFlow};
use memoni::x11_key_converter::X11KeyConverter;
use memoni::x11_window::X11Window;
use memoni::{AppMode, ordered_hash_map::OrderedHashMapView};
use memoni::{
    atspi_caret_tracker::{self, AtspiCaretTracker, CaretTrackerEvent},
    selection::Pause,
};
use memoni::{input::Input, utils::x11_cursor_name};
use memoni::{opengl_context::OpenGLContext, selection::SelectionType};
use mio::{Waker, unix::SourceFd};
use signal_hook::consts::TERM_SIGNALS;
use signal_hook_mio::v1_0::Signals;
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    os::fd::{AsFd as _, AsRawFd as _},
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use std::{
    os::unix::net::{UnixListener, UnixStream},
    process,
    str::FromStr,
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
const ATSPI_TOKEN: mio::Token = mio::Token(5);

enum Args {
    Client(ClientArgs),
    Server(ServerArgs),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ClientCommand {
    ShowWin,
    HideWin,
    Pause(PauseCmd),
    Resume,
    PauseState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PauseCmd {
    For(Option<PauseDuration>),
    Next(u32),
}

#[derive(Debug)]
struct ClientArgs {
    selection: SelectionType,
    command: ClientCommand,
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
    let mut command = ClientCommand::ShowWin;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('s') | Long("selection") => {
                let selection_str: String = parser.value()?.parse()?;
                selection_type = match selection_str.as_str() {
                    "PRIMARY" => SelectionType::PRIMARY,
                    "CLIPBOARD" => SelectionType::CLIPBOARD,
                    _ => bail!("invalid selection type {selection_str:?}"),
                };
            }
            Short('l') | Long("log-level") => {
                log_level = parser.value()?.parse().map_err(|err| match err {
                    lexopt::Error::ParsingFailed { value, .. } => {
                        anyhow!("invalid log level {value:?}")
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
            Value(ref subcommand) if !is_server_mode => {
                let subcommand = subcommand.to_string_lossy();
                command = match subcommand.as_ref() {
                    "show" => ClientCommand::ShowWin,
                    "hide" => ClientCommand::HideWin,
                    "pause" => {
                        let arg = match parser.value() {
                            Ok(value) => Some(value.to_string_lossy().into_owned()),
                            Err(lexopt::Error::MissingValue { .. }) => None,
                            Err(err) => return Err(err.into()),
                        };
                        match arg.as_deref() {
                            None => ClientCommand::Pause(PauseCmd::For(None)),
                            Some("resume") => ClientCommand::Resume,
                            Some("state") => ClientCommand::PauseState,
                            Some("next") => {
                                let arg = match parser.value() {
                                    Ok(value) => Some(value.to_string_lossy().into_owned()),
                                    Err(lexopt::Error::MissingValue { .. }) => None,
                                    Err(err) => return Err(err.into()),
                                };
                                let count = match arg.as_deref() {
                                    None => 1,
                                    Some(raw) => match raw.parse() {
                                        Ok(0) => bail!("pause next count must be at least 1"),
                                        Ok(count) => count,
                                        Err(_) => bail!("invalid pause next count: {raw:?}"),
                                    },
                                };
                                ClientCommand::Pause(PauseCmd::Next(count))
                            }
                            Some(raw) => {
                                let duration = match PauseDuration::from_str(raw) {
                                    Ok(d) => d,
                                    Err(err) => {
                                        return Err((lexopt::Error::ParsingFailed {
                                            value: raw.to_string(),
                                            error: Box::new(err),
                                        })
                                        .into());
                                    }
                                };
                                ClientCommand::Pause(PauseCmd::For(Some(duration)))
                            }
                        }
                    }
                    _ => return Err(arg.unexpected().into()),
                };
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
Control the memoni server if it is running.
To run in server mode, use: memoni server [OPTIONS]

USAGE:
  memoni [OPTIONS] [COMMAND]

  If no command is given, `show` is used.

COMMANDS:
  show
     Shows memoni window

  hide
     Hides memoni window

  pause [DURATION|next [N]|inf|resume|state]
     Pauses recording new clipboard items. Each pause command replaces the current pause condition.

     (default)
        Stops recording new clipboard items. Auto-resume follows pause_auto_resume_after config

     DURATION
        Stops recording new clipboard items and auto-resumes after DURATION.

        DURATION format:
           <n>s, <n>m, <n>h or their combination (e.g., '1h30m');
           a bare number is seconds (e.g., '90' is 90 seconds)
        e.g., `pause 30m`, `pause 1h30m`, `pause 90`

     next [N]
        Stops recording new clipboard items for the next N clipboard changes received.
        This count is independent of whether the item would be recorded (e.g., copies
        from blocked apps still count). Default for N is 1.
        e.g., `pause next`, `pause next 3`

     inf
        Stops recording new clipboard items until `memoni pause resume` is run

     resume
        Resumes recording new clipboard items

     state
        Prints the current pause state

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
                command,
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

    let command = match &args.command {
        ClientCommand::ShowWin => "show_win".to_string(),
        ClientCommand::HideWin => "hide_win".to_string(),
        ClientCommand::Pause(pause_cmd) => match pause_cmd {
            PauseCmd::For(Some(d)) => format!("pause_for_{d}"),
            PauseCmd::For(None) => "pause_for_".to_string(),
            PauseCmd::Next(n) => format!("pause_next_{n}"),
        },
        ClientCommand::Resume => "pause_resume".to_string(),
        ClientCommand::PauseState => "pause_state".to_string(),
    };

    debug!("connecting to socket: {socket_path:?}");
    let mut stream = UnixStream::connect(socket_path)?;

    info!("sending '{command}' to server");
    stream.write_all(command.as_bytes())?;

    let mut reply = String::new();
    if let Ok(n) = stream.read_to_string(&mut reply)
        && n > 0
    {
        if let Some(err) = reply.strip_prefix("err:") {
            eprintln!("Error: {err}");
            process::exit(1);
        }
        println!("{}", reply.strip_prefix("ok:").unwrap_or(&reply));
    }

    Ok(())
}

fn server(args: ServerArgs, socket_path: &Path, display_id: Option<String>) -> Result<()> {
    let config = Config::load(args.selection)?;

    let window = X11Window::new(&config, args.selection)?;
    let mut gl_context = OpenGLContext::new(&window, &config)?;
    let key_converter = X11KeyConverter::new(&window.conn)?;
    let mut input = Input::new(&window, &key_converter)?;
    let mut keymap_action = KeymapAction::new()?;
    let mut search = Search::new();

    let persistence = Persistence::new(args.selection, &display_id)?;
    let (history, mut bookmarks) = persistence.load_selection_data()?;
    let mut selection = Selection::new(
        history,
        &window,
        args.selection,
        &config,
        // XFixes sends a SelectionNotify for each change while the user drags the mouse to adjust selection.
        // Debounce to merge consecutive items with similar text.
        args.selection == SelectionType::PRIMARY,
    )?;
    let mut paster = PasteServer::new(&window, &key_converter, args.selection, &config)?;

    let mut ui = Ui::new(&config)?;
    for (_, item) in selection.history.iter().chain(bookmarks.iter()) {
        ui.build_button_widget(item)?;
    }

    let text_caret_requested = config
        .window_position_mode
        .contains(&WindowPositionMode::TextCaret);
    let text_caret_command = if text_caret_requested {
        config.text_caret_command.clone()
    } else {
        None
    };
    let text_caret_tracking_enabled = text_caret_requested && text_caret_command.is_none();
    let mut sources = match create_poll(&window.conn, socket_path, text_caret_tracking_enabled) {
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

    if let Some(channel) = &sources.caret_tracker_channel
        && let Err(err) = AtspiCaretTracker::new(channel.clone())
    {
        warn!("AT-SPI cannot be initiated: {err:?}");
    }

    let main_loop_result = (|| -> Result<()> {
        let mut state = ServerState {
            window_shown: false,
            pointer_button_press_count: 0,
            bookmark_view: false,
            active_id: selection.history.first_unpinned_id(),
            mode: AppMode::Normal,
            prev_mode: AppMode::Normal,
            first_loop: true,
        };

        info!("starting main event loop");
        'main_loop: loop {
            let mut will_show_window = false;
            let mut will_hide_window = false;
            let mut paste_intent = None;

            // non-blocking when window is visible or first-loop pre-rendering, blocking otherwise
            sources
                .poll
                .poll(
                    &mut poll_events,
                    if state.window_shown || state.first_loop {
                        Some(Duration::ZERO)
                    } else {
                        None
                    },
                )
                .or_else(|e| {
                    if e.kind() == io::ErrorKind::Interrupted {
                        // We get interrupt when a signal happens inside poll. That's non-fatal, just
                        // retry.
                        poll_events.clear();
                        Ok::<(), anyhow::Error>(())
                    } else {
                        Err(e.into())
                    }
                })?;
            for event in &poll_events {
                match event.token() {
                    X11_TOKEN => {} // handled below
                    SIGNAL_TOKEN => {
                        if let Some(raw_signal) = sources.signals.pending().next()
                            && let Some(signal) =
                                rustix::process::Signal::from_named_raw(raw_signal)
                        {
                            info!("received {signal:?}, stopping main event loop");
                            break 'main_loop;
                        }
                    }
                    MEMONI_TOKEN => {
                        debug!("accepting client connection");
                        let (mut stream, _) = sources.socket_listener.accept()?;

                        let mut reply = None;
                        let mut buf = [0u8; 1024];
                        match stream.read(&mut buf) {
                            Ok(0) => {
                                warn!("client closed without sending command");
                            }
                            Ok(n) => {
                                let command = String::from_utf8_lossy(&buf[..n]);
                                match command.as_ref() {
                                    "show_win" => {
                                        info!(
                                            "received client command '{command}', showing window"
                                        );
                                        will_show_window = true;
                                    }
                                    "hide_win" => {
                                        info!("received client command '{command}', hiding window");
                                        will_hide_window = true;
                                    }
                                    cmd if let Some(cmd) = cmd.strip_prefix("pause_") => {
                                        match cmd {
                                            "state" => {
                                                debug!("received client command '{command}'");
                                                reply = Some(format!(
                                                    "ok:{}",
                                                    format_pause_state(selection.pause_state())
                                                ));
                                            }
                                            "resume" => {
                                                if !selection.unpause() {
                                                    debug!(
                                                        "received client command '{command}' while still recording"
                                                    );
                                                    reply = Some("err:not paused".to_string());
                                                } else {
                                                    info!(
                                                        "received client command '{command}', resuming recording"
                                                    );
                                                }
                                            }
                                            next_cmd
                                                if let Some(count) = next_cmd
                                                    .strip_prefix("next_")
                                                    .and_then(|count| count.parse().ok()) =>
                                            {
                                                info!(
                                                    "received client command '{command}', pausing recording for the next {count} items"
                                                );

                                                selection.pause(Pause::AfterItems(count));
                                                reply = Some(format!(
                                                    "ok:{}",
                                                    format_pause_state(selection.pause_state())
                                                ));
                                            }
                                            for_cmd
                                                if let Some(duration) =
                                                    for_cmd.strip_prefix("for_") =>
                                            'blk: {
                                                let pause_duration = if duration.is_empty() {
                                                    config.pause_auto_resume_after
                                                } else {
                                                    match PauseDuration::from_str(duration) {
                                                        Ok(pd) => pd,
                                                        Err(err) => {
                                                            warn!(
                                                                "invalid pause command '{command}': {err}"
                                                            );
                                                            reply = Some(format!("err:{err}"));
                                                            break 'blk;
                                                        }
                                                    }
                                                };
                                                info!(
                                                    "received client command '{command}', pausing recording for '{pause_duration}'"
                                                );

                                                selection.pause(Pause::Until(
                                                    pause_duration
                                                        .duration()
                                                        .map(|d| Instant::now() + d),
                                                ));
                                                reply = Some(format!(
                                                    "ok:{}",
                                                    format_pause_state(selection.pause_state())
                                                ));
                                            }
                                            _ => {
                                                warn!("invalid pause command '{command}'");
                                                reply =
                                                    Some("err:invalid pause command".to_string());
                                            }
                                        }
                                    }
                                    _ => {
                                        warn!("unknown client command: {command}");
                                        reply = Some("err:unknown client command".to_string());
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("failed to read client command: {e:?}");
                            }
                        }

                        if let Some(reply) = reply {
                            debug!("replying to client with: {reply:?}");
                            stream.write_all(reply.as_bytes())?;
                            stream.flush()?;
                            stream.shutdown(std::net::Shutdown::Both)?;
                        }
                    }
                    KEYBOARD_GRAB_RETRY_TOKEN => {
                        sources.keyboard_grab_timer.clear_event()?;
                        window.grab_keyboard(&sources.keyboard_grab_timer)?;
                    }
                    POINTER_GRAB_RETRY_TOKEN => {
                        sources.pointer_grab_timer.clear_event()?;
                        window.grab_pointer(&sources.pointer_grab_timer)?;
                    }
                    ATSPI_TOKEN if let Some(channel) = &sources.caret_tracker_channel => {
                        if let Some(event) = channel.take() {
                            match event {
                                CaretTrackerEvent::CaretPosition { x, y, caret_height } => {
                                    window.set_text_caret(x, y, caret_height)
                                }
                                CaretTrackerEvent::NoCaret => {
                                    window.clear_text_caret();
                                }
                                CaretTrackerEvent::Error(err) => {
                                    warn!("received caret tracker error: {err}");
                                    window.clear_text_caret();
                                }
                            }
                        }
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
                    for (_, item) in selection.history.iter().chain(bookmarks.iter()) {
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

                if text_caret_tracking_enabled
                    && let Event::PropertyNotify(ev) = event
                    && ev.window == window.screen.root
                    && ev.atom == window.atoms._NET_ACTIVE_WINDOW
                {
                    debug!("active window changed");
                    window.clear_text_caret();
                }

                if let Event::ButtonPress(_) = event {
                    state.pointer_button_press_count += 1;
                }

                // When clicking outside of the window, only a release event is sent
                if let Event::ButtonRelease(ev) = event {
                    if state.pointer_button_press_count == 0 {
                        if ev.event != window.win_id.get() {
                            info!("pointer released outside window, hiding window");
                            will_hide_window = true;
                        }
                    } else {
                        state.pointer_button_press_count -= 1;
                    }
                }

                input.handle_event(&event, state.mode == AppMode::Search)?;
                if !paster.handle_event(&event)? {
                    match selection.handle_event(&event)? {
                        EventHandlingResult::HistoryUpdated {
                            added: new_item_id,
                            removed,
                        } => {
                            if let Some(new_item_id) = new_item_id
                                && let Some(item) = selection.history.get(new_item_id)
                            {
                                ui.build_button_widget(item)?;
                            }
                            ui.remove_button_widgets(
                                removed
                                    .into_iter()
                                    .filter(|item| !bookmarks.contains(item.id())),
                            );

                            if state.mode == AppMode::Search && !state.bookmark_view {
                                search_resync_visible_first(
                                    &selection.history,
                                    &mut search,
                                    &mut ui,
                                    &mut state.active_id,
                                );
                            }

                            persistence.save_selection_data(&selection.history, &bookmarks)?;
                            items_updated = true;
                        }
                        EventHandlingResult::SelectionOwnerDied => {
                            let id = selection.history.first_unpinned_id();
                            if id == 0 {
                                debug!(
                                    "selection owner died but no items to serve, leaving selection cleared"
                                );
                            } else {
                                info!("selection owner died, taking over ownership with item {id}");
                                let item = selection.history.get(id).unwrap();
                                paster.claim_selection_ownership(item.data().clone())?;
                            }
                        }
                        EventHandlingResult::None => {}
                    }
                }
            }

            if will_show_window && state.window_shown {
                debug!("window already shown, ignoring show request");
                will_show_window = false;
            }

            if will_hide_window && !state.window_shown {
                debug!("window already hidden, ignoring hide request");
                will_hide_window = false;
            }

            if will_show_window {
                state.mode = AppMode::Normal;
                state.bookmark_view = false;
                state.active_id = selection.history.first_unpinned_id();

                if let Some(command) = &text_caret_command {
                    match query_text_caret(command) {
                        Some((x, y, caret_height)) => window.set_text_caret(x, y, caret_height),
                        None => {
                            debug!("text caret command did not return a usable position");
                            window.clear_text_caret();
                        }
                    }
                }

                window.update_window_pos()?;
                input.update_pointer_pos()?;
                ui.reset();
                search.reset();
            }

            if state.first_loop || items_updated || state.window_shown || will_show_window {
                if state.first_loop {
                    debug!("pre-rendering the window on startup");
                }

                let (key_actions, pointer_actions) =
                    keymap_action.process_input(&mut input.egui_input, state.mode);
                let mut deps = Deps {
                    selection: &mut selection,
                    bookmarks: &mut bookmarks,
                    config: &config,
                    paster: &mut paster,
                    persistence: &persistence,
                    ui: &mut ui,
                    input: &mut input,
                    search: &mut search,
                };
                let mut loop_out = LoopIntent {
                    scroll_actions: Vec::new(),
                    quick_paste_action: None,
                    will_hide_window: false,
                    paste_intent: None,
                };
                for action in key_actions {
                    handle_key_action(action, &mut deps, &mut state, &mut loop_out)?;
                }
                let scroll_actions = loop_out.scroll_actions;
                let quick_paste_index = loop_out.quick_paste_action;
                will_hide_window |= loop_out.will_hide_window;
                paste_intent = loop_out.paste_intent.or(paste_intent);

                if state.mode != state.prev_mode && state.mode == AppMode::Search {
                    search_resync(
                        active_list(&selection, &bookmarks, state.bookmark_view),
                        &mut search,
                        &mut ui,
                    );
                }

                let pause = selection.pause_state();

                let selection_items_view = match state.mode {
                    AppMode::Search => OrderedHashMapView::new(
                        &active_list(&selection, &bookmarks, state.bookmark_view)
                            .items()
                            .map,
                        &search.visible_ids,
                    ),
                    _ => active_list(&selection, &bookmarks, state.bookmark_view).to_view(),
                };

                let quick_paste_base = ui.quick_paste_base_idx();
                if let Some(quick_paste) = quick_paste_index
                    && let Some((&id, _)) =
                        selection_items_view.get_by_index(quick_paste_base + quick_paste.index)
                {
                    info!(
                        "quickpaste item {id} (index {}) selected by key action, hiding window",
                        quick_paste_base + quick_paste.index
                    );
                    will_hide_window = true;
                    paste_intent = Some(PasteIntent {
                        id,
                        modifier: quick_paste.modifier,
                    });
                }

                let ui_flow = if window.is_win_placed_above_pointer() {
                    UiFlow::BottomToTop
                } else {
                    UiFlow::TopToBottom
                };
                let (full_output, clicked_item) = ui.run(
                    input.egui_input.take(),
                    state.mode,
                    ui_flow,
                    &selection_items_view,
                    &scroll_actions,
                    &search.matches,
                    &mut state.active_id,
                    &mut keymap_action.pending_keys,
                    &mut search.query,
                    &search.state,
                    state.bookmark_view,
                    bookmarks.items(),
                    pause,
                )?;

                handle_pointer_actions(
                    pointer_actions,
                    clicked_item,
                    &mut will_hide_window,
                    &mut paste_intent,
                );

                if let Some(cursor_rect) =
                    full_output.platform_output.ime.map(|ime| ime.cursor_rect)
                {
                    input.update_xim_cursor_rect(cursor_rect)?;
                }

                let cursor_icon = full_output.platform_output.cursor_icon;
                window.change_pointer_icon(x11_cursor_name(cursor_icon))?;

                gl_context.render(&ui.egui_ctx, full_output)?;

                if state.first_loop {
                    debug!("pre-rendered the window on startup");
                }
            }

            if will_show_window {
                window.show_window()?;
                window.grab_keyboard(&sources.keyboard_grab_timer)?;
                window.grab_pointer(&sources.pointer_grab_timer)?;
                window.enable_events()?;
                window.conn.flush()?;
                state.window_shown = true;
                info!("window shown");
            }

            if will_hide_window {
                window.hide_window()?;
                window.cancel_grab_retries(
                    &sources.keyboard_grab_timer,
                    &sources.pointer_grab_timer,
                )?;
                window.ungrab_input()?;
                window.disable_events()?;
                window.conn.flush()?;
                state.window_shown = false;
                input.reset_state();
                info!("window hidden");
            }

            if let Some(paste) = paste_intent.take() {
                let active_list =
                    active_list_mut(&mut selection, &mut bookmarks, state.bookmark_view);
                if let Some(item) = active_list.get(paste.id) {
                    paster.paste(
                        item.data().clone(),
                        window.win_opened_pointer.get(),
                        paste.modifier,
                    )?;
                    active_list.move_after_pinned_if_unpinned(paste.id);
                    persistence.save_selection_data(&selection.history, &bookmarks)?;
                } else {
                    warn!("paste item {} not found", paste.id);
                }
            }

            if state.mode == AppMode::Search && (search.query_changed() || search.mode_changed()) {
                search_resync_visible_first(
                    active_list(&selection, &bookmarks, state.bookmark_view),
                    &mut search,
                    &mut ui,
                    &mut state.active_id,
                );
                ui.reset_scroll_offset();
            }

            if state.mode != state.prev_mode
                && (state.mode == AppMode::Search || state.prev_mode == AppMode::Search)
            {
                input.handle_emit_text_events_changed(state.mode == AppMode::Search)?;
            }

            state.first_loop = false;
            state.prev_mode = state.mode;
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

fn handle_key_action(
    action: KeyAction,
    deps: &mut Deps,
    state: &mut ServerState,
    out: &mut LoopIntent,
) -> Result<()> {
    let Deps {
        selection,
        bookmarks,
        config,
        paster,
        persistence,
        ui,
        input,
        search,
    } = &mut *deps;
    match action {
        KeyAction::Paste(modifier) => {
            out.will_hide_window = true;
            out.paste_intent = Some(PasteIntent {
                id: state.active_id,
                modifier,
            });
        }
        KeyAction::Scroll(scroll_action) => out.scroll_actions.push(scroll_action),
        KeyAction::Remove => {
            let active_list = active_list_mut(selection, bookmarks, state.bookmark_view);
            if active_list.is_pinned(state.active_id) {
                info!("ignoring remove action on pinned item {}", state.active_id);
                ui.show_error("Cannot remove pinned item");
            } else {
                let removed_item = active_list.remove(state.active_id);
                if let Some(item) = removed_item {
                    let still_referenced =
                        selection.history.contains(item.id()) || bookmarks.contains(item.id());
                    if !still_referenced {
                        ui.remove_button_widgets(std::iter::once(item));
                    }
                }

                if state.mode == AppMode::Search {
                    search.remove_id(state.active_id);
                }

                info!("selection item {} removed", state.active_id);
                persistence.save_selection_data(&selection.history, bookmarks)?;
            }
        }
        KeyAction::Pin => {
            let is_pinned = active_list_mut(selection, bookmarks, state.bookmark_view)
                .toggle_pin(state.active_id)?;
            if is_pinned {
                info!("selection item {} pinned", state.active_id);
            } else {
                info!("selection item {} unpinned", state.active_id);
            }
            persistence.save_selection_data(&selection.history, bookmarks)?;
        }
        KeyAction::QuickPaste(index, modifier) => {
            out.quick_paste_action = Some(QuickPasteAction { index, modifier });
        }
        KeyAction::ToggleBookmarkView => {
            state.bookmark_view = !state.bookmark_view;
            info!(
                "switching to {} view",
                if state.bookmark_view {
                    "bookmark"
                } else {
                    "main"
                }
            );

            if state.mode == AppMode::Search {
                search_resync_visible_first(
                    active_list(selection, bookmarks, state.bookmark_view),
                    search,
                    ui,
                    &mut state.active_id,
                );
            } else {
                state.active_id =
                    active_list(selection, bookmarks, state.bookmark_view).first_unpinned_id();
                ui.reset_scroll_offset();
            }
        }
        KeyAction::ToggleBookmark => {
            if state.bookmark_view {
                warn!("cannot toggle bookmark in bookmark view, use remove action instead");
            } else {
                if bookmarks.contains(state.active_id) {
                    bookmarks.remove(state.active_id);
                    info!("selection item {} unbookmarked", state.active_id);
                } else {
                    if let Some(item) = selection.history.get(state.active_id) {
                        bookmarks.insert_after_pinned(state.active_id, item.data().clone());
                        info!("selection item {} bookmarked", state.active_id);
                    } else {
                        warn!(
                            "selection item to be bookmarked {} not found",
                            state.active_id
                        );
                    };
                }

                persistence.save_selection_data(&selection.history, bookmarks)?;
            }
        }
        KeyAction::ShowSearch => {
            search.reset();
            info!("opening search with mode {:?}", search.state.mode);
            state.mode = AppMode::Search;
        }
        KeyAction::CycleSearchMode => {
            let next_search_mode = search.state.mode.cycle();
            info!("switching search mode to {next_search_mode:?}");
            search.state.mode = next_search_mode;
        }
        KeyAction::CopySearchSelection => {
            if let Some(text) = ui.search_selected_text(&search.query) {
                info!("copying highlighted search text");
                let (new_item, removed_items) = selection.insert_text_item(text)?;

                if let Some(item) = new_item {
                    ui.build_button_widget(item)?;
                    paster.claim_selection_ownership(item.data().clone())?;
                }
                ui.remove_button_widgets(
                    removed_items
                        .into_iter()
                        .filter(|item| !bookmarks.contains(item.id())),
                );

                search_resync_visible_first(
                    active_list(selection, bookmarks, state.bookmark_view),
                    search,
                    ui,
                    &mut state.active_id,
                );

                ui.show_success("Copied to clipboard");
                persistence.save_selection_data(&selection.history, bookmarks)?;
            }
        }
        KeyAction::SwitchIme => match &config.switch_ime_command {
            Some(command) => {
                debug!("switching IME with command: {command:?}");
                let command = command.clone();
                std::thread::spawn(move || {
                    let result = std::process::Command::new("sh")
                        .arg("-c")
                        .arg(&command)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                    if let Err(e) = result {
                        warn!("failed to run IME switch command: {e}");
                    }
                });
            }
            None => warn!("received IME switch keymap but switch_ime_command is not configured"),
        },
        KeyAction::TriggerUnicode => match &config.ime_unicode_input_command {
            Some(command) => {
                debug!("triggering IME unicode input with command: {command:?}");
                let command = command.clone();
                std::thread::spawn(move || {
                    let result = std::process::Command::new("sh")
                        .arg("-c")
                        .arg(&command)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                    if let Err(e) = result {
                        warn!("failed to run unicode input trigger command: {e}");
                    }
                });
            }
            None => warn!(
                "received unicode input trigger keymap but ime_unicode_input_command is not configured"
            ),
        },
        KeyAction::TriggerUnicodeHex => match &config.ime_unicode_hex_input_command {
            Some(command) => {
                debug!("triggering IME unicode hex input with command: {command:?}");
                let command = command.clone();
                std::thread::spawn(move || {
                    let result = std::process::Command::new("sh")
                        .arg("-c")
                        .arg(&command)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                    if let Err(e) = result {
                        warn!("failed to run unicode hex input trigger command: {e}");
                    }
                });
            }
            None => warn!(
                "received unicode hex input trigger keymap but ime_unicode_hex_input_command is not configured"
            ),
        },
        KeyAction::ShowHelp => {
            info!("switching to Help mode");
            state.mode = AppMode::Help;
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
        KeyAction::Close => match state.mode {
            AppMode::Normal => {
                info!("received Close action in Normal mode, hiding window");
                out.will_hide_window = true;
            }
            AppMode::Search => {
                info!("exiting search mode");
                if !search.query.is_empty() {
                    state.active_id =
                        active_list(selection, bookmarks, state.bookmark_view).first_unpinned_id();
                    ui.invalidate_text_caches();
                    ui.reset_scroll_offset();
                }
                state.mode = AppMode::Normal;
            }
            AppMode::Help => {
                info!("switching to Normal mode");
                state.mode = AppMode::Normal;
            }
        },
    }
    Ok(())
}

struct PasteIntent {
    id: u64,
    modifier: PasteModifier,
}

struct QuickPasteAction {
    index: usize,
    modifier: PasteModifier,
}

struct Deps<'w, 'a> {
    selection: &'a mut Selection<'w>,
    bookmarks: &'a mut ItemList,
    config: &'w Config,
    paster: &'a mut PasteServer<'w>,
    persistence: &'a Persistence,
    ui: &'a mut Ui<'w>,
    input: &'a mut Input<'w>,
    search: &'a mut Search,
}

struct LoopIntent {
    scroll_actions: Vec<memoni::keymap_spec::ScrollAction>,
    quick_paste_action: Option<QuickPasteAction>,
    will_hide_window: bool,
    paste_intent: Option<PasteIntent>,
}

struct ServerState {
    window_shown: bool,
    pointer_button_press_count: u32,
    bookmark_view: bool,
    active_id: u64,
    mode: AppMode,
    prev_mode: AppMode,
    first_loop: bool,
}

fn active_list<'m>(
    selection: &'m Selection,
    bookmarks: &'m ItemList,
    bookmark_view: bool,
) -> &'m ItemList {
    if bookmark_view {
        bookmarks
    } else {
        &selection.history
    }
}

fn active_list_mut<'m>(
    selection: &'m mut Selection,
    bookmarks: &'m mut ItemList,
    bookmark_view: bool,
) -> &'m mut ItemList {
    if bookmark_view {
        bookmarks
    } else {
        &mut selection.history
    }
}

fn handle_pointer_actions(
    actions: Vec<PointerAction>,
    clicked_item: Option<u64>,
    will_hide_window: &mut bool,
    paste_intent: &mut Option<PasteIntent>,
) {
    if let Some(clicked_id) = clicked_item {
        for action in actions {
            match action {
                PointerAction::Paste(modifier) => {
                    info!("paste item {clicked_id} selected by pointer, hiding window");
                    *will_hide_window = true;
                    *paste_intent = Some(PasteIntent {
                        id: clicked_id,
                        modifier,
                    });
                }
            }
        }
    } else if !actions.is_empty() {
        debug!("pointer actions received when no items getting clicked");
    }
}

fn search_resync(items: &ItemList, search: &mut Search, ui: &mut Ui) {
    search.refresh(items.items());
    ui.invalidate_text_caches();
}

fn search_resync_visible_first(
    items: &ItemList,
    search: &mut Search,
    ui: &mut Ui,
    active_id: &mut u64,
) {
    search_resync(items, search, ui);
    if !search.query.is_empty() {
        *active_id = search.visible_ids.first().copied().unwrap_or(0);
    } else {
        *active_id = items.first_unpinned_id();
    }
    ui.reset_scroll_offset();
}

fn create_poll<P: AsRef<Path> + std::fmt::Debug>(
    conn: &XCBConnection,
    socket_path: P,
    atspi_enabled: bool,
) -> Result<PollSources> {
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

    let caret_tracker_channel = if atspi_enabled {
        debug!("registering AT-SPI caret tracker's slot and waker");
        Some(Arc::new(atspi_caret_tracker::EventChannel {
            slot: Mutex::new(None),
            waker: Waker::new(poll.registry(), ATSPI_TOKEN)?,
        }))
    } else {
        None
    };

    Ok(PollSources {
        poll,
        socket_listener: listener,
        signals,
        keyboard_grab_timer,
        pointer_grab_timer,
        caret_tracker_channel,
    })
}

struct PollSources {
    poll: mio::Poll,
    socket_listener: UnixListener,
    signals: Signals,
    keyboard_grab_timer: TimerfdSource,
    pointer_grab_timer: TimerfdSource,
    caret_tracker_channel: Option<Arc<atspi_caret_tracker::EventChannel<CaretTrackerEvent>>>,
}

fn query_text_caret(command: &str) -> Option<(i32, i32, i32)> {
    debug!("querying text caret position with command: {command:?}");
    let output = match std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            debug!("text caret command failed: {err:?}");
            return None;
        }
    };

    if !output.status.success() {
        debug!("text caret command exited with failure: {output:?}");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    debug!("text caret command result: {stdout:?}");

    let mut values = stdout.split_whitespace();
    let x: i32 = values.next()?.parse().ok()?;
    let y: i32 = values.next()?.parse().ok()?;
    let _w: i32 = values.next()?.parse().ok()?;
    let h: i32 = values.next()?.parse().ok()?;
    if values.next().is_some() {
        debug!("text caret command printed more than four values");
        return None;
    }

    Some((x, y, h))
}

fn format_pause_state(pause: Pause) -> String {
    pause
        .describe(Instant::now())
        .map(|d| format!("paused ({d})"))
        .unwrap_or_else(|| "not paused".to_string())
}
