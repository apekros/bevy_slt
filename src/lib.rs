//! Bevy integration for [`slt`](https://docs.rs/superlighttui/latest/slt/).
//!
//! This crate drives SLT's custom backend API from a Bevy `Update` system. It
//! renders into [`SltOutput`], which you can display with Bevy UI, a texture, a
//! debug overlay, or anything else that can consume a terminal-cell grid.

mod terminal;

pub use crate::terminal::{SltContext, SltTerminalPlugin};

use bevy_app::{App, Plugin, Update};
use bevy_ecs::prelude::*;
use slt::{AppState, Backend, Buffer, Cell, Context, Event, Rect, RunConfig};
use std::io;

/// Adds the SLT render system and default resources.
///
/// Add a render closure with [`SltAppExt::insert_slt_ui`].
#[derive(Debug, Default)]
pub struct SltPlugin;

/// System sets exposed by this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SystemSet)]
pub enum SltSet {
    /// Runs the configured SLT closure and writes [`SltOutput`].
    Render,
}

impl Plugin for SltPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SltEvents>()
            .init_resource::<SltOutput>()
            .init_non_send_resource::<SltState>()
            .add_systems(Update, render_slt.in_set(SltSet::Render));
    }
}

/// Extension methods for wiring an SLT UI closure into a Bevy app.
pub trait SltAppExt {
    /// Inserts the immediate-mode SLT closure that will run once per Bevy update.
    fn insert_slt_ui<F>(&mut self, render: F) -> &mut Self
    where
        F: FnMut(&mut Context) + 'static;
}

impl SltAppExt for App {
    fn insert_slt_ui<F>(&mut self, render: F) -> &mut Self
    where
        F: FnMut(&mut Context) + 'static,
    {
        self.insert_non_send_resource(SltRender::new(render))
    }
}

/// Persistent SLT runtime state.
///
/// This is a Bevy non-send resource because `slt::AppState` is intentionally not
/// `Send`/`Sync`.
pub struct SltState {
    app: AppState,
    backend: SltBackend,
    config: RunConfig,
    keep_going: bool,
    last_error: Option<io::Error>,
}

impl SltState {
    /// Creates state with an 80x24-cell surface and the default SLT config.
    pub fn new() -> Self {
        Self::with_size(80, 24)
    }

    /// Creates state with the requested cell dimensions.
    pub fn with_size(width: u32, height: u32) -> Self {
        Self {
            app: AppState::new(),
            backend: SltBackend::new(width, height),
            config: RunConfig::default(),
            keep_going: true,
            last_error: None,
        }
    }

    /// Returns the configured cell dimensions.
    pub fn size(&self) -> (u32, u32) {
        self.backend.size()
    }

    /// Resizes the backing SLT surface.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.backend.resize(width, height);
    }

    /// Immutable access to the SLT run config.
    pub fn config(&self) -> &RunConfig {
        &self.config
    }

    /// Mutable access to the SLT run config.
    pub fn config_mut(&mut self) -> &mut RunConfig {
        &mut self.config
    }

    /// `false` after the UI calls `Context::quit()` or a frame error occurs.
    pub fn keep_going(&self) -> bool {
        self.keep_going
    }

    /// The last frame error, if rendering failed.
    pub fn last_error(&self) -> Option<&io::Error> {
        self.last_error.as_ref()
    }

    /// The underlying SLT buffer for advanced integrations.
    pub fn buffer(&self) -> &Buffer {
        &self.backend.buffer
    }
}

impl Default for SltState {
    fn default() -> Self {
        Self::new()
    }
}

/// Input events that will be passed into the next SLT frame.
///
/// Systems may push keyboard, mouse, paste, focus, or resize events here. The
/// render system drains this queue every frame.
#[derive(Debug, Default, Resource)]
pub struct SltEvents {
    events: Vec<Event>,
}

impl SltEvents {
    /// Queue a single event for the next frame.
    pub fn push(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Queue multiple events for the next frame.
    pub fn extend(&mut self, events: impl IntoIterator<Item = Event>) {
        self.events.extend(events);
    }

    /// Clears queued events without rendering them.
    pub fn clear(&mut self) {
        self.events.clear();
    }

    fn drain(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }
}

/// The latest rendered SLT frame, as both text and cells.
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

    fn update_from(&mut self, backend: &SltBackend) {
        let (width, height) = backend.size();
        self.width = width;
        self.height = height;
        self.cells.clone_from(&backend.buffer.content);
        self.text = backend.to_plain_text();
    }
}

/// Non-send render closure resource used by [`SltPlugin`].
pub struct SltRender {
    render: Box<dyn FnMut(&mut Context) + 'static>,
}

impl SltRender {
    /// Creates a render closure resource.
    pub fn new<F>(render: F) -> Self
    where
        F: FnMut(&mut Context) + 'static,
    {
        Self {
            render: Box::new(render),
        }
    }
}

struct SltBackend {
    buffer: Buffer,
}

impl SltBackend {
    fn new(width: u32, height: u32) -> Self {
        Self {
            buffer: Buffer::empty(Rect::new(0, 0, width, height)),
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        let area = Rect::new(0, 0, width, height);
        if self.buffer.area != area {
            self.buffer.resize(area);
        }
    }

    fn to_plain_text(&self) -> String {
        let width = self.buffer.area.width;
        let height = self.buffer.area.height;
        let mut text = String::new();

        for y in 0..height {
            if y > 0 {
                text.push('\n');
            }

            for x in 0..width {
                if let Some(cell) = self.buffer.try_get(x, y) {
                    text.push_str(cell.symbol.as_str());
                }
            }
        }

        text
    }
}

impl Backend for SltBackend {
    fn size(&self) -> (u32, u32) {
        (self.buffer.area.width, self.buffer.area.height)
    }

    fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn render_slt(
    mut state: NonSendMut<SltState>,
    mut render: Option<NonSendMut<SltRender>>,
    mut events: ResMut<SltEvents>,
    mut output: ResMut<SltOutput>,
) {
    let Some(render) = render.as_deref_mut() else {
        return;
    };

    if !state.keep_going {
        return;
    }

    let events = events.drain();
    let state = &mut *state;
    let SltState {
        app,
        backend,
        config,
        keep_going,
        last_error,
    } = state;

    match slt::frame(backend, app, config, &events, &mut render.render) {
        Ok(continue_running) => {
            *keep_going = continue_running;
            *last_error = None;
            output.update_from(backend);
        }
        Err(error) => {
            *keep_going = false;
            *last_error = Some(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{SltAppExt, SltEvents, SltOutput, SltPlugin, SltState};
    use bevy_app::App;
    use slt::Event;

    #[test]
    fn renders_slt_closure_into_output() {
        let mut app = App::new();
        app.add_plugins(SltPlugin).insert_slt_ui(|ui| {
            ui.text("hello, bevy");
        });

        app.update();

        let output = app.world().resource::<SltOutput>();
        assert!(output.text.contains("hello, bevy"));
    }

    #[test]
    fn drains_queued_events() {
        let mut events = SltEvents::default();
        events.push(Event::key_char('x'));

        assert_eq!(events.drain().len(), 1);
        assert!(events.drain().is_empty());
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

    #[test]
    fn state_can_resize() {
        let mut state = SltState::with_size(10, 5);
        state.resize(20, 8);

        assert_eq!(state.size(), (20, 8));
    }
}
