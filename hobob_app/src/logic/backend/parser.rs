use crate::*;
use bilibili_api_rs::plugin::ApiTaskResultEvent;
use chrono::naive::NaiveDateTime;
use ui::following::{
    data::{self, Data},
    event::ParsedApiResult,
};

pub struct ModPlugin();

impl Plugin for ModPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_system(first_parse_api_result.system())
            .add_system(sort_key_api_result.system());
    }
}

fn first_parse_api_result(
    mut raw_result: EventReader<ApiTaskResultEvent>,
    mut parsed: EventWriter<ParsedApiResult>,
    app_res: Res<AppResource>,
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
                    live_room_url: resp["live_room"]["url"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    live_room_title: resp["live_room"]["title"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    live_open: resp["live_room"]["liveStatus"].as_i64().map(|x| x != 0),
                    live_entropy: resp["live_room"]["online"].as_u64().unwrap_or_default(),
                    face_url: resp["face"].as_str().unwrap_or_default().to_string(),
                }),
            }),
            "new-video" => {
                if let Some(list) = resp["list"]["vlist"].as_array() {
                    let v = list.iter().reduce(|a, b| {
                        let t1 = a["created"].as_i64().unwrap_or_default();
                        let t2 = b["created"].as_i64().unwrap_or_default();
                        if t1 > t2 {
                            a
                        } else {
                            b
                        }
                    });
                    parsed.send(ParsedApiResult {
                        uid,
                        data: Data::NewVideo(match v {
                            Some(vid) => data::NewVideo {
                                title: vid["title"].as_str().unwrap_or("N/A").to_string(),
                                date_time: NaiveDateTime::from_timestamp(
                                    vid["created"].as_i64().unwrap_or_default(),
                                    0,
                                )
                                .format("%Y-%m-%d %H:%M ")
                                .to_string(),
                                timestamp_sec: vid["created"].as_u64().unwrap_or_default(),
                            },
                            None => data::NewVideo {
                                title: app_res.no_video_text.clone(),
                                date_time: Default::default(),
                                timestamp_sec: Default::default(),
                            },
                        }),
                    });
                }
            }
            _ => error!("result with unimplemented cmd: {}", cmd),
        }
    }
}

fn sort_key_api_result(
    mut sort_key_query: Query<(&mut ui::following::data::SortKey, &ui::following::data::Uid)>,
    mut result_chan: EventReader<ParsedApiResult>,
) {
    for ParsedApiResult { uid, data } in result_chan.iter() {
        if let Data::Face(_) = data {
            continue;
        }
        if let Some((mut key, _)) = sort_key_query.iter_mut().find(|(_, id)| id.0 == *uid) {
            match data {
                Data::Info(info) => {
                    key.live_entropy = if let Some(true) = info.live_open {
                        info.live_entropy
                    } else {
                        0
                    }
                }
                Data::NewVideo(vid) => key.video_pub_ts = vid.timestamp_sec,
                _ => panic!("unimplement handler of {:?}", data),
            }
        }
    }
}
