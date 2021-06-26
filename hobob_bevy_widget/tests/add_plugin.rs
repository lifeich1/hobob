use bevy::prelude::*;
use hobob_bevy_widget::scroll::ScrollWidgetsPlugin;

#[test]
fn add_scroll_widgets() {
    App::build()
        .add_plugins(MinimalPlugins)
        .add_plugin(ScrollWidgetsPlugin());
}
