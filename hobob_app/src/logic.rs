use super::*;
use bevy::{
    app::AppExit,
    input::{
        keyboard::{KeyCode, KeyboardInput},
        ElementState,
    },
};
use hobob_bevy_widget::scroll;
use bilibili_api_rs::plugin::{ApiRequestEvent, ApiTaskResultEvent};
use serde_json::json;

pub struct LogicPlugin();

impl Plugin for LogicPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_system(ui.system())
            .add_system(handle_actions.system())
            .add_system(button_refresh.system())
            .add_system(nickname_api_result.system());
    }
}

fn ui(
    mut _commands: Commands,
    mut keyboard_ev: EventReader<KeyboardInput>,
    mut exit_ev: EventWriter<AppExit>,
    mut show_scroll_progression_query: Query<&mut Text, With<ShowScrollProgression>>,
    changed_scroll_progression_query: Query<
        &scroll::ScrollProgression,
        Changed<scroll::ScrollProgression>,
    >,
) {
    for ev in keyboard_ev.iter() {
        match ev {
            KeyboardInput {
                scan_code: _,
                key_code: Some(KeyCode::Escape),
                state: ElementState::Released,
            } => {
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

fn handle_actions(
    mut action_chan: EventReader<ui::following::event::Action>,
    mut api_req_chan: EventWriter<ApiRequestEvent>,
    api_ctx: Res<api::Context>,
    visible_nickname_query: Query<(&ui::following::Nickname, &Visible)>
) {
    for action in action_chan.iter() {
        match action {
            ui::following::event::Action::RefreshVisible => refresh_visible(&mut api_req_chan, &api_ctx, &visible_nickname_query),
            _ => error!("trigger not implemented action {:?}", action),
        }
    }
}

fn refresh_visible(
    api_req_chan: &mut EventWriter<ApiRequestEvent>,
    api_ctx: &Res<api::Context>,
    visible_nickname_query: &Query<(&ui::following::Nickname, &Visible)>
) {
    for (nickname, visible) in visible_nickname_query.iter() {
        if visible.is_visible {
            let uid: u64 = nickname.0;
            api_req_chan.send(ApiRequestEvent {
                req: api_ctx.new_user(uid).get_info(),
                tag: json!({"uid": uid, "cmd": "refresh"}).into(),
            });
        }
    }
}


fn first_parse_api_result(ev: &ApiTaskResultEvent) -> Option<(&serde_json::Value, u64, &str)> {
    let resp = match ev.result.as_ref() {
        Ok(r) => r,
        Err(e) => {
            error!("api error: {}", e);
            return None;
        }
    };
    let uid = match ev.tag["uid"].as_u64() {
        Some(u) => u,
        None => {
            debug!("result without uid: {:?}", ev);
            return None;
        },
    };
    let cmd = match ev.tag["cmd"].as_str() {
        Some(s) => s,
        None => {
            debug!("result without cmd: {:?}", ev);
            return None;
        },
    };
    Some((resp, uid, cmd))
}


fn nickname_api_result(
    mut nickname_query: Query<(&mut Text, &ui::following::Nickname)>,
    mut result_chan: EventReader<ApiTaskResultEvent>,
) {
    for ev in result_chan.iter() {
        if let Some((resp, uid, cmd)) = first_parse_api_result(ev) {
            for (mut text, nickname) in nickname_query.iter_mut() {
                if nickname.0 != uid {
                    continue;
                }
                match cmd {
                    "refresh" => {
                        match resp["name"].as_str() {
                            Some(s) => text.sections[0].value = s.to_string(),
                            None => error!("result without 'name': {}", resp),
                        }
                    }
                    _ => error!("result with unimplemented cmd: {:?}", ev),
                }
                break;
            }
        }
    }
}

fn button_refresh(
    app_res: Res<AppResource>,
    mut interaction_query: Query<
        (&Interaction, &mut Handle<ColorMaterial>),
        (With<Button>, Changed<Interaction>, With<ui::add::RefreshVisible>)
    >,
    mut action_chan: EventWriter<ui::following::event::Action>,
) {
    for (interaction, mut material) in interaction_query.iter_mut() {
        match *interaction {
            Interaction::Clicked => {
                info!("button refresh trigger!");
                *material = app_res.btn_press_col.clone();
                action_chan.send(ui::following::event::Action::RefreshVisible);
            }
            Interaction::Hovered => {
                *material = app_res.btn_hover_col.clone();
            }
            Interaction::None => {
                *material = app_res.btn_none_col.clone();
            }
        }
    }
}
