use std::io::{self, BufWriter, IsTerminal, Stdout, Write};
use std::sync::{Mutex, Once};
use std::time::Duration;

use bevy_app::{App, AppExit, Plugin, PreUpdate, Startup};
use bevy_ecs::error::Result;
use bevy_ecs::message::{Message, MessageReader, MessageWriter};
use bevy_ecs::prelude::*;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
use crossterm::style::{
    Attribute, Color as CtColor, Print, ResetColor, SetAttribute, SetBackgroundColor,
    SetForegroundColor,
};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};
use slt::{
    Backend, Buffer, Color, ColorDepth, Event, KeyCode, KeyEvent, KeyModifiers, ModifierKey,
    Modifiers, MouseButton, MouseEvent, MouseKind, Rect, RunConfig, Style, UnderlineStyle,
};

use crate::SltContext;

/// [`SltContext`] rendering to the real terminal.
pub type SltTerminalContext = SltContext<TerminalBackend>;

impl SltContext<TerminalBackend> {
    /// Enters raw mode and the alternate screen with the supplied SLT config.
    pub fn terminal(config: RunConfig) -> io::Result<Self> {
        if !io::stdout().is_terminal() {
            return Err(io::Error::other("stdout is not a terminal"));
        }

        let backend = TerminalBackend::new(&config)?;
        Ok(Self::from_parts(backend, config))
    }

    /// Enables or disables terminal mouse capture for future frames.
    pub fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        self.config_mut().mouse = enabled;
        self.backend_mut().set_mouse_capture(enabled)
    }
}

/// Terminal-backed SLT plugin, modeled after `bevy_ratatui`.
///
/// Installs an [`SltTerminalContext`] non-send resource at startup, forwards
/// terminal input as Bevy messages ([`SltKeyMessage`], [`SltMouseMessage`],
/// [`SltFocusMessage`], [`SltPasteMessage`], [`SltResizeMessage`]) in
/// `PreUpdate`, and restores the terminal on panic. Draw from your own system
/// by calling [`SltContext::draw`].
pub struct SltTerminalPlugin {
    // RunConfig is not Clone, and Plugin::build only gets `&self`.
    config: Mutex<Option<RunConfig>>,
    ctrl_c_exit: bool,
}

impl SltTerminalPlugin {
    /// Creates the plugin with the given SLT run config.
    pub fn new(config: RunConfig) -> Self {
        Self {
            config: Mutex::new(Some(config)),
            ctrl_c_exit: true,
        }
    }

    /// Whether `Ctrl+C` writes [`AppExit`]. Defaults to `true`.
    pub fn ctrl_c_exit(mut self, enabled: bool) -> Self {
        self.ctrl_c_exit = enabled;
        self
    }
}

impl Default for SltTerminalPlugin {
    fn default() -> Self {
        Self::new(RunConfig::default())
    }
}

/// Startup-only carrier for the plugin's [`RunConfig`].
#[derive(Resource)]
struct PendingTerminalConfig(RunConfig);

impl Plugin for SltTerminalPlugin {
    fn build(&self, app: &mut App) {
        install_panic_hook();

        let config = self
            .config
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .unwrap_or_default();

        app.insert_resource(PendingTerminalConfig(config))
            .add_message::<SltKeyMessage>()
            .add_message::<SltMouseMessage>()
            .add_message::<SltFocusMessage>()
            .add_message::<SltPasteMessage>()
            .add_message::<SltResizeMessage>()
            .add_systems(Startup, init_terminal_context)
            .add_systems(PreUpdate, poll_terminal_events);

        if self.ctrl_c_exit {
            app.add_systems(PreUpdate, ctrl_c_exit_system.after(poll_terminal_events));
        }
    }
}

/// A key event read from the terminal.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct SltKeyMessage(pub KeyEvent);

/// A mouse event read from the terminal.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct SltMouseMessage(pub MouseEvent);

/// The terminal gained or lost focus.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SltFocusMessage {
    Gained,
    Lost,
}

