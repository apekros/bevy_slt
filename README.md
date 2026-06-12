# bevy_slt

Bevy integration for [SuperLightTUI](https://docs.rs/superlighttui/0.22.0/slt/).

## Usage

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

## Compatibility

| bevy_slt | Bevy |
| --- | --- |
| 0.1.x | 0.18 |
| 0.2.x | 0.19 |
