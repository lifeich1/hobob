use super::*;
use bevy::{
    app::AppExit,
    input::{
        keyboard::{KeyCode, KeyboardInput},
        ElementState,
    },
};
use bilibili_api_rs::plugin::{ApiRequestEvent, ApiTaskResultEvent};
use hobob_bevy_widget::scroll;
use serde_json::json;
use ui::following::{event::ParsedApiResult, data::{Data, self}};

pub struct LogicPlugin();

impl Plugin for LogicPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app
            .add_system(live_info_api_result.system())
            .add_system(first_parse_api_result.system())
            .add_system(ui.system())
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
    visible_nickname_query: Query<(&ui::following::Nickname, &Visible)>,
) {
    for action in action_chan.iter() {
        match action {
            ui::following::event::Action::RefreshVisible => {
                refresh_visible(&mut api_req_chan, &api_ctx, &visible_nickname_query)
            }
            _ => error!("trigger not implemented action {:?}", action),
        }
    }
}

fn refresh_visible(
    api_req_chan: &mut EventWriter<ApiRequestEvent>,
    api_ctx: &Res<api::Context>,
    visible_nickname_query: &Query<(&ui::following::Nickname, &Visible)>,
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

fn first_parse_api_result(
    mut raw_result: EventReader<ApiTaskResultEvent>,
    mut parsed: EventWriter<ParsedApiResult>,
) {
    for ev in raw_result.iter() {
        let resp = match ev.result.as_ref() {
            Ok(r) => r,
            Err(e) => {
                error!("api error: {}", e);
                continue;
            }
        };
        let uid = match ev.tag["uid"].as_u64() {
            Some(u) => u,
            None => {
                debug!("result without uid: {:?}", ev);
                continue;
            }
        };
        let cmd = match ev.tag["cmd"].as_str() {
            Some(s) => s,
            None => {
                debug!("result without cmd: {:?}", ev);
                continue;
            }
        };
        match cmd {
            "refresh" => parsed.send(ParsedApiResult {
                uid,
                data: Data::Info(data::Info {
                    nickname: resp["name"].as_str().unwrap_or_default().to_string(),
                    live_room_url: resp["live_room"]["url"].as_str().unwrap_or_default().to_string(),
                    live_room_title: resp["live_room"]["title"].as_str().unwrap_or_default().to_string(),
                    live_open: resp["live_room"]["liveStatus"].as_i64().map(|x| x != 0),
                    live_entropy: resp["live_room"]["online"].as_u64().unwrap_or_default(),
                    face_url: resp["face"].as_str().unwrap_or_default().to_string(),
                }),
            }),
            _ => error!("result with unimplemented cmd: {}", cmd),
        }
    }
}

fn nickname_api_result(
    mut nickname_query: Query<(&mut Text, &ui::following::Nickname)>,
    mut result_chan: EventReader<ParsedApiResult>,
) {
    for ParsedApiResult{ uid, data } in result_chan.iter() {
        if let Data::Info(info) = data {
            for (mut text, nickname) in nickname_query.iter_mut() {
                if nickname.0 != *uid {
                    continue;
                }
                text.sections[0].value = info.nickname.clone();
                break;
            }
        }
    }
}

fn live_info_api_result(
    mut livetitle_query: Query<(&mut Text, &ui::following::LiveRoomTitle)>,
    mut result_chan: EventReader<ParsedApiResult>,
    app_res: Res<AppResource>,
) {
    for ParsedApiResult{ uid, data } in result_chan.iter() {
        if let Data::Info(info) = data {
            if matches!(info.live_open, None) {
                continue;
            }
            for (mut text, livetitle) in livetitle_query.iter_mut() {
                if livetitle.0 != *uid {
                    continue;
                }
                if text.sections.len() != 3 {
                    text.sections = vec![
                        TextSection {
                            value: "".to_string(),
                            style: TextStyle {
                                font: app_res.font.clone(),
                                font_size: 15.0,
                                color: Color::WHITE,
                            },
                        },
                        TextSection {
                            value: "".to_string(),
                            style: TextStyle {
                                font: app_res.font.clone(),
                                font_size: 10.0,
                                color: Color::RED,
                            },
                        },
                        TextSection {
                            value: "".to_string(),
                            style: TextStyle {
                                font: app_res.font.clone(),
                                font_size: 15.0,
                                color: Color::BLUE,
                            },
                        },
                    ];
                }
                if let Some(true) = info.live_open {
                    text.sections[0].value = app_res.live_on_text.clone();
                    text.sections[0].style.color = Color::BLUE;
                    text.sections[1].value = info.live_entropy.to_string();
                    text.sections[1].style.color = Color::RED;
                } else {
                    text.sections[0].value = app_res.live_off_text.clone();
                    text.sections[0].style.color = Color::GRAY;
                    text.sections[1].value = info.live_entropy.to_string();
                    text.sections[1].style.color = Color::GRAY;
                }
                text.sections[2].value = info.live_room_title.clone();
            }
        }
    }
}

fn button_refresh(
    app_res: Res<AppResource>,
    mut interaction_query: Query<
        (&Interaction, &mut Handle<ColorMaterial>),
        (
            With<Button>,
            Changed<Interaction>,
            With<ui::add::RefreshVisible>,
        ),
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
