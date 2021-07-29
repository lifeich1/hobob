use crate::{
    ui::{self, following::event::ParsedApiResult},
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
    for (uid, info) in result_chan
        .iter()
        .filter_map(|ParsedApiResult { uid, data }| data.as_new_video().map(|v| (uid, v)))
    {
        if let Some((mut text, _)) = videoinfo_query
            .iter_mut()
            .find(|(_, videoinfo)| videoinfo.0 == *uid)
        {
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
            text.sections[1].value = if info.title.is_empty() {
                app_res.no_video_text.clone()
            } else {
                info.title.clone()
            };
        }
    }
}

fn live_info_api_result(
    mut livetitle_query: Query<(&mut Text, &ui::following::LiveRoomTitle)>,
    mut livebutton_query: Query<&mut ui::following::LiveRoomOpenButton>,
    mut result_chan: EventReader<ParsedApiResult>,
    app_res: Res<AppResource>,
) {
    for (uid, info) in result_chan
        .iter()
        .filter_map(|ParsedApiResult { uid, data }| data.as_info().map(|v| (uid, v)))
        .filter(|(_, info)| matches!(info.live_open, Some(_)))
    {
        if let Some(mut button) = livebutton_query.iter_mut().find(|b| b.0 == *uid) {
            button.1 = info.live_room_url.clone();
        }
        if let Some((mut text, _)) = livetitle_query
            .iter_mut()
            .find(|(_, livetitle)| livetitle.0 == *uid)
        {
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
    if let Ok(p) = changed_scroll_progression_query.single() {
        let mut text = show_scroll_progression_query
            .single_mut()
            .expect("there must be only one scroll progression display pad");
        text.sections[0].value = format!("{}%", p.0);
    }
}

fn nickname_api_result(
    mut nickname_query: Query<(&mut Text, &ui::following::Nickname)>,
    mut result_chan: EventReader<ParsedApiResult>,
) {
    for (uid, info) in result_chan
        .iter()
        .filter_map(|ParsedApiResult { uid, data }| data.as_info().map(|v| (uid, v)))
    {
        if let Some((mut text, _)) = nickname_query
            .iter_mut()
            .find(|(_, nickname)| nickname.0 == *uid)
        {
            text.sections[0].value = info.nickname.clone();
        }
    }
}
