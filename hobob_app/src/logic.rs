use super::*;
use hobob_bevy_widget::scroll;
use bevy::{
    app::AppExit,
    input::{
        ElementState,
        keyboard::{KeyCode, KeyboardInput, },
    }
};

pub fn ui(
    mut _commands: Commands,
    mut keyboard_ev: EventReader<KeyboardInput>,
    mut exit_ev: EventWriter<AppExit>,
    mut show_scroll_progression_query: Query<&mut Text, With<ShowScrollProgression>>,
    changed_scroll_progression_query: Query<&scroll::ScrollProgression, Changed<scroll::ScrollProgression>>,
) {
    for ev in keyboard_ev.iter() {
        match ev {
            KeyboardInput { scan_code: _, key_code: Some(KeyCode::Escape), state: ElementState::Released } => {
                info!("key ESC released");
                exit_ev.send(AppExit {});
            }
            _ => (),
        }
    }

    for p in changed_scroll_progression_query.iter() {
        for mut text in show_scroll_progression_query.iter_mut() {
            text.sections[0].value = format!("{}%", p.0);
        }
    }
}
