# bevy_slt

Bevy integration for [SuperLightTUI](https://docs.rs/superlighttui/0.22.0/slt/).

## Terminal apps

`SltTerminalPlugin` (feature `terminal`, on by default) renders to the real
terminal. It installs an `SltTerminalContext` non-send resource at startup,
forwards terminal input as Bevy messages (`SltKeyMessage`, `SltMouseMessage`,
`SltFocusMessage`, `SltPasteMessage`, `SltResizeMessage`) in `PreUpdate`,
writes `AppExit` on `Ctrl+C` (disable with `.ctrl_c_exit(false)`), and
restores the terminal on exit or panic.

```rust,no_run
use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use bevy_slt::{SltTerminalContext, SltTerminalPlugin};
use slt::RunConfig;

fn main() {
    App::new()
        .add_plugins((
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f32(1.0 / 30.0))),
            SltTerminalPlugin::new(RunConfig::default().mouse(true)),
        ))
        .add_systems(Update, draw)
        .run();
}

fn draw(mut context: NonSendMut<SltTerminalContext>, mut exit: MessageWriter<AppExit>) -> Result {
    let keep_going = context.draw(|ui| {
        if ui.key('q') {
            ui.quit();
        }
        ui.text("hello from slt inside bevy, q quits");
    })?;

    if !keep_going {
        exit.write(AppExit::Success);
    }
    Ok(())
}
```

See `examples/widget_demo.rs` for tabs, tables, inputs, and mouse handling.

## Headless rendering

`SltHeadlessPlugin` renders to an in-memory buffer and publishes each frame as
the `SltOutput` resource (plain text plus styled cells), for display with Bevy
UI, a texture, or assertions in tests. Queue input with
`SltContext::push_event`.

```rust
use bevy_app::App;
use bevy_ecs::error::Result;
use bevy_ecs::prelude::*;
use bevy_slt::{SltHeadlessContext, SltHeadlessPlugin, SltOutput};

let mut app = App::new();
app.add_plugins(SltHeadlessPlugin::default()).add_systems(
    bevy_app::Update,
    |mut context: NonSendMut<SltHeadlessContext>| -> Result {
        context.draw(|ui| {
            ui.text("hello from slt");
        })?;
        Ok(())
    },
);

app.update();
let output = app.world().resource::<SltOutput>();
assert!(output.text.contains("hello from slt"));
```

Headless-only consumers can depend on `bevy_slt` with
`default-features = false` to skip the terminal machinery entirely.

## Compatibility

| bevy_slt | Bevy |
| --- | --- |
| 0.1.x | 0.18 |
| 0.2.x | 0.19 |