/// Text pasted into the terminal.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct SltPasteMessage(pub String);

/// The terminal was resized, in cells.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SltResizeMessage {
    pub width: u32,
    pub height: u32,
}

/// Restores the terminal to its normal state.
///
/// Safe to call even when the terminal was never entered; used by the panic
/// hook, which cannot reach the [`SltTerminalContext`] resource.
pub fn restore_terminal() -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(
        stdout,
        ResetColor,
        SetAttribute(Attribute::Reset),
        Show,
        DisableMouseCapture,
        DisableFocusChange,
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    disable_raw_mode()
}

/// Restores the terminal before the panic message prints, so it lands on a
/// readable screen instead of inside the alternate screen in raw mode.
fn install_panic_hook() {
    // Panic hooks are process-global; Once keeps repeated plugin builds from
    // stacking restore layers.
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = restore_terminal();
            original(panic_info);
        }));
    });
}

fn init_terminal_context(world: &mut World) -> Result {
    let config = world
        .remove_resource::<PendingTerminalConfig>()
        .map(|pending| pending.0)
        .unwrap_or_default();
    let context = SltContext::terminal(config)?;
    world.insert_non_send_resource(context);
    Ok(())
}

fn poll_terminal_events(
    mut context: NonSendMut<SltTerminalContext>,
    mut keys: MessageWriter<SltKeyMessage>,
    mut mouse: MessageWriter<SltMouseMessage>,
    mut focus: MessageWriter<SltFocusMessage>,
    mut paste: MessageWriter<SltPasteMessage>,
    mut resize: MessageWriter<SltResizeMessage>,
) -> Result {
    while event::poll(Duration::ZERO)? {
        let raw = event::read()?;
        let Some(event) = to_slt_event(raw) else {
            continue;
        };

        match &event {
            Event::Key(key) => {
                keys.write(SltKeyMessage(key.clone()));
            }
            Event::Mouse(mouse_event) => {
                mouse.write(SltMouseMessage(mouse_event.clone()));
            }
            Event::FocusGained => {
                focus.write(SltFocusMessage::Gained);
            }
            Event::FocusLost => {
                focus.write(SltFocusMessage::Lost);
            }
            Event::Paste(text) => {
                paste.write(SltPasteMessage(text.clone()));
            }
            Event::Resize(width, height) => {
                resize.write(SltResizeMessage {
                    width: *width,
                    height: *height,
                });
            }
            _ => {}
        }

        context.push_event(event)?;
    }
    Ok(())
}

fn ctrl_c_exit_system(mut keys: MessageReader<SltKeyMessage>, mut exit: MessageWriter<AppExit>) {
    for key in keys.read() {
        if key.0.is_ctrl_char('c') {
            exit.write(AppExit::Success);
        }
    }
}

/// Crossterm-backed slt [`Backend`] writing to stdout.
pub struct TerminalBackend {
    stdout: BufWriter<Stdout>,
    current: Buffer,
    previous: Buffer,
    color_depth: ColorDepth,
    entered: bool,
    mouse_enabled: bool,
}

impl TerminalBackend {
    fn new(config: &RunConfig) -> io::Result<Self> {
        let (cols, rows) = terminal::size()?;
        let mut stdout = io::stdout();

        enable_raw_mode()?;
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableFocusChange,
            Hide,
            Clear(ClearType::All),
            MoveTo(0, 0)
        )?;
        if config.mouse {
            execute!(stdout, EnableMouseCapture)?;
        }

        let area = Rect::new(0, 0, u32::from(cols), u32::from(rows));
        Ok(Self {
            stdout: BufWriter::with_capacity(65_536, stdout),
            current: Buffer::empty(area),
            previous: Buffer::empty(area),
            color_depth: config.color_depth.unwrap_or_else(ColorDepth::detect),
            entered: true,
            mouse_enabled: config.mouse,
        })
    }

    fn set_mouse_capture(&mut self, enabled: bool) -> io::Result<()> {
        if self.mouse_enabled == enabled {
            return Ok(());
        }

        if enabled {
            execute!(self.stdout, EnableMouseCapture)?;
        } else {
            execute!(self.stdout, DisableMouseCapture)?;
        }
        self.stdout.flush()?;
        self.mouse_enabled = enabled;
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.entered {
            return Ok(());
        }
        self.entered = false;
        self.stdout.flush()?;
        restore_terminal()
    }
}

