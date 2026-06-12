# bevy_slt

Bevy integration for [SuperLightTUI](https://docs.rs/superlighttui/0.22.0/slt/).

```rust
use bevy_app::App;
use bevy_slt::{SltAppExt, SltOutput, SltPlugin};

let mut app = App::new();
app.add_plugins(SltPlugin).insert_slt_ui(|ui| {
    ui.text("hello from slt");
});

app.update();
let output = app.world().resource::<SltOutput>();
assert!(output.text.contains("hello from slt"));
```

`SltPlugin` runs your SLT closure once per Bevy update and stores the rendered
terminal grid in `SltOutput`. Display `SltOutput::text` with Bevy text, or use
`SltOutput::cells` for a styled renderer.
