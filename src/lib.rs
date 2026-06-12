//! Bevy integration for [`slt`](https://docs.rs/superlighttui/latest/slt/).
//!
//! Draw an SLT UI from a Bevy system by calling [`SltContext::draw`] on a
//! context resource. Two backends are provided:
//!
//! - [`SltTerminalPlugin`] (feature `terminal`, on by default) renders to the
//!   real terminal, forwards terminal input as Bevy messages, and restores the
//!   terminal on exit or panic.
//! - [`SltHeadlessPlugin`] renders to an in-memory buffer and publishes each
//!   frame as the [`SltOutput`] resource, for display via Bevy UI, a texture,
//!   or assertions in tests.

#[cfg(feature = "terminal")]
mod terminal;

#[cfg(feature = "terminal")]
pub use crate::terminal::{
    SltFocusMessage, SltKeyMessage, SltMouseMessage, SltPasteMessage, SltResizeMessage,
    SltTerminalContext, SltTerminalPlugin, TerminalBackend, restore_terminal,
};

use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use slt::{AppState, Backend, Buffer, Cell, Context, Event, Rect, RunConfig};
use std::io;
use std::sync::Mutex;

/// A surface [`SltContext`] can drive: an slt [`Backend`] that can also resize
/// and expose the last completed frame.
pub trait SltBackend: Backend {
    /// Resizes the backing surface to the given cell dimensions.
    fn resize(&mut self, width: u32, height: u32) -> io::Result<()>;

    /// The last frame that completed a [`Backend::flush`].
    fn frame_buffer(&self) -> &Buffer;
}

/// An SLT session (persistent [`AppState`], [`RunConfig`], pending input)
/// driven from Bevy systems.
///
/// This is a Bevy non-send resource because slt's `AppState` holds
/// `Box<dyn Any>` hook state and is intentionally not `Send`.
pub struct SltContext<B: SltBackend> {
    backend: B,
    state: AppState,
    config: RunConfig,
    events: Vec<Event>,
    last_mouse_pos: Option<(u32, u32)>,
}

impl<B: SltBackend> SltContext<B> {
    fn from_parts(backend: B, config: RunConfig) -> Self {
        Self {
            backend,
            state: AppState::new(),
            config,
            events: Vec::new(),
            last_mouse_pos: None,
        }
    }

    /// Renders one SLT frame, consuming the events queued since the last call.
    ///
    /// Returns `Ok(false)` after the UI calls `Context::quit()`; map that to
    /// [`bevy_app::AppExit`] in your system if quitting should end the app.
    pub fn draw(&mut self, mut render: impl FnMut(&mut Context)) -> io::Result<bool> {
        // slt only persists the hover position inside its built-in run loop;
        // the frame() path forgets it between frames. Re-feed the last known
        // position whenever this frame carries no fresher mouse event so
        // hover states survive event-free frames.
        if let Some((x, y)) = self.last_mouse_pos
            && !self.events.iter().any(Event::is_mouse)
        {
            self.events.push(Event::mouse_move(x, y));
        }

        slt::frame_owned(
            &mut self.backend,
            &mut self.state,
            &self.config,
            std::mem::take(&mut self.events),
            &mut render,
        )
    }

    /// Queues an input event for the next [`SltContext::draw`].
    ///
    /// Resize events resize the backend immediately; mouse and focus events
    /// update the persistent hover position.
    pub fn push_event(&mut self, event: Event) -> io::Result<()> {
        match &event {
            Event::Resize(width, height) => self.backend.resize(*width, *height)?,
            Event::Mouse(mouse) => self.last_mouse_pos = Some((mouse.x, mouse.y)),
            Event::FocusLost => self.last_mouse_pos = None,
            _ => {}
        }
        self.events.push(event);
        Ok(())
    }

    /// Immutable access to the SLT run config.
    pub fn config(&self) -> &RunConfig {
        &self.config
    }

    /// Mutable access to the SLT run config.
    pub fn config_mut(&mut self) -> &mut RunConfig {
        &mut self.config
    }

    /// The current cell dimensions.
    pub fn size(&self) -> (u32, u32) {
        self.backend.size()
    }

    /// Resizes the backing surface.
    pub fn resize(&mut self, width: u32, height: u32) -> io::Result<()> {
        self.backend.resize(width, height)
    }

    /// The last completed frame.
    pub fn frame_buffer(&self) -> &Buffer {
        self.backend.frame_buffer()
    }

    #[cfg(feature = "terminal")]
    pub(crate) fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
}

/// In-memory backend with no terminal attached.
pub struct HeadlessBackend {
    target: Buffer,
    frame: Buffer,
}

impl HeadlessBackend {
    /// Creates a backend with the requested cell dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        let area = Rect::new(0, 0, width, height);
        Self {
            target: Buffer::empty(area),
            frame: Buffer::empty(area),
        }
    }
}

impl Backend for HeadlessBackend {
    fn size(&self) -> (u32, u32) {
        (self.target.area.width, self.target.area.height)
    }

    fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.target
    }

    fn flush(&mut self) -> io::Result<()> {
        // Publish the rendered frame and hand the kernel a clean buffer for
        // the next one; the backend owns clearing, not the frame kernel.
        std::mem::swap(&mut self.target, &mut self.frame);
        self.target.reset();
        Ok(())
    }
}

impl SltBackend for HeadlessBackend {
    fn resize(&mut self, width: u32, height: u32) -> io::Result<()> {
        let area = Rect::new(0, 0, width, height);
        if self.target.area != area {
            self.target.resize(area);
            self.frame.resize(area);
        }
        Ok(())
    }

    fn frame_buffer(&self) -> &Buffer {
        &self.frame
    }
}