impl Drop for TerminalBackend {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

impl Backend for TerminalBackend {
    fn size(&self) -> (u32, u32) {
        (self.current.area.width, self.current.area.height)
    }

    fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.current
    }

    fn flush(&mut self) -> io::Result<()> {
        let width = self.current.area.width;
        if width > 0 {
            queue!(self.stdout, BeginSynchronizedUpdate)?;

            let mut last_style = None;
            let mut active_link: Option<&str> = None;
            for (index, cell) in self.current.content.iter().enumerate() {
                // Wide glyphs leave their trailer cell's symbol empty; the
                // leading cell's print already covered that column.
                if cell.symbol.is_empty() {
                    continue;
                }
                if self.previous.content.get(index) == Some(cell) {
                    continue;
                }

                let x = index as u32 % width;
                let y = index as u32 / width;
                queue!(self.stdout, MoveTo(x as u16, y as u16))?;
                if last_style != Some(cell.style) {
                    queue_style(&mut self.stdout, cell.style, self.color_depth)?;
                    last_style = Some(cell.style);
                }

                let link = cell.hyperlink.as_deref().filter(|url| valid_osc8_url(url));
                if link != active_link {
                    queue_osc8(&mut self.stdout, link)?;
                    active_link = link;
                }

                queue!(self.stdout, Print(cell.symbol.as_str()))?;
            }
            if active_link.is_some() {
                queue_osc8(&mut self.stdout, None)?;
            }

            queue!(
                self.stdout,
                ResetColor,
                SetAttribute(Attribute::Reset),
                EndSynchronizedUpdate
            )?;
            self.stdout.flush()?;
        }

        std::mem::swap(&mut self.current, &mut self.previous);
        self.current.reset();
        Ok(())
    }
}

impl crate::SltBackend for TerminalBackend {
    fn resize(&mut self, width: u32, height: u32) -> io::Result<()> {
        let area = Rect::new(0, 0, width, height);
        if self.current.area != area {
            self.current.resize(area);
            self.previous.resize(area);
            execute!(self.stdout, Clear(ClearType::All), MoveTo(0, 0))?;
        }
        Ok(())
    }

    fn frame_buffer(&self) -> &Buffer {
        // flush() swaps the just-rendered frame into `previous`.
        &self.previous
    }
}

/// Reject URLs that could smuggle control bytes into the OSC 8 payload.
fn valid_osc8_url(url: &str) -> bool {
    url.bytes().all(|byte| !byte.is_ascii_control())
}

fn queue_osc8(stdout: &mut impl Write, link: Option<&str>) -> io::Result<()> {
    match link {
        Some(url) => write!(stdout, "\x1b]8;;{url}\x1b\\"),
        None => write!(stdout, "\x1b]8;;\x1b\\"),
    }
}

