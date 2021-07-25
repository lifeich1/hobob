use crate::{
    ui::{
        self,
        following::{data::Data, event::ParsedApiResult},
    },
    *,
};
use hobob_bevy_widget::scroll;

pub struct ModPlugin();

impl Plugin for ModPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_system(video_info_api_result.system())
            .add_system(live_info_api_result.system())
            .add_system(show_scroll_progression.system())
            .add_system(nickname_api_result.system());
    }
}

fn video_info_api_result(
    mut videoinfo_query: Query<(&mut Text, &ui::following::VideoInfo)>,
    mut result_chan: EventReader<ParsedApiResult>,
    app_res: Res<AppResource>,
) {
    for ParsedApiResult { uid, data } in result_chan.iter().filter(|ParsedApiResult { data, .. }| matches!(data, Data::NewVideo(_))) {
        let 
        if let Some((mut text, _)) in videoinfo_query.iter_mut().find(|(_, videoinfo)| videoinfo.0 == *uid) {
            if text.sections.len() != 2 {
                text.sections = vec![
                    TextSection {
                        value: "".to_string(),
                        style: TextStyle {
                            font: app_res.font.clone(),
                            font_size: 15.0,
                            color: Color::GRAY,
                        },
                    },
                    TextSection {
                        value: "".to_string(),
                        style: TextStyle {
                            font: app_res.font.clone(),
                            font_size: 15.0,
                            color: Color::BLACK,
                        },
                    },
                ];
            }
            text.sections[0].value = info.date_time.clone();
            text.sections[1].value = info.title.clone();
        }
    }
}

fn live_info_api_result(
    mut livetitle_query: Query<(&mut Text, &ui::following::LiveRoomTitle)>,
    mut livebutton_query: Query<&mut ui::following::LiveRoomOpenButton>,
    mut result_chan: EventReader<ParsedApiResult>,
    app_res: Res<AppResource>,
) {
    for ParsedApiResult { uid, data } in result_chan.iter().filter(|ParsedApiResult { data, .. }| matches!(data, Data::Info(_))) {
        if matches!(info.live_open, None) {
            continue;
        }
        for mut button in livebutton_query.iter_mut() {
            if button.0 != *uid {
                continue;
            }
            button.1 = info.live_room_url.clone();
            break;
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
                            font_size: 16.0,
                            color: Color::WHITE,
                        },
                    },
                    TextSection {
                        value: "".to_string(),
                        style: TextStyle {
                            font: app_res.font.clone(),
                            font_size: 15.0,
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
            break;
        }
    }
}

fn show_scroll_progression(
    mut show_scroll_progression_query: Query<&mut Text, With<ui::ShowScrollProgression>>,
    changed_scroll_progression_query: Query<
        &scroll::ScrollProgression,
        Changed<scroll::ScrollProgression>,
    >,
) {
    for p in changed_scroll_progression_query.iter() {
        for mut text in show_scroll_progression_query.iter_mut() {
            text.sections[0].value = format!("{}%", p.0);
        }
    }
}

fn nickname_api_result(
    mut nickname_query: Query<(&mut Text, &ui::following::Nickname)>,
    mut result_chan: EventReader<ParsedApiResult>,
) {
    for ParsedApiResult { uid, data } in result_chan.iter() {
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