/// [`SltContext`] rendering to an in-memory buffer.
pub type SltHeadlessContext = SltContext<HeadlessBackend>;

impl SltContext<HeadlessBackend> {
    /// Creates a headless context with the given cell dimensions and config.
    pub fn headless(width: u32, height: u32, config: RunConfig) -> Self {
        Self::from_parts(HeadlessBackend::new(width, height), config)
    }
}

/// Adds an [`SltHeadlessContext`] non-send resource and publishes each frame
/// as [`SltOutput`] in `PostUpdate`.
///
/// With the `terminal` feature enabled (the default), slt probes terminal
/// capabilities once on the first frame, writing a few escape codes to
/// stdout. Headless-only consumers can avoid that by depending on `bevy_slt`
/// with `default-features = false`.
///
/// Draw from your own system:
///
/// ```ignore
/// fn draw(mut context: NonSendMut<SltHeadlessContext>) -> Result {
///     context.draw(|ui| ui.text("hello"))?;
///     Ok(())
/// }
/// ```
pub struct SltHeadlessPlugin {
    width: u32,
    height: u32,
    // RunConfig is not Clone, and Plugin::build only gets `&self`.
    config: Mutex<Option<RunConfig>>,
}

impl SltHeadlessPlugin {
    /// Creates a plugin with the requested cell dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            config: Mutex::new(None),
        }
    }

    /// Sets the SLT run config for the context.
    pub fn config(self, config: RunConfig) -> Self {
        Self {
            config: Mutex::new(Some(config)),
            ..self
        }
    }
}

impl Default for SltHeadlessPlugin {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

impl Plugin for SltHeadlessPlugin {
    fn build(&self, app: &mut App) {
        let config = self
            .config
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .unwrap_or_default();
        app.insert_non_send_resource(SltContext::headless(self.width, self.height, config))
            .init_resource::<SltOutput>()
            .add_systems(PostUpdate, publish_headless_output);
    }
}

fn publish_headless_output(context: NonSend<SltHeadlessContext>, mut output: ResMut<SltOutput>) {
    output.update_from(context.frame_buffer());
}

/// The latest rendered headless frame, as both text and cells.
///
/// Unlike [`SltHeadlessContext`] this is a plain `Send` resource, so parallel
/// systems (text display, texture upload) can read it freely.
#[derive(Debug, Default, Clone, Resource)]
pub struct SltOutput {
    /// Terminal-cell width.
    pub width: u32,
    /// Terminal-cell height.
    pub height: u32,
    /// Flat row-major cells from SLT.
    pub cells: Vec<Cell>,
    /// Plain text representation, one line per SLT row.
    pub text: String,
}

impl SltOutput {
    /// Returns the cell at `(x, y)`, or `None` when out of bounds.
    pub fn cell(&self, x: u32, y: u32) -> Option<&Cell> {
        if x >= self.width || y >= self.height {
            return None;
        }

        let index = y.checked_mul(self.width)?.checked_add(x)? as usize;
        self.cells.get(index)
    }

    fn update_from(&mut self, buffer: &Buffer) {
        self.width = buffer.area.width;
        self.height = buffer.area.height;
        self.cells.clone_from(&buffer.content);

        self.text.clear();
        for y in 0..self.height {
            if y > 0 {
                self.text.push('\n');
            }
            for x in 0..self.width {
                if let Some(cell) = buffer.try_get(x, y) {
                    self.text.push_str(cell.symbol.as_str());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{SltHeadlessContext, SltHeadlessPlugin, SltOutput};
    use bevy_app::App;
    use bevy_ecs::error::Result;
    use bevy_ecs::prelude::*;
    use slt::Event;

    #[test]
    fn headless_plugin_publishes_output() -> Result {
        let mut app = App::new();
        app.add_plugins(SltHeadlessPlugin::default()).add_systems(
            bevy_app::Update,
            |mut context: NonSendMut<SltHeadlessContext>| -> Result {
                context.draw(|ui| {
                    ui.text("hello, bevy");
                })?;
                Ok(())
            },
        );

        app.update();

        let output = app.world().resource::<SltOutput>();
        assert!(output.text.contains("hello, bevy"), "{:?}", output.text);
        Ok(())
    }

    #[test]
    fn resize_event_resizes_backend() -> Result {
        let mut context = SltHeadlessContext::headless(10, 5, slt::RunConfig::default());
        context.push_event(Event::resize(20, 8))?;

        assert_eq!(context.size(), (20, 8));
        Ok(())
    }

    #[test]
    fn hover_survives_event_free_frames() -> Result {
        let mut context = SltHeadlessContext::headless(40, 6, slt::RunConfig::default());

        // Frame 1 registers the button's hit area.
        context.draw(|ui| {
            let _ = ui.button("press me");
        })?;

        // Frame 2 moves the mouse over the button.
        context.push_event(Event::mouse_move(2, 0))?;
        let mut hovered = false;
        context.draw(|ui| {
            hovered = ui.button("press me").hovered;
        })?;
        assert!(hovered, "mouse move over the button must hover it");

        // Frame 3 has no events; the synthetic mouse position must keep the
        // hover alive.
        let mut still_hovered = false;
        context.draw(|ui| {
            still_hovered = ui.button("press me").hovered;
        })?;
        assert!(still_hovered, "hover must survive an event-free frame");
        Ok(())
    }

    #[test]
    fn output_cell_bounds_are_checked() {
        let mut output = SltOutput {
            width: 1,
            height: 1,
            ..Default::default()
        };

        assert!(output.cell(1, 0).is_none());
        assert!(output.cell(0, 1).is_none());

        output.cells.push(Default::default());
        assert!(output.cell(0, 0).is_some());
    }
}