fn queue_style(stdout: &mut impl Write, style: Style, color_depth: ColorDepth) -> io::Result<()> {
    queue!(stdout, ResetColor, SetAttribute(Attribute::Reset))?;

    if let Some(fg) = style.fg {
        queue!(
            stdout,
            SetForegroundColor(to_crossterm_color(fg, color_depth))
        )?;
    }
    if let Some(bg) = style.bg {
        queue!(
            stdout,
            SetBackgroundColor(to_crossterm_color(bg, color_depth))
        )?;
    }

    queue_modifier(stdout, style.modifiers, Modifiers::BOLD, Attribute::Bold)?;
    queue_modifier(stdout, style.modifiers, Modifiers::DIM, Attribute::Dim)?;
    queue_modifier(
        stdout,
        style.modifiers,
        Modifiers::ITALIC,
        Attribute::Italic,
    )?;
    queue_modifier(
        stdout,
        style.modifiers,
        Modifiers::UNDERLINE,
        Attribute::Underlined,
    )?;
    queue_modifier(
        stdout,
        style.modifiers,
        Modifiers::REVERSED,
        Attribute::Reverse,
    )?;
    queue_modifier(
        stdout,
        style.modifiers,
        Modifiers::STRIKETHROUGH,
        Attribute::CrossedOut,
    )?;
    queue_modifier(
        stdout,
        style.modifiers,
        Modifiers::BLINK,
        Attribute::SlowBlink,
    )?;

    if style.underline_style != UnderlineStyle::Straight
        && style.modifiers.contains(Modifiers::UNDERLINE)
    {
        let code = match style.underline_style {
            UnderlineStyle::Straight => 1,
            UnderlineStyle::Double => 2,
            UnderlineStyle::Curly => 3,
            UnderlineStyle::Dotted => 4,
            UnderlineStyle::Dashed => 5,
            _ => 1,
        };
        write!(stdout, "\x1b[4:{code}m")?;
    }

    Ok(())
}

fn queue_modifier(
    stdout: &mut impl Write,
    modifiers: Modifiers,
    modifier: Modifiers,
    attribute: Attribute,
) -> io::Result<()> {
    if modifiers.contains(modifier) {
        queue!(stdout, SetAttribute(attribute))?;
    }
    Ok(())
}

fn to_crossterm_color(color: Color, color_depth: ColorDepth) -> CtColor {
    match color.downsampled(color_depth) {
        Color::Reset => CtColor::Reset,
        Color::Black => CtColor::Black,
        Color::Red => CtColor::DarkRed,
        Color::Green => CtColor::DarkGreen,
        Color::Yellow => CtColor::DarkYellow,
        Color::Blue => CtColor::DarkBlue,
        Color::Magenta => CtColor::DarkMagenta,
        Color::Cyan => CtColor::DarkCyan,
        Color::White => CtColor::Grey,
        Color::DarkGray => CtColor::DarkGrey,
        Color::LightRed => CtColor::Red,
        Color::LightGreen => CtColor::Green,
        Color::LightYellow => CtColor::Yellow,
        Color::LightBlue => CtColor::Blue,
        Color::LightMagenta => CtColor::Magenta,
        Color::LightCyan => CtColor::Cyan,
        Color::LightWhite => CtColor::White,
        Color::Rgb(r, g, b) => CtColor::Rgb { r, g, b },
        Color::Indexed(index) => CtColor::AnsiValue(index),
        _ => CtColor::Reset,
    }
}

fn to_slt_event(raw: event::Event) -> Option<Event> {
    match raw {
        event::Event::Key(key) => to_slt_key_event(key),
        event::Event::Mouse(mouse) => Some(Event::Mouse(MouseEvent::new(
            to_mouse_kind(mouse.kind),
            u32::from(mouse.column),
            u32::from(mouse.row),
            to_key_modifiers(mouse.modifiers),
            None,
            None,
        ))),
        event::Event::Resize(width, height) => {
            Some(Event::Resize(u32::from(width), u32::from(height)))
        }
        event::Event::Paste(text) => Some(Event::Paste(text)),
        event::Event::FocusGained => Some(Event::FocusGained),
        event::Event::FocusLost => Some(Event::FocusLost),
    }
}

fn to_slt_key_event(key: event::KeyEvent) -> Option<Event> {
    let code = to_key_code(key.code)?;
    let modifiers = to_key_modifiers(key.modifiers);
    match key.kind {
        // slt's KeyEvent is #[non_exhaustive], so events can only be built
        // through its constructors: key_mod (Press) and key_release (bare
        // char Release). Repeat collapses to Press, and releases that the
        // API cannot express are dropped.
        event::KeyEventKind::Press | event::KeyEventKind::Repeat => {
            Some(Event::key_mod(code, modifiers))
        }
        event::KeyEventKind::Release => match code {
            KeyCode::Char(c) if modifiers == KeyModifiers::NONE => Some(Event::key_release(c)),
            _ => None,
        },
    }
}

