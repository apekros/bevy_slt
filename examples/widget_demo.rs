use std::time::{Duration, Instant};

use bevy::app::{AppExit, ScheduleRunnerPlugin};
use bevy::prelude::*;
use bevy_slt::{SltTerminalContext, SltTerminalPlugin};
use slt::{
    AlertLevel, Border, Color, KeyCode, ListState, RunConfig, SpinnerState, TableState, TabsState,
    TextInputState,
};

fn main() {
    let frame_time = Duration::from_secs_f32(1.0 / 30.0);
    App::new()
        .insert_non_send_resource(DemoState::new())
        .add_plugins((
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(frame_time)),
            SltTerminalPlugin::new(RunConfig::default().mouse(true)),
        ))
        .add_systems(Update, draw_demo)
        .run();
}

struct DemoState {
    started_at: Instant,
    tabs: TabsState,
    input: TextInputState,
    list: ListState,
    table: TableState,
    spinner: SpinnerState,
    dark_mode: bool,
    mouse_capture: bool,
    volume: f64,
    deploys: u32,
}

impl DemoState {
    fn new() -> Self {
        let mut table = TableState::new(
            vec!["Crate", "Role", "Status"],
            vec![
                vec!["bevy_slt", "integration", "running"],
                vec!["bevy", "scheduler", "30 fps"],
                vec!["slt", "widgets", "rendering"],
                vec!["crossterm", "terminal", "events"],
            ],
        );
        table.page_size = 4;

        Self {
            started_at: Instant::now(),
            tabs: TabsState::new(vec!["Overview", "Widgets", "Data"]),
            input: TextInputState::with_placeholder("Type here, Tab changes focus, q quits"),
            list: ListState::new(vec!["Dashboard", "Inspector", "Console", "Settings"]),
            table,
            spinner: SpinnerState::dots(),
            dark_mode: true,
            mouse_capture: true,
            volume: 0.62,
            deploys: 0,
        }
    }
}

fn draw_demo(
    mut context: NonSendMut<SltTerminalContext>,
    mut demo: NonSendMut<DemoState>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    let demo = &mut *demo;
    // Ctrl+C is handled by SltTerminalPlugin; q and Esc are this app's choice.
    let keep_going = context.draw(|ui| {
        if ui.key('q') || ui.key_code(KeyCode::Esc) {
            ui.quit();
        }

        let elapsed = demo.started_at.elapsed().as_secs_f64();
        let progress = (elapsed.sin() + 1.0) / 2.0;
        let sparkline: Vec<f64> = (0..32)
            .map(|i| ((elapsed * 2.0) + f64::from(i) * 0.35).sin() * 0.5 + 0.5)
            .collect();

        let _ = ui
            .bordered(Border::Rounded)
            .title("bevy_slt widget demo")
            .p(1)
            .gap(1)
            .col(|ui| {
                let _ = ui.row(|ui| {
                    let _ = ui.spinner(&demo.spinner);
                    ui.text(" SLT rendered through SltContext::draw")
                        .bold()
                        .fg(Color::Cyan);
                    ui.spacer();
                    let _ = ui.badge("q / Esc quits");
                });

                let _ = ui.tabs(&mut demo.tabs);
                let _ = ui.separator();

                match demo.tabs.selected {
                    0 => render_overview(ui, demo, progress),
                    1 => render_widgets(ui, demo),
                    _ => render_data(ui, demo, &sparkline),
                }
            });
    })?;

    if !keep_going {
        exit.write(AppExit::Success);
    }
    Ok(())
}

fn render_overview(ui: &mut slt::Context, demo: &mut DemoState, progress: f64) {
    let _ = ui.row(|ui| {
        let _ = ui
            .bordered(Border::Rounded)
            .title("Runtime")
            .p(1)
            .col(|ui| {
                ui.text("This example calls SltContext::draw from a Bevy system.");
                ui.text("Bevy owns scheduling, SLT owns frame rendering.")
                    .dim();
                let _ = ui.progress(progress);
                ui.gauge(progress)
                    .label(format!("{:.0}%", progress * 100.0));
            });

        let _ = ui
            .bordered(Border::Rounded)
            .title("Actions")
            .p(1)
            .col(|ui| {
                if ui.button("Deploy").clicked {
                    demo.deploys += 1;
                }
                ui.text(format!("deploy clicks: {}", demo.deploys));
                let _ = ui.checkbox("dark mode", &mut demo.dark_mode);
                let _ = ui.toggle("mouse capture", &mut demo.mouse_capture);
                let _ = ui.slider("volume", &mut demo.volume, 0.0..=1.0);
                ui.text(format!("volume: {:.0}%", demo.volume * 100.0))
                    .dim();
            });
    });

    let _ = ui.alert(
        "Input, focus, mouse, resize, and animation are flowing through Bevy resources.",
        AlertLevel::Info,
    );
}

fn render_widgets(ui: &mut slt::Context, demo: &mut DemoState) {
    let _ = ui.row(|ui| {
        let _ = ui.bordered(Border::Rounded).title("Input").p(1).col(|ui| {
            let _ = ui.text_input(&mut demo.input);
            ui.text("Try typing, Backspace, arrows, mouse clicks.")
                .dim();
        });

        let _ = ui.bordered(Border::Rounded).title("List").p(1).col(|ui| {
            let _ = ui.list(&mut demo.list);
        });
    });
}

fn render_data(ui: &mut slt::Context, demo: &mut DemoState, sparkline: &[f64]) {
    let plain_keyboard_nav = ui.raw_key_code(KeyCode::Up)
        || ui.raw_key_code(KeyCode::Down)
        || ui.raw_key_code(KeyCode::Char('k'))
        || ui.raw_key_code(KeyCode::Char('j'))
        || ui.raw_key_code(KeyCode::PageUp)
        || ui.raw_key_code(KeyCode::PageDown);

    let _ = ui.bordered(Border::Rounded).title("Table").p(1).col(|ui| {
        let _ = ui.table(&mut demo.table);
    });
    if plain_keyboard_nav {
        demo.table.clear_selection();
    }

    let _ = ui
        .bordered(Border::Rounded)
        .title("Sparkline")
        .p(1)
        .col(|ui| {
            let _ = ui.sparkline(sparkline, 48);
        });
}
