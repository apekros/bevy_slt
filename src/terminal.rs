use std::io::{self, BufWriter, IsTerminal, Stdout, Write};

use bevy_app::{App, AppExit, Plugin, PreStartup, PreUpdate};
use bevy_ecs::message::MessageWriter;
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
    AppState, Backend, Buffer, Color, ColorDepth, Context, Event, KeyCode, KeyModifiers, Modifiers,
    MouseButton, MouseEvent, MouseKind, Rect, RunConfig, Style, UnderlineStyle,
};

/// Terminal-backed SLT plugin, modeled after `bevy_ratatui`.
///
/// This installs a [`SltContext`] resource. Draw from a Bevy system by calling
/// [`SltContext::draw`], instead of serializing [`crate::SltOutput`] yourself.
#[derive(Debug, Default)]
pub struct SltTerminalPlugin;

impl Plugin for SltTerminalPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreStartup, init_terminal_context)
            .add_systems(PreUpdate, poll_terminal_events);
    }
}

/// A Bevy resource that owns an SLT terminal session and frame state.
///
/// The shape mirrors `bevy_ratatui::RatatuiContext`: put the terminal context in
/// ECS, then let the TUI library's frame machinery render into its backend.
pub struct SltContext {
    terminal: SltTerminal,
    state: AppState,
    config: RunConfig,
    events: Vec<Event>,
}

impl SltContext {
    /// Enters raw mode and the alternate screen.
    pub fn init() -> io::Result<Self> {
        Self::init_with(RunConfig::default())
    }

    /// Enters raw mode and the alternate screen with the supplied SLT config.
    pub fn init_with(config: RunConfig) -> io::Result<Self> {
        if !io::stdout().is_terminal() {
            return Err(io::Error::other("stdout is not a terminal"));
        }

        Ok(Self {
            terminal: SltTerminal::new(&config)?,
            state: AppState::new(),
            config,
            events: Vec::new(),
        })
    }

    /// Draw one SLT frame to the real terminal.
    pub fn draw(&mut self, mut render: impl FnMut(&mut Context)) -> io::Result<bool> {
        slt::frame_owned(
            &mut self.terminal,
            &mut self.state,
            &self.config,
            std::mem::take(&mut self.events),
            &mut render,
        )
    }

    /// Mutable access to SLT run configuration.
    pub fn config_mut(&mut self) -> &mut RunConfig {
        &mut self.config
    }

    /// Queue an event for the next call to [`SltContext::draw`].
    pub fn push_event(&mut self, event: Event) {
        if let Event::Resize(width, height) = event {
            let _ = self.terminal.resize(width, height);
        }
        self.events.push(event);
    }
}

struct SltTerminal {
    stdout: BufWriter<Stdout>,
    current: Buffer,
    previous: Buffer,
    color_depth: ColorDepth,
    entered: bool,
}

impl SltTerminal {
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
        })
    }

    fn resize(&mut self, width: u32, height: u32) -> io::Result<()> {
        let area = Rect::new(0, 0, width, height);
        self.current.resize(area);
        self.previous.resize(area);
        execute!(self.stdout, Clear(ClearType::All), MoveTo(0, 0))?;
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.entered {
            return Ok(());
        }
        self.entered = false;
        execute!(
            self.stdout,
            ResetColor,
            SetAttribute(Attribute::Reset),
            Show,
            DisableMouseCapture,
            DisableFocusChange,
            DisableBracketedPaste,
            LeaveAlternateScreen
        )?;
        self.stdout.flush()?;
        disable_raw_mode()
    }
}

impl Drop for SltTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

impl Backend for SltTerminal {
    fn size(&self) -> (u32, u32) {
        (self.current.area.width, self.current.area.height)
    }

    fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.current
    }

    fn flush(&mut self) -> io::Result<()> {
        queue!(self.stdout, BeginSynchronizedUpdate)?;

        if self.current.area != self.previous.area {
            queue!(self.stdout, Clear(ClearType::All), MoveTo(0, 0))?;
        }

        let mut last_style = None;
        for (index, cell) in self.current.content.iter().enumerate() {
            let previous = self.previous.content.get(index);
            if previous == Some(cell) {
                continue;
            }

            let x = index as u32 % self.current.area.width;
            let y = index as u32 / self.current.area.width;
            queue!(self.stdout, MoveTo(x as u16, y as u16))?;
            if last_style != Some(cell.style) {
                queue_style(&mut self.stdout, cell.style, self.color_depth)?;
                last_style = Some(cell.style);
            }
            queue!(self.stdout, Print(cell.symbol.as_str()))?;
        }

        queue!(
            self.stdout,
            ResetColor,
            SetAttribute(Attribute::Reset),
            EndSynchronizedUpdate
        )?;
        self.stdout.flush()?;

        std::mem::swap(&mut self.current, &mut self.previous);
        self.current.reset();
        Ok(())
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

fn init_terminal_context(world: &mut World) {
    match SltContext::init() {
        Ok(mut context) => {
            context.config_mut().mouse = true;
            world.insert_non_send_resource(context);
        }
        Err(error) => {
            eprintln!("failed to initialize SLT terminal: {error}");
        }
    }
}

fn poll_terminal_events(context: Option<NonSendMut<SltContext>>, mut exit: MessageWriter<AppExit>) {
    let Some(mut context) = context else {
        return;
    };

    while event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        let Ok(raw) = event::read() else {
            continue;
        };
        let Some(event) = to_slt_event(raw) else {
            continue;
        };

        if is_exit_event(&event) {
            exit.write(AppExit::Success);
        }
        context.push_event(event);
    }
}

fn is_exit_event(event: &Event) -> bool {
    match event {
        Event::Key(key) => key.is_char('q') || key.is_code(KeyCode::Esc) || key.is_ctrl_char('c'),
        _ => false,
    }
}

fn to_slt_event(raw: event::Event) -> Option<Event> {
    match raw {
        event::Event::Key(key) => {
            if key.kind == event::KeyEventKind::Release {
                return None;
            }
            let code = to_key_code(key.code)?;
            let modifiers = to_key_modifiers(key.modifiers);
            Some(match code {
                KeyCode::Char(c) => Event::key_mod(KeyCode::Char(c), modifiers),
                code if modifiers == KeyModifiers::NONE => Event::key(code),
                _ => Event::key_mod(code, modifiers),
            })
        }
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
        event::KeyCode::Media(_) | event::KeyCode::Modifier(_) => return None,
    })
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