fn to_key_code(code: event::KeyCode) -> Option<KeyCode> {
    Some(match code {
        event::KeyCode::Backspace => KeyCode::Backspace,
        event::KeyCode::Enter => KeyCode::Enter,
        event::KeyCode::Left => KeyCode::Left,
        event::KeyCode::Right => KeyCode::Right,
        event::KeyCode::Up => KeyCode::Up,
        event::KeyCode::Down => KeyCode::Down,
        event::KeyCode::Home => KeyCode::Home,
        event::KeyCode::End => KeyCode::End,
        event::KeyCode::PageUp => KeyCode::PageUp,
        event::KeyCode::PageDown => KeyCode::PageDown,
        event::KeyCode::Tab => KeyCode::Tab,
        event::KeyCode::BackTab => KeyCode::BackTab,
        event::KeyCode::Delete => KeyCode::Delete,
        event::KeyCode::Insert => KeyCode::Insert,
        event::KeyCode::F(n) => KeyCode::F(n),
        event::KeyCode::Char(c) => KeyCode::Char(c),
        event::KeyCode::Null => KeyCode::Null,
        event::KeyCode::Esc => KeyCode::Esc,
        event::KeyCode::CapsLock => KeyCode::CapsLock,
        event::KeyCode::ScrollLock => KeyCode::ScrollLock,
        event::KeyCode::NumLock => KeyCode::NumLock,
        event::KeyCode::PrintScreen => KeyCode::PrintScreen,
        event::KeyCode::Pause => KeyCode::Pause,
        event::KeyCode::Menu => KeyCode::Menu,
        event::KeyCode::KeypadBegin => KeyCode::KeypadBegin,
        event::KeyCode::Modifier(modifier) => KeyCode::Modifier(to_modifier_key(modifier)),
        event::KeyCode::Media(_) => return None,
    })
}

fn to_modifier_key(modifier: event::ModifierKeyCode) -> ModifierKey {
    match modifier {
        event::ModifierKeyCode::LeftShift => ModifierKey::LeftShift,
        event::ModifierKeyCode::LeftControl => ModifierKey::LeftCtrl,
        event::ModifierKeyCode::LeftAlt => ModifierKey::LeftAlt,
        event::ModifierKeyCode::LeftSuper => ModifierKey::LeftSuper,
        event::ModifierKeyCode::LeftHyper => ModifierKey::LeftHyper,
        event::ModifierKeyCode::LeftMeta => ModifierKey::LeftMeta,
        event::ModifierKeyCode::RightShift => ModifierKey::RightShift,
        event::ModifierKeyCode::RightControl => ModifierKey::RightCtrl,
        event::ModifierKeyCode::RightAlt => ModifierKey::RightAlt,
        event::ModifierKeyCode::RightSuper => ModifierKey::RightSuper,
        event::ModifierKeyCode::RightHyper => ModifierKey::RightHyper,
        event::ModifierKeyCode::RightMeta => ModifierKey::RightMeta,
        event::ModifierKeyCode::IsoLevel3Shift => ModifierKey::IsoLevel3Shift,
        event::ModifierKeyCode::IsoLevel5Shift => ModifierKey::IsoLevel5Shift,
    }
}

fn to_key_modifiers(modifiers: event::KeyModifiers) -> KeyModifiers {
    let mut bits = KeyModifiers::NONE.0;
    if modifiers.contains(event::KeyModifiers::SHIFT) {
        bits |= KeyModifiers::SHIFT.0;
    }
    if modifiers.contains(event::KeyModifiers::CONTROL) {
        bits |= KeyModifiers::CONTROL.0;
    }
    if modifiers.contains(event::KeyModifiers::ALT) {
        bits |= KeyModifiers::ALT.0;
    }
    if modifiers.contains(event::KeyModifiers::SUPER) {
        bits |= KeyModifiers::SUPER.0;
    }
    if modifiers.contains(event::KeyModifiers::HYPER) {
        bits |= KeyModifiers::HYPER.0;
    }
    if modifiers.contains(event::KeyModifiers::META) {
        bits |= KeyModifiers::META.0;
    }
    KeyModifiers(bits)
}

fn to_mouse_kind(kind: event::MouseEventKind) -> MouseKind {
    match kind {
        event::MouseEventKind::Down(button) => MouseKind::Down(to_mouse_button(button)),
        event::MouseEventKind::Up(button) => MouseKind::Up(to_mouse_button(button)),
        event::MouseEventKind::Drag(button) => MouseKind::Drag(to_mouse_button(button)),
        event::MouseEventKind::Moved => MouseKind::Moved,
        event::MouseEventKind::ScrollDown => MouseKind::ScrollDown,
        event::MouseEventKind::ScrollUp => MouseKind::ScrollUp,
        event::MouseEventKind::ScrollLeft => MouseKind::ScrollLeft,
        event::MouseEventKind::ScrollRight => MouseKind::ScrollRight,
    }
}

fn to_mouse_button(button: event::MouseButton) -> MouseButton {
    match button {
        event::MouseButton::Left => MouseButton::Left,
        event::MouseButton::Right => MouseButton::Right,
        event::MouseButton::Middle => MouseButton::Middle,
    }
}

#[cfg(test)]
mod tests {
    use super::{to_slt_event, valid_osc8_url};
    use crossterm::event::{self, KeyEventKind, KeyEventState};
    use slt::{Event, KeyCode, KeyEventKind as SltKeyEventKind, KeyModifiers};

    fn key_event(
        code: event::KeyCode,
        modifiers: event::KeyModifiers,
        kind: KeyEventKind,
    ) -> event::Event {
        event::Event::Key(event::KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn press_preserves_modifiers() {
        let event = to_slt_event(key_event(
            event::KeyCode::Char('c'),
            event::KeyModifiers::CONTROL,
            KeyEventKind::Press,
        ))
        .expect("press must convert");

        let Event::Key(key) = event else {
            panic!("expected key event");
        };
        assert!(key.is_ctrl_char('c'));
        assert_eq!(key.kind, SltKeyEventKind::Press);
    }

    #[test]
    fn bare_char_release_preserves_kind() {
        let event = to_slt_event(key_event(
            event::KeyCode::Char('x'),
            event::KeyModifiers::NONE,
            KeyEventKind::Release,
        ))
        .expect("bare char release must convert");

        let Event::Key(key) = event else {
            panic!("expected key event");
        };
        assert_eq!(key.code, KeyCode::Char('x'));
        assert_eq!(key.kind, SltKeyEventKind::Release);
    }

    #[test]
    fn inexpressible_release_is_dropped() {
        // slt has no public constructor for non-char or modified releases.
        let dropped = to_slt_event(key_event(
            event::KeyCode::Esc,
            event::KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        assert!(dropped.is_none());
    }

    #[test]
    fn repeat_collapses_to_press() {
        let event = to_slt_event(key_event(
            event::KeyCode::Char('w'),
            event::KeyModifiers::NONE,
            KeyEventKind::Repeat,
        ))
        .expect("repeat must convert");

        let Event::Key(key) = event else {
            panic!("expected key event");
        };
        assert_eq!(key.kind, SltKeyEventKind::Press);
    }

    #[test]
    fn modifier_keys_are_translated() {
        let event = to_slt_event(key_event(
            event::KeyCode::Modifier(event::ModifierKeyCode::LeftShift),
            event::KeyModifiers::NONE,
            KeyEventKind::Press,
        ))
        .expect("modifier key must convert");

        let Event::Key(key) = event else {
            panic!("expected key event");
        };
        assert!(matches!(key.code, KeyCode::Modifier(_)));
        assert_eq!(key.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn osc8_urls_reject_control_bytes() {
        assert!(valid_osc8_url("https://example.com"));
        assert!(!valid_osc8_url("https://example.com/\x1b]evil"));
    }
}
